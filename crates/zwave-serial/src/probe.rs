//! Probing a port: is there a Z-Wave controller, and which?
//!
//! The probe writes a NAK (harmless; it only asks a controller to drop a
//! partial frame) and `GET_SERIAL_API_CAPABILITIES`, which only reads. Only
//! if that is answered like a Z-Wave controller does it ask a few more
//! read-only questions (version, controller id, init data, controller
//! capabilities, RF region). Nothing writes the controller's memory, changes
//! its state or resets it. The whole probe is bounded by [`PROBE_TIMEOUT`]
//! and the port is closed when it returns, so a device that is not a Z-Wave
//! controller sees a handful of bytes and nothing more.

use crate::driver::{Config, SerialApi, SerialError};
use crate::frame::DataFrame;
use crate::function::FunctionId as F;
use crate::message::{
    setup, ControllerId, ControllerVersion, InitData, RfRegion, SerialApiCapabilities,
};
use crate::port::Port;
use crate::NodeId;
use std::time::Duration;

/// Upper bound on a whole probe.
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(2900);

/// The controller chip's series.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Series {
    /// 500 series (ZW050x).
    S500,
    /// 700 series (EFR32ZG14 / ZGM130).
    S700,
    /// 800 series (EFR32ZG23 / ZGM230).
    S800,
}

impl Series {
    /// From the chip type in the init data.
    pub fn from_chip_type(chip: u8) -> Option<Series> {
        match chip {
            0x05 => Some(Series::S500),
            0x07 => Some(Series::S700),
            0x08 => Some(Series::S800),
            _ => None,
        }
    }
}

/// What answered on the port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerIdentity {
    /// Serial API firmware version, `major.minor`.
    pub firmware: String,
    /// Z-Wave manufacturer id of the stick.
    pub manufacturer_id: u16,
    /// Product type.
    pub product_type: u16,
    /// Product id.
    pub product_id: u16,
    /// The full capability response.
    pub capabilities: SerialApiCapabilities,
    /// Library version string, e.g. `Z-Wave 7.21`.
    pub library_version: Option<String>,
    /// Serial API version from the init data.
    pub api_version: Option<u8>,
    /// Chip type and version from the init data.
    pub chip: Option<(u8, u8)>,
    /// Chip series, when the chip type is known.
    pub series: Option<Series>,
    /// Home id of the network the controller is in.
    pub home_id: Option<u32>,
    /// The controller's own node id.
    pub node_id: Option<NodeId>,
    /// Whether it is the primary controller of its network (not a secondary).
    pub primary: Option<bool>,
    /// Whether it is the SUC/SIS.
    pub sis: Option<bool>,
    /// Nodes in its network, the controller excluded.
    pub node_count: Option<usize>,
    /// RF region, if the controller reports it.
    pub region: Option<RfRegion>,
}

impl ControllerIdentity {
    /// A best-effort name for well-known sticks.
    pub fn known_name(&self) -> Option<&'static str> {
        known_name(self.manufacturer_id, self.product_type, self.product_id)
    }
}

/// Names of a few common sticks, by (manufacturer, product type, product id).
pub fn known_name(manufacturer: u16, product_type: u16, product_id: u16) -> Option<&'static str> {
    Some(match (manufacturer, product_type, product_id) {
        (0x0000, 0x0004, 0x0004) => {
            "Silicon Labs 800 series controller (e.g. Zooz ZST39 LR, HomeSeer SmartStick G8)"
        }
        (0x0000, 0x0004, 0x0001) | (0x0000, 0x0004, 0x0002) => "Silicon Labs 700 series controller",
        (0x0086, 0x0001, 0x005a) => "Aeotec Z-Stick Gen5",
        (0x0086, 0x0001, 0x005d) => "Aeotec Z-Stick 7",
        (0x0115, 0x0400, 0x0001) => "Z-Wave.Me UZB",
        _ => return None,
    })
}

const STEP: Duration = Duration::from_millis(400);

