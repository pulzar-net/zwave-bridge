//! The MQTT API: every payload the bridge publishes or reads, as a type.
//!
//! The bridge serializes these and nothing else, so they are the contract.
//! With the `unstable-asyncapi-docs` feature each one also derives a JSON
//! Schema and an AsyncAPI message, and `tests/asyncapi.rs` assembles them
//! into the AsyncAPI description the docs site renders as the MQTT
//! reference. That feature is docs tooling, not API: it may change in any
//! release, and the bridge's own builds never turn it on.
//!
//! Topics are under the base topic (`mqtt.base_topic`, default `zwave`).
//! `<node>` is a node id. Network keys never appear on MQTT.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json};

#[cfg(feature = "unstable-asyncapi-docs")]
use asyncapi_rust::ToAsyncApiMessage;
#[cfg(feature = "unstable-asyncapi-docs")]
use schemars::JsonSchema;

// ---------------------------------------------------------------------------
// Availability

/// The bridge's availability, retained on `bridge/state`, as plain text.
/// The broker publishes `offline` as the bridge's last will when the
/// connection drops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "unstable-asyncapi-docs",
    derive(JsonSchema, ToAsyncApiMessage)
)]
#[cfg_attr(
    feature = "unstable-asyncapi-docs",
    asyncapi(
        summary = "The bridge is online or offline",
        content_type = "text/plain"
    )
)]
#[serde(transparent)]
pub struct BridgeState(pub Availability);

/// Online or offline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    /// Running and connected to the controller.
    Online,
    /// Stopped, or the MQTT connection dropped.
    Offline,
}

impl BridgeState {
    /// The payload the bridge publishes: the bare word, not JSON.
    pub fn payload(state: Availability) -> Vec<u8> {
        match state {
            Availability::Online => b"online".to_vec(),
            Availability::Offline => b"offline".to_vec(),
        }
    }
}

// ---------------------------------------------------------------------------
// Bridge info

/// The bridge, its controller and its network, retained on `bridge/info`.
/// Republished on connecting and when the controller is ready.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(
    feature = "unstable-asyncapi-docs",
    derive(JsonSchema, ToAsyncApiMessage)
)]
#[cfg_attr(
    feature = "unstable-asyncapi-docs",
    asyncapi(summary = "The bridge's version, controller and network")
)]
pub struct BridgeInfo {
    /// The network's home id, 8 hex digits.
    pub home_id: String,
    /// The controller's own node id.
    pub node_id: u16,
    /// The controller's Z-Wave library, such as `Z-Wave 7.21`.
    pub library_version: String,
    /// The library type, such as `Static Controller`.
    pub library_type: String,
    /// The controller's Serial API firmware version.
    pub firmware: String,
    /// The controller's manufacturer id.
    pub manufacturer_id: u16,
    /// The controller's product type.
    pub product_type: u16,
    /// The controller's product id.
    pub product_id: u16,
    /// The chip series: 7 for 700 series, 8 for 800 series; `null` when the
    /// controller does not say (500 series).
    pub chip_type: Option<u8>,
    /// The controller is the network's SUC/SIS.
    pub sis: bool,
    /// The RF region, such as `EU` or `US_LR`; `null` when the controller
    /// does not report it.
    pub region: Option<String>,
    /// The controller's own DSK, for adding it to another controller's
    /// network. Not secret.
    pub dsk: String,
    /// The node ids in the network, without the controller.
    pub nodes: Vec<u16>,
    /// The bridge's version.
    pub version: String,
}

// ---------------------------------------------------------------------------
// Nodes

/// A security class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub enum SecurityClass {
    /// S2 Access Control: locks and garage doors. Needs the device's PIN.
    #[serde(rename = "s2_access_control")]
    S2AccessControl,
    /// S2 Authenticated. Needs the device's PIN.
    #[serde(rename = "s2_authenticated")]
    S2Authenticated,
    /// S2 Unauthenticated.
    #[serde(rename = "s2_unauthenticated")]
    S2Unauthenticated,
    /// Security 0, the legacy scheme.
    #[serde(rename = "s0_legacy")]
    S0Legacy,
}

impl From<zwave_security::SecurityClass> for SecurityClass {
    fn from(c: zwave_security::SecurityClass) -> Self {
        use zwave_security::SecurityClass as C;
        match c {
            C::S2AccessControl => SecurityClass::S2AccessControl,
            C::S2Authenticated => SecurityClass::S2Authenticated,
            C::S2Unauthenticated => SecurityClass::S2Unauthenticated,
            C::S0Legacy => SecurityClass::S0Legacy,
        }
    }
}

impl From<SecurityClass> for zwave_security::SecurityClass {
    fn from(c: SecurityClass) -> Self {
        use zwave_security::SecurityClass as C;
        match c {
            SecurityClass::S2AccessControl => C::S2AccessControl,
            SecurityClass::S2Authenticated => C::S2Authenticated,
            SecurityClass::S2Unauthenticated => C::S2Unauthenticated,
            SecurityClass::S0Legacy => C::S0Legacy,
        }
    }
}

/// Security classes as the API writes them, in the host's order.
pub fn classes(c: &[zwave_security::SecurityClass]) -> Vec<SecurityClass> {
    c.iter().copied().map(SecurityClass::from).collect()
}

/// Security classes from a request, highest first, each once.
pub fn host_classes(c: &[SecurityClass]) -> Vec<zwave_security::SecurityClass> {
    zwave_security::SecurityClass::HIGHEST_FIRST
        .into_iter()
        .filter(|h| {
            c.iter()
                .any(|x| zwave_security::SecurityClass::from(*x) == *h)
        })
        .collect()
}

/// How far a node's interview got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum InterviewStage {
    /// Nothing known yet.
    New,
    /// Protocol information (listening, routing, device class) known.
    ProtocolInfo,
    /// The node information frame (command classes) known.
    NodeInfo,
    /// The secure command classes known.
    Security,
    /// Manufacturer, product and Z-Wave Plus information known.
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

