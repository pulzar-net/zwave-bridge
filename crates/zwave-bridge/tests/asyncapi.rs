//! The MQTT reference on the docs site is an AsyncAPI description generated
//! from the payload types in `zwave_bridge::api`, the types the bridge
//! publishes, so it cannot drift from the code. This test fails when the
//! committed description differs from what the types say; regenerate it with
//!
//! ```sh
//! UPDATE_DOCS=1 cargo test -p zwave-bridge --features unstable-asyncapi-docs --test asyncapi
//! ```
//!
//! Topics are written with the default base topic (`zwave`) and discovery
//! prefix (`homeassistant`); the description says how to change them.

#![allow(clippy::duplicated_attributes)] // operations share channels by design

use std::path::PathBuf;

use asyncapi_rust::AsyncApi;
use zwave_bridge::api::*;

const SPEC: &str = "docs/public/asyncapi.json";

/// The bridge's MQTT API.
#[derive(AsyncApi)]
#[asyncapi(
    title = "zwave-bridge MQTT API",
    version = "0.0.0",
    description = "Everything zwave-bridge publishes and accepts on MQTT.\n\nTopics here use the default base topic, `zwave` (`mqtt.base_topic` in the configuration), and Home Assistant's default discovery prefix, `homeassistant` (`homeassistant.discovery_prefix`). `{node}` is a node id: 2 to 232, or 256 and up for Long Range.\n\nThe bridge publishes and subscribes at QoS 1. Payloads are JSON unless a message says otherwise. Requests go to `zwave/bridge/request/<name>`; the answer comes on `zwave/bridge/response/<name>` with `ok` and the result's fields, or `ok: false` and an `error`, echoing the request's `transaction` so a client can match the two. Network keys never appear on MQTT."
)]
#[asyncapi_server(
    name = "broker",
    host = "localhost:1883",
    protocol = "mqtt",
    description = "Your MQTT broker: `mqtt.host` and `mqtt.port` in the configuration. The bridge connects with its `mqtt.client_id` and `zwave/bridge/state` set to offline as its last will.",
    mqtt(
        client_id = "zwave-bridge",
        clean_session = true,
        keep_alive = 30,
        last_will(
            topic = "zwave/bridge/state",
            qos = 1,
            message = "offline",
            retain = true
        )
    )
)]
// The bridge.
#[asyncapi_channel(
    name = "bridgeState",
    address = "zwave/bridge/state",
    description = "The bridge's availability, retained, as plain text. `offline` is also the bridge's last will."
)]
#[asyncapi_channel(
    name = "bridgeInfo",
    address = "zwave/bridge/info",
    description = "The bridge, its controller and its network, retained."
)]
#[asyncapi_channel(
    name = "bridgeEvent",
    address = "zwave/bridge/event",
    description = "Inclusion, exclusion, security, interviews, firmware updates and SmartStart, as they happen. `event` names the event."
)]
// Nodes.
#[asyncapi_channel(
    name = "node",
    address = "zwave/nodes/{node}",
    description = "A node's record: protocol information, interview, security, endpoints, name and status. Retained; an empty retained message clears it when the node leaves.",
    parameter(
        name = "node",
        description = "The node id.",
        examples = ["5", "260"]
    )
)]
#[asyncapi_channel(
    name = "nodeStatus",
    address = "zwave/nodes/{node}/status",
    description = "A node's status as plain text, retained; an empty retained message clears it when the node leaves.",
    parameter(
        name = "node",
        description = "The node id.",
        examples = ["5", "260"]
    )
)]
#[asyncapi_channel(
    name = "nodeParameters",
    address = "zwave/nodes/{node}/parameters",
    description = "A node's configuration parameters from its device file, retained. Only with `[devices] dir`.",
    parameter(
        name = "node",
        description = "The node id.",
        examples = ["5", "260"]
    )
)]
#[asyncapi_channel(
    name = "nodeValue",
    address = "zwave/nodes/{node}/{command_class}/{property}",
    description = "A value each time the node reports it, retained. A Multi Channel endpoint's value has the endpoint after the property, as in `binary_switch/current/2`.",
    parameter(
        name = "node",
        description = "The node id.",
        examples = ["5", "260"]
    ),
    parameter(
        name = "command_class",
        description = "The command class's name.",
        examples = ["multilevel_sensor", "binary_switch", "notification"]
    ),
    parameter(
        name = "property",
        description = "The value's property, with `/<endpoint>` after it for a Multi Channel endpoint.",
        examples = ["air_temperature", "current", "home_security_motion_sensor_status"]
    )
)]
#[asyncapi_channel(
    name = "nodeValueSet",
    address = "zwave/nodes/{node}/{command_class}/{property}/set",
    description = "Set a value: switch, dim or turn on and off. For a Multi Channel endpoint the endpoint goes before `/set`.",
    parameter(
        name = "node",
        description = "The node id.",
        examples = ["5", "260"]
    ),
    parameter(
        name = "command_class",
        description = "The command class's name.",
        examples = ["multilevel_sensor", "binary_switch", "notification"]
    ),
    parameter(
        name = "property",
        description = "The value's property, with `/<endpoint>` after it for a Multi Channel endpoint.",
        examples = ["air_temperature", "current", "home_security_motion_sensor_status"]
    )
)]
#[asyncapi_channel(
    name = "nodeRoute",
    address = "zwave/nodes/{node}/route",
    description = "The route the node was last reached by, retained.",
    parameter(
        name = "node",
        description = "The node id.",
        examples = ["5", "260"]
    )
)]
#[asyncapi_channel(
    name = "nodeCommand",
    address = "zwave/nodes/{node}/command",
    description = "Commands the node sends that are not values: scenes, central scene key presses, unsupported reports.",
    parameter(
        name = "node",
        description = "The node id.",
        examples = ["5", "260"]
    )
)]
// Requests and their responses.
#[asyncapi_channel(
    name = "includeRequest",
    address = "zwave/bridge/request/include",
    description = "Include a device. Answers when a device joined or listening timed out."
)]
#[asyncapi_channel(
    name = "includeResponse",
    address = "zwave/bridge/response/include",
    description = "The answer to `include`."
)]
#[asyncapi_channel(
    name = "excludeRequest",
    address = "zwave/bridge/request/exclude",
    description = "Exclude a device. Answers when a device left or listening timed out."
)]
#[asyncapi_channel(
    name = "excludeResponse",
    address = "zwave/bridge/response/exclude",
    description = "The answer to `exclude`."
)]
#[asyncapi_channel(
    name = "stopRequest",
    address = "zwave/bridge/request/stop",
    description = "Stop an inclusion or exclusion."
)]
#[asyncapi_channel(
    name = "stopResponse",
    address = "zwave/bridge/response/stop",
    description = "The answer to `stop`."
)]
#[asyncapi_channel(
    name = "grantRequest",
    address = "zwave/bridge/request/grant",
    description = "Grant security classes to the joining node."
)]
#[asyncapi_channel(
    name = "grantResponse",
    address = "zwave/bridge/response/grant",
    description = "The answer to `grant`."
)]
#[asyncapi_channel(
    name = "validateDskRequest",
    address = "zwave/bridge/request/validate_dsk",
    description = "Confirm the joining node's PIN."
)]
#[asyncapi_channel(
    name = "validateDskResponse",
    address = "zwave/bridge/response/validate_dsk",
    description = "The answer to `validate_dsk`."
)]
#[asyncapi_channel(
    name = "provisionRequest",
    address = "zwave/bridge/request/provision",
    description = "Add a SmartStart device to the provisioning list."
)]
#[asyncapi_channel(
    name = "provisionResponse",
    address = "zwave/bridge/response/provision",
    description = "The answer to `provision`."
)]
#[asyncapi_channel(
    name = "unprovisionRequest",
    address = "zwave/bridge/request/unprovision",
    description = "Remove a device from the provisioning list."
)]
#[asyncapi_channel(
    name = "unprovisionResponse",
    address = "zwave/bridge/response/unprovision",
    description = "The answer to `unprovision`."
)]
#[asyncapi_channel(
    name = "provisioningListRequest",
    address = "zwave/bridge/request/provisioning_list",
    description = "Read the SmartStart provisioning list."
)]
#[asyncapi_channel(
    name = "provisioningListResponse",
    address = "zwave/bridge/response/provisioning_list",
    description = "The answer to `provisioning_list`."
)]
#[asyncapi_channel(
    name = "backupRequest",
    address = "zwave/bridge/request/backup",
    description = "Back up the controller's memory to the store directory. The backup holds the network, so it is never published."
)]
#[asyncapi_channel(
    name = "backupResponse",
    address = "zwave/bridge/response/backup",
    description = "The answer to `backup`."
)]
#[asyncapi_channel(
    name = "restoreRequest",
    address = "zwave/bridge/request/restore",
    description = "Restore a backup onto the controller, converting it between 700 and 800 series. The bridge then exits for its service manager to restart it."
)]
#[asyncapi_channel(
    name = "restoreResponse",
    address = "zwave/bridge/response/restore",
    description = "The answer to `restore`."
)]
#[asyncapi_channel(
    name = "reinterviewRequest",
    address = "zwave/bridge/request/reinterview",
    description = "Interview a node again."
)]
#[asyncapi_channel(
    name = "reinterviewResponse",
    address = "zwave/bridge/response/reinterview",
    description = "The answer to `reinterview`."
)]
#[asyncapi_channel(
    name = "removeFailedRequest",
    address = "zwave/bridge/request/remove_failed",
    description = "Remove a node that no longer answers."
)]
#[asyncapi_channel(
    name = "removeFailedResponse",
    address = "zwave/bridge/response/remove_failed",
    description = "The answer to `remove_failed`."
)]
#[asyncapi_channel(
    name = "renameRequest",
    address = "zwave/bridge/request/rename",
    description = "Name a node and give it a location."
)]
#[asyncapi_channel(
    name = "renameResponse",
    address = "zwave/bridge/response/rename",
    description = "The answer to `rename`."
)]
#[asyncapi_channel(
    name = "firmwareUpdateRequest",
    address = "zwave/bridge/request/firmware_update",
    description = "Update a node's firmware from the firmware directory. Answers when the update ends."
)]
#[asyncapi_channel(
    name = "firmwareUpdateResponse",
    address = "zwave/bridge/response/firmware_update",
    description = "The answer to `firmware_update`."
)]
#[asyncapi_channel(
    name = "configureRequest",
    address = "zwave/bridge/request/configure",
    description = "Set a node's configuration parameters by key, from its device file."
)]
#[asyncapi_channel(
    name = "configureResponse",
    address = "zwave/bridge/response/configure",
    description = "The answer to `configure`."
)]
#[asyncapi_channel(
    name = "topologyRequest",
    address = "zwave/bridge/request/topology",
    description = "Ask every node for its neighbors, then read the mesh."
)]
#[asyncapi_channel(
    name = "topologyResponse",
    address = "zwave/bridge/response/topology",
    description = "The answer to `topology`."
)]
// Home Assistant.
#[asyncapi_channel(
    name = "homeAssistantDiscovery",
    address = "homeassistant/{component}/{node_id}/{object_id}/config",
    description = "Home Assistant MQTT discovery: one retained config per value, sent the first time the value is reported after the bridge connects. Only with `homeassistant.enabled`.",
    parameter(
        name = "component",
        description = "The entity type.",
        enum_values = ["sensor", "binary_sensor", "switch", "light"]
    ),
    parameter(
        name = "node_id",
        description = "The node, as `zwave_<home id>_<node>`.",
        examples = ["zwave_c0ffee01_5"]
    ),
    parameter(
        name = "object_id",
        description = "The value's key with `_` for `/`, as `multilevel_sensor_air_temperature`.",
        examples = ["multilevel_sensor_air_temperature"]
    )
)]
// What the bridge publishes and receives.
#[asyncapi_operation(name = "publishBridgeState", summary = "The bridge is online or offline", action = "send", channel = "bridgeState", description = "Published `online` on connecting and `offline` on stopping.", messages = [BridgeState], mqtt(qos = 1, retain = true))]
#[asyncapi_operation(name = "publishBridgeInfo", summary = "The bridge's version, controller and network", action = "send", channel = "bridgeInfo", description = "Published on connecting and when the controller is ready.", messages = [BridgeInfo], mqtt(qos = 1, retain = true))]
#[asyncapi_operation(name = "publishBridgeEvent", summary = "Something happened in the network", action = "send", channel = "bridgeEvent", description = "Published as each event happens.", messages = [BridgeEvent], mqtt(qos = 1, retain = false))]
#[asyncapi_operation(name = "publishNode", summary = "A node, its interview and its status", action = "send", channel = "node", description = "Published on connecting, and when the node joins, its interview moves on, its status changes or it is renamed.", messages = [Node], mqtt(qos = 1, retain = true))]
#[asyncapi_operation(name = "publishNodeStatus", summary = "Whether the node answers", action = "send", channel = "nodeStatus", description = "Published with the node's record.", messages = [NodeStatusMessage], mqtt(qos = 1, retain = true))]
#[asyncapi_operation(name = "publishNodeParameters", summary = "The node's configuration parameters", action = "send", channel = "nodeParameters", description = "Published when the node's interview finishes and its device file is found.", messages = [NodeParameters], mqtt(qos = 1, retain = true))]
#[asyncapi_operation(name = "publishNodeValue", summary = "A value the node reported", action = "send", channel = "nodeValue", description = "Published for each report.", messages = [ValueMessage], mqtt(qos = 1, retain = true))]
#[asyncapi_operation(name = "publishNodeRoute", summary = "The route the node was last reached by", action = "send", channel = "nodeRoute", description = "Published when a frame reaches the node by a new route.", messages = [Route], mqtt(qos = 1, retain = true))]
#[asyncapi_operation(name = "publishNodeCommand", summary = "A command the node sent", action = "send", channel = "nodeCommand", description = "Published for each such command.", messages = [NodeCommand], mqtt(qos = 1, retain = false))]
#[asyncapi_operation(name = "publishDiscovery", summary = "A Home Assistant entity", action = "send", channel = "homeAssistantDiscovery", description = "Published per value.", messages = [DiscoveryConfig], mqtt(qos = 1, retain = true))]
#[asyncapi_operation(name = "receiveNodeValueSet", summary = "Switch, dim or set a value", action = "receive", channel = "nodeValueSet", description = "A bare value or `{\"value\": ...}`; the node's next report confirms it.", messages = [SetRequest], mqtt(qos = 1))]
#[asyncapi_operation(name = "receiveInclude", summary = "Include a device", action = "receive", channel = "includeRequest", description = "Read from any client; the answer comes on the matching response channel.", messages = [IncludeRequest], mqtt(qos = 1))]
#[asyncapi_operation(name = "sendInclude", summary = "A device is included", action = "send", channel = "includeResponse", description = "Published once per request, echoing its `transaction`. Not retained.", messages = [IncludeResponse], mqtt(qos = 1, retain = false))]
#[asyncapi_operation(name = "receiveExclude", summary = "Exclude a device", action = "receive", channel = "excludeRequest", description = "Read from any client; the answer comes on the matching response channel.", messages = [ExcludeRequest], mqtt(qos = 1))]
#[asyncapi_operation(name = "sendExclude", summary = "A device is excluded", action = "send", channel = "excludeResponse", description = "Published once per request, echoing its `transaction`. Not retained.", messages = [ExcludeResponse], mqtt(qos = 1, retain = false))]
#[asyncapi_operation(name = "receiveStop", summary = "Stop including or excluding", action = "receive", channel = "stopRequest", description = "Read from any client; the answer comes on the matching response channel.", messages = [StopRequest], mqtt(qos = 1))]
#[asyncapi_operation(name = "sendStop", summary = "Including or excluding stopped", action = "send", channel = "stopResponse", description = "Published once per request, echoing its `transaction`. Not retained.", messages = [StopResponse], mqtt(qos = 1, retain = false))]
#[asyncapi_operation(name = "receiveGrant", summary = "Grant security classes", action = "receive", channel = "grantRequest", description = "Read from any client; the answer comes on the matching response channel.", messages = [GrantRequest], mqtt(qos = 1))]
#[asyncapi_operation(name = "sendGrant", summary = "The classes are granted", action = "send", channel = "grantResponse", description = "Published once per request, echoing its `transaction`. Not retained.", messages = [GrantResponse], mqtt(qos = 1, retain = false))]
#[asyncapi_operation(name = "receiveValidateDsk", summary = "Confirm the joining node's PIN", action = "receive", channel = "validateDskRequest", description = "Read from any client; the answer comes on the matching response channel.", messages = [ValidateDskRequest], mqtt(qos = 1))]
#[asyncapi_operation(name = "sendValidateDsk", summary = "The PIN is passed on", action = "send", channel = "validateDskResponse", description = "Published once per request, echoing its `transaction`. Not retained.", messages = [ValidateDskResponse], mqtt(qos = 1, retain = false))]
#[asyncapi_operation(name = "receiveProvision", summary = "Add a SmartStart device", action = "receive", channel = "provisionRequest", description = "Read from any client; the answer comes on the matching response channel.", messages = [ProvisionRequest], mqtt(qos = 1))]
#[asyncapi_operation(name = "sendProvision", summary = "The device is provisioned", action = "send", channel = "provisionResponse", description = "Published once per request, echoing its `transaction`. Not retained.", messages = [ProvisionResponse], mqtt(qos = 1, retain = false))]
#[asyncapi_operation(name = "receiveUnprovision", summary = "Remove a SmartStart device", action = "receive", channel = "unprovisionRequest", description = "Read from any client; the answer comes on the matching response channel.", messages = [UnprovisionRequest], mqtt(qos = 1))]
#[asyncapi_operation(name = "sendUnprovision", summary = "The device is no longer provisioned", action = "send", channel = "unprovisionResponse", description = "Published once per request, echoing its `transaction`. Not retained.", messages = [UnprovisionResponse], mqtt(qos = 1, retain = false))]
#[asyncapi_operation(name = "receiveProvisioningList", summary = "Read the provisioning list", action = "receive", channel = "provisioningListRequest", description = "Read from any client; the answer comes on the matching response channel.", messages = [ProvisioningListRequest], mqtt(qos = 1))]
#[asyncapi_operation(name = "sendProvisioningList", summary = "The provisioning list", action = "send", channel = "provisioningListResponse", description = "Published once per request, echoing its `transaction`. Not retained.", messages = [ProvisioningListResponse], mqtt(qos = 1, retain = false))]
#[asyncapi_operation(name = "receiveBackup", summary = "Back up the controller", action = "receive", channel = "backupRequest", description = "Read from any client; the answer comes on the matching response channel.", messages = [BackupRequest], mqtt(qos = 1))]
#[asyncapi_operation(name = "sendBackup", summary = "The backup is written", action = "send", channel = "backupResponse", description = "Published once per request, echoing its `transaction`. Not retained.", messages = [BackupResponse], mqtt(qos = 1, retain = false))]
#[asyncapi_operation(name = "receiveRestore", summary = "Restore a backup", action = "receive", channel = "restoreRequest", description = "Read from any client; the answer comes on the matching response channel.", messages = [RestoreRequest], mqtt(qos = 1))]
#[asyncapi_operation(name = "sendRestore", summary = "The backup is restored", action = "send", channel = "restoreResponse", description = "Published once per request, echoing its `transaction`. Not retained.", messages = [RestoreResponse], mqtt(qos = 1, retain = false))]
#[asyncapi_operation(name = "receiveReinterview", summary = "Interview a node again", action = "receive", channel = "reinterviewRequest", description = "Read from any client; the answer comes on the matching response channel.", messages = [ReinterviewRequest], mqtt(qos = 1))]
#[asyncapi_operation(name = "sendReinterview", summary = "The interview is queued", action = "send", channel = "reinterviewResponse", description = "Published once per request, echoing its `transaction`. Not retained.", messages = [ReinterviewResponse], mqtt(qos = 1, retain = false))]
#[asyncapi_operation(name = "receiveRemoveFailed", summary = "Remove a failed node", action = "receive", channel = "removeFailedRequest", description = "Read from any client; the answer comes on the matching response channel.", messages = [RemoveFailedRequest], mqtt(qos = 1))]
#[asyncapi_operation(name = "sendRemoveFailed", summary = "The node is removed", action = "send", channel = "removeFailedResponse", description = "Published once per request, echoing its `transaction`. Not retained.", messages = [RemoveFailedResponse], mqtt(qos = 1, retain = false))]
#[asyncapi_operation(name = "receiveRename", summary = "Name a node", action = "receive", channel = "renameRequest", description = "Read from any client; the answer comes on the matching response channel.", messages = [RenameRequest], mqtt(qos = 1))]
#[asyncapi_operation(name = "sendRename", summary = "The node is renamed", action = "send", channel = "renameResponse", description = "Published once per request, echoing its `transaction`. Not retained.", messages = [RenameResponse], mqtt(qos = 1, retain = false))]
#[asyncapi_operation(name = "receiveFirmwareUpdate", summary = "Update a node's firmware", action = "receive", channel = "firmwareUpdateRequest", description = "Read from any client; the answer comes on the matching response channel.", messages = [FirmwareUpdateRequest], mqtt(qos = 1))]
#[asyncapi_operation(name = "sendFirmwareUpdate", summary = "The firmware update ended", action = "send", channel = "firmwareUpdateResponse", description = "Published once per request, echoing its `transaction`. Not retained.", messages = [FirmwareUpdateResponse], mqtt(qos = 1, retain = false))]
#[asyncapi_operation(name = "receiveConfigure", summary = "Set configuration parameters", action = "receive", channel = "configureRequest", description = "Read from any client; the answer comes on the matching response channel.", messages = [ConfigureRequest], mqtt(qos = 1))]
#[asyncapi_operation(name = "sendConfigure", summary = "The parameters are set", action = "send", channel = "configureResponse", description = "Published once per request, echoing its `transaction`. Not retained.", messages = [ConfigureResponse], mqtt(qos = 1, retain = false))]
#[asyncapi_operation(name = "receiveTopology", summary = "Map the mesh", action = "receive", channel = "topologyRequest", description = "Read from any client; the answer comes on the matching response channel.", messages = [TopologyRequest], mqtt(qos = 1))]
#[asyncapi_operation(name = "sendTopology", summary = "The mesh", action = "send", channel = "topologyResponse", description = "Published once per request, echoing its `transaction`. Not retained.", messages = [TopologyResponse], mqtt(qos = 1, retain = false))]
struct MqttApi;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn render() -> String {
    let mut spec = MqttApi::asyncapi_spec();
    // The description is versioned with the bridge.
    spec.info.version = env!("CARGO_PKG_VERSION").into();
    serde_json::to_string_pretty(&spec).unwrap() + "\n"
}

#[test]
fn asyncapi_description_is_current() {
    let json = render();
    let path = repo_root().join(SPEC);
    if std::env::var_os("UPDATE_DOCS").is_some() {
        std::fs::write(&path, &json).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == json,
        "{SPEC} is stale: regenerate it with \
         UPDATE_DOCS=1 cargo test -p zwave-bridge --features unstable-asyncapi-docs --test asyncapi"
    );
}