async fn details(api: &SerialApi, id: &mut ControllerIdentity) {
    let c = &id.capabilities;
    if c.supports(F::GET_CONTROLLER_VERSION) {
        if let Ok(r) = api
            .request(DataFrame::request(F::GET_CONTROLLER_VERSION, vec![]), STEP)
            .await
        {
            id.library_version = ControllerVersion::parse(&r.payload).ok().map(|v| v.version);
        }
    }
    if c.supports(F::GET_CONTROLLER_ID) {
        if let Ok(r) = api
            .request(DataFrame::request(F::GET_CONTROLLER_ID, vec![]), STEP)
            .await
        {
            if let Ok(cid) = ControllerId::parse(&r.payload, 1) {
                id.home_id = Some(cid.home_id);
                id.node_id = Some(cid.node_id);
            }
        }
    }
    if c.supports(F::GET_INIT_DATA) {
        if let Ok(r) = api
            .request(DataFrame::request(F::GET_INIT_DATA, vec![]), STEP)
            .await
        {
            if let Ok(init) = InitData::parse(&r.payload) {
                id.api_version = Some(init.api_version);
                id.chip = init.chip_type.zip(init.chip_version);
                id.series = init.chip_type.and_then(Series::from_chip_type);
                id.sis = Some(init.is_sis());
                let own = id.node_id.unwrap_or(1);
                id.node_count = Some(init.nodes.iter().filter(|&&n| n != own).count());
            }
        }
    }
    if c.supports(F::GET_CONTROLLER_CAPABILITIES) {
        if let Ok(r) = api
            .request(
                DataFrame::request(F::GET_CONTROLLER_CAPABILITIES, vec![]),
                STEP,
            )
            .await
        {
            id.primary = r.payload.first().map(|b| b & 0x01 == 0);
        }
    }
    if c.supports(F::SERIAL_API_SETUP) {
        if let Ok(r) = api
            .request(
                DataFrame::request(F::SERIAL_API_SETUP, vec![setup::GET_RF_REGION]),
                STEP,
            )
            .await
        {
            if r.payload.first() == Some(&setup::GET_RF_REGION) {
                id.region = r
                    .payload
                    .get(1)
                    .map(|&b| RfRegion::from_u8(b))
                    .filter(|r| !matches!(r, RfRegion::Other(_)));
            }
        }
    }
}

/// Probes an open port. `Ok(None)` means nothing answered like a Z-Wave
/// controller; `Err` is an I/O failure. The port is closed on return.
pub async fn probe_port<P: Port>(port: P) -> Result<Option<ControllerIdentity>, SerialError> {
    let config = Config {
        max_attempts: 1,
        ack_timeout: Duration::from_millis(500),
        ..Config::default()
    };
    let (api, _unsolicited) = SerialApi::start(port, config);
    let run = async {
        let frame = DataFrame::request(F::GET_SERIAL_API_CAPABILITIES, vec![]);
        let resp = match api.request(frame, Duration::from_millis(800)).await {
            Ok(r) => r,
            Err(
                SerialError::NoAck(_) | SerialError::Rejected(_) | SerialError::ResponseTimeout,
            ) => return Ok(None),
            Err(e) => return Err(e),
        };
        let Ok(c) = SerialApiCapabilities::parse(&resp.payload) else {
            return Ok(None);
        };
        let mut id = ControllerIdentity {
            firmware: format!("{}.{}", c.firmware_major, c.firmware_minor),
            manufacturer_id: c.manufacturer_id,
            product_type: c.product_type,
            product_id: c.product_id,
            capabilities: c,
            library_version: None,
            api_version: None,
            chip: None,
            series: None,
            home_id: None,
            node_id: None,
            primary: None,
            sis: None,
            node_count: None,
            region: None,
        };
        // Best effort: whatever the time bound leaves room for.
        let _ = tokio::time::timeout(
            PROBE_TIMEOUT.saturating_sub(Duration::from_millis(1400)),
            details(&api, &mut id),
        )
        .await;
        Ok(Some(id))
    };
    let r = tokio::time::timeout(PROBE_TIMEOUT, run)
        .await
        .unwrap_or(Ok(None));
    // Dropping the last handle stops the link task, which closes the port.
    drop(api);
    tokio::task::yield_now().await;
    r
}

/// Opens `path` (a device or `tcp://host:port`) and probes it.
pub async fn probe(path: &str) -> Result<Option<ControllerIdentity>, SerialError> {
    let port = crate::port::open(path)
        .await
        .map_err(|e| SerialError::Closed(e.to_string()))?;
    probe_port(port).await
}

/// [`probe`] for callers without an async runtime: runs on a private
/// current-thread runtime, which is dropped (closing the port) before it returns.
pub fn probe_blocking(path: &str) -> Result<Option<ControllerIdentity>, SerialError> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|e| SerialError::Closed(e.to_string()))?;
    let r = rt.block_on(probe(path));
    drop(rt);
    r
}