impl From<zwave_host::node::InterviewStage> for InterviewStage {
    fn from(s: zwave_host::node::InterviewStage) -> Self {
        use zwave_host::node::InterviewStage as S;
        match s {
            S::New => InterviewStage::New,
            S::ProtocolInfo => InterviewStage::ProtocolInfo,
            S::NodeInfo => InterviewStage::NodeInfo,
            S::Security => InterviewStage::Security,
            S::Identity => InterviewStage::Identity,
            S::Versions => InterviewStage::Versions,
            S::Endpoints => InterviewStage::Endpoints,
            S::CommandClasses => InterviewStage::CommandClasses,
            S::Complete => InterviewStage::Complete,
        }
    }
}

/// Whether a node answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    /// Not heard from since the bridge started.
    Unknown,
    /// Listening and answering.
    Alive,
    /// A sleeping node that is awake now.
    Awake,
    /// A sleeping node that is asleep: requests wait for its next wake-up.
    Asleep,
    /// Listening but not answering.
    Dead,
}

impl From<zwave_host::NodeStatus> for NodeStatus {
    fn from(s: zwave_host::NodeStatus) -> Self {
        use zwave_host::NodeStatus as S;
        match s {
            S::Unknown => NodeStatus::Unknown,
            S::Alive => NodeStatus::Alive,
            S::Awake => NodeStatus::Awake,
            S::Asleep => NodeStatus::Asleep,
            S::Dead => NodeStatus::Dead,
        }
    }
}

impl NodeStatus {
    /// The status as the word the bridge publishes.
    pub fn as_str(self) -> &'static str {
        match self {
            NodeStatus::Unknown => "unknown",
            NodeStatus::Alive => "alive",
            NodeStatus::Awake => "awake",
            NodeStatus::Asleep => "asleep",
            NodeStatus::Dead => "dead",
        }
    }
}

/// A node's status, retained on `nodes/<node>/status`, as plain text: the
/// same word as the node record's `status`, for availability in Home
/// Assistant and scripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "unstable-asyncapi-docs",
    derive(JsonSchema, ToAsyncApiMessage)
)]
#[cfg_attr(
    feature = "unstable-asyncapi-docs",
    asyncapi(summary = "Whether the node answers", content_type = "text/plain")
)]
#[serde(transparent)]
pub struct NodeStatusMessage(pub NodeStatus);

/// A node's protocol information, from the controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub struct ProtocolInfo {
    /// Always listening (mains powered).
    pub listening: bool,
    /// Frequently listening (FLiRS): reachable with a beam, such as a lock.
    pub flirs: bool,
    /// The node repeats frames for others.
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

impl From<zwave_host::node::ProtocolInfo> for ProtocolInfo {
    fn from(p: zwave_host::node::ProtocolInfo) -> Self {
        ProtocolInfo {
            listening: p.listening,
            flirs: p.flirs,
            routing: p.routing,
            controller: p.controller,
            basic: p.basic,
            generic: p.generic,
            specific: p.specific,
        }
    }
}

/// A node's manufacturer and product, from Manufacturer Specific.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub struct Manufacturer {
    /// Manufacturer id.
    pub manufacturer_id: u16,
    /// Product type.
    pub product_type: u16,
    /// Product id.
    pub product_id: u16,
}

/// A command class on an endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub struct CcSupport {
    /// The version the node implements; 0 until the interview reads it.
    pub version: u8,
    /// Supported only with security.
    pub secure: bool,
}

/// What an endpoint's command classes reported in the interview. Empty
/// entries are left out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub struct Capabilities {
    /// Multilevel sensor types and their scales.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sensors: BTreeMap<u8, Vec<u8>>,
    /// The meter type and its scales.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meter: Option<(u8, Vec<u8>)>,
    /// Notification types and their events.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub notifications: BTreeMap<u8, Vec<u8>>,
    /// Binary sensor types.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub binary_sensors: Vec<u8>,
    /// The multilevel switch's primary type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub switch_type: Option<u8>,
}

/// An endpoint: the root device (`0`) or a Multi Channel endpoint.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub struct Endpoint {
    /// Generic device class (Multi Channel endpoints only; 0 on the root).
    pub generic: u8,
    /// Specific device class (Multi Channel endpoints only; 0 on the root).
    pub specific: u8,
    /// Supported command classes, by command class id in decimal.
    pub ccs: BTreeMap<u16, CcSupport>,
    /// What the command classes reported in the interview.
    pub caps: Capabilities,
}

impl From<&zwave_host::node::Endpoint> for Endpoint {
    fn from(e: &zwave_host::node::Endpoint) -> Self {
        Endpoint {
            generic: e.generic,
            specific: e.specific,
            ccs: e
                .ccs
                .iter()
                .map(|(cc, s)| {
                    (
                        *cc,
                        CcSupport {
                            version: s.version,
                            secure: s.secure,
                        },
                    )
                })
                .collect(),
            caps: Capabilities {
                sensors: e.caps.sensors.clone(),
                meter: e.caps.meter.clone(),
                notifications: e.caps.notifications.clone(),
                binary_sensors: e.caps.binary_sensors.clone(),
                switch_type: e.caps.switch_type,
            },
        }
    }
}

/// The device file that describes a node, when `[devices] dir` has one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub struct DeviceFile {
    /// The file, relative to the devices directory.
    pub file: String,
    /// The manufacturer's name.
    pub manufacturer: String,
    /// The product's label.
    pub label: String,
    /// The product's description.
    pub description: String,
    /// What a multi-channel device calls each endpoint, such as
    /// `{"1": "Relay 1", "2": "Relay 2"}`. Endpoints without a name are left
    /// out.
    pub endpoints: BTreeMap<String, String>,
}

impl From<&zwave_host::device_config::DeviceConfig> for DeviceFile {
    fn from(d: &zwave_host::device_config::DeviceConfig) -> Self {
        DeviceFile {
            file: d.file.clone(),
            manufacturer: d.manufacturer.clone(),
            label: d.label.clone(),
            description: d.description.clone(),
            endpoints: endpoint_labels(d.endpoints.as_ref()),
        }
    }
}

