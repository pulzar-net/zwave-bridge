//! The controller: initialization, the inbound dispatcher, the outbound
//! encapsulation pipeline, requests with replies, wake-up handling and the
//! store writer.

use crate::inclusion::InclusionState;
use crate::node::{InterviewStage, NodeRecord, NodeStatus, ProtocolInfo};
use crate::store::{self, StoreData};
use crate::values;
use crate::{Error, Event, NetworkKeys, NodeId};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, Notify};
use tracing::{debug, info, warn};
use zwave_cc::{multi_channel, supervision, transport_service, wake_up, Command};
use zwave_security::kex::KeyPair;
use zwave_security::s0::{S0Manager, S0Payload};
use zwave_security::s2::{EncryptOptions, KeyId, S2Manager};
use zwave_security::SecurityClass;
use zwave_serial::message::{
    self, tx_options, ApplicationCommand, ApplicationUpdate, ControllerId, ControllerVersion,
    InitData, NodeProtocolInfo, SendDataResult, SerialApiCapabilities, TransmitStatus,
};
use zwave_serial::port::Port;
use zwave_serial::{DataFrame, FunctionId as F, Request, SerialApi};

/// Events kept for a consumer that has fallen behind before new ones are dropped.
pub const MAX_PENDING_EVENTS: usize = 16 * 1024;

/// The stream of [`Event`]s from a controller. Read it continuously; events
/// beyond [`MAX_PENDING_EVENTS`] unread ones are dropped.
#[derive(Debug)]
pub struct Events {
    rx: mpsc::UnboundedReceiver<Event>,
    pending: Arc<AtomicUsize>,
}

impl Events {
    /// The next event; `None` once the controller has stopped.
    pub async fn recv(&mut self) -> Option<Event> {
        let e = self.rx.recv().await;
        if e.is_some() {
            self.pending.fetch_sub(1, Ordering::Relaxed);
        }
        e
    }
}

/// Largest command class frame sent in one SendData before Transport Service splits it.
pub const MAX_SEND_PAYLOAD: usize = 46;

/// Configuration of the host stack.
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// Where the node store lives; `None` keeps it in memory only.
    pub store_path: Option<PathBuf>,
    /// The network keys.
    pub keys: NetworkKeys,
    /// How long to wait for a node's reply.
    pub request_timeout: Duration,
    /// How long to wait for a nonce.
    pub nonce_timeout: Duration,
    /// How long inclusion and exclusion wait for a node.
    pub inclusion_timeout: Duration,
    /// How long to wait for a person to grant keys or type a PIN.
    pub user_timeout: Duration,
    /// The wake-up interval to configure on sleeping nodes, seconds.
    pub wake_up_interval: u32,
    /// How long an awake node may sit idle before it is sent back to sleep.
    pub awake_idle: Duration,
    /// How long changes collect before the store is written.
    pub store_delay: Duration,
    /// Serial link timing.
    pub serial: zwave_serial::Config,
}

impl HostConfig {
    /// Defaults with these keys and store.
    pub fn new(keys: NetworkKeys, store_path: Option<PathBuf>) -> Self {
        HostConfig {
            store_path,
            keys,
            request_timeout: Duration::from_secs(5),
            nonce_timeout: Duration::from_secs(3),
            inclusion_timeout: Duration::from_secs(60),
            user_timeout: Duration::from_secs(240),
            wake_up_interval: 3600,
            awake_idle: Duration::from_millis(1000),
            store_delay: Duration::from_secs(1),
            serial: zwave_serial::Config::default(),
        }
    }
}

/// What the controller says about itself.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ControllerInfo {
    /// Home id.
    pub home_id: u32,
    /// Own node id.
    pub node_id: NodeId,
    /// Library version (e.g. `Z-Wave 7.21`).
    pub library_version: String,
    /// Library type name.
    pub library_type: String,
    /// Serial API firmware version.
    pub firmware: String,
    /// Manufacturer id of the stick.
    pub manufacturer_id: u16,
    /// Product type.
    pub product_type: u16,
    /// Product id.
    pub product_id: u16,
    /// Chip type (7 for 700 series, 8 for 800 series).
    pub chip_type: Option<u8>,
    /// The controller is the SIS.
    pub sis: bool,
    /// The controller is a secondary controller.
    pub secondary: bool,
    /// RF region, if the controller reports it.
    pub region: Option<String>,
    /// Nodes in the network (without the controller).
    pub nodes: Vec<NodeId>,
}

/// The network's topology as the host knows it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Topology {
    /// The controller's node id.
    pub controller: NodeId,
    /// Every node.
    pub nodes: Vec<TopologyNode>,
}

/// One node in the topology.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyNode {
    /// Node id.
    pub id: NodeId,
    /// Neighbors in the controller's routing table.
    pub neighbors: Vec<NodeId>,
    /// Repeaters of the last frame that reached it.
    pub last_route: Option<Vec<NodeId>>,
    /// RSSI of its last ACK at the controller.
    pub last_rssi: Option<i8>,
    /// Listening (mains powered) or not.
    pub listening: bool,
}

/// Security of a received command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RxSecurity {
    S2(KeyId),
    S0,
}

/// How to protect an outgoing frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TxSecurity {
    Plain,
    S2(KeyId),
    S0 {
        temp: bool,
    },
    /// Whatever the node's record says for this command class.
    Auto,
}

/// Something a waiter can receive.
#[derive(Debug, Clone)]
pub(crate) enum Inbound {
    Command {
        endpoint: u8,
        command: Command,
        security: Option<RxSecurity>,
    },
    S2Nonce,
    S0Nonce,
    NodeInfo(message::NodeInfo),
    NodeInfoFailed,
}

type Matcher = Box<dyn Fn(&Inbound) -> bool + Send>;

enum WaiterTx {
    Once(Option<oneshot::Sender<Inbound>>),
    Stream(mpsc::UnboundedSender<Inbound>),
}

impl WaiterTx {
    fn live(&self) -> bool {
        match self {
            WaiterTx::Once(t) => t.is_some(),
            WaiterTx::Stream(t) => !t.is_closed(),
        }
    }
}

struct Waiter {
    id: u32,
    node: NodeId,
    broadcast: bool,
    matcher: Matcher,
    tx: WaiterTx,
}

/// Removes its waiter when dropped.
pub(crate) struct WaitGuard {
    inner: Arc<Inner>,
    id: u32,
}

impl Drop for WaitGuard {
    fn drop(&mut self) {
        if let Ok(mut w) = self.inner.waiters.lock() {
            w.retain(|w| w.id != self.id);
        }
    }
}

#[derive(Default)]
pub(crate) struct Runtime {
    pub(crate) status: NodeStatus,
    wake_tickets: Vec<oneshot::Sender<()>>,
    inflight: u32,
    last_activity: Option<Instant>,
    sleeper: bool,
    pub(crate) last_route: Option<(Vec<NodeId>, Option<i8>)>,
    /// The last S2 command sent, to re-send once if the node asks for a resync.
    last_s2: Option<(Vec<u8>, KeyId, Instant)>,
}

