//! Serial API function ids.

/// A Serial API function id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FunctionId(pub u8);

impl From<FunctionId> for u8 {
    fn from(f: FunctionId) -> u8 {
        f.0
    }
}

macro_rules! functions {
    ($($name:ident = $v:expr, $doc:literal;)*) => {
        impl FunctionId {
            $(#[doc = $doc] pub const $name: FunctionId = FunctionId($v);)*

            /// The function's name, for logs.
            pub fn name(self) -> &'static str {
                match self.0 {
                    $($v => stringify!($name),)*
                    _ => "UNKNOWN",
                }
            }
        }
    };
}

functions! {
    GET_INIT_DATA = 0x02, "Serial API init data: API version, capabilities, node list, chip.";
    SET_APPLICATION_NODE_INFO = 0x03, "Sets the node information frame the controller sends.";
    APPLICATION_COMMAND_HANDLER = 0x04, "A command class frame received from a node.";
    GET_CONTROLLER_CAPABILITIES = 0x05, "Controller role flags (primary, SIS, SUC).";
    SET_SERIAL_API_TIMEOUTS = 0x06, "Serial API ACK and byte timeouts.";
    GET_SERIAL_API_CAPABILITIES = 0x07, "Firmware version, manufacturer, product and supported functions.";
    SOFT_RESET = 0x08, "Restarts the controller firmware.";
    GET_PROTOCOL_VERSION = 0x09, "Z-Wave protocol version.";
    SERIAL_API_STARTED = 0x0a, "Sent by the controller after it starts.";
    SERIAL_API_SETUP = 0x0b, "Serial API setup subcommands (TX status report, region, power).";
    SEND_NODE_INFORMATION = 0x12, "Broadcasts or sends the controller's node information frame.";
    SEND_DATA = 0x13, "Sends a command class frame to a node.";
    SEND_DATA_MULTICAST = 0x14, "Sends a command class frame to several nodes.";
    GET_CONTROLLER_VERSION = 0x15, "Library version string and library type.";
    SEND_DATA_ABORT = 0x16, "Aborts the SendData in progress.";
    GET_RANDOM = 0x1c, "Random bytes from the chip.";
    GET_CONTROLLER_ID = 0x20, "Home id and own node id.";
    GET_NVM_ID = 0x29, "External NVM manufacturer, type and size (500 series).";
    EXT_NVM_READ_LONG_BUFFER = 0x2a, "Reads from the external NVM (500 series).";
    NVM_OPERATIONS = 0x2e, "NVM backup and restore (700 and 800 series, 16-bit offsets).";
    GET_BACKGROUND_RSSI = 0x3b, "Background noise per channel.";
    EXTENDED_NVM_OPERATIONS = 0x3d, "NVM backup and restore (32-bit offsets).";
    REMOVE_NODE_ID_FROM_NETWORK = 0x3f, "Removes a specific node id (exclusion).";
    GET_NODE_PROTOCOL_INFO = 0x41, "Listening, routing, security and device class of a node.";
    SET_DEFAULT = 0x42, "Resets the controller to factory defaults (new home id).";
    ASSIGN_RETURN_ROUTE = 0x46, "Assigns a return route from a node to another.";
    DELETE_RETURN_ROUTE = 0x47, "Deletes a node's return routes.";
    REQUEST_NODE_NEIGHBOR_UPDATE = 0x48, "Asks a node to rediscover its neighbors.";
    APPLICATION_UPDATE = 0x49, "Node information and SmartStart events.";
    ADD_NODE_TO_NETWORK = 0x4a, "Inclusion.";
    REMOVE_NODE_FROM_NETWORK = 0x4b, "Exclusion.";
    ASSIGN_SUC_RETURN_ROUTE = 0x51, "Assigns a return route to the SUC.";
    SET_SUC_NODE_ID = 0x54, "Makes a node the SUC/SIS.";
    GET_SUC_NODE_ID = 0x56, "The SUC's node id.";
    REQUEST_NODE_INFO = 0x60, "Asks a node for its node information frame.";
    REMOVE_FAILED_NODE = 0x61, "Removes a node that no longer responds.";
    IS_FAILED_NODE = 0x62, "Whether the controller considers a node failed.";
    REPLACE_FAILED_NODE = 0x63, "Includes a new node in a failed node's id.";
    GET_ROUTING_INFO = 0x80, "A node's neighbor table.";
    GET_PRIORITY_ROUTE = 0x92, "A node's priority route.";
    SET_PRIORITY_ROUTE = 0x93, "Sets a node's priority route.";
    BRIDGE_APPLICATION_COMMAND_HANDLER = 0xa8, "A command class frame received, bridge format.";
    SEND_DATA_BRIDGE = 0xa9, "SendData from a virtual node.";
    GET_LONG_RANGE_NODES = 0xda, "Long Range node list.";
    GET_LONG_RANGE_CHANNEL = 0xdb, "Long Range channel.";
    SET_LONG_RANGE_CHANNEL = 0xdc, "Sets the Long Range channel.";
}

impl std::fmt::Display for FunctionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({:#04x})", self.name(), self.0)
    }
}