/// A device file's endpoint names, `{"1": "Relay 1", "2": "Relay 2"}`.
/// Endpoints without a label are left out, so a device file that names none
/// gives `{}`.
pub fn endpoint_labels(endpoints: Option<&Json>) -> BTreeMap<String, String> {
    endpoints
        .and_then(Json::as_object)
        .map(|eps| {
            eps.iter()
                .filter_map(|(ep, e)| Some((ep.clone(), e.get("label")?.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// Everything the bridge knows about a node, retained on `nodes/<node>`.
/// Republished when the node joins, its interview moves on, its status
/// changes or it is renamed. An empty retained message clears it when the
/// node leaves.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(
    feature = "unstable-asyncapi-docs",
    derive(JsonSchema, ToAsyncApiMessage)
)]
#[cfg_attr(
    feature = "unstable-asyncapi-docs",
    asyncapi(summary = "A node, its interview and its status")
)]
pub struct Node {
    /// The node id: 2 to 232, or 256 and up for Long Range.
    pub id: u16,
    /// Protocol information from the controller.
    pub protocol: ProtocolInfo,
    /// How far the interview got.
    pub stage: InterviewStage,
    /// The security classes granted, highest first; empty when included
    /// without security.
    pub security: Vec<SecurityClass>,
    /// The node's secure command class list is known.
    pub secure_ccs_known: bool,
    /// The manufacturer and product, once the interview reads them.
    pub manufacturer: Option<Manufacturer>,
    /// The firmware version, `major.minor`.
    pub firmware: Option<String>,
    /// Z-Wave Plus role type, node type, installer icon and user icon.
    pub zwave_plus: Option<(u8, u8, u16, u16)>,
    /// The endpoints, by number; `0` is the root device.
    pub endpoints: BTreeMap<u8, Endpoint>,
    /// The wake-up interval the bridge set, in seconds (sleeping nodes).
    pub wake_up_interval: Option<u32>,
    /// Neighbors from the controller's routing table.
    pub neighbors: Vec<u16>,
    /// The name a person gave it with `rename`.
    pub name: Option<String>,
    /// The location a person gave it with `rename`.
    pub location: Option<String>,
    /// The DSK it was included with (S2), for SmartStart and support.
    pub dsk: Option<String>,
    /// Whether it answers.
    pub status: NodeStatus,
    /// The device file that describes it, when `[devices] dir` has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceFile>,
}

impl Node {
    /// The node record as the API writes it.
    pub fn new(
        r: &zwave_host::NodeRecord,
        status: zwave_host::NodeStatus,
        device: Option<&zwave_host::device_config::DeviceConfig>,
    ) -> Self {
        Node {
            id: r.id,
            protocol: r.protocol.into(),
            stage: r.stage.into(),
            security: classes(&r.security),
            secure_ccs_known: r.secure_ccs_known,
            manufacturer: r.manufacturer.map(|m| Manufacturer {
                manufacturer_id: m.manufacturer_id,
                product_type: m.product_type,
                product_id: m.product_id,
            }),
            firmware: r.firmware.clone(),
            zwave_plus: r.zwave_plus,
            endpoints: r.endpoints.iter().map(|(n, e)| (*n, e.into())).collect(),
            wake_up_interval: r.wake_up_interval,
            neighbors: r.neighbors.clone(),
            name: r.name.clone(),
            location: r.location.clone(),
            dsk: r.dsk.clone(),
            status: status.into(),
            device: device.map(DeviceFile::from),
        }
    }
}

/// A named value of a configuration parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub struct ParameterOption {
    /// The value.
    pub value: i64,
    /// What it means.
    pub label: String,
}

/// A configuration parameter from the node's device file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub struct Parameter {
    /// The key `configure` takes: the parameter number, as `"41"`, or a
    /// partial parameter with its bit mask, as `"41[0xf00]"`.
    pub key: String,
    /// The parameter's name.
    pub label: String,
    /// What it does.
    pub description: Option<String>,
    /// Its size in bytes: 1, 2 or 4.
    pub size: u8,
    /// The smallest value.
    pub min: i64,
    /// The largest value.
    pub max: i64,
    /// The value the device ships with.
    pub default: i64,
    /// The value is unsigned.
    pub unsigned: bool,
    /// The parameter cannot be written.
    pub read_only: bool,
    /// The value's unit.
    pub unit: Option<String>,
    /// Named values; empty when any value in range goes.
    pub options: Vec<ParameterOption>,
}

impl From<&zwave_host::device_config::ParamConfig> for Parameter {
    fn from(p: &zwave_host::device_config::ParamConfig) -> Self {
        Parameter {
            key: p.key.clone(),
            label: p.label.clone(),
            description: p.description.clone(),
            size: p.size,
            min: p.min,
            max: p.max,
            default: p.default,
            unsigned: p.unsigned,
            read_only: p.read_only,
            unit: p.unit.clone(),
            options: p
                .options
                .iter()
                .map(|(value, label)| ParameterOption {
                    value: *value,
                    label: label.clone(),
                })
                .collect(),
        }
    }
}

/// A node's configuration parameters, retained on `nodes/<node>/parameters`,
/// from its device file. Published after the node's interview when
/// `[devices] dir` has a file for it; set them with `configure`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(
    feature = "unstable-asyncapi-docs",
    derive(JsonSchema, ToAsyncApiMessage)
)]
#[cfg_attr(
    feature = "unstable-asyncapi-docs",
    asyncapi(summary = "The node's configuration parameters")
)]
#[serde(transparent)]
pub struct NodeParameters(pub Vec<Parameter>);

// ---------------------------------------------------------------------------
// Values, routes and commands

/// A value's reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
#[serde(untagged)]
pub enum Reading {
    /// On or off, open or closed, detected or idle.
    Bool(bool),
    /// A level, 0 to 99; 255 is "on at the last level".
    Level(u8),
    /// A measurement, in `unit`.
    Number(f64),
}

