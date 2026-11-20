//! A Z-Wave Serial API controller simulator.
//!
//! [`Simulator`] speaks the controller side of the Serial API over any byte
//! stream (an in-memory pipe in tests, TCP for the `zwave-sim` binary), with
//! simulated end nodes behind it: a mains-powered switch, a sleeping S0
//! sensor, S2 sensors and a SmartStart node ([`node::NodeSpec`]). Nodes join
//! when a test presses their button during inclusion, run their side of the
//! S0 and S2 key exchanges with this repository's security crate, answer the
//! interview, sleep and wake up, and send reports.
//!
//! It is a test double, not an emulator of any chip's quirks; bench
//! recordings cover those.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod node;

use futures_util::{SinkExt, StreamExt};
use node::{NodeSpec, SimNode};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};
use tokio_util::codec::Framed;
use tracing::debug;
use zwave_cc::Command;
use zwave_security::SecurityClass;
use zwave_serial::frame::{DataFrame, Frame, FrameCodec, FrameType};
use zwave_serial::message::{
    add_mode, AddNodeStatus, ApplicationCommand, ApplicationUpdate, ControllerId,
    ControllerVersion, InitData, NvmResponse, RemoveNodeStatus, RxStatus, SerialApiCapabilities,
    TxReport,
};
use zwave_serial::FunctionId as F;

/// A byte stream the simulator serves.
pub trait Port: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> Port for T {}

/// The simulated controller's identity.
#[derive(Debug, Clone)]
pub struct SimConfig {
    /// Home id.
    pub home_id: u32,
    /// Size of the simulated NVM image.
    pub nvm_size: usize,
    /// Pause between frames the simulator sends, so a host reads them one by one.
    pub frame_gap: Duration,
    /// Accept 16-bit node ids (and so Long Range), as 700 and 800 series sticks with Long Range do.
    pub long_range: bool,
    /// The NVM image to start with (by default `nvm_size` bytes of a pattern).
    pub nvm: Option<Vec<u8>>,
}

impl Default for SimConfig {
    fn default() -> Self {
        SimConfig {
            home_id: 0xc0ff_ee01,
            nvm_size: 4096,
            frame_gap: Duration::from_millis(2),
            long_range: true,
            nvm: None,
        }
    }
}

enum Mode {
    Idle,
    Adding {
        cb: u8,
    },
    AddingDsk {
        cb: u8,
        nwi: [u8; 4],
        long_range: bool,
    },
    Listening,
    Removing {
        cb: u8,
    },
    Replacing {
        cb: u8,
        node: u16,
    },
}

type Inspector = Box<dyn FnOnce(Option<&SimNode>) + Send>;

enum Cmd {
    Press(String, oneshot::Sender<Option<u16>>),
    PowerOn(String, oneshot::Sender<bool>),
    Wake(String, oneshot::Sender<bool>),
    SetSensor(String, u8, f64, oneshot::Sender<bool>),
    Motion(String, oneshot::Sender<bool>),
    Inspect(String, Inspector),
    Nvm(oneshot::Sender<Vec<u8>>),
    PowerOff(String, oneshot::Sender<bool>),
    Attach(Box<dyn PortBox>),
}

/// A boxed port.
pub trait PortBox: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> PortBox for T {}

/// A simulated controller with its nodes.
pub struct Simulator {
    cfg: SimConfig,
    nodes: Vec<SimNode>,
    mode: Mode,
    next_id: u16,
    suc: u16,
    nvm: Vec<u8>,
    last_added: u16,
    /// Bytes per node id on the Serial API (2 after SetNodeIDType Long).
    nid: usize,
    next_lr_id: u16,
}

/// Controls a running simulator.
#[derive(Clone)]
pub struct SimHandle {
    tx: mpsc::Sender<Cmd>,
}

impl Simulator {
    /// A controller with no nodes in its network.
    pub fn new(cfg: SimConfig) -> Self {
        let nvm = cfg
            .nvm
            .clone()
            .unwrap_or_else(|| (0..cfg.nvm_size).map(|i| (i * 7 + 3) as u8).collect());
        Simulator {
            cfg,
            nodes: vec![],
            mode: Mode::Idle,
            next_id: 2,
            suc: 0,
            nvm,
            last_added: 0,
            nid: 1,
            next_lr_id: 256,
        }
    }

