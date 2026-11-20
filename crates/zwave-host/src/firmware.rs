//! Firmware updates over the air (Firmware Update Meta Data command class).
//!
//! The controller offers an image; the node asks for it in numbered
//! fragments, each carrying a CRC-16, checks the whole image's CRC-16, and
//! reports the outcome. Images may need activation afterwards.

use crate::controller::{Controller, Inbound};
use crate::{Error, Event, NodeId};
use std::time::Duration;
use tracing::{debug, info};
use zwave_cc::{crc16, firmware_update_md as fw, Command};

/// How long the node may take between fragment requests.
pub const FRAGMENT_TIMEOUT: Duration = Duration::from_secs(60);
/// How long the node may take to report the result after the last fragment.
pub const STATUS_TIMEOUT: Duration = Duration::from_secs(120);
/// Largest image accepted.
pub const MAX_IMAGE: usize = 8 * 1024 * 1024;

/// How an update ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirmwareUpdateResult {
    /// The node's status: `0xFF` done and restarting, `0xFE` done without
    /// restart, `0xFD` waiting for activation; below `0x10` an error.
    pub status: u8,
    /// Seconds until the node is back, if it says.
    pub wait_time: Option<u16>,
    /// Whether activation succeeded, if it was needed.
    pub activated: Option<bool>,
}

impl FirmwareUpdateResult {
    /// Whether the node accepted the image.
    pub fn success(&self) -> bool {
        self.status >= 0xfd && self.activated != Some(false)
    }
}

/// Reads a firmware file: Intel HEX (`.hex`, or text starting with `:`) is
/// turned into a binary image from its lowest address with gaps filled with
/// `0xFF`; anything else (`.gbl`, `.bin`, `.ota`) is sent as it is.
pub fn read_image(name: &str, bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.len() > MAX_IMAGE * 3 {
        return Err("file too large".into());
    }
    let is_hex = name.to_ascii_lowercase().ends_with(".hex") || bytes.first() == Some(&b':');
    if !is_hex {
        if bytes.is_empty() || bytes.len() > MAX_IMAGE {
            return Err("image empty or too large".into());
        }
        return Ok(bytes.to_vec());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "Intel HEX file is not text".to_string())?;
    let mut chunks: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut base: u32 = 0;
    for rec in ihex::Reader::new(text) {
        match rec.map_err(|e| e.to_string())? {
            ihex::Record::Data { offset, value } => chunks.push((base + u32::from(offset), value)),
            ihex::Record::ExtendedLinearAddress(a) => base = u32::from(a) << 16,
            ihex::Record::ExtendedSegmentAddress(a) => base = u32::from(a) << 4,
            ihex::Record::EndOfFile => break,
            _ => {}
        }
    }
    let start = chunks
        .iter()
        .map(|c| c.0)
        .min()
        .ok_or("no data in Intel HEX file")?;
    let end = chunks
        .iter()
        .map(|c| c.0 as usize + c.1.len())
        .max()
        .unwrap_or(0);
    let len = end - start as usize;
    if len > MAX_IMAGE {
        return Err("image too large".into());
    }
    let mut image = vec![0xffu8; len];
    for (addr, data) in chunks {
        let o = (addr - start) as usize;
        image[o..o + data.len()].copy_from_slice(&data);
    }
    Ok(image)
}

/// A Firmware Update Meta Data Report command (a fragment) with its CRC.
pub fn fragment(number: u16, last: bool, data: &[u8]) -> Vec<u8> {
    let header = (number & 0x7fff) | if last { 0x8000 } else { 0 };
    let mut v = Command::from(fw::Report {
        header,
        data: data.to_vec(),
    })
    .encode();
    let crc = crc16::checksum(&v);
    v.extend_from_slice(&crc.to_be_bytes());
    v
}

/// Splits a received fragment command into number, last flag and data,
/// checking its CRC; `None` if it does not check out.
pub fn parse_fragment(frame: &[u8]) -> Option<(u16, bool, &[u8])> {
    if frame.len() < 6 || frame[0] != 0x7a || frame[1] != 0x06 {
        return None;
    }
    let (body, crc) = frame.split_at(frame.len() - 2);
    if crc16::checksum(body) != u16::from_be_bytes([crc[0], crc[1]]) {
        return None;
    }
    let header = u16::from_be_bytes([body[2], body[3]]);
    Some((header & 0x7fff, header & 0x8000 != 0, &body[4..]))
}

async fn next_fw(
    node: NodeId,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Inbound>,
    t: Duration,
) -> Result<fw::Command, Error> {
    match tokio::time::timeout(t, rx.recv()).await {
        Ok(Some(Inbound::Command {
            command: Command::FirmwareUpdateMd(c),
            ..
        })) => Ok(c),
        Ok(_) => Err(Error::Stopped),
        Err(_) => Err(Error::Timeout(node)),
    }
}

