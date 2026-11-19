//! Typed Serial API messages: request builders and parsers for responses,
//! callbacks and unsolicited frames.
//!
//! Every parser takes the frame payload and returns `Err(Malformed)` rather
//! than panicking on short or inconsistent input; lengths inside a payload
//! are clamped to what the payload actually holds.

use crate::frame::{DataFrame, FrameType};
use crate::function::FunctionId as F;
use crate::NodeId;

/// A payload that does not have the shape its function id promises.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("malformed {0} payload")]
pub struct Malformed(pub &'static str);

type Result<T> = std::result::Result<T, Malformed>;

fn need(p: &[u8], n: usize, what: &'static str) -> Result<()> {
    if p.len() < n {
        Err(Malformed(what))
    } else {
        Ok(())
    }
}

/// Reads a node id of `len` bytes (1 classic, 2 Long Range) at `at`.
fn node_id(p: &[u8], at: usize, len: usize, what: &'static str) -> Result<NodeId> {
    need(p, at + len, what)?;
    Ok(if len == 2 {
        u16::from_be_bytes([p[at], p[at + 1]])
    } else {
        u16::from(p[at])
    })
}

/// Parses a command class list, with two-byte ids from `0xF1`, splitting
/// supported from controlled at the `0xEF` mark.
pub fn parse_cc_list(p: &[u8]) -> (Vec<u16>, Vec<u16>) {
    let mut supported = Vec::new();
    let mut controlled = Vec::new();
    let mut after_mark = false;
    let mut i = 0;
    while i < p.len() {
        let b = p[i];
        if b == 0xef {
            after_mark = true;
            i += 1;
            continue;
        }
        let cc = if b >= 0xf1 {
            let Some(&lo) = p.get(i + 1) else { break };
            i += 2;
            (u16::from(b) << 8) | u16::from(lo)
        } else {
            i += 1;
            u16::from(b)
        };
        if after_mark {
            controlled.push(cc);
        } else {
            supported.push(cc);
        }
    }
    (supported, controlled)
}

/// Encodes a command class list (no controlled part).
pub fn encode_cc_list(ccs: &[u16], out: &mut Vec<u8>) {
    for &cc in ccs {
        if cc > 0xff {
            out.extend_from_slice(&cc.to_be_bytes());
        } else {
            out.push(cc as u8);
        }
    }
}

/// Parses a node bit mask (bit 0 of byte 0 is node `first`).
pub fn parse_node_mask(mask: &[u8], first: NodeId) -> Vec<NodeId> {
    let mut v = Vec::new();
    for (i, b) in mask.iter().enumerate() {
        for bit in 0..8 {
            if b & (1 << bit) != 0 {
                v.push(first + (i * 8 + bit) as NodeId);
            }
        }
    }
    v
}

/// Encodes a node bit mask of `len` bytes starting at node 1.
pub fn encode_node_mask(nodes: &[NodeId], len: usize) -> Vec<u8> {
    let mut m = vec![0u8; len];
    for &n in nodes {
        let i = usize::from(n.saturating_sub(1));
        if n >= 1 && i / 8 < len {
            m[i / 8] |= 1 << (i % 8);
        }
    }
    m
}

/// Response to `GET_SERIAL_API_CAPABILITIES`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerialApiCapabilities {
    /// Firmware (application) version, major.
    pub firmware_major: u8,
    /// Firmware (application) version, minor.
    pub firmware_minor: u8,
    /// Manufacturer id of the stick.
    pub manufacturer_id: u16,
    /// Product type.
    pub product_type: u16,
    /// Product id.
    pub product_id: u16,
    /// Bit mask of supported function ids (bit `n-1` for function `n`).
    pub functions: [u8; 32],
}

impl SerialApiCapabilities {
    /// Parses the response payload.
    pub fn parse(p: &[u8]) -> Result<Self> {
        need(p, 8, "serial API capabilities")?;
        let mut functions = [0u8; 32];
        let mask = &p[8..p.len().min(40)];
        functions[..mask.len()].copy_from_slice(mask);
        Ok(SerialApiCapabilities {
            firmware_major: p[0],
            firmware_minor: p[1],
            manufacturer_id: u16::from_be_bytes([p[2], p[3]]),
            product_type: u16::from_be_bytes([p[4], p[5]]),
            product_id: u16::from_be_bytes([p[6], p[7]]),
            functions,
        })
    }

    /// Encodes the payload (simulators).
    pub fn encode(&self) -> Vec<u8> {
        let mut v = vec![self.firmware_major, self.firmware_minor];
        v.extend_from_slice(&self.manufacturer_id.to_be_bytes());
        v.extend_from_slice(&self.product_type.to_be_bytes());
        v.extend_from_slice(&self.product_id.to_be_bytes());
        v.extend_from_slice(&self.functions);
        v
    }

    /// Whether the controller implements a function.
    pub fn supports(&self, f: F) -> bool {
        let i = usize::from(f.0.wrapping_sub(1));
        f.0 != 0 && self.functions[i / 8] & (1 << (i % 8)) != 0
    }

    /// A function mask with these functions set.
    pub fn mask(functions: &[F]) -> [u8; 32] {
        let mut m = [0u8; 32];
        for f in functions {
            if f.0 != 0 {
                let i = usize::from(f.0 - 1);
                m[i / 8] |= 1 << (i % 8);
            }
        }
        m
    }
}

/// Response to `GET_INIT_DATA`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitData {
    /// Serial API version.
    pub api_version: u8,
    /// Capability flags.
    pub capabilities: u8,
    /// Nodes in the network.
    pub nodes: Vec<NodeId>,
    /// Chip type (e.g. 0x07 for 700 series, 0x08 for 800 series).
    pub chip_type: Option<u8>,
    /// Chip version.
    pub chip_version: Option<u8>,
}