    /// Adds a node that is not in the network yet.
    pub fn with_node(mut self, spec: NodeSpec) -> Self {
        self.nodes.push(SimNode::new(spec));
        self
    }

    /// Adds a node that is already in the network (insecure), as if included earlier.
    pub fn with_joined_node(mut self, spec: NodeSpec) -> Self {
        let mut n = SimNode::new(spec);
        n.join(self.next_id, self.cfg.home_id);
        n.lifeline = vec![1];
        self.next_id += 1;
        self.nodes.push(n);
        self
    }

    /// Serves `port` until it closes; control the simulation with the handle.
    pub fn spawn<P: Port>(self, port: P) -> SimHandle {
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(self.run(Box::new(port), rx));
        SimHandle { tx }
    }

    fn node_mut(&mut self, id: u16) -> Option<&mut SimNode> {
        self.nodes.iter_mut().find(|n| n.id == Some(id))
    }

    fn by_name(&mut self, name: &str) -> Option<&mut SimNode> {
        self.nodes.iter_mut().find(|n| n.spec.name == name)
    }

    fn included(&self) -> Vec<u16> {
        self.nodes.iter().filter_map(|n| n.id).collect()
    }

    async fn run(mut self, port: Box<dyn PortBox>, mut rx: mpsc::Receiver<Cmd>) {
        let mut framed = Framed::new(port, FrameCodec::default());
        // Without a handle the simulator still serves its port.
        let mut commands_open = true;
        loop {
            let out: Vec<DataFrame> = tokio::select! {
                f = framed.next() => match f {
                    Some(Ok(Frame::Data(df))) => {
                        if framed.send(Frame::Ack).await.is_err() { return; }
                        self.handle(df)
                    }
                    Some(Ok(Frame::Invalid)) => { let _ = framed.send(Frame::Nak).await; vec![] }
                    Some(Ok(_)) => vec![],
                    Some(Err(_)) | None => {
                        // Wait for a new port, as a stick does when the host reconnects.
                        loop {
                            match rx.recv().await {
                                Some(Cmd::Attach(p)) => { framed = Framed::new(p, FrameCodec::default()); break; }
                                Some(other) => { let _ = self.command(other); }
                                None => return,
                            }
                        }
                        vec![]
                    }
                },
                c = rx.recv(), if commands_open => match c {
                    Some(Cmd::Attach(p)) => { framed = Framed::new(p, FrameCodec::default()); vec![] }
                    Some(c) => self.command(c),
                    None => { commands_open = false; vec![] }
                },
            };
            for f in out {
                if framed.send(Frame::Data(f)).await.is_err() {
                    break;
                }
                // Let the host read and acknowledge before the next frame.
                if self.cfg.frame_gap.is_zero() {
                    tokio::task::yield_now().await;
                } else {
                    tokio::time::sleep(self.cfg.frame_gap).await;
                }
            }
        }
    }

    fn app_frames(&self, id: u16, frames: Vec<Vec<u8>>) -> Vec<DataFrame> {
        frames
            .into_iter()
            .map(|c| {
                ApplicationCommand {
                    rx_status: RxStatus(0),
                    source: id,
                    destination: None,
                    command: c,
                    rssi: Some(-58),
                }
                .to_frame(self.nid)
            })
            .collect()
    }

