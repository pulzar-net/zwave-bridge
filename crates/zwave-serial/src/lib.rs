#![doc = include_str!("../README.md")]

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
pub use probe::{probe, probe_blocking, ControllerIdentity, Series};

/// A node id. Classic Z-Wave uses 1 to 232; Long Range uses 256 to 4000.
pub type NodeId = u16;
