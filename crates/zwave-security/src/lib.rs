#![doc = include_str!("../README.md")]

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod ckdf;
pub mod crypto;
pub mod drbg;
pub mod dsk;
pub mod kex;
pub mod provisioning;
pub mod qr;
pub mod s0;
pub mod s2;

use std::fmt;

/// A Z-Wave security class, the unit in which network keys are granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum SecurityClass {
    /// Security 0, the legacy scheme.
    S0Legacy,
    /// S2 Unauthenticated.
    S2Unauthenticated,
    /// S2 Authenticated (needs the DSK PIN).
    S2Authenticated,
    /// S2 Access Control (needs the DSK PIN; locks and garage doors).
    S2AccessControl,
}

impl SecurityClass {
    /// Every class, highest first: the order in which a receiver tries keys.
    pub const HIGHEST_FIRST: [SecurityClass; 4] = [
        SecurityClass::S2AccessControl,
        SecurityClass::S2Authenticated,
        SecurityClass::S2Unauthenticated,
        SecurityClass::S0Legacy,
    ];

    /// The bit for this class in the KEX "requested/granted keys" byte.
    pub const fn key_bit(self) -> u8 {
        match self {
            SecurityClass::S2Unauthenticated => 0x01,
            SecurityClass::S2Authenticated => 0x02,
            SecurityClass::S2AccessControl => 0x04,
            SecurityClass::S0Legacy => 0x80,
        }
    }

    /// The class for a single KEX key bit.
    pub const fn from_key_bit(bit: u8) -> Option<SecurityClass> {
        match bit {
            0x01 => Some(SecurityClass::S2Unauthenticated),
            0x02 => Some(SecurityClass::S2Authenticated),
            0x04 => Some(SecurityClass::S2AccessControl),
            0x80 => Some(SecurityClass::S0Legacy),
            _ => None,
        }
    }

    /// The classes set in a KEX keys byte, highest first. Unknown bits are ignored.
    pub fn from_key_bits(bits: u8) -> Vec<SecurityClass> {
        Self::HIGHEST_FIRST
            .into_iter()
            .filter(|c| bits & c.key_bit() != 0)
            .collect()
    }

    /// The KEX keys byte for a set of classes.
    pub fn to_key_bits(classes: &[SecurityClass]) -> u8 {
        classes.iter().fold(0, |acc, c| acc | c.key_bit())
    }

    /// Whether granting this class needs the user to confirm the DSK PIN.
    pub const fn needs_pin(self) -> bool {
        matches!(
            self,
            SecurityClass::S2Authenticated | SecurityClass::S2AccessControl
        )
    }

    /// Whether this is one of the S2 classes.
    pub const fn is_s2(self) -> bool {
        !matches!(self, SecurityClass::S0Legacy)
    }

    /// A short stable name (`s2_access_control`, ...).
    pub const fn name(self) -> &'static str {
        match self {
            SecurityClass::S0Legacy => "s0_legacy",
            SecurityClass::S2Unauthenticated => "s2_unauthenticated",
            SecurityClass::S2Authenticated => "s2_authenticated",
            SecurityClass::S2AccessControl => "s2_access_control",
        }
    }
}

impl fmt::Display for SecurityClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A 128-bit network key.
pub type NetworkKey = [u8; 16];

/// Errors from this crate.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The frame is shorter than its format needs or a length field is inconsistent.
    #[error("malformed frame: {0}")]
    Malformed(&'static str),
    /// No shared nonce (SPAN) with the peer: a Nonce Get / Nonce Report exchange is needed first.
    #[error("no nonce established with the peer")]
    NoSpan,
    /// The peer's S0 nonce is missing or expired.
    #[error("no valid nonce from the peer")]
    NoNonce,
    /// Authentication failed with every applicable key and nonce.
    #[error("decryption failed")]
    DecryptFailed,
    /// The frame repeats a sequence number already seen (a retransmission).
    #[error("duplicate frame")]
    Duplicate,
    /// No key is set for the security class this needs.
    #[error("no key for {0}")]
    NoKey(SecurityClass),
    /// A critical extension this implementation does not know.
    #[error("unsupported critical extension {0:#04x}")]
    UnsupportedExtension(u8),
    /// An invalid DSK, PIN or QR code string.
    #[error("invalid {0}")]
    Invalid(&'static str),
    /// The operating system's random number source failed.
    #[error("random number source failed")]
    Random,
}

/// Fills `buf` from the operating system's CSPRNG.
pub fn random_fill(buf: &mut [u8]) -> Result<(), Error> {
    getrandom::fill(buf).map_err(|_| Error::Random)
}

/// Returns `N` random bytes from the operating system's CSPRNG.
pub fn random_bytes<const N: usize>() -> Result<[u8; N], Error> {
    let mut b = [0u8; N];
    random_fill(&mut b)?;
    Ok(b)
}