    fn command(&mut self, c: Cmd) -> Vec<DataFrame> {
        match c {
            Cmd::Press(name, reply) => {
                let (frames, id) = self.press(&name);
                let _ = reply.send(id);
                frames
            }
            Cmd::PowerOn(name, reply) => {
                let listening = matches!(self.mode, Mode::Listening);
                let Some(n) = self.by_name(&name) else {
                    let _ = reply.send(false);
                    return vec![];
                };
                if !n.spec.smartstart || n.id.is_some() || !listening {
                    let _ = reply.send(false);
                    return vec![];
                }
                let nwi = n.spec.dsk().nwi_home_id();
                let info = n.spec.nif();
                let long_range = n.spec.long_range;
                let _ = reply.send(true);
                vec![ApplicationUpdate::SmartStartHomeIdReceived {
                    nwi_home_id: nwi,
                    long_range,
                    info,
                }
                .to_frame(self.nid)]
            }
            Cmd::Wake(name, reply) => {
                let Some(n) = self.by_name(&name) else {
                    let _ = reply.send(false);
                    return vec![];
                };
                let Some(id) = n.id else {
                    let _ = reply.send(false);
                    return vec![];
                };
                let f = n.wake();
                let _ = reply.send(true);
                self.app_frames(id, f)
            }
            Cmd::SetSensor(name, t, v, reply) => {
                let Some(n) = self.by_name(&name) else {
                    let _ = reply.send(false);
                    return vec![];
                };
                n.sensors.insert(t, v);
                let (Some(id), Some(r)) = (n.id, n.sensor_report(t)) else {
                    let _ = reply.send(false);
                    return vec![];
                };
                let ok = !n.lifeline.is_empty() && n.reachable();
                let _ = reply.send(ok);
                if ok {
                    let f = n.unsolicited(r);
                    self.app_frames(id, f)
                } else {
                    vec![]
                }
            }
            Cmd::Motion(name, reply) => {
                let Some(n) = self.by_name(&name) else {
                    let _ = reply.send(false);
                    return vec![];
                };
                let Some(id) = n.id else {
                    let _ = reply.send(false);
                    return vec![];
                };
                let r: Command = zwave_cc::notification::Report {
                    v1_alarm_type: 0,
                    v1_alarm_level: 0,
                    zensor_net_source: Some(0),
                    status: Some(0xff),
                    notification_type: Some(7),
                    event: Some(8),
                    sequence: Some(false),
                    params: vec![],
                    sequence_number: None,
                }
                .into();
                let _ = reply.send(true);
                let f = n.unsolicited(r);
                self.app_frames(id, f)
            }
            Cmd::Inspect(name, f) => {
                f(self.nodes.iter().find(|n| n.spec.name == name));
                vec![]
            }
            Cmd::PowerOff(name, reply) => {
                let found = self.by_name(&name).map(|n| n.powered = false).is_some();
                let _ = reply.send(found);
                vec![]
            }
            Cmd::Nvm(reply) => {
                let _ = reply.send(self.nvm.clone());
                vec![]
            }
            Cmd::Attach(_) => vec![],
        }
    }

    fn press(&mut self, name: &str) -> (Vec<DataFrame>, Option<u16>) {
        let home = self.cfg.home_id;
        match self.mode {
            Mode::Adding { cb } => {
                let next = self.next_id;
                let Some(n) = self.by_name(name) else {
                    return (vec![], None);
                };
                if n.id.is_some() {
                    return (vec![], None);
                }
                n.join(next, home);
                let info = n.spec.nif();
                self.next_id += 1;
                self.last_added = next;
                self.mode = Mode::Idle;
                (
                    vec![
                        AddNodeStatus::NodeFound.to_frame(cb, self.nid),
                        AddNodeStatus::AddingEndNode { node: next, info }.to_frame(cb, self.nid),
                        AddNodeStatus::ProtocolDone.to_frame(cb, self.nid),
                    ],
                    Some(next),
                )
            }
            Mode::Replacing { cb, node } => {
                let Some(n) = self.by_name(name) else {
                    return (vec![], None);
                };
                if n.id.is_some() {
                    return (vec![], None);
                }
                n.join(node, home);
                self.mode = Mode::Idle;
                (
                    vec![DataFrame::request(F::REPLACE_FAILED_NODE, vec![cb, 0x04])],
                    Some(node),
                )
            }
            Mode::Removing { cb } => {
                let Some(n) = self.by_name(name) else {
                    return (vec![], None);
                };
                let id = n.id.unwrap_or(0);
                n.leave();
                self.mode = Mode::Idle;
                (
                    vec![
                        RemoveNodeStatus::NodeFound.to_frame(cb, self.nid),
                        RemoveNodeStatus::RemovingEndNode { node: id }.to_frame(cb, self.nid),
                        RemoveNodeStatus::Done { node: id }.to_frame(cb, self.nid),
                    ],
                    Some(id),
                )
            }
            _ => (vec![], None),
        }
    }

    fn response(f: F, payload: Vec<u8>) -> DataFrame {
        DataFrame::response(f, payload)
    }

