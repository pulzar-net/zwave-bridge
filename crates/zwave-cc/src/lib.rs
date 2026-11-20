#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod crc16;
mod reader;
pub mod tables;
pub mod transport_service;
mod value;

pub use value::{decode_duration, encode_duration, Value};

#[allow(
    missing_docs,
    clippy::ptr_arg,
    clippy::identity_op,
    clippy::needless_late_init
)]
mod generated {
    include!(concat!(env!("OUT_DIR"), "/generated.rs"));
}
pub use generated::*;

/// A command the definitions do not cover, kept as bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawCommand {
    /// Command class id.
    pub cc: u16,
    /// Command id.
    pub command: u8,
    /// Payload after the command id.
    pub payload: Vec<u8>,
}

/// Metadata of a command class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CcInfo {
    /// Command class id.
    pub id: u16,
    /// Snake-case name.
    pub name: &'static str,
    /// Title.
    pub title: &'static str,
    /// Highest version the definitions cover.
    pub version: u8,
}

/// The metadata for a command class id.
pub fn cc_info(id: u16) -> Option<&'static CcInfo> {
    COMMAND_CLASSES.iter().find(|c| c.id == id)
}

/// A command whose bytes do not match its definition.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("malformed command {cc:#04x}/{command:#04x}: {kind}")]
pub struct ParseError {
    /// Command class id.
    pub cc: u16,
    /// Command id.
    pub command: u8,
    /// What was wrong.
    pub kind: ParseErrorKind,
}

/// What was wrong with a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ParseErrorKind {
    /// The frame ended before a required field.
    #[error("too short")]
    TooShort,
    /// A size field held something other than 0, 1, 2 or 4.
    #[error("invalid size {0}")]
    InvalidSize(u8),
    /// No bytes at all.
    #[error("empty frame")]
    Empty,
}

impl Command {
    /// Parses a command class frame: class id (two bytes from `0xF1`),
    /// command id, payload. Unknown classes and commands come back as
    /// [`Command::Unknown`].
    pub fn parse(frame: &[u8]) -> Result<Command, ParseError> {
        let err = |kind| ParseError {
            cc: 0,
            command: 0,
            kind,
        };
        let (&first, rest) = frame.split_first().ok_or(err(ParseErrorKind::Empty))?;
        if first == 0x00 {
            return Ok(Command::NoOperation);
        }
        let (cc, rest) = if first >= 0xf1 {
            let (&lo, rest) = rest.split_first().ok_or(err(ParseErrorKind::TooShort))?;
            ((u16::from(first) << 8) | u16::from(lo), rest)
        } else {
            (u16::from(first), rest)
        };
        let Some((&command, payload)) = rest.split_first() else {
            return Err(ParseError {
                cc,
                command: 0,
                kind: ParseErrorKind::TooShort,
            });
        };
        match Command::parse_known(cc, command, payload)? {
            Some(c) => Ok(c),
            None => Ok(Command::Unknown(RawCommand {
                cc,
                command,
                payload: payload.to_vec(),
            })),
        }
    }

    /// Serializes the command: class id, command id, payload.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Vec::with_capacity(16);
        if matches!(self, Command::NoOperation) {
            w.push(0);
            return w;
        }
        let cc = self.cc();
        if cc > 0xff {
            w.extend_from_slice(&cc.to_be_bytes());
        } else {
            w.push(cc as u8);
        }
        w.push(self.command_id());
        self.encode_payload(&mut w);
        w
    }
}

/// Decodes a bit mask: bit `n` of the mask is value `n + offset`.
pub fn decode_bitmask(mask: &[u8], offset: u16) -> Vec<u16> {
    let mut v = Vec::new();
    for (i, b) in mask.iter().enumerate() {
        for bit in 0..8 {
            if b & (1 << bit) != 0 {
                v.push(offset + (i * 8 + bit) as u16);
            }
        }
    }
    v
}

/// Encodes values as a bit mask, the shortest that holds the largest.
pub fn encode_bitmask(values: &[u16], offset: u16) -> Vec<u8> {
    let Some(max) = values.iter().filter(|&&v| v >= offset).max() else {
        return Vec::new();
    };
    let mut m = vec![0u8; usize::from(max - offset) / 8 + 1];
    for &v in values.iter().filter(|&&v| v >= offset) {
        let i = usize::from(v - offset);
        m[i / 8] |= 1 << (i % 8);
    }
    m
}

/// Decodes a command class list (two-byte ids from `0xF1`), stopping at the
/// `0xEF` mark that separates supported from controlled classes.
pub fn decode_cc_list(p: &[u8]) -> Vec<u16> {
    let mut v = Vec::new();
    let mut i = 0;
    while i < p.len() {
        let b = p[i];
        if b == 0xef {
            break;
        }
        if b >= 0xf1 {
            let Some(&lo) = p.get(i + 1) else { break };
            v.push((u16::from(b) << 8) | u16::from(lo));
            i += 2;
        } else {
            v.push(u16::from(b));
            i += 1;
        }
    }
    v
}

/// Encodes a command class list.
pub fn encode_cc_list(ccs: &[u16], w: &mut Vec<u8>) {
    for &cc in ccs {
        if cc > 0xff {
            w.extend_from_slice(&cc.to_be_bytes());
        } else {
            w.push(cc as u8);
        }
    }
}

/// Writes `v` as a big-endian integer of `size` bytes (0, 1, 2 or 4).
pub fn write_sized(w: &mut Vec<u8>, v: i32, size: u8) {
    let b = v.to_be_bytes();
    match size {
        1 => w.push(b[3]),
        2 => w.extend_from_slice(&b[2..]),
        4 => w.extend_from_slice(&b),
        _ => {}
    }
}