pub(crate) struct Inner {
    pub(crate) api: SerialApi,
    pub(crate) cfg: HostConfig,
    pub(crate) caps: SerialApiCapabilities,
    pub(crate) info: Mutex<ControllerInfo>,
    pub(crate) store: Mutex<StoreData>,
    pub(crate) runtime: Mutex<HashMap<NodeId, Runtime>>,
    pub(crate) s2: Mutex<S2Manager>,
    /// S2 for Long Range nodes: its own keys, as Long Range requires.
    pub(crate) s2_lr: Mutex<S2Manager>,
    /// Bytes per node id on the Serial API: 1, or 2 once 16-bit ids are on.
    pub(crate) nid: usize,
    pub(crate) s0: Mutex<S0Manager>,
    pub(crate) keypair: KeyPair,
    waiters: Mutex<Vec<Waiter>>,
    next_waiter: AtomicU32,
    events: mpsc::UnboundedSender<Event>,
    pending_events: Arc<AtomicUsize>,
    dirty: Notify,
    store_failed: Mutex<Option<String>>,
    node_locks: Mutex<HashMap<NodeId, Arc<tokio::sync::Mutex<()>>>>,
    reassembler: Mutex<transport_service::Reassembler>,
    supervision_session: AtomicU8,
    transport_session: AtomicU8,
    pub(crate) inclusion: Mutex<InclusionState>,
    pub(crate) interviews: Mutex<HashSet<NodeId>>,
}

/// A handle to a running controller. Clones share it.
#[derive(Clone)]
pub struct Controller {
    pub(crate) inner: Arc<Inner>,
}

impl std::fmt::Debug for Controller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let info = self.info();
        write!(f, "Controller({:08x}, node {})", info.home_id, info.node_id)
    }
}

/// Counts a node as busy while alive, so it is not sent back to sleep.
pub(crate) struct Activity {
    inner: Arc<Inner>,
    node: NodeId,
}

impl Drop for Activity {
    fn drop(&mut self) {
        if let Ok(mut rt) = self.inner.runtime.lock() {
            let r = rt.entry(self.node).or_default();
            r.inflight = r.inflight.saturating_sub(1);
            r.last_activity = Some(Instant::now());
        }
    }
}

fn region_name(r: message::RfRegion) -> String {
    format!("{r:?}")
}

impl Controller {
    /// Initializes the controller on `port`, loads the store, starts the
    /// dispatcher and resumes interviews. Events arrive on the returned
    /// channel; read it, or events are dropped when it fills.
    pub async fn start<P: Port>(port: P, cfg: HostConfig) -> Result<(Controller, Events), Error> {
        let (api, unsolicited) = SerialApi::start(port, cfg.serial.clone());
        let t = Duration::from_secs(2);
        let caps = SerialApiCapabilities::parse(
            &api.request(
                DataFrame::request(F::GET_SERIAL_API_CAPABILITIES, vec![]),
                t,
            )
            .await?
            .payload,
        )
        .map_err(|e| Error::Controller(e.to_string()))?;
        let version = ControllerVersion::parse(
            &api.request(DataFrame::request(F::GET_CONTROLLER_VERSION, vec![]), t)
                .await?
                .payload,
        )
        .map_err(|e| Error::Controller(e.to_string()))?;
        // 16-bit node ids where the controller offers them (needed for Long Range).
        let mut nid = 1;
        if caps.supports(F::SERIAL_API_SETUP) {
            if let Ok(r) = api
                .request(
                    DataFrame::request(
                        F::SERIAL_API_SETUP,
                        vec![
                            message::setup::SET_NODE_ID_TYPE,
                            message::setup::NODE_ID_LONG,
                        ],
                    ),
                    t,
                )
                .await
            {
                if r.payload.first() == Some(&message::setup::SET_NODE_ID_TYPE)
                    && r.payload.get(1).is_some_and(|&b| b != 0)
                {
                    nid = 2;
                }
            }
        }
        let id = ControllerId::parse(
            &api.request(DataFrame::request(F::GET_CONTROLLER_ID, vec![]), t)
                .await?
                .payload,
            nid,
        )
        .map_err(|e| Error::Controller(e.to_string()))?;
        let init = InitData::parse(
            &api.request(DataFrame::request(F::GET_INIT_DATA, vec![]), t)
                .await?
                .payload,
        )
        .map_err(|e| Error::Controller(e.to_string()))?;
        if init.is_end_node() {
            return Err(Error::Controller(
                "the device runs the end node API, not a controller".into(),
            ));
        }
        let mut region = None;
        if caps.supports(F::SERIAL_API_SETUP) {
            let r = api
                .request(
                    DataFrame::request(
                        F::SERIAL_API_SETUP,
                        vec![message::setup::SET_TX_STATUS_REPORT, 0xff],
                    ),
                    t,
                )
                .await;
            if let Err(e) = r {
                warn!("could not enable transmit reports: {e}");
            }
            if let Ok(r) = api
                .request(
                    DataFrame::request(F::SERIAL_API_SETUP, vec![message::setup::GET_RF_REGION]),
                    t,
                )
                .await
            {
                if r.payload.first() == Some(&message::setup::GET_RF_REGION) {
                    region = r
                        .payload
                        .get(1)
                        .map(|&b| region_name(message::RfRegion::from_u8(b)));
                }
            }
        }
        let mut sis = init.is_sis();
        if !init.is_secondary() && caps.supports(F::GET_SUC_NODE_ID) {
            let suc = api
                .request(DataFrame::request(F::GET_SUC_NODE_ID, vec![]), t)
                .await?;
            if suc.payload.first() == Some(&0) && caps.supports(F::SET_SUC_NODE_ID) {
                // No SUC in the network: make the controller the SIS, as hosts do.
                let r = api
                    .request(
                        DataFrame::request(F::SET_SUC_NODE_ID, {
                            let mut v = Vec::new();
                            message::node_bytes(id.node_id, nid, &mut v);
                            v.extend_from_slice(&[0x01, 0x00, 0x01, 0x00]);
                            v
                        }),
                        t,
                    )
                    .await?;
                sis = r.payload.first() == Some(&1);
                info!("controller made SIS: {sis}");
            } else {
                sis = message::parse_node_id(&suc.payload, nid) == Some(id.node_id);
            }
        }

        let mut data = match &cfg.store_path {
            Some(p) => store::load(p).map_err(Error::Store)?,
            None => StoreData::default(),
        };
        if let (Some(h), Some(p)) = (data.home_id, &cfg.store_path) {
            if h != id.home_id {
                let aside = p.with_extension(format!("json.{h:08x}"));
                warn!(
                    "store {} is for home {h:08x}, the controller is in {:08x}; moving it to {}",
                    p.display(),
                    id.home_id,
                    aside.display()
                );
                std::fs::rename(p, &aside).map_err(|e| Error::Store(e.to_string()))?;
                data = StoreData::default();
            }
        }
        data.home_id = Some(id.home_id);
        let mut nodes: Vec<NodeId> = init
            .nodes
            .iter()
            .copied()
            .filter(|&n| n != id.node_id)
            .collect();
        if nid == 2 && caps.supports(F::GET_LONG_RANGE_NODES) {
            for segment in 0..4u8 {
                let r = api
                    .request(
                        DataFrame::request(F::GET_LONG_RANGE_NODES, vec![segment]),
                        t,
                    )
                    .await?;
                let (more, _, lr) = message::parse_long_range_nodes(&r.payload)
                    .map_err(|e| Error::Controller(e.to_string()))?;
                nodes.extend(lr);
                if !more {
                    break;
                }
            }
        }
        data.nodes.retain(|n, _| nodes.contains(n));
        for &n in &nodes {
            data.nodes.entry(n).or_insert_with(|| NodeRecord::new(n));
        }

        let mut s2 = S2Manager::new(id.node_id, id.home_id);
        for class in SecurityClass::HIGHEST_FIRST {
            s2.set_network_key(class, cfg.keys.key(class));
        }
        let mut s2_lr = S2Manager::new(id.node_id, id.home_id);
        for class in [
            SecurityClass::S2AccessControl,
            SecurityClass::S2Authenticated,
        ] {
            if let Some(k) = cfg.keys.long_range_key(class) {
                s2_lr.set_network_key(class, k);
            }
        }
        for (n, r) in &data.nodes {
            if !r.security.is_empty() {
                if crate::is_long_range(*n) {
                    s2_lr.set_peer_classes(*n, &r.security);
                } else {
                    s2.set_peer_classes(*n, &r.security);
                }
            }
        }
        let s0 = S0Manager::new(id.node_id, &cfg.keys.s0);
        let keypair = KeyPair::from_private(cfg.keys.ecdh_private);
        let info = ControllerInfo {
            home_id: id.home_id,
            node_id: id.node_id,
            library_version: version.version.clone(),
            library_type: version.library_name().to_string(),
            firmware: format!("{}.{}", caps.firmware_major, caps.firmware_minor),
            manufacturer_id: caps.manufacturer_id,
            product_type: caps.product_type,
            product_id: caps.product_id,
            chip_type: init.chip_type,
            sis,
            secondary: init.is_secondary(),
            region,
            nodes: nodes.clone(),
        };
        let (etx, erx) = mpsc::unbounded_channel();
        let pending_events = Arc::new(AtomicUsize::new(0));
        let erx = Events {
            rx: erx,
            pending: pending_events.clone(),
        };
        let inner = Arc::new(Inner {
            api,
            cfg,
            caps,
            info: Mutex::new(info.clone()),
            store: Mutex::new(data),
            runtime: Mutex::new(HashMap::new()),
            s2: Mutex::new(s2),
            s2_lr: Mutex::new(s2_lr),
            nid,
            s0: Mutex::new(s0),
            keypair,
            waiters: Mutex::new(Vec::new()),
            next_waiter: AtomicU32::new(1),
            events: etx,
            pending_events,
            dirty: Notify::new(),
            store_failed: Mutex::new(None),
            node_locks: Mutex::new(HashMap::new()),
            reassembler: Mutex::new(transport_service::Reassembler::default()),
            supervision_session: AtomicU8::new(0),
            transport_session: AtomicU8::new(0),
            inclusion: Mutex::new(InclusionState::default()),
            interviews: Mutex::new(HashSet::new()),
        });
        let c = Controller { inner };
        c.mark_dirty();
        tokio::spawn(c.clone().store_writer());
        tokio::spawn(c.clone().dispatch(unsolicited));
        c.emit(Event::Ready(info));
        for n in nodes {
            let needs_protocol = c
                .with_node(n, |r| r.stage < InterviewStage::ProtocolInfo)
                .unwrap_or(false);
            if needs_protocol {
                if let Err(e) = c.fetch_protocol_info(n).await {
                    warn!(node = n, "protocol info: {e}");
                }
            }
            let sleeping = c.with_node(n, |r| r.protocol.sleeping()).unwrap_or(false);
            c.set_status(
                n,
                if sleeping {
                    NodeStatus::Asleep
                } else {
                    NodeStatus::Unknown
                },
            );
            if c.with_node(n, |r| r.stage < InterviewStage::Complete)
                .unwrap_or(false)
            {
                c.spawn_interview(n);
            }
        }
        c.update_smartstart().await;
        Ok((c, erx))
    }