/// A value, retained on `nodes/<node>/<command class>/<property>`, or
/// `.../<property>/<endpoint>` for a Multi Channel endpoint. Published each
/// time the node reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "unstable-asyncapi-docs",
    derive(JsonSchema, ToAsyncApiMessage)
)]
#[cfg_attr(
    feature = "unstable-asyncapi-docs",
    asyncapi(summary = "A value the node reported")
)]
pub struct ValueMessage {
    /// When the bridge received it, in milliseconds since the Unix epoch.
    pub time: u64,
    /// What the value is, such as `Air temperature`.
    pub label: String,
    /// The reading. For a notification, the event's id (0 is idle).
    pub value: Reading,
    /// For a notification, the event's name, such as `Motion detection`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// The unit, such as `°C`, `%` or `kWh`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

impl ValueMessage {
    /// A value update as the API writes it, received at `time`.
    pub fn new(v: &zwave_host::ValueUpdate, time: u64) -> Self {
        use zwave_host::ValueKind;
        let (value, state) = match &v.value {
            ValueKind::Bool(b) => (Reading::Bool(*b), None),
            ValueKind::Number(n) => (Reading::Number(*n), None),
            ValueKind::Level(l) => (Reading::Level(*l), None),
            ValueKind::State { id, label } => (Reading::Level(*id), Some(label.clone())),
        };
        ValueMessage {
            time,
            label: v.label.clone(),
            value,
            state,
            unit: v.unit.map(String::from),
        }
    }
}

/// What to set a value to, on `nodes/<node>/<command class>/<property>/set`
/// (with `/<endpoint>` before `/set` for a Multi Channel endpoint). The
/// payload is a bare value or `{"value": ...}`: `true`/`false`, a level
/// 0 to 99 (255 is "on at the last level"), or `ON`/`OFF` in any case,
/// quoted or not. A binary switch takes any number as on unless it is 0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "unstable-asyncapi-docs",
    derive(JsonSchema, ToAsyncApiMessage)
)]
#[cfg_attr(
    feature = "unstable-asyncapi-docs",
    asyncapi(summary = "Switch, dim or set a value")
)]
#[serde(transparent)]
pub struct SetRequest(pub SetPayload);

/// A bare value, or one in an object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
#[serde(untagged)]
pub enum SetPayload {
    /// The value in an object.
    Wrapped {
        /// The value.
        value: SetValue,
    },
    /// The value alone.
    Bare(SetValue),
}

/// A value to set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
#[serde(untagged)]
pub enum SetValue {
    /// On or off.
    Bool(bool),
    /// A level, 0 to 99, or 255 for on at the last level.
    Number(f64),
    /// `ON`, `OFF`, `TRUE` or `FALSE`, in any case.
    Text(String),
}

impl SetRequest {
    /// Reads a payload: JSON, or a bare word such as `ON`.
    pub fn parse(payload: &[u8]) -> Self {
        let text = String::from_utf8_lossy(payload).trim().to_string();
        serde_json::from_str(&text).unwrap_or(SetRequest(SetPayload::Bare(SetValue::Text(text))))
    }

    /// The value, wrapped or not.
    pub fn value(&self) -> &SetValue {
        match &self.0 {
            SetPayload::Wrapped { value } | SetPayload::Bare(value) => value,
        }
    }
}

/// The route of the last frame that reached a node, retained on
/// `nodes/<node>/route`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "unstable-asyncapi-docs",
    derive(JsonSchema, ToAsyncApiMessage)
)]
#[cfg_attr(
    feature = "unstable-asyncapi-docs",
    asyncapi(summary = "The route the node was last reached by")
)]
pub struct Route {
    /// The repeaters, from the controller outwards; empty when direct, and
    /// always for Long Range.
    pub repeaters: Vec<u16>,
    /// The RSSI of the node's acknowledgement at the controller, in dBm.
    pub rssi: Option<i8>,
    /// When, in milliseconds since the Unix epoch.
    pub time: u64,
}

/// A command a node sent that the bridge did not turn into a value, on
/// `nodes/<node>/command`: a scene, a central scene key press, an
/// unsupported report. Not retained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "unstable-asyncapi-docs",
    derive(JsonSchema, ToAsyncApiMessage)
)]
#[cfg_attr(
    feature = "unstable-asyncapi-docs",
    asyncapi(summary = "A command the node sent")
)]
pub struct NodeCommand {
    /// The endpoint it came from; 0 is the root device.
    pub endpoint: u8,
    /// The command class id.
    pub cc: u16,
    /// The command's name, such as `central_scene_notification`.
    pub command: String,
    /// The command as received, hex.
    pub frame: String,
}

// ---------------------------------------------------------------------------
// Events

/// A SmartStart provisioning list entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub struct ProvisioningEntry {
    /// The node's DSK.
    pub dsk: String,
    /// The security classes it gets.
    pub classes: Vec<SecurityClass>,
    /// The node id, once it is included.
    pub node: Option<u16>,
    /// A name for people.
    pub name: Option<String>,
    /// The node is included when it asks; `false` keeps the entry but
    /// ignores it.
    pub active: bool,
}

impl From<&zwave_security::provisioning::Entry> for ProvisioningEntry {
    fn from(e: &zwave_security::provisioning::Entry) -> Self {
        ProvisioningEntry {
            dsk: e.dsk.to_string(),
            classes: classes(&e.granted),
            node: e.node_id,
            name: e.name.clone(),
            active: e.status == Default::default(),
        }
    }
}

/// The provisioning list as the API writes it.
pub fn provisioning(entries: &[zwave_security::provisioning::Entry]) -> Vec<ProvisioningEntry> {
    entries.iter().map(ProvisioningEntry::from).collect()
}