    fn handle(&mut self, df: DataFrame) -> Vec<DataFrame> {
        if df.kind != FrameType::Request {
            return vec![];
        }
        let p = &df.payload;
        let home = self.cfg.home_id;
        match F(df.function) {
            F::GET_SERIAL_API_CAPABILITIES => {
                let functions = SerialApiCapabilities::mask(&[
                    F::GET_INIT_DATA,
                    F::APPLICATION_COMMAND_HANDLER,
                    F::GET_CONTROLLER_CAPABILITIES,
                    F::GET_SERIAL_API_CAPABILITIES,
                    F::SERIAL_API_SETUP,
                    F::SEND_DATA,
                    F::GET_CONTROLLER_VERSION,
                    F::GET_CONTROLLER_ID,
                    F::GET_NODE_PROTOCOL_INFO,
                    F::APPLICATION_UPDATE,
                    F::ADD_NODE_TO_NETWORK,
                    F::REMOVE_NODE_FROM_NETWORK,
                    F::SET_SUC_NODE_ID,
                    F::GET_SUC_NODE_ID,
                    F::REQUEST_NODE_INFO,
                    F::REMOVE_FAILED_NODE,
                    F::REPLACE_FAILED_NODE,
                    F::GET_ROUTING_INFO,
                    F::EXTENDED_NVM_OPERATIONS,
                    F::GET_LONG_RANGE_NODES,
                ]);
                let c = SerialApiCapabilities {
                    firmware_major: 7,
                    firmware_minor: 22,
                    manufacturer_id: 0x0000,
                    product_type: 0x0004,
                    product_id: 0x0004,
                    functions,
                };
                vec![Self::response(F::GET_SERIAL_API_CAPABILITIES, c.encode())]
            }
            F::GET_CONTROLLER_VERSION => vec![Self::response(
                F::GET_CONTROLLER_VERSION,
                ControllerVersion {
                    version: "Z-Wave 7.22".into(),
                    library_type: 7,
                }
                .encode(),
            )],
            F::GET_CONTROLLER_ID => vec![Self::response(
                F::GET_CONTROLLER_ID,
                ControllerId {
                    home_id: home,
                    node_id: 1,
                }
                .encode(self.nid),
            )],
            F::GET_INIT_DATA => {
                let mut nodes = vec![1];
                nodes.extend(self.included().into_iter().filter(|&n| n <= 255));
                let caps = if self.suc == 1 { 0x08 } else { 0 };
                vec![Self::response(
                    F::GET_INIT_DATA,
                    InitData {
                        api_version: 9,
                        capabilities: caps,
                        nodes,
                        chip_type: Some(8),
                        chip_version: Some(0),
                    }
                    .encode(),
                )]
            }
            F::GET_CONTROLLER_CAPABILITIES => {
                vec![Self::response(F::GET_CONTROLLER_CAPABILITIES, vec![0x1c])]
            }
            F::SERIAL_API_SETUP => match p.first() {
                Some(&0x02) => vec![Self::response(F::SERIAL_API_SETUP, vec![0x02, 0x01])],
                // USA Long Range.
                Some(&0x20) => vec![Self::response(F::SERIAL_API_SETUP, vec![0x20, 0x09])],
                Some(&0x80) if self.cfg.long_range => {
                    self.nid = if p.get(1) == Some(&0x02) { 2 } else { 1 };
                    vec![Self::response(F::SERIAL_API_SETUP, vec![0x80, 0x01])]
                }
                _ => vec![Self::response(F::SERIAL_API_SETUP, vec![0x00])],
            },
            F::GET_SUC_NODE_ID => {
                let mut v = Vec::new();
                zwave_serial::message::node_bytes(self.suc, self.nid, &mut v);
                vec![Self::response(F::GET_SUC_NODE_ID, v)]
            }
            F::GET_LONG_RANGE_NODES => {
                let lr: Vec<u16> = self.included().into_iter().filter(|&n| n > 255).collect();
                let seg = p.first().copied().unwrap_or(0);
                vec![Self::response(
                    F::GET_LONG_RANGE_NODES,
                    zwave_serial::message::encode_long_range_nodes(false, seg, &lr),
                )]
            }
            F::SET_SUC_NODE_ID => {
                self.suc = zwave_serial::message::parse_node_id(p, self.nid).unwrap_or(0);
                vec![Self::response(F::SET_SUC_NODE_ID, vec![0x01])]
            }
            F::GET_NODE_PROTOCOL_INFO => {
                let id = zwave_serial::message::parse_node_id(p, self.nid).unwrap_or(0);
                let info = self
                    .node_mut(id)
                    .map(|n| n.spec.protocol_info().encode())
                    .unwrap_or(vec![0; 6]);
                vec![Self::response(F::GET_NODE_PROTOCOL_INFO, info)]
            }
            F::GET_ROUTING_INFO => {
                let id = zwave_serial::message::parse_node_id(p, self.nid).unwrap_or(0);
                let mut neighbors: Vec<u16> = vec![1];
                neighbors.extend(
                    self.nodes
                        .iter()
                        .filter(|n| n.spec.listening)
                        .filter_map(|n| n.id)
                        .filter(|&n| n != id),
                );
                vec![Self::response(
                    F::GET_ROUTING_INFO,
                    zwave_serial::message::encode_node_mask(&neighbors, 29),
                )]
            }
            F::REQUEST_NODE_INFO => {
                let id = zwave_serial::message::parse_node_id(p, self.nid).unwrap_or(0);
                let mut out = vec![Self::response(F::REQUEST_NODE_INFO, vec![0x01])];
                match self.node_mut(id) {
                    Some(n) if n.reachable() => out.push(
                        ApplicationUpdate::NodeInfoReceived {
                            node: id,
                            info: n.spec.nif(),
                        }
                        .to_frame(self.nid),
                    ),
                    _ => out.push(ApplicationUpdate::NodeInfoRequestFailed.to_frame(self.nid)),
                }
                out
            }
            F::SEND_DATA => self.send_data(p),
            F::ADD_NODE_TO_NETWORK => {
                let mode = p.first().copied().unwrap_or(0) & 0x0f;
                let cb = p.get(1).copied().unwrap_or(0);
                match mode {
                    add_mode::ANY => {
                        self.mode = Mode::Adding { cb };
                        vec![AddNodeStatus::Ready.to_frame(cb, self.nid)]
                    }
                    add_mode::SMART_START_LISTEN => {
                        self.mode = Mode::Listening;
                        vec![]
                    }
                    add_mode::SMART_START_DSK => {
                        let nwi: [u8; 4] = p
                            .get(2..6)
                            .and_then(|s| s.try_into().ok())
                            .unwrap_or([0; 4]);
                        let long_range = p
                            .first()
                            .is_some_and(|f| f & add_mode::PROTOCOL_LONG_RANGE != 0);
                        self.mode = Mode::AddingDsk {
                            cb,
                            nwi,
                            long_range,
                        };
                        self.include_smartstart()
                    }
                    add_mode::STOP => {
                        let was_listening = matches!(self.mode, Mode::Listening);
                        self.mode = Mode::Idle;
                        if cb != 0 && !was_listening {
                            vec![AddNodeStatus::Done {
                                node: self.last_added,
                            }
                            .to_frame(cb, self.nid)]
                        } else {
                            vec![]
                        }
                    }
                    _ => vec![],
                }
            }
            F::REMOVE_NODE_FROM_NETWORK => {
                let mode = p.first().copied().unwrap_or(0) & 0x0f;
                let cb = p.get(1).copied().unwrap_or(0);
                match mode {
                    add_mode::ANY => {
                        self.mode = Mode::Removing { cb };
                        vec![RemoveNodeStatus::Ready.to_frame(cb, self.nid)]
                    }
                    _ => {
                        self.mode = Mode::Idle;
                        vec![]
                    }
                }
            }
            F::REPLACE_FAILED_NODE => {
                let id = zwave_serial::message::parse_node_id(p, self.nid).unwrap_or(0);
                let cb = p.get(self.nid).copied().unwrap_or(0);
                match self.node_mut(id).map(|n| n.reachable()) {
                    None => vec![Self::response(F::REPLACE_FAILED_NODE, vec![0x01])],
                    Some(true) => vec![
                        Self::response(F::REPLACE_FAILED_NODE, vec![0x00]),
                        DataFrame::request(F::REPLACE_FAILED_NODE, vec![cb, 0x00]),
                    ],
                    Some(false) => {
                        if let Some(n) = self.node_mut(id) {
                            n.leave();
                        }
                        self.mode = Mode::Replacing { cb, node: id };
                        vec![
                            Self::response(F::REPLACE_FAILED_NODE, vec![0x00]),
                            DataFrame::request(F::REPLACE_FAILED_NODE, vec![cb, 0x03]),
                        ]
                    }
                }
            }
            F::REMOVE_FAILED_NODE => {
                let id = zwave_serial::message::parse_node_id(p, self.nid).unwrap_or(0);
                let cb = p.get(self.nid).copied().unwrap_or(0);
                let failed = self.node_mut(id).map(|n| !n.reachable()).unwrap_or(true);
                let mut out = vec![Self::response(F::REMOVE_FAILED_NODE, vec![0x00])];
                if failed {
                    if let Some(n) = self.node_mut(id) {
                        n.leave();
                    }
                    out.push(DataFrame::request(F::REMOVE_FAILED_NODE, vec![cb, 0x01]));
                } else {
                    out.push(DataFrame::request(F::REMOVE_FAILED_NODE, vec![cb, 0x02]));
                }
                out
            }
            F::EXTENDED_NVM_OPERATIONS => {
                let op = p.first().copied().unwrap_or(0xff);
                let r = match op {
                    0x00 => NvmResponse {
                        status: 0,
                        offset_or_size: self.nvm.len() as u32,
                        data: vec![],
                    },
                    0x01 => {
                        let len = usize::from(p.get(1).copied().unwrap_or(0));
                        let off = p
                            .get(2..6)
                            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
                            .unwrap_or(0) as usize;
                        let end = (off + len).min(self.nvm.len());
                        let data = self.nvm.get(off..end).unwrap_or(&[]).to_vec();
                        NvmResponse {
                            status: if end >= self.nvm.len() { 0xff } else { 0 },
                            offset_or_size: off as u32,
                            data,
                        }
                    }
                    0x02 => {
                        let len = usize::from(p.get(1).copied().unwrap_or(0));
                        let off = p
                            .get(2..6)
                            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
                            .unwrap_or(0) as usize;
                        match p.get(6..6 + len) {
                            Some(data) if off + len <= self.nvm.len() => {
                                self.nvm[off..off + len].copy_from_slice(data);
                                NvmResponse {
                                    status: 0,
                                    offset_or_size: off as u32,
                                    data: vec![],
                                }
                            }
                            _ => NvmResponse {
                                status: 0x01,
                                offset_or_size: off as u32,
                                data: vec![],
                            },
                        }
                    }
                    0x03 => NvmResponse {
                        status: 0,
                        offset_or_size: 0,
                        data: vec![],
                    },
                    _ => NvmResponse {
                        status: 0x04,
                        offset_or_size: 0,
                        data: vec![],
                    },
                };
                vec![Self::response(F::EXTENDED_NVM_OPERATIONS, r.encode(true))]
            }
            other => {
                debug!("simulator ignores {other}");
                vec![]
            }
        }
    }