impl InitData {
    /// The controller runs the end node (slave) API, not a controller API.
    pub fn is_end_node(&self) -> bool {
        self.capabilities & 0x01 != 0
    }
    /// The controller is a secondary controller.
    pub fn is_secondary(&self) -> bool {
        self.capabilities & 0x04 != 0
    }
    /// The controller is the static update controller (SIS).
    pub fn is_sis(&self) -> bool {
        self.capabilities & 0x08 != 0
    }

    /// Parses the response payload.
    pub fn parse(p: &[u8]) -> Result<Self> {
        need(p, 3, "init data")?;
        let len = usize::from(p[2]).min(p.len() - 3);
        let nodes = parse_node_mask(&p[3..3 + len], 1);
        Ok(InitData {
            api_version: p[0],
            capabilities: p[1],
            nodes,
            chip_type: p.get(3 + len).copied(),
            chip_version: p.get(4 + len).copied(),
        })
    }

    /// Encodes the payload (simulators).
    pub fn encode(&self) -> Vec<u8> {
        let mut v = vec![self.api_version, self.capabilities, 29];
        v.extend_from_slice(&encode_node_mask(&self.nodes, 29));
        v.extend(self.chip_type);
        v.extend(self.chip_version);
        v
    }
}

/// Response to `GET_CONTROLLER_VERSION`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerVersion {
    /// The library version string, e.g. `Z-Wave 7.21`.
    pub version: String,
    /// The library type (1 static controller, 7 bridge controller, ...).
    pub library_type: u8,
}

impl ControllerVersion {
    /// Parses the response payload.
    pub fn parse(p: &[u8]) -> Result<Self> {
        let nul = p
            .iter()
            .position(|&b| b == 0)
            .ok_or(Malformed("controller version"))?;
        let version = String::from_utf8_lossy(&p[..nul]).trim().to_string();
        Ok(ControllerVersion {
            version,
            library_type: p.get(nul + 1).copied().unwrap_or(0),
        })
    }

    /// Encodes the payload (simulators).
    pub fn encode(&self) -> Vec<u8> {
        let mut v = self.version.as_bytes().to_vec();
        v.push(0);
        v.push(self.library_type);
        v
    }

    /// A readable library type.
    pub fn library_name(&self) -> &'static str {
        match self.library_type {
            0x01 => "static controller",
            0x02 => "controller",
            0x03 => "enhanced end node",
            0x04 => "end node",
            0x05 => "installer",
            0x06 => "routing end node",
            0x07 => "bridge controller",
            0x08 => "device under test",
            _ => "unknown",
        }
    }
}

/// Response to `GET_CONTROLLER_ID`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControllerId {
    /// Home id.
    pub home_id: u32,
    /// The controller's own node id.
    pub node_id: NodeId,
}

impl ControllerId {
    /// Parses the response payload (`node_id_len` 1 or 2).
    pub fn parse(p: &[u8], node_id_len: usize) -> Result<Self> {
        need(p, 4 + node_id_len, "controller id")?;
        Ok(ControllerId {
            home_id: u32::from_be_bytes([p[0], p[1], p[2], p[3]]),
            node_id: node_id(p, 4, node_id_len, "controller id")?,
        })
    }

    /// Encodes the payload (simulators, classic node ids).
    pub fn encode(&self) -> Vec<u8> {
        let mut v = self.home_id.to_be_bytes().to_vec();
        v.push(self.node_id as u8);
        v
    }
}

/// Response to `GET_NODE_PROTOCOL_INFO`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeProtocolInfo {
    /// Always listening (mains powered).
    pub listening: bool,
    /// Routing node.
    pub routing: bool,
    /// Frequently listening, woken by a beam every 1000 ms.
    pub flirs_1000: bool,
    /// Frequently listening, woken by a beam every 250 ms.
    pub flirs_250: bool,
    /// The node supports beaming.
    pub beaming: bool,
    /// The node is a controller.
    pub controller: bool,
    /// The protocol security flag.
    pub security: bool,
    /// Protocol version code (bits 0-2 of the capability byte; 3 for current devices).
    pub protocol_version: u8,
    /// Supported speeds: bit 0 9.6k, bit 1 40k, bit 2 100k.
    pub speeds: u8,
    /// Basic device class.
    pub basic: u8,
    /// Generic device class.
    pub generic: u8,
    /// Specific device class.
    pub specific: u8,
}

impl NodeProtocolInfo {
    /// Whether the node is reachable at any time (listening or FLiRS).
    pub fn reachable(&self) -> bool {
        self.listening || self.flirs_250 || self.flirs_1000
    }

    /// Parses the response payload. An all-zero response means "no such node".
    pub fn parse(p: &[u8]) -> Result<Option<Self>> {
        need(p, 6, "node protocol info")?;
        if p[3] == 0 && p[4] == 0 {
            return Ok(None);
        }
        let cap = p[0];
        let sec = p[1];
        let max_baud = (cap >> 3) & 0x07;
        let mut speeds = match max_baud {
            0b010 => 0b011,
            0b001 => 0b001,
            _ => 0b001,
        };
        if p[2] & 0x01 != 0 {
            speeds |= 0b100;
        }
        Ok(Some(NodeProtocolInfo {
            listening: cap & 0x80 != 0,
            routing: cap & 0x40 != 0,
            flirs_1000: sec & 0x40 != 0,
            flirs_250: sec & 0x20 != 0,
            beaming: sec & 0x10 != 0,
            controller: sec & 0x02 != 0,
            security: sec & 0x01 != 0,
            protocol_version: cap & 0x07,
            speeds,
            basic: p[3],
            generic: p[4],
            specific: p[5],
        }))
    }