    /// The controller's own information.
    pub fn info(&self) -> ControllerInfo {
        self.inner.info.lock().expect("info lock").clone()
    }

    /// The serial link, for functions this crate does not wrap.
    pub fn serial(&self) -> &SerialApi {
        &self.inner.api
    }

    /// The controller's S2 DSK (its key pair's first 16 bytes).
    pub fn dsk(&self) -> zwave_security::dsk::Dsk {
        zwave_security::dsk::Dsk::from_public_key(self.inner.keypair.public())
    }

    /// A node's record.
    pub fn node(&self, node: NodeId) -> Option<NodeRecord> {
        self.inner
            .store
            .lock()
            .expect("store lock")
            .nodes
            .get(&node)
            .cloned()
    }

    /// Every node's record.
    pub fn nodes(&self) -> Vec<NodeRecord> {
        self.inner
            .store
            .lock()
            .expect("store lock")
            .nodes
            .values()
            .cloned()
            .collect()
    }

    /// A node's runtime status.
    pub fn status(&self, node: NodeId) -> NodeStatus {
        self.inner
            .runtime
            .lock()
            .expect("runtime lock")
            .get(&node)
            .map(|r| r.status)
            .unwrap_or_default()
    }

    /// Sets a node's name and location (persisted).
    pub fn set_name(
        &self,
        node: NodeId,
        name: Option<String>,
        location: Option<String>,
    ) -> Result<(), Error> {
        self.update_node(node, |r| {
            r.name = name;
            r.location = location;
        })?;
        self.emit(Event::NodeUpdated { node });
        Ok(())
    }

    /// The last store write error, if the last write failed.
    pub fn store_error(&self) -> Option<String> {
        self.inner.store_failed.lock().expect("lock").clone()
    }

    /// Writes the store now (e.g. before shutting down).
    pub fn flush(&self) -> Result<(), Error> {
        let Some(path) = self.inner.cfg.store_path.clone() else {
            return Ok(());
        };
        let data = self.inner.store.lock().expect("store lock").clone();
        store::save(&path, &data).map_err(Error::Store)
    }