    fn include_smartstart(&mut self) -> Vec<DataFrame> {
        let Mode::AddingDsk {
            cb,
            nwi,
            long_range,
        } = self.mode
        else {
            return vec![];
        };
        // A Long Range node only joins over Long Range, with a 16-bit id.
        if long_range && self.nid != 2 {
            return vec![AddNodeStatus::Failed.to_frame(cb, self.nid)];
        }
        let next = if long_range {
            self.next_lr_id
        } else {
            self.next_id
        };
        let home = self.cfg.home_id;
        let Some(n) = self.nodes.iter_mut().find(|n| {
            n.spec.smartstart
                && n.spec.long_range == long_range
                && n.id.is_none()
                && n.spec.dsk().nwi_home_id() == nwi
        }) else {
            return vec![AddNodeStatus::Failed.to_frame(cb, self.nid)];
        };
        n.join(next, home);
        let info = n.spec.nif();
        if long_range {
            self.next_lr_id += 1;
        } else {
            self.next_id += 1;
        }
        self.last_added = next;
        self.mode = Mode::Idle;
        vec![
            AddNodeStatus::Ready.to_frame(cb, self.nid),
            AddNodeStatus::NodeFound.to_frame(cb, self.nid),
            AddNodeStatus::AddingEndNode { node: next, info }.to_frame(cb, self.nid),
            AddNodeStatus::ProtocolDone.to_frame(cb, self.nid),
        ]
    }