    /// Encodes the payload (simulators).
    pub fn encode(&self) -> Vec<u8> {
        let cap = (if self.listening { 0x80 } else { 0 })
            | (if self.routing { 0x40 } else { 0 })
            | (0b010 << 3)
            | (self.protocol_version & 0x07);
        let sec = (if self.flirs_1000 { 0x40 } else { 0 })
            | (if self.flirs_250 { 0x20 } else { 0 })
            | (if self.beaming { 0x10 } else { 0 })
            | (if self.controller { 0x02 } else { 0 })
            | (if self.security { 0x01 } else { 0 })
            | 0x04;
        vec![
            cap,
            sec,
            if self.speeds & 0b100 != 0 { 1 } else { 0 },
            self.basic,
            self.generic,
            self.specific,
        ]
    }
}

/// A node information frame: device classes and command classes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NodeInfo {
    /// Basic device class.
    pub basic: u8,
    /// Generic device class.
    pub generic: u8,
    /// Specific device class.
    pub specific: u8,
    /// Supported command classes.
    pub supported: Vec<u16>,
    /// Controlled command classes (after the `0xEF` mark).
    pub controlled: Vec<u16>,
}

impl NodeInfo {
    /// Parses `[basic, generic, specific, ccs..]`.
    pub fn parse(p: &[u8]) -> Result<Self> {
        need(p, 3, "node info")?;
        let (supported, controlled) = parse_cc_list(&p[3..]);
        Ok(NodeInfo {
            basic: p[0],
            generic: p[1],
            specific: p[2],
            supported,
            controlled,
        })
    }

    /// Encodes `[basic, generic, specific, ccs..]`.
    pub fn encode(&self) -> Vec<u8> {
        let mut v = vec![self.basic, self.generic, self.specific];
        encode_cc_list(&self.supported, &mut v);
        if !self.controlled.is_empty() {
            v.push(0xef);
            encode_cc_list(&self.controlled, &mut v);
        }
        v
    }
}

/// Reception flags of a received frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RxStatus(pub u8);

impl RxStatus {
    /// Received at low power.
    pub fn low_power(self) -> bool {
        self.0 & 0x02 != 0
    }
    /// Sent as broadcast.
    pub fn broadcast(self) -> bool {
        self.0 & 0x0c == 0x04
    }
    /// Sent as multicast.
    pub fn multicast(self) -> bool {
        self.0 & 0x0c == 0x08
    }
    /// The frame was routed through repeaters.
    pub fn routed(self) -> bool {
        self.0 & 0x01 != 0
    }
}

/// A command class frame received from a node (`APPLICATION_COMMAND_HANDLER`
/// or its bridge variant).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationCommand {
    /// Reception flags.
    pub rx_status: RxStatus,
    /// Sending node.
    pub source: NodeId,
    /// Destination (the bridge variant can address virtual nodes); `None` for the classic variant.
    pub destination: Option<NodeId>,
    /// The command class frame.
    pub command: Vec<u8>,
    /// Received signal strength, dBm, when the controller reports it.
    pub rssi: Option<i8>,
}

fn rssi(b: Option<&u8>) -> Option<i8> {
    // 0x7D..0x7F are "not available" / "below sensitivity" / "max power saturated".
    b.map(|&v| v as i8).filter(|v| *v < 0x7d)
}

impl ApplicationCommand {
    /// Parses an `APPLICATION_COMMAND_HANDLER` payload.
    pub fn parse(p: &[u8], node_id_len: usize) -> Result<Self> {
        need(p, 2 + node_id_len, "application command")?;
        let source = node_id(p, 1, node_id_len, "application command")?;
        let at = 1 + node_id_len;
        let len = usize::from(p[at]);
        let body = p
            .get(at + 1..at + 1 + len)
            .ok_or(Malformed("application command"))?;
        Ok(ApplicationCommand {
            rx_status: RxStatus(p[0]),
            source,
            destination: None,
            command: body.to_vec(),
            rssi: rssi(p.get(at + 1 + len)),
        })
    }

    /// Parses a `BRIDGE_APPLICATION_COMMAND_HANDLER` payload.
    pub fn parse_bridge(p: &[u8], node_id_len: usize) -> Result<Self> {
        need(p, 2 + 2 * node_id_len, "bridge application command")?;
        let destination = node_id(p, 1, node_id_len, "bridge application command")?;
        let source = node_id(
            p,
            1 + node_id_len,
            node_id_len,
            "bridge application command",
        )?;
        let at = 1 + 2 * node_id_len;
        let len = usize::from(p[at]);
        let body = p
            .get(at + 1..at + 1 + len)
            .ok_or(Malformed("bridge application command"))?;
        let mut rest = at + 1 + len;
        // Multicast node mask: length byte and mask.
        if let Some(&mlen) = p.get(rest) {
            rest += 1 + usize::from(mlen);
        }
        Ok(ApplicationCommand {
            rx_status: RxStatus(p[0]),
            source,
            destination: Some(destination),
            command: body.to_vec(),
            rssi: rssi(p.get(rest)),
        })
    }

    /// Encodes as an `APPLICATION_COMMAND_HANDLER` request frame (simulators, classic ids).
    pub fn to_frame(&self) -> DataFrame {
        let mut v = vec![
            self.rx_status.0,
            self.source as u8,
            self.command.len() as u8,
        ];
        v.extend_from_slice(&self.command);
        if let Some(r) = self.rssi {
            v.push(r as u8);
        }
        DataFrame::request(F::APPLICATION_COMMAND_HANDLER, v)
    }
}

