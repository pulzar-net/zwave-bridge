//! Device Specific Keys: the first 16 bytes of a node's public key.
//!
//! Printed on the device as eight five-digit groups
//! (`34028-23669-20938-46346-33746-07431-56821-14553`); the first group is
//! the PIN a user types to authenticate an S2 Authenticated or Access Control
//! inclusion, because the node blanks those two bytes in its Public Key Report.

use crate::Error;
use std::fmt;
use std::str::FromStr;

/// A 16-byte Device Specific Key.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Dsk(pub [u8; 16]);

impl Dsk {
    /// The DSK of a public key.
    pub fn from_public_key(key: &[u8; 32]) -> Self {
        let mut d = [0u8; 16];
        d.copy_from_slice(&key[..16]);
        Dsk(d)
    }

    /// The five-digit PIN: the first group.
    pub fn pin(&self) -> u16 {
        u16::from_be_bytes([self.0[0], self.0[1]])
    }

    /// The DSK with its PIN bytes zeroed, as a node reports it before the PIN is entered.
    pub fn without_pin(&self) -> Self {
        let mut d = *self;
        d.0[0] = 0;
        d.0[1] = 0;
        d
    }

    /// The SmartStart NWI home id a node with this DSK uses while it asks for inclusion.
    pub fn nwi_home_id(&self) -> [u8; 4] {
        let mut h = [self.0[8], self.0[9], self.0[10], self.0[11]];
        h[0] |= 0b1100_0000;
        h[3] &= 0b1111_1110;
        h
    }

    /// The SmartStart authentication home id the controller sends to include it.
    pub fn auth_home_id(&self) -> [u8; 4] {
        let mut h = [self.0[12], self.0[13], self.0[14], self.0[15]];
        h[0] &= 0b0011_1111;
        h[3] |= 0b0000_0001;
        h
    }

    /// The DSK text with the first group shown as `xxxxx`, for confirming a PIN.
    pub fn masked(&self) -> String {
        let s = self.to_string();
        format!("xxxxx{}", &s[5..])
    }
}

/// Parses a PIN string ("12345").
pub fn parse_pin(s: &str) -> Result<u16, Error> {
    let s = s.trim();
    if s.len() != 5 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::Invalid("PIN"));
    }
    s.parse::<u32>()
        .ok()
        .and_then(|v| u16::try_from(v).ok())
        .ok_or(Error::Invalid("PIN"))
}

/// Restores the PIN bytes a node blanked in its public key.
pub fn apply_pin(public_key: &mut [u8; 32], pin: u16) {
    public_key[..2].copy_from_slice(&pin.to_be_bytes());
}

impl fmt::Display for Dsk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, pair) in self.0.chunks(2).enumerate() {
            if i > 0 {
                f.write_str("-")?;
            }
            write!(f, "{:05}", u16::from_be_bytes([pair[0], pair[1]]))?;
        }
        Ok(())
    }
}

impl fmt::Debug for Dsk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Dsk({self})")
    }
}

impl FromStr for Dsk {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        let groups: Vec<&str> = s.trim().split('-').collect();
        if groups.len() != 8 {
            return Err(Error::Invalid("DSK"));
        }
        let mut d = [0u8; 16];
        for (i, g) in groups.iter().enumerate() {
            let v = parse_pin(g).map_err(|_| Error::Invalid("DSK"))?;
            d[i * 2..i * 2 + 2].copy_from_slice(&v.to_be_bytes());
        }
        Ok(Dsk(d))
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for Dsk {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Dsk {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_roundtrip() {
        let s = "34028-23669-20938-46346-33746-07431-56821-14553";
        let d: Dsk = s.parse().unwrap();
        assert_eq!(d.to_string(), s);
        assert_eq!(d.pin(), 34028);
        assert_eq!(
            d.masked(),
            "xxxxx-23669-20938-46346-33746-07431-56821-14553"
        );
        assert!("34028-23669".parse::<Dsk>().is_err());
        assert!("70000-23669-20938-46346-33746-07431-56821-14553"
            .parse::<Dsk>()
            .is_err());
        assert_eq!(parse_pin(" 00042 ").unwrap(), 42);
        assert!(parse_pin("1234").is_err());
    }

    #[test]
    fn smartstart_home_ids() {
        let d = Dsk([0xff; 16]);
        assert_eq!(d.nwi_home_id(), [0xff, 0xff, 0xff, 0xfe]);
        assert_eq!(d.auth_home_id(), [0x3f, 0xff, 0xff, 0xff]);
        let d = Dsk([0; 16]);
        assert_eq!(d.nwi_home_id(), [0xc0, 0, 0, 0]);
        assert_eq!(d.auth_home_id(), [0, 0, 0, 1]);
    }
}