    fn send_data(&mut self, p: &[u8]) -> Vec<DataFrame> {
        let id = zwave_serial::message::parse_node_id(p, self.nid).unwrap_or(0);
        let at = self.nid;
        let len = usize::from(p.get(at).copied().unwrap_or(0));
        let Some(data) = p.get(at + 1..at + 1 + len).map(<[u8]>::to_vec) else {
            return vec![Self::response(F::SEND_DATA, vec![0x00])];
        };
        let cb = p.get(at + 2 + len).copied().unwrap_or(0);
        let mut out = vec![Self::response(F::SEND_DATA, vec![0x01])];
        let reply = match self.node_mut(id) {
            Some(n) if n.reachable() => Some(n.receive(&data)),
            _ => None,
        };
        let status = if reply.is_some() { 0x00 } else { 0x01 };
        if cb != 0 {
            let report = TxReport {
                ticks: 2,
                repeaters: vec![],
                ack_rssi: Some(-55),
                repeater_rssi: vec![],
                ack_channel: Some(0),
                tx_channel: 0,
                route_scheme: 0,
                speed: 3,
                attempts: 1,
                failed_from: None,
                failed_to: None,
            };
            let mut cbp = vec![cb, status];
            cbp.extend_from_slice(&report.encode());
            out.push(DataFrame::request(F::SEND_DATA, cbp));
        }
        if let Some(frames) = reply {
            out.extend(self.app_frames(id, frames));
        }
        out
    }
}