/// `APPLICATION_UPDATE` frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplicationUpdate {
    /// A node information frame arrived (answer to Request Node Info, or the node sent it).
    NodeInfoReceived {
        /// The node.
        node: NodeId,
        /// Its node information.
        info: NodeInfo,
    },
    /// Request Node Info failed.
    NodeInfoRequestFailed,
    /// A SmartStart node asks to be included.
    SmartStartHomeIdReceived {
        /// Its NWI home id (from its DSK).
        nwi_home_id: [u8; 4],
        /// Long Range request.
        long_range: bool,
        /// Its node information.
        info: NodeInfo,
    },
    /// An already included SmartStart node powered up.
    SmartStartIncludedNodeInfo {
        /// Its NWI home id.
        nwi_home_id: [u8; 4],
    },
    /// Another controller added a node.
    NodeAdded {
        /// The node.
        node: NodeId,
    },
    /// Another controller removed a node.
    NodeRemoved {
        /// The node.
        node: NodeId,
    },
    /// The SUC changed.
    SucIdChanged,
    /// Anything else.
    Other {
        /// Update type.
        kind: u8,
    },
}

impl ApplicationUpdate {
    /// Parses an `APPLICATION_UPDATE` payload.
    pub fn parse(p: &[u8], node_id_len: usize) -> Result<Self> {
        need(p, 1, "application update")?;
        let kind = p[0];
        let rest = &p[1..];
        let node_info = |rest: &[u8]| -> Result<(NodeId, NodeInfo)> {
            let node = node_id(rest, 0, node_id_len, "node info update")?;
            let len_at = node_id_len;
            need(rest, len_at + 1, "node info update")?;
            let len = usize::from(rest[len_at]);
            let body = &rest[len_at + 1..rest.len().min(len_at + 1 + len)];
            Ok((node, NodeInfo::parse(body)?))
        };
        Ok(match kind {
            0x84 => {
                let (node, info) = node_info(rest)?;
                ApplicationUpdate::NodeInfoReceived { node, info }
            }
            0x81 => ApplicationUpdate::NodeInfoRequestFailed,
            0x85 | 0x87 => {
                // [remote node id, rx status, NWI home id (4), length, basic, generic, specific, ccs]
                let at = node_id_len + 1;
                need(rest, at + 5, "smartstart request")?;
                let nwi: [u8; 4] = rest[at..at + 4].try_into().expect("4 bytes");
                let body = &rest[at + 5..];
                let info = NodeInfo::parse(body).unwrap_or_default();
                ApplicationUpdate::SmartStartHomeIdReceived {
                    nwi_home_id: nwi,
                    long_range: kind == 0x87,
                    info,
                }
            }
            0x86 => {
                let at = node_id_len + 1;
                need(rest, at + 4, "smartstart node info")?;
                ApplicationUpdate::SmartStartIncludedNodeInfo {
                    nwi_home_id: rest[at..at + 4].try_into().expect("4 bytes"),
                }
            }
            0x40 => ApplicationUpdate::NodeAdded {
                node: node_id(rest, 0, node_id_len, "node added")?,
            },
            0x20 => ApplicationUpdate::NodeRemoved {
                node: node_id(rest, 0, node_id_len, "node removed")?,
            },
            0x10 => ApplicationUpdate::SucIdChanged,
            k => ApplicationUpdate::Other { kind: k },
        })
    }

    /// Encodes as a request frame (simulators, classic ids).
    pub fn to_frame(&self) -> DataFrame {
        let p = match self {
            ApplicationUpdate::NodeInfoReceived { node, info } => {
                let body = info.encode();
                let mut v = vec![0x84, *node as u8, body.len() as u8];
                v.extend_from_slice(&body);
                v
            }
            ApplicationUpdate::NodeInfoRequestFailed => vec![0x81, 0, 0],
            ApplicationUpdate::SmartStartHomeIdReceived {
                nwi_home_id,
                long_range,
                info,
            } => {
                let body = info.encode();
                let mut v = vec![if *long_range { 0x87 } else { 0x85 }, 0, 0];
                v.extend_from_slice(nwi_home_id);
                v.push(body.len().saturating_sub(3) as u8);
                v.extend_from_slice(&body);
                v
            }
            ApplicationUpdate::SmartStartIncludedNodeInfo { nwi_home_id } => {
                let mut v = vec![0x86, 0, 0];
                v.extend_from_slice(nwi_home_id);
                v
            }
            ApplicationUpdate::NodeAdded { node } => vec![0x40, *node as u8, 0],
            ApplicationUpdate::NodeRemoved { node } => vec![0x20, *node as u8, 0],
            ApplicationUpdate::SucIdChanged => vec![0x10, 0, 0],
            ApplicationUpdate::Other { kind } => vec![*kind, 0, 0],
        };
        DataFrame::request(F::APPLICATION_UPDATE, p)
    }
}

/// How a SendData ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransmitStatus {
    /// Delivered and acknowledged.
    Ok,
    /// Sent but not acknowledged (the node is asleep, out of range or gone).
    NoAck,
    /// The radio could not transmit (jammed channel).
    Fail,
    /// The controller was busy.
    NotIdle,
    /// No route to the node.
    NoRoute,
    /// Another value.
    Other(u8),
}

impl TransmitStatus {
    /// From the wire byte.
    pub fn from_u8(v: u8) -> Self {
        match v {
            0x00 => TransmitStatus::Ok,
            0x01 => TransmitStatus::NoAck,
            0x02 => TransmitStatus::Fail,
            0x03 => TransmitStatus::NotIdle,
            0x04 => TransmitStatus::NoRoute,
            o => TransmitStatus::Other(o),
        }
    }
    /// To the wire byte.
    pub fn to_u8(self) -> u8 {
        match self {
            TransmitStatus::Ok => 0,
            TransmitStatus::NoAck => 1,
            TransmitStatus::Fail => 2,
            TransmitStatus::NotIdle => 3,
            TransmitStatus::NoRoute => 4,
            TransmitStatus::Other(o) => o,
        }
    }
}

