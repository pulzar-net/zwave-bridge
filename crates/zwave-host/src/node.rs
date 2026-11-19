//! What the host knows about each node. Persisted in the [`store`](crate::store).

use crate::NodeId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use zwave_security::SecurityClass;

/// Interview progress, persisted so an interrupted interview resumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InterviewStage {
    /// Nothing known yet.
    #[default]
    New,
    /// Protocol information (listening, routing, device class) known.
    ProtocolInfo,
    /// The node information frame (command classes) known.
    NodeInfo,
    /// The secure command classes known.
    Security,
    /// Manufacturer, product and Z-Wave Plus info known.
    Identity,
    /// Command class versions known.
    Versions,
    /// Endpoints known.
    Endpoints,
    /// Each command class's capabilities known and the node configured.
    CommandClasses,
    /// Done.
    Complete,
}

/// Runtime status (not persisted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    /// Not heard from yet.
    #[default]
    Unknown,
    /// Listening and answering.
    Alive,
    /// A sleeping node that is awake now.
    Awake,
    /// A sleeping node that is asleep.
    Asleep,
    /// Listening but not answering.
    Dead,
}

/// Protocol information from the controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProtocolInfo {
    /// Always listening.
    pub listening: bool,
    /// Frequently listening (FLiRS), reachable with a beam.
    pub flirs: bool,
    /// A routing node (can repeat).
    pub routing: bool,
    /// The node is a controller.
    pub controller: bool,
    /// Basic device class.
    pub basic: u8,
    /// Generic device class.
    pub generic: u8,
    /// Specific device class.
    pub specific: u8,
}

impl ProtocolInfo {
    /// A node that sleeps and is reachable only when it wakes up.
    pub fn sleeping(&self) -> bool {
        !self.listening && !self.flirs
    }
}

/// One command class on an endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CcSupport {
    /// The version the node implements (0 until known).
    pub version: u8,
    /// Supported only with security.
    pub secure: bool,
}

/// What each command class on an endpoint reported in the interview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Capabilities {
    /// Multilevel sensor types and their scales.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sensors: BTreeMap<u8, Vec<u8>>,
    /// Meter type and scales.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meter: Option<(u8, Vec<u8>)>,
    /// Notification types and their events.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub notifications: BTreeMap<u8, Vec<u8>>,
    /// Binary sensor types.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub binary_sensors: Vec<u8>,
    /// Multilevel switch primary type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub switch_type: Option<u8>,
}

/// An endpoint: the root device (0) or a Multi Channel endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Endpoint {
    /// Generic device class (endpoints only).
    #[serde(default)]
    pub generic: u8,
    /// Specific device class (endpoints only).
    #[serde(default)]
    pub specific: u8,
    /// Supported command classes.
    #[serde(default)]
    pub ccs: BTreeMap<u16, CcSupport>,
    /// Capabilities found in the interview.
    #[serde(default)]
    pub caps: Capabilities,
}

impl Endpoint {
    /// Whether the endpoint supports a command class.
    pub fn supports(&self, cc: u16) -> bool {
        self.ccs.contains_key(&cc)
    }

    /// The version of a command class (1 if supported but unknown, 0 if unsupported).
    pub fn version(&self, cc: u16) -> u8 {
        self.ccs.get(&cc).map(|c| c.version.max(1)).unwrap_or(0)
    }
}

/// Manufacturer identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manufacturer {
    /// Manufacturer id.
    pub manufacturer_id: u16,
    /// Product type.
    pub product_type: u16,
    /// Product id.
    pub product_id: u16,
}

/// Everything the host stores about a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NodeRecord {
    /// Node id.
    pub id: NodeId,
    /// Protocol information.
    #[serde(default)]
    pub protocol: ProtocolInfo,
    /// Interview progress.
    #[serde(default)]
    pub stage: InterviewStage,
    /// Granted security classes, highest first (empty: insecure).
    #[serde(default)]
    pub security: Vec<SecurityClass>,
    /// Whether the secure command class list is known.
    #[serde(default)]
    pub secure_ccs_known: bool,
    /// Manufacturer identity.
    #[serde(default)]
    pub manufacturer: Option<Manufacturer>,
    /// Firmware version (`major.minor`).
    #[serde(default)]
    pub firmware: Option<String>,
    /// Z-Wave Plus role and node type, and the icons.
    #[serde(default)]
    pub zwave_plus: Option<(u8, u8, u16, u16)>,
    /// Endpoints, `0` being the root device.
    #[serde(default)]
    pub endpoints: BTreeMap<u8, Endpoint>,
    /// Wake-up interval configured, seconds.
    #[serde(default)]
    pub wake_up_interval: Option<u32>,
    /// Neighbors from the controller's routing table.
    #[serde(default)]
    pub neighbors: Vec<NodeId>,
    /// A name given by people.
    #[serde(default)]
    pub name: Option<String>,
    /// A location given by people.
    #[serde(default)]
    pub location: Option<String>,
    /// The DSK the node was included with (S2).
    #[serde(default)]
    pub dsk: Option<String>,
}

impl NodeRecord {
    /// A record for a new node.
    pub fn new(id: NodeId) -> Self {
        let mut r = NodeRecord { id, ..Default::default() };
        r.endpoints.insert(0, Endpoint::default());
        r
    }

    /// The root endpoint.
    pub fn root(&self) -> &Endpoint {
        static EMPTY: std::sync::OnceLock<Endpoint> = std::sync::OnceLock::new();
        self.endpoints.get(&0).unwrap_or_else(|| EMPTY.get_or_init(Endpoint::default))
    }

    /// The root endpoint, mutable.
    pub fn root_mut(&mut self) -> &mut Endpoint {
        self.endpoints.entry(0).or_default()
    }

    /// The highest granted S2 class.
    pub fn highest_s2(&self) -> Option<SecurityClass> {
        self.security.iter().copied().filter(|c| c.is_s2()).max()
    }

    /// Whether the node was included with S0 only.
    pub fn s0_only(&self) -> bool {
        self.highest_s2().is_none() && self.security.contains(&SecurityClass::S0Legacy)
    }

    /// Whether a command class must be sent with security to this endpoint.
    pub fn needs_security(&self, endpoint: u8, cc: u16) -> bool {
        if self.security.is_empty() {
            return false;
        }
        // The security classes' own commands go plain or are handled by the stack.
        if matches!(cc, 0x98 | 0x9f) {
            return false;
        }
        match self.endpoints.get(&endpoint).and_then(|e| e.ccs.get(&cc)) {
            Some(s) => s.secure || !self.secure_ccs_known,
            // Unknown to us: secure until the secure list says otherwise.
            None => !self.secure_ccs_known || endpoint != 0,
        }
    }
}
