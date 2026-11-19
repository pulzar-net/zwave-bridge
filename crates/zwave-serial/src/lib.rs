//! The host side of the Z-Wave Serial API, the protocol between a host and
//! a Z-Wave controller chip (USB sticks and UART modules on the 500, 700 and
//! 800 series), as described in Silicon Labs' public *Z-Wave Host API
//! Specification*.
//!
//! - [`frame`]: the byte-level frames (SOF, length, type, function, payload,
//!   checksum; ACK, NAK and CAN) and a streaming decoder that never panics
//!   and never allocates more than one frame.
//! - [`function`]: the function ids.
//! - [`message`]: typed requests and parsers for the controller's responses
//!   and callbacks.
//! - [`driver`]: [`SerialApi`], which owns the port and runs the link layer:
//!   one exchange at a time, ACK within 1.6 s, up to three transmissions with
//!   back-off, callback ids, response and callback timeouts, and every frame
//!   the controller sends on its own handed to the caller.
//! - [`mod@probe`]: say what controller (if any) answers on a port, with one
//!   read-only frame.
//! - [`port`]: open a serial device or a `tcp://host:port` (ser2net) link.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod driver;
pub mod frame;
pub mod function;
pub mod message;
pub mod port;
pub mod probe;

pub use driver::{Config, Exchange, Request, SerialApi, SerialError};
pub use frame::{DataFrame, Frame, FrameType};
pub use function::FunctionId;
pub use probe::{probe, ControllerIdentity};

/// A node id. Classic Z-Wave uses 1 to 232; Long Range uses 256 to 4000.
pub type NodeId = u16;