/// The transmit report the controller appends to a SendData callback once
/// TX status reporting is enabled: the route the frame took and its signal.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TxReport {
    /// Time on air until the ACK, in 10 ms ticks.
    pub ticks: u16,
    /// Repeaters used, nearest the controller first.
    pub repeaters: Vec<NodeId>,
    /// RSSI of the ACK at the controller, dBm.
    pub ack_rssi: Option<i8>,
    /// RSSI of the ACK at each repeater, dBm.
    pub repeater_rssi: Vec<i8>,
    /// Channel the ACK came in on.
    pub ack_channel: Option<u8>,
    /// Channel the frame went out on.
    pub tx_channel: u8,
    /// Route scheme state (which routing attempt succeeded).
    pub route_scheme: u8,
    /// Speed: 1 = 9.6k, 2 = 40k, 3 = 100k.
    pub speed: u8,
    /// Routing attempts.
    pub attempts: u8,
    /// Last working node before a failed link, if any.
    pub failed_from: Option<NodeId>,
    /// First node after a failed link, if any.
    pub failed_to: Option<NodeId>,
}

impl TxReport {
    /// Parses the bytes after the transmit status. Returns `None` if the
    /// controller sent no report.
    pub fn parse(p: &[u8]) -> Option<Self> {
        if p.len() < 17 {
            return None;
        }
        let n = usize::from(p[2]).min(4);
        let repeaters: Vec<NodeId> = p[11..11 + n].iter().map(|&b| NodeId::from(b)).collect();
        let repeater_rssi = p[4..4 + n]
            .iter()
            .map(|&b| b as i8)
            .filter(|v| *v < 0x7d)
            .collect();
        let failed = |i: usize| p.get(i).copied().filter(|&b| b != 0).map(NodeId::from);
        Some(TxReport {
            ticks: u16::from_be_bytes([p[0], p[1]]),
            repeaters,
            ack_rssi: rssi(p.get(3)),
            repeater_rssi,
            ack_channel: Some(p[8]),
            tx_channel: p[9],
            route_scheme: p[10],
            speed: p[15] & 0x07,
            attempts: p[16],
            failed_from: failed(17),
            failed_to: failed(18),
        })
    }

    /// Encodes the report bytes (simulators).
    pub fn encode(&self) -> Vec<u8> {
        let mut v = self.ticks.to_be_bytes().to_vec();
        v.push(self.repeaters.len() as u8);
        v.push(self.ack_rssi.map(|r| r as u8).unwrap_or(0x7f));
        for i in 0..4 {
            v.push(self.repeater_rssi.get(i).map(|r| *r as u8).unwrap_or(0x7f));
        }
        v.push(self.ack_channel.unwrap_or(0));
        v.push(self.tx_channel);
        v.push(self.route_scheme);
        for i in 0..4 {
            v.push(self.repeaters.get(i).map(|r| *r as u8).unwrap_or(0));
        }
        v.push(self.speed);
        v.push(self.attempts);
        v.push(self.failed_from.unwrap_or(0) as u8);
        v.push(self.failed_to.unwrap_or(0) as u8);
        v
    }
}

/// A SendData callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendDataResult {
    /// The transmit status.
    pub status: TransmitStatus,
    /// The route and signal report, when enabled.
    pub report: Option<TxReport>,
}

impl SendDataResult {
    /// Parses a SendData callback payload (`[callback id, status, report..]`).
    pub fn parse(p: &[u8]) -> Result<Self> {
        need(p, 2, "send data callback")?;
        Ok(SendDataResult {
            status: TransmitStatus::from_u8(p[1]),
            report: TxReport::parse(&p[2..]),
        })
    }
}

/// Transmit options for SendData.
pub mod tx_options {
    /// Ask for an acknowledgement.
    pub const ACK: u8 = 0x01;
    /// Transmit at low power.
    pub const LOW_POWER: u8 = 0x02;
    /// Use routes if direct transmission fails.
    pub const AUTO_ROUTE: u8 = 0x04;
    /// Never route.
    pub const NO_ROUTE: u8 = 0x10;
    /// Use explorer frames as a last resort.
    pub const EXPLORE: u8 = 0x20;
    /// What hosts normally send.
    pub const DEFAULT: u8 = ACK | AUTO_ROUTE | EXPLORE;
}

/// Builds a SendData request (classic node ids).
pub fn send_data(node: NodeId, command: &[u8], options: u8, callback_id: u8) -> DataFrame {
    let mut v = Vec::with_capacity(command.len() + 4);
    v.push(node as u8);
    v.push(command.len() as u8);
    v.extend_from_slice(command);
    v.push(options);
    v.push(callback_id);
    DataFrame::request(F::SEND_DATA, v)
}

/// Inclusion modes for `ADD_NODE_TO_NETWORK`.
pub mod add_mode {
    /// Any node.
    pub const ANY: u8 = 0x01;
    /// Stop inclusion.
    pub const STOP: u8 = 0x05;
    /// Include a SmartStart node by its DSK-derived home ids.
    pub const SMART_START_DSK: u8 = 0x08;
    /// Listen for SmartStart inclusion requests.
    pub const SMART_START_LISTEN: u8 = 0x09;
    /// Transmit at full power.
    pub const HIGH_POWER: u8 = 0x80;
    /// Network-wide inclusion (explorer frames).
    pub const NETWORK_WIDE: u8 = 0x40;
}