    pub(crate) fn emit(&self, e: Event) {
        if self.inner.pending_events.load(Ordering::Relaxed) >= MAX_PENDING_EVENTS {
            warn!("event dropped, the consumer is not reading events: {e:?}");
            return;
        }
        if self.inner.events.send(e).is_ok() {
            self.inner.pending_events.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn mark_dirty(&self) {
        self.inner.dirty.notify_one();
    }

    async fn store_writer(self) {
        let Some(path) = self.inner.cfg.store_path.clone() else {
            return;
        };
        let weak = Arc::downgrade(&self.inner);
        drop(self);
        loop {
            let Some(inner) = weak.upgrade() else { return };
            inner.dirty.notified().await;
            let delay = inner.cfg.store_delay;
            drop(inner);
            tokio::time::sleep(delay).await;
            let Some(inner) = weak.upgrade() else { return };
            let data = inner.store.lock().expect("store lock").clone();
            let p = path.clone();
            let res = tokio::task::spawn_blocking(move || store::save(&p, &data)).await;
            let err = match res {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e),
                Err(e) => Some(e.to_string()),
            };
            if let Some(e) = &err {
                warn!("store write failed: {e}");
            }
            *inner.store_failed.lock().expect("lock") = err;
        }
    }

    pub(crate) fn with_node<T>(&self, node: NodeId, f: impl FnOnce(&NodeRecord) -> T) -> Option<T> {
        self.inner
            .store
            .lock()
            .expect("store lock")
            .nodes
            .get(&node)
            .map(f)
    }

    pub(crate) fn update_node(
        &self,
        node: NodeId,
        f: impl FnOnce(&mut NodeRecord),
    ) -> Result<(), Error> {
        {
            let mut s = self.inner.store.lock().expect("store lock");
            let r = s.nodes.get_mut(&node).ok_or(Error::UnknownNode(node))?;
            let before = r.clone();
            f(r);
            if *r == before {
                return Ok(());
            }
        }
        self.mark_dirty();
        Ok(())
    }

    pub(crate) fn set_status(&self, node: NodeId, status: NodeStatus) {
        let changed = {
            let mut rt = self.inner.runtime.lock().expect("runtime lock");
            let r = rt.entry(node).or_default();
            let changed = r.status != status;
            r.status = status;
            if status == NodeStatus::Awake {
                for t in r.wake_tickets.drain(..) {
                    let _ = t.send(());
                }
            }
            changed
        };
        if changed {
            self.emit(Event::NodeStatus { node, status });
        }
    }

    pub(crate) fn activity(&self, node: NodeId) -> Activity {
        let mut rt = self.inner.runtime.lock().expect("runtime lock");
        let r = rt.entry(node).or_default();
        r.inflight += 1;
        r.last_activity = Some(Instant::now());
        Activity {
            inner: self.inner.clone(),
            node,
        }
    }

    fn node_lock(&self, node: NodeId) -> Arc<tokio::sync::Mutex<()>> {
        self.inner
            .node_locks
            .lock()
            .expect("lock")
            .entry(node)
            .or_default()
            .clone()
    }

    /// Registers a waiter for something from `node`.
    pub(crate) fn wait_for(
        &self,
        node: NodeId,
        broadcast: bool,
        matcher: impl Fn(&Inbound) -> bool + Send + 'static,
    ) -> (WaitGuard, oneshot::Receiver<Inbound>) {
        let id = self.inner.next_waiter.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.inner
            .waiters
            .lock()
            .expect("waiters lock")
            .push(Waiter {
                id,
                node,
                broadcast,
                matcher: Box::new(matcher),
                tx: WaiterTx::Once(Some(tx)),
            });
        (
            WaitGuard {
                inner: self.inner.clone(),
                id,
            },
            rx,
        )
    }

    /// Registers a waiter that receives every match until its guard is dropped.
    pub(crate) fn subscribe(
        &self,
        node: NodeId,
        matcher: impl Fn(&Inbound) -> bool + Send + 'static,
    ) -> (WaitGuard, mpsc::UnboundedReceiver<Inbound>) {
        let id = self.inner.next_waiter.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::unbounded_channel();
        self.inner
            .waiters
            .lock()
            .expect("waiters lock")
            .push(Waiter {
                id,
                node,
                broadcast: false,
                matcher: Box::new(matcher),
                tx: WaiterTx::Stream(tx),
            });
        (
            WaitGuard {
                inner: self.inner.clone(),
                id,
            },
            rx,
        )
    }

    /// Hands `what` to waiters for `node`; true if one took it.
    fn deliver(&self, node: NodeId, what: Inbound) -> bool {
        let mut w = self.inner.waiters.lock().expect("waiters lock");
        let mut taken = false;
        for waiter in w.iter_mut() {
            if waiter.node == node && waiter.tx.live() && (waiter.matcher)(&what) {
                match &mut waiter.tx {
                    WaiterTx::Once(t) => {
                        if let Some(tx) = t.take() {
                            let _ = tx.send(what.clone());
                        }
                    }
                    WaiterTx::Stream(tx) => {
                        let _ = tx.send(what.clone());
                    }
                }
                taken = true;
                if !waiter.broadcast {
                    break;
                }
            }
        }
        w.retain(|w| w.tx.live());
        taken
    }

    // ---------------------------------------------------------------- inbound

    async fn dispatch(self, mut rx: mpsc::Receiver<DataFrame>) {
        while let Some(f) = rx.recv().await {
            match F(f.function) {
                F::APPLICATION_COMMAND_HANDLER => {
                    match ApplicationCommand::parse(&f.payload, self.inner.nid) {
                        Ok(ac) => self.on_application_command(ac).await,
                        Err(e) => debug!("{e}"),
                    }
                }
                F::BRIDGE_APPLICATION_COMMAND_HANDLER => {
                    match ApplicationCommand::parse_bridge(&f.payload, self.inner.nid) {
                        Ok(ac) => self.on_application_command(ac).await,
                        Err(e) => debug!("{e}"),
                    }
                }
                F::APPLICATION_UPDATE => match ApplicationUpdate::parse(&f.payload, self.inner.nid)
                {
                    Ok(u) => self.on_application_update(u).await,
                    Err(e) => debug!("{e}"),
                },
                F::ADD_NODE_TO_NETWORK => self.on_add_node_status(&f.payload),
                F::REMOVE_NODE_FROM_NETWORK => self.on_remove_node_status(&f.payload),
                F::SERIAL_API_STARTED => warn!("the controller restarted"),
                other => debug!("unsolicited {other} ignored"),
            }
        }
        debug!("dispatcher stopped");
    }

    async fn on_application_update(&self, u: ApplicationUpdate) {
        match u {
            ApplicationUpdate::NodeInfoReceived { node, info } => {
                let _ = self.update_node(node, |r| {
                    for cc in &info.supported {
                        r.root_mut().ccs.entry(*cc).or_default();
                    }
                });
                if !self.deliver(node, Inbound::NodeInfo(info)) {
                    debug!(node, "unrequested node information");
                }
            }
            ApplicationUpdate::NodeInfoRequestFailed => {
                let waiting: Vec<NodeId> = self
                    .inner
                    .waiters
                    .lock()
                    .expect("lock")
                    .iter()
                    .map(|w| w.node)
                    .collect();
                for n in waiting {
                    self.deliver(n, Inbound::NodeInfoFailed);
                }
            }
            ApplicationUpdate::SmartStartHomeIdReceived {
                nwi_home_id,
                long_range,
                ..
            } => self.on_smartstart_request(nwi_home_id, long_range),
            ApplicationUpdate::NodeRemoved { node } => self.forget_node(node),
            other => debug!("application update {other:?}"),
        }
    }

    pub(crate) fn forget_node(&self, node: NodeId) {
        let existed = self
            .inner
            .store
            .lock()
            .expect("store lock")
            .nodes
            .remove(&node)
            .is_some();
        self.inner.runtime.lock().expect("lock").remove(&node);
        self.s2(node).lock().expect("lock").forget_peer(node);
        {
            let mut s = self.inner.store.lock().expect("store lock");
            for e in s.provisioning.entries().to_vec() {
                if e.node_id == Some(node) {
                    if let Some(m) = s.provisioning.get_mut(&e.dsk) {
                        m.node_id = None;
                    }
                }
            }
        }
        self.inner
            .info
            .lock()
            .expect("lock")
            .nodes
            .retain(|&n| n != node);
        if existed {
            self.mark_dirty();
            self.emit(Event::NodeRemoved { node });
        }
    }

    /// Unwraps a received frame and acts on it.
    async fn on_application_command(&self, ac: ApplicationCommand) {
        let node = ac.source;
        if node == self.info().node_id {
            return;
        }
        if ac.rx_status.multicast() || ac.rx_status.broadcast() {
            debug!(node, "multicast or broadcast frame");
        }
        let mut frame = ac.command;
        let mut endpoint = 0u8;
        let mut security: Option<RxSecurity> = None;
        let mut supervision: Option<(u8, bool)> = None;
        for _ in 0..8 {
            let (Some(&cc), cmd) = (frame.first(), frame.get(1).copied()) else {
                return;
            };
            match (cc, cmd) {
                (transport_service::CC, _) => match transport_service::parse(&frame) {
                    Ok(
                        seg @ (transport_service::Segment::First { .. }
                        | transport_service::Segment::Subsequent { .. }),
                    ) => {
                        let session = match &seg {
                            transport_service::Segment::First { session, .. }
                            | transport_service::Segment::Subsequent { session, .. } => *session,
                            _ => 0,
                        };
                        let r = self.inner.reassembler.lock().expect("lock").push(node, seg);
                        match r {
                            Ok(transport_service::Reassembly::Done(d)) => {
                                self.send_detached(
                                    node,
                                    transport_service::complete(session),
                                    TxSecurity::Plain,
                                );
                                frame = d;
                                continue;
                            }
                            Ok(transport_service::Reassembly::Pending) => return,
                            Err(e) => {
                                debug!(node, "transport service: {e}");
                                return;
                            }
                        }
                    }
                    Ok(other) => {
                        debug!(node, "transport service {other:?}");
                        return;
                    }
                    Err(e) => {
                        debug!(node, "transport service: {e}");
                        return;
                    }
                },
                (zwave_cc::crc16::CC, Some(0x01)) => match zwave_cc::crc16::decapsulate(&frame) {
                    Some(inner) => frame = inner.to_vec(),
                    None => {
                        debug!(node, "CRC-16 mismatch");
                        return;
                    }
                },
                (0x9f, Some(0x01)) => {
                    // The node wants to send us something encrypted.
                    let report = self
                        .s2(node)
                        .lock()
                        .expect("lock")
                        .nonce_report(node, false);
                    if let Ok(r) = report {
                        self.transmit_logged(node, r).await;
                    }
                    return;
                }
                (0x9f, Some(0x02)) => {
                    let r = self
                        .s2(node)
                        .lock()
                        .expect("lock")
                        .handle_nonce_report(node, &frame);
                    match r {
                        Ok(report) => {
                            let ours = self.deliver(node, Inbound::S2Nonce);
                            if !ours && report.sos {
                                // The node could not decrypt our last frame: send it once more on the new SPAN.
                                let last = {
                                    let mut rt = self.inner.runtime.lock().expect("lock");
                                    rt.get_mut(&node).and_then(|r| r.last_s2.take())
                                };
                                if let Some((bytes, key, at)) = last {
                                    if at.elapsed() < Duration::from_secs(3) {
                                        debug!(node, "re-sending after an S2 resync");
                                        self.send_detached(node, bytes, TxSecurity::S2(key));
                                    }
                                }
                            }
                        }
                        Err(e) => debug!(node, "nonce report: {e}"),
                    }
                    return;
                }
                (0x9f, Some(0x03)) => {
                    let r = if ac.rx_status.multicast() {
                        self.s2(node)
                            .lock()
                            .expect("lock")
                            .decrypt_multicast(node, &frame)
                    } else {
                        self.s2(node).lock().expect("lock").decrypt(node, &frame)
                    };
                    match r {
                        Ok(d) => {
                            security = Some(RxSecurity::S2(d.key));
                            frame = d.command;
                        }
                        Err(zwave_security::Error::Duplicate) => return,
                        Err(e) => {
                            debug!(node, "S2 decryption failed: {e}; asking for a resync");
                            let mos = ac.rx_status.multicast();
                            let report =
                                self.s2(node).lock().expect("lock").nonce_report(node, mos);
                            if let Ok(r) = report {
                                self.transmit_logged(node, r).await;
                            }
                            return;
                        }
                    }
                }
                (0x98, Some(0x40)) => {
                    let report = self.inner.s0.lock().expect("lock").nonce_report(node);
                    if let Ok(r) = report {
                        self.transmit_logged(node, r).await;
                    }
                    return;
                }
                (0x98, Some(0x80)) => {
                    if self
                        .inner
                        .s0
                        .lock()
                        .expect("lock")
                        .handle_nonce_report(node, &frame)
                        .is_ok()
                    {
                        self.deliver(node, Inbound::S0Nonce);
                    }
                    return;
                }
                (0x98, Some(c @ (0x81 | 0xc1))) => {
                    let r = self
                        .inner
                        .s0
                        .lock()
                        .expect("lock")
                        .decapsulate(node, &frame, false);
                    if c == 0xc1 {
                        let report = self.inner.s0.lock().expect("lock").nonce_report(node);
                        if let Ok(r) = report {
                            self.transmit_logged(node, r).await;
                        }
                    }
                    match r {
                        Ok(S0Payload::Complete(cmd)) => {
                            security = Some(RxSecurity::S0);
                            frame = cmd;
                        }
                        Ok(S0Payload::FirstHalf) => return,
                        Err(e) => {
                            debug!(node, "S0 decapsulation failed: {e}");
                            return;
                        }
                    }
                }
                (multi_channel::ID8, Some(multi_channel::CommandEncapsulation::ID)) => {
                    match multi_channel::CommandEncapsulation::parse(&frame[2..]) {
                        Ok(e) => {
                            endpoint = e.source_end_point;
                            frame = e.command;
                        }
                        Err(e) => {
                            debug!(node, "{e}");
                            return;
                        }
                    }
                }
                (supervision::ID8, Some(supervision::Get::ID)) => {
                    match supervision::Get::parse(&frame[2..]) {
                        Ok(g) => {
                            supervision = Some((g.session_id, g.status_updates));
                            frame = g.command;
                        }
                        Err(e) => {
                            debug!(node, "{e}");
                            return;
                        }
                    }
                }
                _ => break,
            }
        }
        let command = match Command::parse(&frame) {
            Ok(c) => c,
            Err(e) => {
                debug!(node, "{e}");
                return;
            }
        };
        // A secure node's secure command classes are only accepted with security.
        let cc = command.cc();
        if security.is_none() {
            let must_be_secure = self
                .with_node(node, |r| {
                    r.secure_ccs_known
                        && r.endpoints
                            .get(&endpoint)
                            .and_then(|e| e.ccs.get(&cc))
                            .is_some_and(|s| s.secure)
                })
                .unwrap_or(false);
            if must_be_secure {
                warn!(
                    node,
                    cc, "dropped a secure command class sent without security"
                );
                return;
            }
        }
        if let Some((session, _)) = supervision {
            let report: Command = supervision::Report {
                more_status_updates: false,
                wake_up_request: false,
                session_id: session,
                status: 0xff,
                duration: Some(0),
            }
            .into();
            let sec = match security {
                Some(RxSecurity::S2(k)) => TxSecurity::S2(k),
                Some(RxSecurity::S0) => TxSecurity::S0 { temp: false },
                None => TxSecurity::Plain,
            };
            let bytes = wrap_endpoint(report.encode(), endpoint);
            self.send_detached(node, bytes, sec);
        }
        self.on_command(node, endpoint, command, security).await;
    }

    async fn on_command(
        &self,
        node: NodeId,
        endpoint: u8,
        command: Command,
        security: Option<RxSecurity>,
    ) {
        {
            let mut rt = self.inner.runtime.lock().expect("lock");
            let r = rt.entry(node).or_default();
            r.last_activity = Some(Instant::now());
        }
        let listening = self
            .with_node(node, |r| !r.protocol.sleeping())
            .unwrap_or(true);
        if listening && self.status(node) != NodeStatus::Alive {
            self.set_status(node, NodeStatus::Alive);
        }
        if let Command::WakeUp(wake_up::Command::Notification(_)) = &command {
            self.on_wake_up(node);
        }
        if let Command::DeviceResetLocally(_) = &command {
            warn!(
                node,
                "the node was reset locally; remove it with remove_failed_node"
            );
        }
        let updates = values::from_command(node, endpoint, &command);
        let taken = self.deliver(
            node,
            Inbound::Command {
                endpoint,
                command: command.clone(),
                security,
            },
        );
        if updates.is_empty() {
            if !taken {
                self.emit(Event::Command {
                    node,
                    endpoint,
                    command,
                });
            }
        } else {
            for u in updates {
                self.emit(Event::Value(u));
            }
        }
    }

    fn on_wake_up(&self, node: NodeId) {
        self.set_status(node, NodeStatus::Awake);
        let start = {
            let mut rt = self.inner.runtime.lock().expect("lock");
            let r = rt.entry(node).or_default();
            r.last_activity = Some(Instant::now());
            !std::mem::replace(&mut r.sleeper, true)
        };
        if start {
            tokio::spawn(self.clone().sleeper(node));
        }
    }

    /// Sends a woken node back to sleep once it has been idle.
    async fn sleeper(self, node: NodeId) {
        let idle = self.inner.cfg.awake_idle;
        loop {
            tokio::time::sleep(idle / 2).await;
            let ready = {
                let rt = self.inner.runtime.lock().expect("lock");
                let Some(r) = rt.get(&node) else { return };
                r.inflight == 0
                    && r.wake_tickets.is_empty()
                    && r.last_activity.is_none_or(|t| t.elapsed() >= idle)
            };
            if ready {
                break;
            }
        }
        let nmi: Command = wake_up::NoMoreInformation {}.into();
        if let Err(e) = self.send_now(node, 0, &nmi).await {
            debug!(node, "No More Information: {e}");
        }
        {
            let mut rt = self.inner.runtime.lock().expect("lock");
            if let Some(r) = rt.get_mut(&node) {
                r.sleeper = false;
            }
        }
        self.set_status(node, NodeStatus::Asleep);
    }

    /// Marks a just-included sleeping node awake so its interview can run.
    pub(crate) fn assume_awake(&self, node: NodeId) {
        if self
            .with_node(node, |r| r.protocol.sleeping())
            .unwrap_or(false)
        {
            self.on_wake_up(node);
        } else {
            self.set_status(node, NodeStatus::Alive);
        }
    }

    // --------------------------------------------------------------- outbound

    /// Sends a frame without waiting for its result (replies from the dispatcher).
    fn send_detached(&self, node: NodeId, bytes: Vec<u8>, sec: TxSecurity) {
        let c = self.clone();
        tokio::spawn(async move {
            if let Err(e) = c.send_bytes(node, bytes, sec).await {
                debug!(node, "reply not sent: {e}");
            }
        });
    }

    async fn transmit_logged(&self, node: NodeId, frame: Vec<u8>) {
        if let Err(e) = self.transmit(node, frame).await {
            debug!(node, "transmit: {e}");
        }
    }

    /// One SendData (or a Transport Service train) with the controller's callback.
    pub(crate) async fn transmit(&self, node: NodeId, frame: Vec<u8>) -> Result<(), Error> {
        if frame.len() > MAX_SEND_PAYLOAD {
            let session = self.inner.transport_session.fetch_add(1, Ordering::Relaxed) & 0x0f;
            for seg in
                transport_service::segment(&frame, session, transport_service::DEFAULT_SEGMENT)
            {
                self.send_data(node, seg).await?;
            }
            return Ok(());
        }
        self.send_data(node, frame).await
    }

    async fn send_data(&self, node: NodeId, frame: Vec<u8>) -> Result<(), Error> {
        let api = &self.inner.api;
        let cb = api.next_callback_id();
        let req = Request::with_callback(
            message::send_data(
                node,
                self.inner.nid,
                &frame,
                if crate::is_long_range(node) {
                    tx_options::ACK
                } else {
                    tx_options::DEFAULT
                },
                cb,
            ),
            Duration::from_secs(10),
            cb,
            Duration::from_secs(65),
        );
        let ex = api.execute(req).await?;
        let resp = ex
            .response
            .ok_or(Error::Controller("no SendData response".into()))?;
        if resp.payload.first() != Some(&1) {
            return Err(Error::TransmitFailed(
                node,
                "controller refused SendData".into(),
            ));
        }
        let cbf = ex
            .callback
            .ok_or(Error::Controller("no SendData callback".into()))?;
        let res =
            SendDataResult::parse(&cbf.payload).map_err(|e| Error::Controller(e.to_string()))?;
        let listening = self
            .with_node(node, |r| !r.protocol.sleeping())
            .unwrap_or(true);
        match res.status {
            TransmitStatus::Ok => {
                if let Some(rep) = res.report {
                    self.on_route(node, rep.repeaters, rep.ack_rssi);
                }
                if listening && self.status(node) == NodeStatus::Dead {
                    self.set_status(node, NodeStatus::Alive);
                }
                Ok(())
            }
            TransmitStatus::NoAck => {
                if listening {
                    self.set_status(node, NodeStatus::Dead);
                } else {
                    self.set_status(node, NodeStatus::Asleep);
                }
                Err(Error::NoAck(node))
            }
            s => Err(Error::TransmitFailed(node, format!("{s:?}"))),
        }
    }

    fn on_route(&self, node: NodeId, repeaters: Vec<NodeId>, rssi: Option<i8>) {
        let changed = {
            let mut rt = self.inner.runtime.lock().expect("lock");
            let r = rt.entry(node).or_default();
            let changed = r
                .last_route
                .as_ref()
                .map(|(rep, _)| rep != &repeaters)
                .unwrap_or(true);
            r.last_route = Some((repeaters.clone(), rssi));
            changed
        };
        if changed {
            self.emit(Event::Route {
                node,
                repeaters,
                rssi,
            });
        }
    }

    /// Resolves the security for a command class to a node's endpoint.
    fn resolve(&self, node: NodeId, endpoint: u8, cc: u16, sec: TxSecurity) -> TxSecurity {
        if sec != TxSecurity::Auto {
            return sec;
        }
        let Some((needs, s2, s0)) = self.with_node(node, |r| {
            (r.needs_security(endpoint, cc), r.highest_s2(), r.s0_only())
        }) else {
            return TxSecurity::Plain;
        };
        match (needs, s2, s0) {
            (true, Some(c), _) => TxSecurity::S2(KeyId::Class(c)),
            (true, None, true) => TxSecurity::S0 { temp: false },
            _ => TxSecurity::Plain,
        }
    }

    /// Encrypts as needed and transmits, holding the node's lock so nonce
    /// exchanges for one node never interleave.
    pub(crate) async fn send_bytes(
        &self,
        node: NodeId,
        bytes: Vec<u8>,
        sec: TxSecurity,
    ) -> Result<(), Error> {
        let lock = self.node_lock(node);
        let _g = lock.lock().await;
        match sec {
            TxSecurity::Plain | TxSecurity::Auto => self.transmit(node, bytes).await,
            TxSecurity::S2(key) => {
                let frame = self.s2_encrypt(node, &bytes, key).await?;
                {
                    let mut rt = self.inner.runtime.lock().expect("lock");
                    rt.entry(node).or_default().last_s2 = Some((bytes, key, Instant::now()));
                }
                self.transmit(node, frame).await
            }
            TxSecurity::S0 { temp } => self.s0_send(node, &bytes, temp).await,
        }
    }

    async fn s2_encrypt(&self, node: NodeId, bytes: &[u8], key: KeyId) -> Result<Vec<u8>, Error> {
        for attempt in 0..3 {
            let r = self.s2(node).lock().expect("lock").encrypt(
                node,
                bytes,
                &EncryptOptions {
                    key: Some(key),
                    ..Default::default()
                },
            );
            match r {
                Ok(f) => return Ok(f),
                Err(zwave_security::Error::NoSpan) if attempt < 2 => {
                    let (_g, rx) = self.wait_for(node, true, |i| matches!(i, Inbound::S2Nonce));
                    let get = self
                        .s2(node)
                        .lock()
                        .expect("lock")
                        .nonce_get(node)
                        .map_err(|e| Error::Security(e.to_string()))?;
                    self.transmit(node, get).await?;
                    tokio::time::timeout(self.inner.cfg.nonce_timeout, rx)
                        .await
                        .map_err(|_| Error::Timeout(node))?
                        .map_err(|_| Error::Stopped)?;
                }
                Err(e) => return Err(Error::Security(e.to_string())),
            }
        }
        Err(Error::Security("no S2 nonce".into()))
    }

    async fn s0_nonce(&self, node: NodeId) -> Result<(), Error> {
        let (_g, rx) = self.wait_for(node, true, |i| matches!(i, Inbound::S0Nonce));
        self.transmit(node, S0Manager::nonce_get()).await?;
        tokio::time::timeout(self.inner.cfg.nonce_timeout, rx)
            .await
            .map_err(|_| Error::Timeout(node))?
            .map_err(|_| Error::Stopped)?;
        Ok(())
    }

    async fn s0_send(&self, node: NodeId, bytes: &[u8], temp: bool) -> Result<(), Error> {
        let parts = match self.inner.s0.lock().expect("lock").split(bytes) {
            Some(p) => p.to_vec(),
            None => vec![(bytes.to_vec(), 0u8)],
        };
        let n = parts.len();
        let mut have_nonce = false;
        for (i, (part, control)) in parts.into_iter().enumerate() {
            let more = i + 1 < n;
            if !have_nonce {
                self.s0_nonce(node).await?;
            }
            // With a second part to come, the receiver answers the first with a nonce.
            let next = if more {
                Some(self.wait_for(node, true, |i| matches!(i, Inbound::S0Nonce)))
            } else {
                None
            };
            let frame = self
                .inner
                .s0
                .lock()
                .expect("lock")
                .encapsulate_part(node, &part, control, temp, more)
                .map_err(|e| Error::Security(e.to_string()))?;
            self.transmit(node, frame).await?;
            if let Some((_g, rx)) = next {
                tokio::time::timeout(self.inner.cfg.nonce_timeout, rx)
                    .await
                    .map_err(|_| Error::Timeout(node))?
                    .map_err(|_| Error::Stopped)?;
                have_nonce = true;
            }
        }
        Ok(())
    }

    /// Waits until a sleeping node is awake.
    pub(crate) async fn wait_awake(&self, node: NodeId) {
        let rx = {
            let sleeping = self
                .with_node(node, |r| r.protocol.sleeping())
                .unwrap_or(false);
            let mut rt = self.inner.runtime.lock().expect("lock");
            let r = rt.entry(node).or_default();
            if !sleeping || r.status == NodeStatus::Awake {
                return;
            }
            let (tx, rx) = oneshot::channel();
            r.wake_tickets.push(tx);
            rx
        };
        debug!(node, "waiting for the node to wake up");
        let _ = rx.await;
    }

    /// Sends already encoded command bytes for a command class, with the
    /// security the node's record asks for, without waiting for a sleeping node.
    pub(crate) async fn send_encoded(
        &self,
        node: NodeId,
        endpoint: u8,
        cc: u16,
        bytes: Vec<u8>,
    ) -> Result<(), Error> {
        let sec = self.resolve(node, endpoint, cc, TxSecurity::Auto);
        self.send_bytes(node, wrap_endpoint(bytes, endpoint), sec)
            .await
    }

    /// Sends a command to an endpoint now, without waiting for a sleeping node.
    pub(crate) async fn send_now(
        &self,
        node: NodeId,
        endpoint: u8,
        cmd: &Command,
    ) -> Result<(), Error> {
        let sec = self.resolve(node, endpoint, cmd.cc(), TxSecurity::Auto);
        self.send_bytes(node, wrap_endpoint(cmd.encode(), endpoint), sec)
            .await
    }

    /// Sends a command to a node's endpoint. For a sleeping node this waits
    /// until the node wakes up.
    pub async fn send(&self, node: NodeId, endpoint: u8, cmd: &Command) -> Result<(), Error> {
        self.with_node(node, |_| ())
            .ok_or(Error::UnknownNode(node))?;
        let _a = self.activity(node);
        self.wait_awake(node).await;
        self.send_now(node, endpoint, cmd).await
    }

    /// Sends a command with Supervision and returns the node's status
    /// (`0xFF` success, `0x01` working; failure and no support are errors).
    pub async fn send_supervised(
        &self,
        node: NodeId,
        endpoint: u8,
        cmd: &Command,
    ) -> Result<u8, Error> {
        let session = self
            .inner
            .supervision_session
            .fetch_add(1, Ordering::Relaxed)
            & 0x3f;
        let get: Command = supervision::Get {
            status_updates: false,
            session_id: session,
            command: cmd.encode(),
        }
        .into();
        let reply = self
            .request_with(node, endpoint, get, cmd.cc(), self.inner.cfg.request_timeout, move |c| {
                matches!(c, Command::Supervision(supervision::Command::Report(r)) if r.session_id == session)
            })
            .await?;
        let Command::Supervision(supervision::Command::Report(r)) = reply else {
            unreachable!()
        };
        match r.status {
            0xff | 0x01 => Ok(r.status),
            0x00 => Err(Error::Controller(format!(
                "node {node} does not support the command"
            ))),
            _ => Err(Error::Controller(format!("node {node} failed the command"))),
        }
    }

    /// Sends a command and waits for the reply `expect` accepts.
    pub async fn request(
        &self,
        node: NodeId,
        endpoint: u8,
        cmd: Command,
        expect: impl Fn(&Command) -> bool + Send + Sync + 'static,
    ) -> Result<Command, Error> {
        let cc = cmd.cc();
        self.request_with(
            node,
            endpoint,
            cmd,
            cc,
            self.inner.cfg.request_timeout,
            expect,
        )
        .await
    }

    /// Like [`request`](Self::request), extracting the reply with `f`.
    pub(crate) async fn req<T: Send + 'static>(
        &self,
        node: NodeId,
        endpoint: u8,
        cmd: impl Into<Command>,
        f: impl Fn(&Command) -> Option<T> + Send + Sync + 'static,
    ) -> Result<T, Error> {
        let f = Arc::new(f);
        let m = f.clone();
        let c = self
            .request(node, endpoint, cmd.into(), move |c| m(c).is_some())
            .await?;
        Ok(f(&c).expect("matched"))
    }