impl SimHandle {
    async fn ask<T>(&self, make: impl FnOnce(oneshot::Sender<T>) -> Cmd) -> Option<T> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(make(tx)).await.ok()?;
        rx.await.ok()
    }

    /// Presses a node's button: it joins while inclusion runs, leaves while exclusion runs.
    /// Returns its node id (new or former).
    pub async fn press(&self, name: &str) -> Option<u16> {
        self.ask(|tx| Cmd::Press(name.into(), tx)).await.flatten()
    }

    /// Powers on a SmartStart node: it asks for inclusion while the controller listens.
    pub async fn power_on(&self, name: &str) -> bool {
        self.ask(|tx| Cmd::PowerOn(name.into(), tx))
            .await
            .unwrap_or(false)
    }

    /// Wakes a sleeping node: it sends a Wake Up Notification.
    pub async fn wake_up(&self, name: &str) -> bool {
        self.ask(|tx| Cmd::Wake(name.into(), tx))
            .await
            .unwrap_or(false)
    }

    /// Changes a sensor reading; a node with a lifeline reports it.
    pub async fn set_sensor(&self, name: &str, sensor_type: u8, value: f64) -> bool {
        self.ask(|tx| Cmd::SetSensor(name.into(), sensor_type, value, tx))
            .await
            .unwrap_or(false)
    }

    /// The node detects motion and sends a Notification Report.
    pub async fn motion(&self, name: &str) -> bool {
        self.ask(|tx| Cmd::Motion(name.into(), tx))
            .await
            .unwrap_or(false)
    }

    /// Reads something from a node's state.
    pub async fn inspect<T: Send + 'static>(
        &self,
        name: &str,
        f: impl FnOnce(&SimNode) -> T + Send + 'static,
    ) -> Option<T> {
        let (tx, rx) = oneshot::channel();
        let cmd = Cmd::Inspect(
            name.into(),
            Box::new(move |n| {
                let _ = tx.send(n.map(f));
            }),
        );
        self.tx.send(cmd).await.ok()?;
        rx.await.ok().flatten()
    }

    /// The node's id, if included.
    pub async fn node_id(&self, name: &str) -> Option<u16> {
        self.inspect(name, |n| n.id).await.flatten()
    }

    /// The classes the node holds keys for.
    pub async fn granted(&self, name: &str) -> Vec<SecurityClass> {
        self.inspect(name, |n| n.granted.clone())
            .await
            .unwrap_or_default()
    }

    /// Cuts a node's power: it stops answering, so the controller sees it fail.
    pub async fn power_off(&self, name: &str) -> bool {
        self.ask(|tx| Cmd::PowerOff(name.into(), tx))
            .await
            .unwrap_or(false)
    }

    /// The simulated NVM image.
    pub async fn nvm(&self) -> Vec<u8> {
        self.ask(Cmd::Nvm).await.unwrap_or_default()
    }

    /// Serves a new connection (the host reconnected).
    pub async fn attach<P: Port>(&self, port: P) {
        let _ = self.tx.send(Cmd::Attach(Box::new(port))).await;
    }
}