/// Progress of an inclusion (`ADD_NODE_TO_NETWORK` callbacks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddNodeStatus {
    /// The controller is in inclusion mode.
    Ready,
    /// A node answered.
    NodeFound,
    /// An end node is being added; its id and node information.
    AddingEndNode {
        /// The node id assigned.
        node: NodeId,
        /// Node information.
        info: NodeInfo,
    },
    /// A controller is being added.
    AddingController {
        /// The node id assigned.
        node: NodeId,
        /// Node information.
        info: NodeInfo,
    },
    /// The protocol part is done; send Stop.
    ProtocolDone,
    /// Inclusion finished.
    Done {
        /// The node id, if the controller reports it.
        node: NodeId,
    },
    /// Inclusion failed.
    Failed,
    /// Another status.
    Other(u8),
}

impl AddNodeStatus {
    /// Parses a callback payload (`[callback id, status, node, len, info..]`).
    pub fn parse(p: &[u8], node_id_len: usize) -> Result<Self> {
        need(p, 2, "add node status")?;
        let node = || node_id(p, 2, node_id_len, "add node status");
        let info = || -> Result<NodeInfo> {
            let at = 2 + node_id_len;
            need(p, at + 1, "add node status")?;
            let len = usize::from(p[at]);
            NodeInfo::parse(&p[at + 1..p.len().min(at + 1 + len)])
        };
        Ok(match p[1] {
            0x01 => AddNodeStatus::Ready,
            0x02 => AddNodeStatus::NodeFound,
            0x03 => AddNodeStatus::AddingEndNode {
                node: node()?,
                info: info()?,
            },
            0x04 => AddNodeStatus::AddingController {
                node: node()?,
                info: info()?,
            },
            0x05 => AddNodeStatus::ProtocolDone,
            0x06 => AddNodeStatus::Done {
                node: node().unwrap_or(0),
            },
            0x07 => AddNodeStatus::Failed,
            o => AddNodeStatus::Other(o),
        })
    }

    /// Encodes as a callback frame (simulators, classic ids).
    pub fn to_frame(&self, callback_id: u8) -> DataFrame {
        let with_info = |status: u8, node: NodeId, info: &NodeInfo| {
            let body = info.encode();
            let mut v = vec![callback_id, status, node as u8, body.len() as u8];
            v.extend_from_slice(&body);
            v
        };
        let p = match self {
            AddNodeStatus::Ready => vec![callback_id, 0x01, 0, 0],
            AddNodeStatus::NodeFound => vec![callback_id, 0x02, 0, 0],
            AddNodeStatus::AddingEndNode { node, info } => with_info(0x03, *node, info),
            AddNodeStatus::AddingController { node, info } => with_info(0x04, *node, info),
            AddNodeStatus::ProtocolDone => vec![callback_id, 0x05, 0, 0],
            AddNodeStatus::Done { node } => vec![callback_id, 0x06, *node as u8, 0],
            AddNodeStatus::Failed => vec![callback_id, 0x07, 0, 0],
            AddNodeStatus::Other(o) => vec![callback_id, *o, 0, 0],
        };
        DataFrame::request(F::ADD_NODE_TO_NETWORK, p)
    }
}

/// Progress of an exclusion (`REMOVE_NODE_FROM_NETWORK` callbacks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveNodeStatus {
    /// The controller is in exclusion mode.
    Ready,
    /// A node answered.
    NodeFound,
    /// An end node is being removed.
    RemovingEndNode {
        /// Its old node id (0 if it was not in this network).
        node: NodeId,
    },
    /// A controller is being removed.
    RemovingController {
        /// Its old node id.
        node: NodeId,
    },
    /// Exclusion finished.
    Done {
        /// The removed node (0 if it was not in this network).
        node: NodeId,
    },
    /// Exclusion failed.
    Failed,
    /// Another status.
    Other(u8),
}

impl RemoveNodeStatus {
    /// Parses a callback payload.
    pub fn parse(p: &[u8], node_id_len: usize) -> Result<Self> {
        need(p, 2, "remove node status")?;
        let node = || node_id(p, 2, node_id_len, "remove node status").unwrap_or(0);
        Ok(match p[1] {
            0x01 => RemoveNodeStatus::Ready,
            0x02 => RemoveNodeStatus::NodeFound,
            0x03 => RemoveNodeStatus::RemovingEndNode { node: node() },
            0x04 => RemoveNodeStatus::RemovingController { node: node() },
            0x06 => RemoveNodeStatus::Done { node: node() },
            0x07 => RemoveNodeStatus::Failed,
            o => RemoveNodeStatus::Other(o),
        })
    }

    /// Encodes as a callback frame (simulators).
    pub fn to_frame(&self, callback_id: u8) -> DataFrame {
        let (s, n) = match *self {
            RemoveNodeStatus::Ready => (0x01, 0),
            RemoveNodeStatus::NodeFound => (0x02, 0),
            RemoveNodeStatus::RemovingEndNode { node } => (0x03, node),
            RemoveNodeStatus::RemovingController { node } => (0x04, node),
            RemoveNodeStatus::Done { node } => (0x06, node),
            RemoveNodeStatus::Failed => (0x07, 0),
            RemoveNodeStatus::Other(o) => (o, 0),
        };
        DataFrame::request(
            F::REMOVE_NODE_FROM_NETWORK,
            vec![callback_id, s, n as u8, 0],
        )
    }
}

/// Serial API setup subcommands.
pub mod setup {
    /// Which subcommands the controller supports.
    pub const GET_SUPPORTED: u8 = 0x01;
    /// Enable or disable transmit reports on SendData callbacks.
    pub const SET_TX_STATUS_REPORT: u8 = 0x02;
    /// The RF region.
    pub const GET_RF_REGION: u8 = 0x20;
    /// Set the RF region (needs a soft reset to apply).
    pub const SET_RF_REGION: u8 = 0x40;
    /// Set 8- or 16-bit node ids on the Serial API.
    pub const SET_NODE_ID_TYPE: u8 = 0x80;
}