    async fn request_with(
        &self,
        node: NodeId,
        endpoint: u8,
        cmd: Command,
        security_cc: u16,
        timeout: Duration,
        expect: impl Fn(&Command) -> bool + Send + Sync + 'static,
    ) -> Result<Command, Error> {
        self.with_node(node, |_| ())
            .ok_or(Error::UnknownNode(node))?;
        let _a = self.activity(node);
        self.wait_awake(node).await;
        let expect = Arc::new(expect);
        let sec = self.resolve(node, endpoint, security_cc, TxSecurity::Auto);
        let bytes = wrap_endpoint(cmd.encode(), endpoint);
        let mut last = Error::Timeout(node);
        for attempt in 0..2 {
            let e = expect.clone();
            let (_g, rx) = self.wait_for(node, false, move |i| matches!(i, Inbound::Command { endpoint: ep, command, .. } if *ep == endpoint && e(command)));
            match self.send_bytes(node, bytes.clone(), sec).await {
                Ok(()) => {}
                Err(err @ (Error::NoAck(_) | Error::TransmitFailed(..))) if attempt == 0 => {
                    last = err;
                    continue;
                }
                Err(err) => return Err(err),
            }
            match tokio::time::timeout(timeout, rx).await {
                Ok(Ok(Inbound::Command { command, .. })) => return Ok(command),
                Ok(_) => return Err(Error::Stopped),
                Err(_) => {
                    debug!(node, attempt, "no reply to {}", cmd.name());
                    last = Error::Timeout(node);
                }
            }
        }
        Err(last)
    }