impl Controller {
    /// Updates a node's firmware target (0 is the main firmware) with `image`.
    /// Emits [`Event::FirmwareUpdateProgress`] as fragments go out.
    pub async fn update_firmware(
        &self,
        node: NodeId,
        image: &[u8],
        target: u8,
    ) -> Result<FirmwareUpdateResult, Error> {
        if image.is_empty() || image.len() > MAX_IMAGE {
            return Err(Error::Controller(
                "firmware image empty or too large".into(),
            ));
        }
        let (version, security) = self
            .with_node(node, |r| {
                (
                    r.root().version(fw::ID),
                    r.highest_s2().map(|_| 2).or(r.s0_only().then_some(0)),
                )
            })
            .ok_or(Error::UnknownNode(node))?;
        if version == 0 {
            return Err(Error::Controller(format!(
                "node {node} does not support firmware updates"
            )));
        }
        let _a = self.activity(node);
        self.wait_awake(node).await;
        let (_g, mut rx) = self.subscribe(node, |i| {
            matches!(
                i,
                Inbound::Command {
                    command: Command::FirmwareUpdateMd(_),
                    ..
                }
            )
        });
        let send = |c: Command| self.send_encoded(node, 0, fw::ID, c.encode());

        send(fw::MetaDataGet {}.into()).await?;
        let md = loop {
            if let fw::Command::MetaDataReport(r) =
                next_fw(node, &mut rx, self.inner.cfg.request_timeout).await?
            {
                break r;
            }
        };
        if md.upgradable == Some(0) {
            return Err(Error::Controller(format!(
                "node {node} says its firmware is not upgradable"
            )));
        }
        let firmware_id = if target == 0 {
            md.firmware_id
        } else {
            let i = 2 * usize::from(target - 1);
            md.rest
                .get(i..i + 2)
                .map(|b| u16::from_be_bytes([b[0], b[1]]))
                .ok_or(Error::Controller(format!(
                    "node {node} has no firmware target {target}"
                )))?
        };
        // Fragment data per frame: what fits a frame after the report header
        // and CRC, and the security encapsulation.
        let budget: u16 = match security {
            Some(2) => 28,
            Some(_) => 20,
            None => 40,
        };
        let size = match (version >= 3, md.max_fragment_size) {
            (true, Some(m)) if m > 0 => budget.min(m),
            _ => budget,
        };
        let checksum = crc16::checksum(image);
        let mut req = fw::RequestGet {
            manufacturer_id: md.manufacturer_id,
            firmware_id,
            checksum,
            ..Default::default()
        };
        if version >= 3 {
            req.target = Some(target);
            req.fragment_size = Some(size);
        }
        if version >= 4 {
            req.resume = Some(false);
            req.non_secure = Some(false);
            req.activation = Some(false);
        }
        send(req.into()).await?;
        let status = loop {
            if let fw::Command::RequestReport(r) =
                next_fw(node, &mut rx, self.inner.cfg.request_timeout).await?
            {
                break r.status;
            }
        };
        if status != 0xff {
            return Err(Error::Controller(format!(
                "node {node} refused the update (status {status})"
            )));
        }
        info!(node, bytes = image.len(), size, "firmware transfer started");
        let total = image.len().div_ceil(usize::from(size)) as u16;
        let result = loop {
            let t = FRAGMENT_TIMEOUT.max(STATUS_TIMEOUT);
            match next_fw(node, &mut rx, t).await? {
                fw::Command::Get(g) => {
                    let first = g.report & 0x7fff;
                    for n in first..first.saturating_add(u16::from(g.count.max(1))) {
                        if n == 0 || n > total {
                            break;
                        }
                        let start = usize::from(n - 1) * usize::from(size);
                        let end = (start + usize::from(size)).min(image.len());
                        let frame = fragment(n, n == total, &image[start..end]);
                        self.send_encoded(node, 0, fw::ID, frame).await?;
                        self.emit(Event::FirmwareUpdateProgress {
                            node,
                            sent: n,
                            total,
                        });
                    }
                }
                fw::Command::StatusReport(s) => {
                    break FirmwareUpdateResult {
                        status: s.status,
                        wait_time: s.wait_time,
                        activated: None,
                    }
                }
                other => debug!(node, "firmware update: {}", other.name()),
            }
        };
        let mut result = result;
        if result.status == 0xfd {
            send(
                fw::ActivationSet {
                    manufacturer_id: md.manufacturer_id,
                    firmware_id,
                    checksum,
                    target,
                    hardware_version: None,
                }
                .into(),
            )
            .await?;
            loop {
                if let fw::Command::ActivationReport(a) =
                    next_fw(node, &mut rx, STATUS_TIMEOUT).await?
                {
                    result.activated = Some(a.status == 0xff);
                    break;
                }
            }
        }
        info!(node, status = result.status, "firmware update finished");
        self.emit(Event::FirmwareUpdateFinished {
            node,
            status: result.status,
            success: result.success(),
        });
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragments_roundtrip_and_crc() {
        let f = fragment(3, true, &[1, 2, 3]);
        assert_eq!(parse_fragment(&f), Some((3, true, &[1u8, 2, 3][..])));
        let mut bad = f.clone();
        bad[5] ^= 1;
        assert_eq!(parse_fragment(&bad), None);
    }

    #[test]
    fn intel_hex_images() {
        // Two records with a gap, then end of file.
        let hex = ":0400000001020304F2\n:02000600AABB93\n:00000001FF\n";
        assert_eq!(
            read_image("fw.hex", hex.as_bytes()).unwrap(),
            vec![1, 2, 3, 4, 0xff, 0xff, 0xaa, 0xbb]
        );
        assert!(
            read_image("fw.hex", b":0400000001020304F3\n").is_err(),
            "bad checksum"
        );
        assert_eq!(read_image("fw.gbl", &[9, 9]).unwrap(), vec![9, 9]);
    }
}