/// An RF region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RfRegion {
    /// Europe.
    Europe,
    /// USA.
    Usa,
    /// Australia and New Zealand.
    AustraliaNewZealand,
    /// Hong Kong.
    HongKong,
    /// India.
    India,
    /// Israel.
    Israel,
    /// Russia.
    Russia,
    /// China.
    China,
    /// USA with Long Range.
    UsaLongRange,
    /// Europe with Long Range.
    EuropeLongRange,
    /// Japan.
    Japan,
    /// Korea.
    Korea,
    /// Another value.
    Other(u8),
}

impl RfRegion {
    /// From the wire byte.
    pub fn from_u8(v: u8) -> Self {
        match v {
            0x00 => RfRegion::Europe,
            0x01 => RfRegion::Usa,
            0x02 => RfRegion::AustraliaNewZealand,
            0x03 => RfRegion::HongKong,
            0x05 => RfRegion::India,
            0x06 => RfRegion::Israel,
            0x07 => RfRegion::Russia,
            0x08 => RfRegion::China,
            0x09 => RfRegion::UsaLongRange,
            0x0b => RfRegion::EuropeLongRange,
            0x20 => RfRegion::Japan,
            0x21 => RfRegion::Korea,
            o => RfRegion::Other(o),
        }
    }

    /// To the wire byte.
    pub fn to_u8(self) -> u8 {
        match self {
            RfRegion::Europe => 0x00,
            RfRegion::Usa => 0x01,
            RfRegion::AustraliaNewZealand => 0x02,
            RfRegion::HongKong => 0x03,
            RfRegion::India => 0x05,
            RfRegion::Israel => 0x06,
            RfRegion::Russia => 0x07,
            RfRegion::China => 0x08,
            RfRegion::UsaLongRange => 0x09,
            RfRegion::EuropeLongRange => 0x0b,
            RfRegion::Japan => 0x20,
            RfRegion::Korea => 0x21,
            RfRegion::Other(o) => o,
        }
    }
}

/// Neighbor table of a node (`GET_ROUTING_INFO` response).
pub fn parse_routing_info(p: &[u8]) -> Result<Vec<NodeId>> {
    need(p, 29, "routing info")?;
    Ok(parse_node_mask(&p[..29], 1))
}

/// Builds a `GET_ROUTING_INFO` request.
pub fn get_routing_info(node: NodeId) -> DataFrame {
    DataFrame::request(F::GET_ROUTING_INFO, vec![node as u8, 0, 0, 0])
}

/// NVM backup subcommands (`NVM_OPERATIONS`, `EXTENDED_NVM_OPERATIONS`).
pub mod nvm {
    /// Open the NVM for reading or writing; the response holds its size.
    pub const OPEN: u8 = 0x00;
    /// Read a chunk.
    pub const READ: u8 = 0x01;
    /// Write a chunk.
    pub const WRITE: u8 = 0x02;
    /// Close.
    pub const CLOSE: u8 = 0x03;
    /// Status: OK.
    pub const STATUS_OK: u8 = 0x00;
    /// Status: end of file reached (the chunk is the last).
    pub const STATUS_EOF: u8 = 0xff;
}

/// A response to an NVM operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NvmResponse {
    /// Status.
    pub status: u8,
    /// For Open: the NVM size. For Read: the offset.
    pub offset_or_size: u32,
    /// Data read.
    pub data: Vec<u8>,
}

impl NvmResponse {
    /// Parses a response; `extended` for 32-bit offsets.
    pub fn parse(p: &[u8], extended: bool) -> Result<Self> {
        let off_len = if extended { 4 } else { 2 };
        need(p, 2, "NVM response")?;
        let status = p[0];
        let len = usize::from(p[1]);
        let offset_or_size = if p.len() >= 2 + off_len {
            if extended {
                u32::from_be_bytes([p[2], p[3], p[4], p[5]])
            } else {
                u32::from(u16::from_be_bytes([p[2], p[3]]))
            }
        } else {
            0
        };
        let start = (2 + off_len).min(p.len());
        let data = p[start..p.len().min(start + len)].to_vec();
        Ok(NvmResponse {
            status,
            offset_or_size,
            data,
        })
    }

    /// Encodes (simulators).
    pub fn encode(&self, extended: bool) -> Vec<u8> {
        let mut v = vec![self.status, self.data.len() as u8];
        if extended {
            v.extend_from_slice(&self.offset_or_size.to_be_bytes());
        } else {
            v.extend_from_slice(&(self.offset_or_size as u16).to_be_bytes());
        }
        v.extend_from_slice(&self.data);
        v
    }
}

/// Builds an NVM operation request.
pub fn nvm_request(extended: bool, op: u8, length: u8, offset: u32) -> DataFrame {
    let f = if extended {
        F::EXTENDED_NVM_OPERATIONS
    } else {
        F::NVM_OPERATIONS
    };
    let mut v = vec![op];
    if op == nvm::READ {
        v.push(length);
        if extended {
            v.extend_from_slice(&offset.to_be_bytes());
        } else {
            v.extend_from_slice(&(offset as u16).to_be_bytes());
        }
    }
    DataFrame::request(f, v)
}