    /// Sends a command with explicit security and waits for a reply (key exchange).
    pub(crate) async fn request_secured(
        &self,
        node: NodeId,
        bytes: Vec<u8>,
        sec: TxSecurity,
        timeout: Duration,
        expect: impl Fn(&Command, Option<RxSecurity>) -> bool + Send + 'static,
    ) -> Result<(Command, Option<RxSecurity>), Error> {
        let (_g, rx) = self.wait_for(node, false, move |i| matches!(i, Inbound::Command { command, security, .. } if expect(command, *security)));
        self.send_bytes(node, bytes, sec).await?;
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Inbound::Command {
                command, security, ..
            })) => Ok((command, security)),
            Ok(_) => Err(Error::Stopped),
            Err(_) => Err(Error::Timeout(node)),
        }
    }

    /// Sets a value (switches, levels) and refreshes it.
    pub async fn set_value(
        &self,
        id: &values::ValueId,
        value: &values::ValueKind,
    ) -> Result<(), Error> {
        let cmd = values::set_command(id, value)
            .ok_or(Error::Controller(format!("{} is not writable", id.key())))?;
        let supervised = self
            .with_node(id.node, |r| {
                r.endpoints
                    .get(&id.endpoint)
                    .or(Some(r.root()))
                    .is_some_and(|e| e.supports(supervision::ID))
            })
            .unwrap_or(false);
        if supervised {
            self.send_supervised(id.node, id.endpoint, &cmd).await?;
        } else {
            self.send(id.node, id.endpoint, &cmd).await?;
        }
        if let Some(get) = values::get_command(id) {
            let cc = get.cc();
            let _ = self
                .request(id.node, id.endpoint, get, move |c| {
                    c.cc() == cc && c.name().ends_with(".report")
                })
                .await;
        }
        Ok(())
    }

    /// Applies configuration settings keyed as in the node's device file
    /// (`"3"`, `"41[0xf00]"`): partial parameters are read, merged and written
    /// as one value; each written parameter is read back so its value is
    /// reported.
    pub async fn configure(
        &self,
        node: NodeId,
        config: &crate::device_config::DeviceConfig,
        settings: &[(String, i64)],
    ) -> Result<(), Error> {
        use crate::device_config::plan_settings;
        use zwave_cc::configuration;
        let bad = |e: crate::device_config::SettingsError| Error::Controller(e.to_string());
        let first = plan_settings(config, settings, &|_| None).map_err(bad)?;
        let mut current: HashMap<u16, u32> = HashMap::new();
        for w in first.iter().filter(|w| w.needs_current) {
            let p = w.parameter;
            let r = self
                .req(
                    node,
                    0,
                    configuration::Get { parameter: p as u8 },
                    move |c| match c {
                        Command::Configuration(configuration::Command::Report(r))
                            if u16::from(r.parameter) == p =>
                        {
                            Some((r.value, r.size))
                        }
                        _ => None,
                    },
                )
                .await?;
            let raw = match r.1 {
                1 => u32::from(r.0 as u8),
                2 => u32::from(r.0 as u16),
                _ => r.0 as u32,
            };
            current.insert(p, raw);
        }
        let writes = plan_settings(config, settings, &|p| current.get(&p).copied()).map_err(bad)?;
        let supervised = self
            .with_node(node, |r| r.root().supports(supervision::ID))
            .unwrap_or(false);
        for w in writes {
            let set = w.command();
            if supervised {
                self.send_supervised(node, 0, &set).await?;
            } else {
                self.send(node, 0, &set).await?;
            }
            let p = w.parameter;
            let _ = self
                .req(node, 0, configuration::Get { parameter: p as u8 }, move |c| {
                    matches!(c, Command::Configuration(configuration::Command::Report(r)) if u16::from(r.parameter) == p).then_some(())
                })
                .await;
        }
        Ok(())
    }

    /// Pings a node (No Operation); true if it acknowledged.
    pub async fn ping(&self, node: NodeId) -> bool {
        self.transmit(node, vec![0x00]).await.is_ok()
    }

    pub(crate) async fn fetch_protocol_info(&self, node: NodeId) -> Result<(), Error> {
        let r = self
            .inner
            .api
            .request(
                DataFrame::request(F::GET_NODE_PROTOCOL_INFO, self.node_payload(node)),
                Duration::from_secs(2),
            )
            .await?;
        let p =
            NodeProtocolInfo::parse(&r.payload).map_err(|e| Error::Controller(e.to_string()))?;
        let Some(p) = p else {
            return Err(Error::UnknownNode(node));
        };
        self.update_node(node, |r| {
            r.protocol = ProtocolInfo {
                listening: p.listening,
                flirs: p.flirs_1000 || p.flirs_250,
                routing: p.routing,
                controller: p.controller,
                basic: p.basic,
                generic: p.generic,
                specific: p.specific,
            };
            if r.stage < InterviewStage::ProtocolInfo {
                r.stage = InterviewStage::ProtocolInfo;
            }
        })
    }

    /// The S2 state for a node: Long Range nodes have their own keys.
    pub(crate) fn s2(&self, node: NodeId) -> &Mutex<S2Manager> {
        if crate::is_long_range(node) {
            &self.inner.s2_lr
        } else {
            &self.inner.s2
        }
    }

    /// A node id as the Serial API's payload expects it.
    pub(crate) fn node_payload(&self, node: NodeId) -> Vec<u8> {
        let mut v = Vec::with_capacity(2);
        message::node_bytes(node, self.inner.nid, &mut v);
        v
    }

    /// Reads a node's neighbor table from the controller (no radio traffic).
    /// Long Range nodes talk to the controller directly and have none.
    pub async fn refresh_neighbors(&self, node: NodeId) -> Result<Vec<NodeId>, Error> {
        if crate::is_long_range(node) {
            return Ok(vec![]);
        }
        let r = self
            .inner
            .api
            .request(
                message::get_routing_info(node, self.inner.nid),
                Duration::from_secs(2),
            )
            .await?;
        let n = message::parse_routing_info(&r.payload)
            .map_err(|e| Error::Controller(e.to_string()))?;
        let own = self.info().node_id;
        let n: Vec<NodeId> = n.into_iter().filter(|&x| x != node).collect();
        let _ = own;
        self.update_node(node, |r| r.neighbors = n.clone())?;
        Ok(n)
    }

    /// The topology: neighbor tables and the last route to each node.
    pub fn topology(&self) -> Topology {
        let rt = self.inner.runtime.lock().expect("lock");
        let s = self.inner.store.lock().expect("store lock");
        Topology {
            controller: self.info().node_id,
            nodes: s
                .nodes
                .values()
                .map(|r| {
                    let route = rt.get(&r.id).and_then(|x| x.last_route.clone());
                    TopologyNode {
                        id: r.id,
                        neighbors: r.neighbors.clone(),
                        last_route: route.as_ref().map(|(rep, _)| rep.clone()),
                        last_rssi: route.and_then(|(_, rssi)| rssi),
                        listening: r.protocol.listening,
                    }
                })
                .collect(),
        }
    }
}

/// Wraps a command for an endpoint (Multi Channel encapsulation from the root).
pub(crate) fn wrap_endpoint(bytes: Vec<u8>, endpoint: u8) -> Vec<u8> {
    if endpoint == 0 {
        return bytes;
    }
    Command::from(multi_channel::CommandEncapsulation {
        source_end_point: 0,
        bit_address: false,
        destination_end_point: endpoint,
        command: bytes,
    })
    .encode()
}