/// Something that happened in the network, on `bridge/event`. Not
/// retained. `event` names it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "unstable-asyncapi-docs",
    derive(JsonSchema, ToAsyncApiMessage)
)]
#[serde(tag = "event")]
pub enum BridgeEvent {
    /// The controller listens for a device to include.
    #[serde(rename = "inclusion_started")]
    #[cfg_attr(
        feature = "unstable-asyncapi-docs",
        asyncapi(summary = "Inclusion started")
    )]
    InclusionStarted,
    /// The controller stopped listening for a device to include.
    #[serde(rename = "inclusion_stopped")]
    #[cfg_attr(
        feature = "unstable-asyncapi-docs",
        asyncapi(summary = "Inclusion stopped")
    )]
    InclusionStopped,
    /// The controller listens for a device to exclude.
    #[serde(rename = "exclusion_started")]
    #[cfg_attr(
        feature = "unstable-asyncapi-docs",
        asyncapi(summary = "Exclusion started")
    )]
    ExclusionStarted,
    /// The controller stopped listening for a device to exclude.
    #[serde(rename = "exclusion_stopped")]
    #[cfg_attr(
        feature = "unstable-asyncapi-docs",
        asyncapi(summary = "Exclusion stopped")
    )]
    ExclusionStopped,
    /// A node joined the network.
    #[serde(rename = "node_added")]
    #[cfg_attr(
        feature = "unstable-asyncapi-docs",
        asyncapi(summary = "A node joined")
    )]
    NodeAdded {
        /// The new node.
        node: u16,
    },
    /// A node left the network, was excluded or was removed as failed.
    #[serde(rename = "node_removed")]
    #[cfg_attr(feature = "unstable-asyncapi-docs", asyncapi(summary = "A node left"))]
    NodeRemoved {
        /// The node.
        node: u16,
    },
    /// A joining S2 node asks for these classes. Answer with `grant`.
    #[serde(rename = "grant_security_classes")]
    #[cfg_attr(
        feature = "unstable-asyncapi-docs",
        asyncapi(summary = "A joining node asks for security classes")
    )]
    GrantSecurityClasses {
        /// The joining node.
        node: u16,
        /// The classes it asks for, highest first.
        requested: Vec<SecurityClass>,
    },
    /// Confirm the joining node's DSK. Answer with `validate_dsk` and the
    /// PIN, the first five digits of the DSK on the device.
    #[serde(rename = "validate_dsk")]
    #[cfg_attr(
        feature = "unstable-asyncapi-docs",
        asyncapi(summary = "Confirm the joining node's PIN")
    )]
    ValidateDsk {
        /// The joining node.
        node: u16,
        /// Its DSK with the PIN hidden, `xxxxx-12345-...`.
        dsk: String,
    },
    /// The key exchange finished.
    #[serde(rename = "security_bootstrapped")]
    #[cfg_attr(
        feature = "unstable-asyncapi-docs",
        asyncapi(summary = "The node has its keys")
    )]
    SecurityBootstrapped {
        /// The node.
        node: u16,
        /// The classes granted, highest first.
        classes: Vec<SecurityClass>,
    },
    /// The key exchange failed; the node is included without security.
    #[serde(rename = "security_failed")]
    #[cfg_attr(
        feature = "unstable-asyncapi-docs",
        asyncapi(summary = "The key exchange failed")
    )]
    SecurityFailed {
        /// The node.
        node: u16,
        /// Why.
        reason: String,
    },
    /// A node's interview reached a stage.
    #[serde(rename = "interview_stage")]
    #[cfg_attr(
        feature = "unstable-asyncapi-docs",
        asyncapi(summary = "The interview moved on")
    )]
    InterviewStage {
        /// The node.
        node: u16,
        /// The stage just reached.
        stage: InterviewStage,
    },
    /// A node's interview finished.
    #[serde(rename = "interview_completed")]
    #[cfg_attr(
        feature = "unstable-asyncapi-docs",
        asyncapi(summary = "The interview finished")
    )]
    InterviewCompleted {
        /// The node.
        node: u16,
    },
    /// A node's interview stopped; it resumes when the node is next heard,
    /// or with `reinterview`.
    #[serde(rename = "interview_failed")]
    #[cfg_attr(
        feature = "unstable-asyncapi-docs",
        asyncapi(summary = "The interview failed")
    )]
    InterviewFailed {
        /// The node.
        node: u16,
        /// Why.
        reason: String,
    },
    /// Fragments sent so far in a firmware update.
    #[serde(rename = "firmware_update_progress")]
    #[cfg_attr(
        feature = "unstable-asyncapi-docs",
        asyncapi(summary = "A firmware update's progress")
    )]
    FirmwareUpdateProgress {
        /// The node.
        node: u16,
        /// Fragments sent.
        sent: u16,
        /// Fragments in the image.
        total: u16,
    },
    /// A firmware update ended.
    #[serde(rename = "firmware_update_finished")]
    #[cfg_attr(
        feature = "unstable-asyncapi-docs",
        asyncapi(summary = "A firmware update ended")
    )]
    FirmwareUpdateFinished {
        /// The node.
        node: u16,
        /// The node's status: 255 done and restarting, 254 done without a
        /// restart, 253 waiting for activation; below 16 an error.
        status: u8,
        /// The node accepted the image.
        success: bool,
    },
    /// The SmartStart provisioning list changed.
    #[serde(rename = "provisioning_changed")]
    #[cfg_attr(
        feature = "unstable-asyncapi-docs",
        asyncapi(summary = "The provisioning list changed")
    )]
    ProvisioningChanged {
        /// The whole list.
        entries: Vec<ProvisioningEntry>,
    },
}

// ---------------------------------------------------------------------------
// Requests and responses

macro_rules! response {
    ($(#[$doc:meta])* $name:ident, $data:ty, $summary:literal) => {
        $(#[$doc])*
        ///
        /// A response is published once per request, to
        /// `bridge/response/<request>`, and is not retained. Requests that
        /// wait for a device answer when they finish.
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema, ToAsyncApiMessage))]
        #[cfg_attr(feature = "unstable-asyncapi-docs", asyncapi(summary = $summary))]
        pub struct $name {
            /// Whether it was done; when `false`, `error` says why.
            pub ok: bool,
            /// The result's fields, when `ok` is `true`.
            #[serde(flatten, default, skip_serializing_if = "Option::is_none")]
            pub data: Option<$data>,
            /// What went wrong, when `ok` is `false`.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub error: Option<String>,
            /// The request's `transaction`, echoed when it had one.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub transaction: Option<Json>,
        }

        impl $name {
            /// The response to a request with `transaction`.
            pub fn new(result: Result<$data, String>, transaction: Option<Json>) -> Self {
                match result {
                    Ok(data) => $name { ok: true, data: Some(data), error: None, transaction },
                    Err(error) => $name { ok: false, data: None, error: Some(error), transaction },
                }
            }
        }
    };
}

