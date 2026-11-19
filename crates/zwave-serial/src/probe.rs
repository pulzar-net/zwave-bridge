//! Probing a port: is there a Z-Wave controller, and which?
//!
//! The probe writes a NAK (harmless; it only asks a controller to drop a
//! partial frame) and one `GET_SERIAL_API_CAPABILITIES` request, which only
//! reads. It never writes to the controller's memory or changes its state,
//! and it gives up after one ACK timeout, so a device that is not a Z-Wave
//! controller sees a handful of bytes and nothing more.

use crate::driver::{Config, SerialApi, SerialError};
use crate::frame::DataFrame;
use crate::function::FunctionId;
use crate::message::SerialApiCapabilities;
use crate::port::Port;
use std::time::Duration;

/// What answered on the port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerIdentity {
    /// Firmware version, `major.minor`.
    pub firmware: String,
    /// Z-Wave manufacturer id of the stick.
    pub manufacturer_id: u16,
    /// Product type.
    pub product_type: u16,
    /// Product id.
    pub product_id: u16,
    /// The full capability response.
    pub capabilities: SerialApiCapabilities,
}

impl ControllerIdentity {
    /// A best-effort name for well-known sticks.
    pub fn known_name(&self) -> Option<&'static str> {
        crate::probe::known_name(self.manufacturer_id, self.product_type, self.product_id)
    }
}

/// Names of a few common sticks, by (manufacturer, product type, product id).
pub fn known_name(manufacturer: u16, product_type: u16, product_id: u16) -> Option<&'static str> {
    Some(match (manufacturer, product_type, product_id) {
        (0x0000, 0x0004, 0x0004) => "Silicon Labs 800 series controller (e.g. Zooz ZST39 LR, HomeSeer SmartStick G8)",
        (0x0000, 0x0004, 0x0001) | (0x0000, 0x0004, 0x0002) => "Silicon Labs 700 series controller",
        (0x0086, 0x0001, 0x005a) => "Aeotec Z-Stick Gen5",
        (0x0086, 0x0001, 0x005d) => "Aeotec Z-Stick 7",
        (0x0115, 0x0400, 0x0001) => "Z-Wave.Me UZB",
        _ => return None,
    })
}

/// Probes an open port. `Ok(None)` means nothing answered like a Z-Wave controller.
pub async fn probe_port<P: Port>(port: P) -> Result<Option<ControllerIdentity>, SerialError> {
    let config = Config {
        max_attempts: 1,
        ..Config::default()
    };
    let (api, _unsolicited) = SerialApi::start(port, config);
    let frame = DataFrame::request(FunctionId::GET_SERIAL_API_CAPABILITIES, vec![]);
    match api.request(frame, Duration::from_millis(1000)).await {
        Ok(resp) => match SerialApiCapabilities::parse(&resp.payload) {
            Ok(c) => Ok(Some(ControllerIdentity {
                firmware: format!("{}.{}", c.firmware_major, c.firmware_minor),
                manufacturer_id: c.manufacturer_id,
                product_type: c.product_type,
                product_id: c.product_id,
                capabilities: c,
            })),
            Err(_) => Ok(None),
        },
        Err(SerialError::NoAck(_) | SerialError::Rejected(_) | SerialError::ResponseTimeout) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Opens `path` (a device or `tcp://host:port`) and probes it.
pub async fn probe(path: &str) -> Result<Option<ControllerIdentity>, SerialError> {
    let port = crate::port::open(path)
        .await
        .map_err(|e| SerialError::Closed(e.to_string()))?;
    probe_port(port).await
}
