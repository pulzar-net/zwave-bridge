//! SmartStart and S2 QR codes (the Z-Wave Alliance's Node Provisioning QR
//! code format): all decimal digits, `90`, a version, a SHA-1 checksum, the
//! requested keys, the DSK and then TLV blocks.

use crate::dsk::Dsk;
use crate::{Error, SecurityClass};
use sha1::{Digest, Sha1};

/// The shortest valid code: lead-in, version, checksum, keys and the DSK.
pub const MIN_LEN: usize = 52;
/// Longest code accepted; real labels are far shorter.
pub const MAX_LEN: usize = 512;

/// What a QR code says about a node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrCode {
    /// 0 for an S2-only code, 1 for SmartStart.
    pub version: u8,
    /// The keys the node asks for, highest first.
    pub requested_classes: Vec<SecurityClass>,
    /// The node's DSK.
    pub dsk: Dsk,
    /// Generic and specific device class and installer icon, if present.
    pub product_type: Option<ProductType>,
    /// Manufacturer, product and application version, if present.
    pub product_id: Option<ProductId>,
    /// Supported protocols bit mask (bit 0 Z-Wave, bit 1 Long Range), if present.
    pub supported_protocols: Option<u16>,
    /// The longest interval between SmartStart inclusion requests, in seconds, if present.
    pub max_inclusion_request_interval: Option<u32>,
}

/// The QR code's Product Type block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductType {
    /// Generic device class.
    pub generic: u8,
    /// Specific device class.
    pub specific: u8,
    /// Installer icon type.
    pub icon: u16,
}

/// The QR code's Product Id block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductId {
    /// Manufacturer id.
    pub manufacturer: u16,
    /// Product type.
    pub product_type: u16,
    /// Product id.
    pub product: u16,
    /// Application version (major, minor).
    pub version: (u8, u8),
}

impl QrCode {
    /// Whether the code is a SmartStart code (the node joins without a button press).
    pub fn is_smartstart(&self) -> bool {
        self.version == 1
    }
}

fn num(s: &[u8], at: usize, digits: usize, max: u32) -> Result<u32, Error> {
    let part = s.get(at..at + digits).ok_or(Error::Invalid("QR code"))?;
    let mut v: u32 = 0;
    for &b in part {
        if !b.is_ascii_digit() {
            return Err(Error::Invalid("QR code"));
        }
        v = v * 10 + u32::from(b - b'0');
    }
    if v > max {
        return Err(Error::Invalid("QR code"));
    }
    Ok(v)
}