macro_rules! request {
    ($(#[$doc:meta])* $name:ident, $summary:literal { $($(#[$fdoc:meta])* $field:ident : $ty:ty),* $(,)? }) => {
        $(#[$doc])*
        ///
        /// An empty payload is the same as `{}`.
        #[derive(Debug, Clone, Default, Serialize, Deserialize)]
        #[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema, ToAsyncApiMessage))]
        #[cfg_attr(feature = "unstable-asyncapi-docs", asyncapi(summary = $summary))]
        pub struct $name {
            $($(#[$fdoc])* #[serde(default)] pub $field: $ty,)*
            /// Any value; the response echoes it, so a client can match the
            /// two.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub transaction: Option<Json>,
        }
    };
}

/// An empty result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub struct Empty {}

/// A node.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub struct NodeData {
    /// The node id.
    pub node: u16,
}

/// How to secure a device being included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub enum InclusionStrategy {
    /// Grant every class the device asks for, without a prompt.
    #[serde(rename = "requested")]
    Requested,
    /// Include the device without security.
    #[serde(rename = "insecure")]
    Insecure,
}

request!(
    /// Include a device, on `bridge/request/include`. Without `strategy`
    /// or `classes`, the bridge asks which classes to grant with a
    /// `grant_security_classes` event and, for S2 Authenticated and Access
    /// Control, the PIN with `validate_dsk`.
    IncludeRequest, "Include a device" {
        /// `requested` grants what the device asks for; `insecure` includes
        /// it without security.
        strategy: Option<InclusionStrategy>,
        /// Grant these classes without a prompt.
        classes: Option<Vec<SecurityClass>>,
        /// The device's PIN, the first five digits of its DSK, given up
        /// front so the bridge does not ask.
        pin: Option<String>,
        /// Seconds to listen, instead of `network.inclusion_timeout`.
        timeout: Option<u64>,
    }
);

/// The result of `include`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub struct IncludeData {
    /// The new node.
    pub node: u16,
    /// The classes granted, highest first; empty when included without
    /// security.
    pub security: Vec<SecurityClass>,
    /// Why security failed, when it did: the node is then included without
    /// it.
    pub security_error: Option<String>,
}

response!(
    /// The response to `include`, once a device joined or listening timed out.
    IncludeResponse, IncludeData, "A device is included"
);

request!(
    /// Exclude a device, on `bridge/request/exclude`: the controller listens
    /// until the device's button is pressed.
    ExcludeRequest, "Exclude a device" {
        /// Seconds to listen, instead of `network.inclusion_timeout`.
        timeout: Option<u64>,
    }
);

/// The result of `exclude`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub struct ExcludeData {
    /// The node that left; `null` for a device from another network, which
    /// is reset all the same.
    pub node: Option<u16>,
}

response!(
    /// The response to `exclude`.
    ExcludeResponse, ExcludeData, "A device is excluded"
);

request!(
    /// Stop an inclusion or exclusion, on `bridge/request/stop`.
    StopRequest, "Stop including or excluding" {}
);

response!(
    /// The response to `stop`: `ok` says whether an inclusion or exclusion
    /// was running.
    StopResponse, Empty, "Including or excluding stopped"
);

request!(
    /// Grant security classes to the joining node, on
    /// `bridge/request/grant`, after a `grant_security_classes` event.
    GrantRequest, "Grant security classes" {
        /// The classes to grant; empty includes the node without security.
        classes: Vec<SecurityClass>,
    }
);

response!(
    /// The response to `grant`: `ok` is `false` when no node was waiting.
    GrantResponse, Empty, "The classes are granted"
);

request!(
    /// Confirm the joining node's PIN, on `bridge/request/validate_dsk`,
    /// after a `validate_dsk` event.
    ValidateDskRequest, "Confirm the joining node's PIN" {
        /// The first five digits of the DSK on the device; absent to
        /// refuse it.
        pin: Option<String>,
    }
);

response!(
    /// The response to `validate_dsk`: `ok` is `false` when no node was
    /// waiting.
    ValidateDskResponse, Empty, "The PIN is passed on"
);

request!(
    /// Add a SmartStart device to the provisioning list, on
    /// `bridge/request/provision`: it is included when it is powered on.
    ProvisionRequest, "Add a SmartStart device" {
        /// The device's QR code, `90...`.
        qr: Option<String>,
        /// Or its DSK, `12345-12345-...`.
        dsk: Option<String>,
        /// With a DSK, the classes to grant; S2 Authenticated and
        /// Unauthenticated when absent.
        classes: Option<Vec<SecurityClass>>,
        /// With a DSK, a name for people.
        name: Option<String>,
    }
);

/// A DSK.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub struct DskData {
    /// The device's DSK.
    pub dsk: String,
}

response!(
    /// The response to `provision`.
    ProvisionResponse, DskData, "The device is provisioned"
);

request!(
    /// Remove a device from the provisioning list, on
    /// `bridge/request/unprovision`.
    UnprovisionRequest, "Remove a SmartStart device" {
        /// The device's DSK.
        dsk: Option<String>,
    }
);

/// The result of `unprovision`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub struct UnprovisionData {
    /// The list had the device.
    pub removed: bool,
}

response!(
    /// The response to `unprovision`.
    UnprovisionResponse, UnprovisionData, "The device is no longer provisioned"
);

request!(
    /// Read the SmartStart provisioning list, on
    /// `bridge/request/provisioning_list`.
    ProvisioningListRequest, "Read the provisioning list" {}
);

