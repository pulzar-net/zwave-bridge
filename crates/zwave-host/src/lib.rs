//! A Z-Wave controller host stack.
//!
//! The controller chip runs the radio and the mesh; this crate does the
//! application side a host has to do, on top of [`zwave_serial`]:
//!
//! - inclusion and exclusion, with S2 (key exchange, DSK PIN check), S0 and
//!   SmartStart from a provisioning list;
//! - the interview: node information, secure command classes, manufacturer,
//!   versions, endpoints, and each command class's capabilities;
//! - a node store written to disk on change (batched, atomic);
//! - wake-up queues: commands for sleeping nodes wait for their next
//!   Wake Up Notification, and the node is sent back to sleep when idle;
//! - encapsulation in both directions: Security 0 and 2, Supervision, Multi
//!   Channel, Transport Service and CRC-16;
//! - values from reports, as [`Event::Value`];
//! - the neighbor tables and the routes transmit reports describe, for
//!   topology;
//! - an NVM backup of the controller.
//!
//! Start one with [`Controller::start`] and read [`Event`]s from the
//! returned channel.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod controller;
mod inclusion;
mod interview;
pub mod node;
mod nvm;
pub mod store;
pub mod values;

pub use controller::{Controller, ControllerInfo, HostConfig, Topology, TopologyNode};
pub use inclusion::{GrantPolicy, InclusionOptions, InclusionResult};
pub use node::{NodeRecord, NodeStatus};
pub use values::{ValueId, ValueKind, ValueUpdate};
pub use zwave_security::SecurityClass;
pub use zwave_serial::NodeId;

use zwave_security::provisioning::Entry as ProvisioningEntry;

/// The network keys and the controller's long-term S2 key pair.
///
/// Generate them once per network with [`NetworkKeys::generate`] and keep
/// them secret: anyone with these keys can read and command the network.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NetworkKeys {
    /// The S0 network key.
    #[serde(with = "hex_key")]
    pub s0: [u8; 16],
    /// S2 Unauthenticated.
    #[serde(with = "hex_key")]
    pub s2_unauthenticated: [u8; 16],
    /// S2 Authenticated.
    #[serde(with = "hex_key")]
    pub s2_authenticated: [u8; 16],
    /// S2 Access Control.
    #[serde(with = "hex_key")]
    pub s2_access_control: [u8; 16],
    /// The controller's Curve25519 private key for S2 key exchange.
    #[serde(with = "hex_key")]
    pub ecdh_private: [u8; 32],
}

impl std::fmt::Debug for NetworkKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NetworkKeys(..)")
    }
}

impl NetworkKeys {
    /// Fresh random keys from the operating system's CSPRNG.
    pub fn generate() -> Result<Self, zwave_security::Error> {
        use zwave_security::random_bytes;
        Ok(NetworkKeys {
            s0: random_bytes()?,
            s2_unauthenticated: random_bytes()?,
            s2_authenticated: random_bytes()?,
            s2_access_control: random_bytes()?,
            ecdh_private: random_bytes()?,
        })
    }

    /// The key for a class.
    pub fn key(&self, class: SecurityClass) -> &[u8; 16] {
        match class {
            SecurityClass::S0Legacy => &self.s0,
            SecurityClass::S2Unauthenticated => &self.s2_unauthenticated,
            SecurityClass::S2Authenticated => &self.s2_authenticated,
            SecurityClass::S2AccessControl => &self.s2_access_control,
        }
    }

    /// The controller's DSK (first 16 bytes of its public key), for a controller-side QR label.
    pub fn controller_dsk(&self) -> zwave_security::dsk::Dsk {
        let kp = zwave_security::kex::KeyPair::from_private(self.ecdh_private);
        zwave_security::dsk::Dsk::from_public_key(kp.public())
    }
}

mod hex_key {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer, const N: usize>(k: &[u8; N], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(k))
    }

    pub fn deserialize<'de, D: Deserializer<'de>, const N: usize>(
        d: D,
    ) -> Result<[u8; N], D::Error> {
        let s = String::deserialize(d)?;
        let v = hex::decode(s.trim()).map_err(serde::de::Error::custom)?;
        v.try_into()
            .map_err(|_| serde::de::Error::custom(format!("expected {N} bytes")))
    }
}