/// Parses a QR code string. Whitespace around it is ignored; the checksum
/// must match.
pub fn parse(code: &str) -> Result<QrCode, Error> {
    let s = code.trim().as_bytes();
    if s.len() < MIN_LEN || s.len() > MAX_LEN || !s.starts_with(b"90") {
        return Err(Error::Invalid("QR code"));
    }
    if !s.iter().all(u8::is_ascii_digit) {
        return Err(Error::Invalid("QR code"));
    }
    let version = num(s, 2, 2, 1)? as u8;
    let checksum = num(s, 4, 5, 0xffff)? as u16;
    let digest = Sha1::digest(&s[9..]);
    if u16::from_be_bytes([digest[0], digest[1]]) != checksum {
        return Err(Error::Invalid("QR code checksum"));
    }
    let keys = num(s, 9, 3, 0xff)? as u8;
    let mut dsk = [0u8; 16];
    for i in 0..8 {
        let v = num(s, 12 + i * 5, 5, 0xffff)? as u16;
        dsk[i * 2..i * 2 + 2].copy_from_slice(&v.to_be_bytes());
    }
    let mut out = QrCode {
        version,
        requested_classes: SecurityClass::from_key_bits(keys),
        dsk: Dsk(dsk),
        product_type: None,
        product_id: None,
        supported_protocols: None,
        max_inclusion_request_interval: None,
    };
    let mut at = MIN_LEN;
    while at + 4 <= s.len() {
        let type_critical = num(s, at, 2, 99)? as u8;
        let len = num(s, at + 2, 2, 99)? as usize;
        let data = s
            .get(at + 4..at + 4 + len)
            .ok_or(Error::Invalid("QR code TLV"))?;
        at += 4 + len;
        let critical = type_critical & 1 == 1;
        match type_critical >> 1 {
            0x00 if len == 10 => {
                let classes = num(data, 0, 5, 0xffff)? as u16;
                out.product_type = Some(ProductType {
                    generic: (classes >> 8) as u8,
                    specific: classes as u8,
                    icon: num(data, 5, 5, 0xffff)? as u16,
                });
            }
            0x01 if len == 20 => {
                let ver = num(data, 15, 5, 0xffff)? as u16;
                out.product_id = Some(ProductId {
                    manufacturer: num(data, 0, 5, 0xffff)? as u16,
                    product_type: num(data, 5, 5, 0xffff)? as u16,
                    product: num(data, 10, 5, 0xffff)? as u16,
                    version: ((ver >> 8) as u8, ver as u8),
                });
            }
            0x02 if len == 2 => {
                out.max_inclusion_request_interval = Some(128 * num(data, 0, 2, 99)?);
            }
            0x04 if matches!(len, 2 | 3 | 5) => {
                out.supported_protocols = Some(num(data, 0, len, 0xffff)? as u16);
            }
            _ if critical => return Err(Error::Invalid("QR code: unknown critical block")),
            _ => {}
        }
    }
    if at != s.len() {
        return Err(Error::Invalid("QR code TLV"));
    }
    Ok(out)
}

/// Builds a QR code string (for tests, simulators and label printing).
pub fn encode(qr: &QrCode) -> String {
    let mut body = format!("{:03}", SecurityClass::to_key_bits(&qr.requested_classes));
    for pair in qr.dsk.0.chunks(2) {
        body += &format!("{:05}", u16::from_be_bytes([pair[0], pair[1]]));
    }
    if let Some(pt) = qr.product_type {
        let classes = (u16::from(pt.generic) << 8) | u16::from(pt.specific);
        body += &format!("0010{classes:05}{:05}", pt.icon);
    }
    if let Some(pi) = qr.product_id {
        let ver = (u16::from(pi.version.0) << 8) | u16::from(pi.version.1);
        body += &format!(
            "0220{:05}{:05}{:05}{ver:05}",
            pi.manufacturer, pi.product_type, pi.product
        );
    }
    let digest = Sha1::digest(body.as_bytes());
    format!(
        "90{:02}{:05}{body}",
        qr.version,
        u16::from_be_bytes([digest[0], digest[1]])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_checksum() {
        let qr = QrCode {
            version: 1,
            requested_classes: vec![
                SecurityClass::S2Authenticated,
                SecurityClass::S2Unauthenticated,
            ],
            dsk: "51525-35455-41654-24356-13311-14526-06640-31770"
                .parse()
                .unwrap(),
            product_type: Some(ProductType {
                generic: 0x10,
                specific: 0x01,
                icon: 0x0700,
            }),
            product_id: Some(ProductId {
                manufacturer: 0x0000,
                product_type: 0x0004,
                product: 0x0003,
                version: (1, 2),
            }),
            supported_protocols: None,
            max_inclusion_request_interval: None,
        };
        let s = encode(&qr);
        assert!(s.starts_with("9001"));
        assert_eq!(parse(&s).unwrap(), qr);
        // One flipped digit breaks the checksum.
        let mut bad = s.into_bytes();
        let i = bad.len() - 1;
        bad[i] = if bad[i] == b'0' { b'1' } else { b'0' };
        assert!(parse(std::str::from_utf8(&bad).unwrap()).is_err());
        assert!(parse("90").is_err());
        assert!(parse(&"9".repeat(60)).is_err());
    }
}