/// The result of `provisioning_list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub struct ProvisioningListData {
    /// Every entry.
    pub entries: Vec<ProvisioningEntry>,
}

response!(
    /// The response to `provisioning_list`.
    ProvisioningListResponse, ProvisioningListData, "The provisioning list"
);

request!(
    /// Back up the controller's memory to the store directory, on
    /// `bridge/request/backup`. The backup holds the network, so it is
    /// written to disk and never published.
    BackupRequest, "Back up the controller" {}
);

/// The result of `backup`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub struct BackupData {
    /// The file written, `<store dir>/nvm-<unix time>.bin`.
    pub file: String,
    /// Its size in bytes.
    pub bytes: usize,
}

response!(
    /// The response to `backup`.
    BackupResponse, BackupData, "The backup is written"
);

request!(
    /// Restore a backup onto the controller, on `bridge/request/restore`.
    /// A backup from a 700 series controller restores onto an 800 series one
    /// and back. The bridge then exits for its service manager to restart it.
    RestoreRequest, "Restore a backup" {
        /// The backup's name in the store directory, such as
        /// `nvm-1763600000.bin`.
        file: Option<String>,
    }
);

/// The result of `restore`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub struct RestoreData {
    /// The backup restored.
    pub file: String,
    /// The bridge is restarting: always `true`.
    pub restart: bool,
}

response!(
    /// The response to `restore`.
    RestoreResponse, RestoreData, "The backup is restored"
);

request!(
    /// Interview a node again, on `bridge/request/reinterview`.
    ReinterviewRequest, "Interview a node again" {
        /// The node.
        node: Option<u16>,
    }
);

response!(
    /// The response to `reinterview`, once the interview is queued.
    ReinterviewResponse, Empty, "The interview is queued"
);

request!(
    /// Remove a node that no longer answers, on
    /// `bridge/request/remove_failed`. The controller refuses a node that
    /// answers.
    RemoveFailedRequest, "Remove a failed node" {
        /// The node.
        node: Option<u16>,
    }
);

response!(
    /// The response to `remove_failed`.
    RemoveFailedResponse, NodeData, "The node is removed"
);

request!(
    /// Give a node a name and a location, on `bridge/request/rename`. Home
    /// Assistant shows the name.
    RenameRequest, "Name a node" {
        /// The node.
        node: Option<u16>,
        /// Its name; absent clears it.
        name: Option<String>,
        /// Its location; absent clears it.
        location: Option<String>,
    }
);

response!(
    /// The response to `rename`.
    RenameResponse, Empty, "The node is renamed"
);

request!(
    /// Update a node's firmware, on `bridge/request/firmware_update`, from a
    /// file in `<store dir>/firmware/`. Intel HEX is converted; `.gbl`, `.bin`
    /// and `.ota` files are sent as they are. Progress comes as events.
    FirmwareUpdateRequest, "Update a node's firmware" {
        /// The node.
        node: Option<u16>,
        /// The file's name in the firmware directory.
        file: Option<String>,
        /// The firmware target: 0, the default, is the node's own firmware.
        target: Option<u8>,
    }
);

/// The result of `firmware_update`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub struct FirmwareUpdateData {
    /// The node.
    pub node: u16,
    /// The node's status: 255 done and restarting, 254 done without a
    /// restart, 253 waiting for activation; below 16 an error.
    pub status: u8,
    /// The node accepted the image.
    pub success: bool,
    /// Seconds until the node is back, when it says.
    pub wait_time: Option<u16>,
    /// Whether activation succeeded, when it was needed.
    pub activated: Option<bool>,
}

response!(
    /// The response to `firmware_update`, once the update ends.
    FirmwareUpdateResponse, FirmwareUpdateData, "The firmware update ended"
);

request!(
    /// Set a node's configuration parameters by key, on
    /// `bridge/request/configure`, from its device file. Partial parameters
    /// are merged into their parameter.
    ConfigureRequest, "Set configuration parameters" {
        /// The node.
        node: Option<u16>,
        /// Values by parameter key, as in `nodes/<node>/parameters`, such as
        /// `{"41": 1, "42[0xff00]": 3}`.
        #[cfg_attr(feature = "unstable-asyncapi-docs", schemars(with = "BTreeMap<String, i64>"))]
        settings: Map<String, Json>,
    }
);

response!(
    /// The response to `configure`, once every parameter is written.
    ConfigureResponse, NodeData, "The parameters are set"
);

request!(
    /// Ask every node for its neighbors, then read the mesh, on
    /// `bridge/request/topology`. Sleeping nodes answer when they wake.
    TopologyRequest, "Map the mesh" {}
);

/// A node in the mesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub struct TopologyNode {
    /// The node id.
    pub id: u16,
    /// Its neighbors in the controller's routing table.
    pub neighbors: Vec<u16>,
    /// The repeaters of the last frame that reached it, from the controller
    /// outwards; `null` until one did.
    pub route: Option<Vec<u16>>,
    /// The RSSI of its last acknowledgement at the controller, in dBm.
    pub rssi: Option<i8>,
    /// Always listening (mains powered).
    pub listening: bool,
}

/// The result of `topology`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub struct TopologyData {
    /// The controller's node id.
    pub controller: u16,
    /// Every node.
    pub nodes: Vec<TopologyNode>,
}

impl From<zwave_host::Topology> for TopologyData {
    fn from(t: zwave_host::Topology) -> Self {
        TopologyData {
            controller: t.controller,
            nodes: t
                .nodes
                .into_iter()
                .map(|n| TopologyNode {
                    id: n.id,
                    neighbors: n.neighbors,
                    route: n.last_route,
                    rssi: n.last_rssi,
                    listening: n.listening,
                })
                .collect(),
        }
    }
}

response!(
    /// The response to `topology`.
    TopologyResponse, TopologyData, "The mesh"
);

response!(
    /// The response to a request the bridge does not know, or one it could
    /// not read; `error` says which.
    ErrorResponse, Empty, "The request is unknown or unreadable"
);