/// Errors from the host stack.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The serial link failed.
    #[error("serial: {0}")]
    Serial(#[from] zwave_serial::SerialError),
    /// The controller answered in a way that makes no sense.
    #[error("controller: {0}")]
    Controller(String),
    /// The node did not acknowledge (asleep, out of range or gone).
    #[error("node {0} did not acknowledge")]
    NoAck(NodeId),
    /// The transmission failed at the radio.
    #[error("transmission to node {0} failed: {1}")]
    TransmitFailed(NodeId, String),
    /// No answer from the node in time.
    #[error("node {0} did not answer in time")]
    Timeout(NodeId),
    /// A security exchange failed.
    #[error("security: {0}")]
    Security(String),
    /// No such node.
    #[error("no node {0}")]
    UnknownNode(NodeId),
    /// Another inclusion or exclusion is running.
    #[error("the controller is busy with another inclusion or exclusion")]
    Busy,
    /// Inclusion or exclusion failed.
    #[error("{0}")]
    Inclusion(String),
    /// The node store could not be read or written.
    #[error("store: {0}")]
    Store(String),
    /// The controller task stopped.
    #[error("controller stopped")]
    Stopped,
}

/// Something that happened on the network.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The controller is initialized.
    Ready(ControllerInfo),
    /// Inclusion mode is on; press the node's button.
    InclusionStarted,
    /// Inclusion mode is off.
    InclusionStopped,
    /// Exclusion mode is on.
    ExclusionStarted,
    /// Exclusion mode is off.
    ExclusionStopped,
    /// A node joined (protocol part done; security and interview follow).
    NodeAdded {
        /// The node.
        node: NodeId,
    },
    /// A node left or was removed.
    NodeRemoved {
        /// The node.
        node: NodeId,
    },
    /// S2: the node asks for these keys; answer with [`Controller::grant_security_classes`].
    GrantSecurityClasses {
        /// The joining node.
        node: NodeId,
        /// Requested classes, highest first.
        requested: Vec<SecurityClass>,
    },
    /// S2: confirm the PIN (first five digits of the DSK) with [`Controller::validate_dsk`].
    ValidateDsk {
        /// The joining node.
        node: NodeId,
        /// The DSK with the PIN masked, `xxxxx-12345-...`.
        dsk: String,
    },
    /// The key exchange finished; these classes are granted.
    SecurityBootstrapped {
        /// The node.
        node: NodeId,
        /// Granted classes, highest first (empty: included without security).
        classes: Vec<SecurityClass>,
    },
    /// The key exchange failed; the node is included without security.
    SecurityFailed {
        /// The node.
        node: NodeId,
        /// Why.
        reason: String,
    },
    /// The interview reached a new stage.
    InterviewStage {
        /// The node.
        node: NodeId,
        /// The stage just reached.
        stage: node::InterviewStage,
    },
    /// The interview finished.
    InterviewCompleted {
        /// The node.
        node: NodeId,
    },
    /// The interview failed for now; it is retried.
    InterviewFailed {
        /// The node.
        node: NodeId,
        /// Why.
        reason: String,
    },
    /// A node's record changed (new metadata); re-read it with [`Controller::node`].
    NodeUpdated {
        /// The node.
        node: NodeId,
    },
    /// Awake, asleep, alive or dead.
    NodeStatus {
        /// The node.
        node: NodeId,
        /// The new status.
        status: NodeStatus,
    },
    /// A value reported by a node.
    Value(ValueUpdate),
    /// A transmission reached the node along this route.
    Route {
        /// The node.
        node: NodeId,
        /// Repeaters from the controller outwards (empty: direct).
        repeaters: Vec<NodeId>,
        /// RSSI of the node's ACK at the controller.
        rssi: Option<i8>,
    },
    /// A command a node sent that the stack did not turn into a value.
    Command {
        /// The node.
        node: NodeId,
        /// The endpoint (0 is the root device).
        endpoint: u8,
        /// The command.
        command: zwave_cc::Command,
    },
    /// The provisioning list changed.
    ProvisioningChanged {
        /// The entries.
        entries: Vec<ProvisioningEntry>,
    },
}