/// Whether a frame is the response to `function`.
pub fn is_response(f: &DataFrame, function: crate::FunctionId) -> bool {
    f.kind == FrameType::Response && f.function == function.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_mask() {
        let c = SerialApiCapabilities {
            firmware_major: 7,
            firmware_minor: 21,
            manufacturer_id: 0,
            product_type: 4,
            product_id: 1,
            functions: SerialApiCapabilities::mask(&[
                F::SEND_DATA,
                F::GET_ROUTING_INFO,
                F::NVM_OPERATIONS,
            ]),
        };
        let back = SerialApiCapabilities::parse(&c.encode()).unwrap();
        assert!(back.supports(F::SEND_DATA));
        assert!(back.supports(F::GET_ROUTING_INFO));
        assert!(!back.supports(F::EXTENDED_NVM_OPERATIONS));
        assert_eq!(back, c);
    }

    #[test]
    fn init_data_and_masks() {
        let d = InitData {
            api_version: 9,
            capabilities: 0x08,
            nodes: vec![1, 2, 5, 232],
            chip_type: Some(8),
            chip_version: Some(0),
        };
        let back = InitData::parse(&d.encode()).unwrap();
        assert_eq!(back, d);
        assert!(back.is_sis());
        assert!(InitData::parse(&[1, 2]).is_err());
        // A length byte larger than the payload is clamped.
        assert!(InitData::parse(&[1, 2, 200, 0xff]).unwrap().nodes.len() == 8);
    }

    #[test]
    fn cc_list_with_mark_and_extended() {
        let (s, c) = parse_cc_list(&[0x5e, 0x25, 0xf1, 0x00, 0xef, 0x20, 0xf2]);
        assert_eq!(s, vec![0x5e, 0x25, 0xf100]);
        assert_eq!(c, vec![0x20]);
    }

    #[test]
    fn application_command_variants() {
        let a = ApplicationCommand {
            rx_status: RxStatus(0),
            source: 4,
            destination: None,
            command: vec![0x31, 0x05, 0x01, 0x22, 0x00, 0xe1],
            rssi: Some(-60),
        };
        let f = a.to_frame();
        assert_eq!(ApplicationCommand::parse(&f.payload, 1).unwrap(), a);
        // Length byte beyond the payload.
        assert!(ApplicationCommand::parse(&[0, 4, 9, 0x20], 1).is_err());
        // Bridge format: rx, dest, src, len, cmd, mask len, rssi.
        let b = ApplicationCommand::parse_bridge(&[0, 1, 4, 2, 0x20, 0x02, 0, 0xc4], 1).unwrap();
        assert_eq!(b.source, 4);
        assert_eq!(b.destination, Some(1));
        assert_eq!(b.command, vec![0x20, 0x02]);
        assert_eq!(b.rssi, Some(-60));
    }

    #[test]
    fn tx_report_roundtrip() {
        let r = TxReport {
            ticks: 3,
            repeaters: vec![7, 9],
            ack_rssi: Some(-71),
            repeater_rssi: vec![-50, -60],
            ack_channel: Some(1),
            tx_channel: 1,
            route_scheme: 3,
            speed: 3,
            attempts: 1,
            failed_from: None,
            failed_to: None,
        };
        let mut p = vec![0x01, 0x00];
        p.extend_from_slice(&r.encode());
        let res = SendDataResult::parse(&p).unwrap();
        assert_eq!(res.status, TransmitStatus::Ok);
        assert_eq!(res.report.unwrap(), r);
        assert_eq!(SendDataResult::parse(&[1, 1]).unwrap().report, None);
    }

    #[test]
    fn add_node_status() {
        let info = NodeInfo {
            basic: 4,
            generic: 0x21,
            specific: 1,
            supported: vec![0x5e, 0x9f],
            controlled: vec![],
        };
        let s = AddNodeStatus::AddingEndNode { node: 7, info };
        assert_eq!(AddNodeStatus::parse(&s.to_frame(3).payload, 1).unwrap(), s);
        assert_eq!(
            AddNodeStatus::parse(&[3, 6, 7, 0], 1).unwrap(),
            AddNodeStatus::Done { node: 7 }
        );
    }

    #[test]
    fn application_updates() {
        let info = NodeInfo {
            basic: 4,
            generic: 0x07,
            specific: 1,
            supported: vec![0x5e, 0x71],
            controlled: vec![],
        };
        let u = ApplicationUpdate::NodeInfoReceived {
            node: 3,
            info: info.clone(),
        };
        assert_eq!(
            ApplicationUpdate::parse(&u.to_frame().payload, 1).unwrap(),
            u
        );
        let s = ApplicationUpdate::SmartStartHomeIdReceived {
            nwi_home_id: [0xc1, 2, 3, 4],
            long_range: false,
            info,
        };
        assert_eq!(
            ApplicationUpdate::parse(&s.to_frame().payload, 1).unwrap(),
            s
        );
    }

    #[test]
    fn protocol_info() {
        let i = NodeProtocolInfo {
            listening: false,
            routing: true,
            flirs_1000: false,
            flirs_250: false,
            beaming: true,
            controller: false,
            security: true,
            protocol_version: 3,
            speeds: 0b111,
            basic: 4,
            generic: 0x21,
            specific: 1,
        };
        assert_eq!(NodeProtocolInfo::parse(&i.encode()).unwrap(), Some(i));
        assert_eq!(NodeProtocolInfo::parse(&[0; 6]).unwrap(), None);
    }

    #[test]
    fn nvm_responses() {
        let r = NvmResponse {
            status: 0,
            offset_or_size: 0x1000,
            data: vec![],
        };
        assert_eq!(NvmResponse::parse(&r.encode(false), false).unwrap(), r);
        let r = NvmResponse {
            status: 0xff,
            offset_or_size: 0x20,
            data: vec![1, 2, 3],
        };
        assert_eq!(NvmResponse::parse(&r.encode(true), true).unwrap(), r);
    }
}