// ---------------------------------------------------------------------------
// Home Assistant

/// A Home Assistant MQTT discovery config, retained on
/// `<discovery_prefix>/<component>/zwave_<home id>_<node>/<value>/config`:
/// one entity per value. Keys are Home Assistant's; see its MQTT integration
/// documentation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "unstable-asyncapi-docs",
    derive(JsonSchema, ToAsyncApiMessage)
)]
#[cfg_attr(
    feature = "unstable-asyncapi-docs",
    asyncapi(summary = "A Home Assistant entity for one of a node's values")
)]
pub struct DiscoveryConfig {
    /// The entity's name: the value's label.
    pub name: String,
    /// Unique across Home Assistant: `zwave_<home id>_<node>_<value>`.
    pub unique_id: String,
    /// The entity id Home Assistant derives, the same as `unique_id`.
    pub object_id: String,
    /// The value's topic.
    pub state_topic: String,
    /// The bridge's availability topic.
    pub availability_topic: String,
    /// The node, as a Home Assistant device.
    pub device: DiscoveryDevice,
    /// The value's `/set` topic, for switches and lights.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_topic: Option<String>,
    /// Picks the state out of the value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_template: Option<String>,
    /// Lights: picks on or off out of the value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_value_template: Option<String>,
    /// Lights: the value's topic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness_state_topic: Option<String>,
    /// Lights: picks the level out of the value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness_value_template: Option<String>,
    /// Lights: the value's `/set` topic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness_command_topic: Option<String>,
    /// Lights: the highest level, 99.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness_scale: Option<u8>,
    /// Lights: `brightness`, so turning on sends the level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_command_type: Option<String>,
    /// The payload for on: `ON`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_on: Option<String>,
    /// The payload for off: `OFF`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_off: Option<String>,
    /// Home Assistant's device class, such as `temperature` or `motion`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_class: Option<String>,
    /// `diagnostic` for battery levels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_category: Option<String>,
    /// The unit, in Home Assistant's spelling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_of_measurement: Option<String>,
    /// `measurement`, or `total_increasing` for energy meters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_class: Option<String>,
}

/// A node, as a Home Assistant device.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-asyncapi-docs", derive(JsonSchema))]
pub struct DiscoveryDevice {
    /// `zwave_<home id>_<node>`.
    pub identifiers: Vec<String>,
    /// The node's name, its device file's manufacturer and label, or
    /// `Z-Wave node <node>`.
    pub name: String,
    /// The controller, `zwave_<home id>_controller`.
    pub via_device: String,
    /// The manufacturer: from the device file, or the manufacturer id in hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    /// The model: from the device file, or product type and id in hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The node's firmware version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sw_version: Option<String>,
    /// The node's location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_area: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn names_the_labelled_endpoints() {
        let file = json!({"0": {"associations": {}}, "1": {"label": "Relay 1"}, "2": {"label": "Relay 2"}});
        assert_eq!(
            serde_json::to_value(endpoint_labels(Some(&file))).unwrap(),
            json!({"1": "Relay 1", "2": "Relay 2"})
        );
        assert!(endpoint_labels(None).is_empty());
    }

    #[test]
    fn the_node_record_reads_as_the_host_writes_it() {
        // The record the bridge publishes is the host's, field for field,
        // plus `status`: what clients read before these types existed.
        let mut r = zwave_host::NodeRecord::new(5);
        r.security = vec![zwave_security::SecurityClass::S2Authenticated];
        r.name = Some("Hallway".into());
        let mut host = serde_json::to_value(&r).unwrap();
        host["status"] = json!("alive");
        let api = serde_json::to_value(Node::new(&r, zwave_host::NodeStatus::Alive, None)).unwrap();
        assert_eq!(api, host);
    }

    #[test]
    fn responses_flatten_their_result() {
        let ok = IncludeResponse::new(
            Ok(IncludeData {
                node: 5,
                security: vec![SecurityClass::S2Authenticated],
                security_error: None,
            }),
            Some(json!("t-1")),
        );
        assert_eq!(
            serde_json::to_value(ok).unwrap(),
            json!({"ok": true, "node": 5, "security": ["s2_authenticated"], "security_error": null, "transaction": "t-1"})
        );
        let err = StopResponse::new(Err("no".into()), None);
        assert_eq!(
            serde_json::to_value(err).unwrap(),
            json!({"ok": false, "error": "no"})
        );
        assert_eq!(
            serde_json::to_value(StopResponse::new(Ok(Empty {}), None)).unwrap(),
            json!({"ok": true})
        );
    }

    #[test]
    fn events_are_tagged_by_name() {
        assert_eq!(
            serde_json::to_value(BridgeEvent::InclusionStarted).unwrap(),
            json!({"event": "inclusion_started"})
        );
        assert_eq!(
            serde_json::to_value(BridgeEvent::GrantSecurityClasses {
                node: 5,
                requested: vec![SecurityClass::S2Authenticated, SecurityClass::S0Legacy]
            })
            .unwrap(),
            json!({"event": "grant_security_classes", "node": 5, "requested": ["s2_authenticated", "s0_legacy"]})
        );
    }

    #[test]
    fn set_payloads() {
        assert_eq!(
            SetRequest::parse(b"ON").value(),
            &SetValue::Text("ON".into())
        );
        assert_eq!(SetRequest::parse(b" 42 ").value(), &SetValue::Number(42.0));
        assert_eq!(
            SetRequest::parse(br#"{"value": true}"#).value(),
            &SetValue::Bool(true)
        );
        assert_eq!(
            SetRequest::parse(br#""off""#).value(),
            &SetValue::Text("off".into())
        );
    }

    #[test]
    fn requested_classes_come_highest_first() {
        assert_eq!(
            host_classes(&[SecurityClass::S0Legacy, SecurityClass::S2AccessControl]),
            vec![
                zwave_security::SecurityClass::S2AccessControl,
                zwave_security::SecurityClass::S0Legacy
            ]
        );
    }
}
