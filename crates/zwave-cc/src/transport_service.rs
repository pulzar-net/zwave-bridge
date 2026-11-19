//! Transport Service: frames longer than one radio frame, split into
//! segments of a datagram with a CRC-16 each.
//!
//! The command byte carries data: the top five bits are the command, the
//! low three are the top of the datagram size (or, in Segment Request, the
//! offset). Hence this module is written by hand.

use crate::crc16;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// The Transport Service command class id.
pub const CC: u8 = 0x55;
/// First Segment.
pub const FIRST_SEGMENT: u8 = 0xc0;
/// Segment Complete.
pub const SEGMENT_COMPLETE: u8 = 0xe8;
/// Segment Request.
pub const SEGMENT_REQUEST: u8 = 0xc8;
/// Segment Wait.
pub const SEGMENT_WAIT: u8 = 0xf0;
/// Subsequent Segment.
pub const SUBSEQUENT_SEGMENT: u8 = 0xe0;

/// Largest datagram the 11-bit size field can describe.
pub const MAX_DATAGRAM: usize = 0x7ff;
/// Segment payload bytes per frame that fit any Z-Wave data rate with room for security.
pub const DEFAULT_SEGMENT: usize = 39;
/// How long a reassembly waits for the next segment.
pub const SEGMENT_TIMEOUT: Duration = Duration::from_secs(1);
/// Reassemblies kept at once (per stack); older ones are dropped.
pub const MAX_SESSIONS: usize = 16;

/// A parsed Transport Service frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// The first segment of a datagram.
    First {
        /// Total datagram size.
        size: u16,
        /// Session id (0 to 15).
        session: u8,
        /// Payload bytes.
        data: Vec<u8>,
    },
    /// A later segment.
    Subsequent {
        /// Total datagram size.
        size: u16,
        /// Session id.
        session: u8,
        /// Where this payload goes in the datagram.
        offset: u16,
        /// Payload bytes.
        data: Vec<u8>,
    },
    /// The receiver asks for the segment at `offset` again.
    Request {
        /// Session id.
        session: u8,
        /// Missing offset.
        offset: u16,
    },
    /// The receiver has the whole datagram.
    Complete {
        /// Session id.
        session: u8,
    },
    /// The receiver is busy; wait.
    Wait {
        /// Segments still pending at the receiver.
        pending: u8,
    },
}

/// Why a frame was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// Too short or a wrong command.
    #[error("malformed transport service frame")]
    Malformed,
    /// The segment's CRC did not match.
    #[error("transport service CRC mismatch")]
    Crc,
    /// A segment points outside its datagram.
    #[error("segment outside the datagram")]
    OutOfBounds,
}

/// Parses a Transport Service frame (starting at `0x55`).
pub fn parse(frame: &[u8]) -> Result<Segment, Error> {
    if frame.len() < 2 || frame[0] != CC {
        return Err(Error::Malformed);
    }
    let cmd = frame[1] & 0xf8;
    let hi = u16::from(frame[1] & 0x07);
    match cmd {
        FIRST_SEGMENT | SUBSEQUENT_SEGMENT => {
            let header = if cmd == FIRST_SEGMENT { 4 } else { 5 };
            if frame.len() < header + 2 {
                return Err(Error::Malformed);
            }
            let (body, crc) = frame.split_at(frame.len() - 2);
            if crc16::checksum(body) != u16::from_be_bytes([crc[0], crc[1]]) {
                return Err(Error::Crc);
            }
            let size = (hi << 8) | u16::from(frame[2]);
            let session = frame[3] >> 4;
            let ext = frame[3] & 0x08 != 0;
            let mut at = header;
            if ext {
                // Header extension: a length byte and that many bytes, skipped.
                let n = usize::from(*body.get(at).ok_or(Error::Malformed)?);
                at += 1 + n;
                if at > body.len() {
                    return Err(Error::Malformed);
                }
            }
            let data = body[at..].to_vec();
            if cmd == FIRST_SEGMENT {
                Ok(Segment::First {
                    size,
                    session,
                    data,
                })
            } else {
                let offset = (u16::from(frame[3] & 0x07) << 8) | u16::from(frame[4]);
                Ok(Segment::Subsequent {
                    size,
                    session,
                    offset,
                    data,
                })
            }
        }
        SEGMENT_REQUEST => {
            if frame.len() < 4 {
                return Err(Error::Malformed);
            }
            Ok(Segment::Request {
                session: frame[2] >> 4,
                offset: (u16::from(frame[2] & 0x07) << 8) | u16::from(frame[3]),
            })
        }
        SEGMENT_COMPLETE => Ok(Segment::Complete {
            session: *frame.get(2).ok_or(Error::Malformed)? >> 4,
        }),
        SEGMENT_WAIT => Ok(Segment::Wait {
            pending: *frame.get(2).ok_or(Error::Malformed)?,
        }),
        _ => Err(Error::Malformed),
    }
}

fn with_crc(mut v: Vec<u8>) -> Vec<u8> {
    let c = crc16::checksum(&v);
    v.extend_from_slice(&c.to_be_bytes());
    v
}

/// Splits `datagram` into Transport Service frames of at most `segment` payload bytes.
pub fn segment(datagram: &[u8], session: u8, segment: usize) -> Vec<Vec<u8>> {
    assert!(datagram.len() <= MAX_DATAGRAM, "datagram too long");
    let segment = segment.max(1);
    let size = datagram.len() as u16;
    let mut out = Vec::new();
    for (i, chunk) in datagram.chunks(segment).enumerate() {
        let offset = (i * segment) as u16;
        let mut v = vec![CC];
        if i == 0 {
            v.push(FIRST_SEGMENT | ((size >> 8) as u8 & 0x07));
            v.push(size as u8);
            v.push((session & 0x0f) << 4);
        } else {
            v.push(SUBSEQUENT_SEGMENT | ((size >> 8) as u8 & 0x07));
            v.push(size as u8);
            v.push(((session & 0x0f) << 4) | ((offset >> 8) as u8 & 0x07));
            v.push(offset as u8);
        }
        v.extend_from_slice(chunk);
        out.push(with_crc(v));
    }
    out
}

/// Builds a Segment Complete.
pub fn complete(session: u8) -> Vec<u8> {
    vec![CC, SEGMENT_COMPLETE, (session & 0x0f) << 4]
}

/// Builds a Segment Request for the segment at `offset`.
pub fn request(session: u8, offset: u16) -> Vec<u8> {
    vec![
        CC,
        SEGMENT_REQUEST,
        ((session & 0x0f) << 4) | ((offset >> 8) as u8 & 0x07),
        offset as u8,
    ]
}

struct Partial {
    size: usize,
    data: Vec<u8>,
    have: Vec<bool>,
    last: Instant,
}

/// What the reassembler did with a segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reassembly {
    /// More segments are needed.
    Pending,
    /// The datagram is whole; answer the sender with [`complete`].
    Done(Vec<u8>),
}

/// Reassembles datagrams from any number of peers.
#[derive(Default)]
pub struct Reassembler {
    sessions: HashMap<(u16, u8), Partial>,
}

impl std::fmt::Debug for Reassembler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Reassembler({} sessions)", self.sessions.len())
    }
}

impl Reassembler {
    /// Feeds a First or Subsequent segment from `peer`.
    pub fn push(&mut self, peer: u16, seg: Segment) -> Result<Reassembly, Error> {
        let now = Instant::now();
        self.sessions
            .retain(|_, p| now.duration_since(p.last) < SEGMENT_TIMEOUT * 4);
        let (size, session, offset, data) = match seg {
            Segment::First {
                size,
                session,
                data,
            } => {
                if self.sessions.len() >= MAX_SESSIONS {
                    if let Some(k) = self
                        .sessions
                        .iter()
                        .min_by_key(|(_, p)| p.last)
                        .map(|(k, _)| *k)
                    {
                        self.sessions.remove(&k);
                    }
                }
                let size = usize::from(size);
                self.sessions.insert(
                    (peer, session),
                    Partial {
                        size,
                        data: vec![0; size],
                        have: vec![false; size],
                        last: now,
                    },
                );
                (size, session, 0usize, data)
            }
            Segment::Subsequent {
                size,
                session,
                offset,
                data,
            } => (usize::from(size), session, usize::from(offset), data),
            _ => return Err(Error::Malformed),
        };
        let p = self
            .sessions
            .get_mut(&(peer, session))
            .ok_or(Error::OutOfBounds)?;
        if p.size != size || offset + data.len() > p.size {
            self.sessions.remove(&(peer, session));
            return Err(Error::OutOfBounds);
        }
        p.data[offset..offset + data.len()].copy_from_slice(&data);
        p.have[offset..offset + data.len()]
            .iter_mut()
            .for_each(|h| *h = true);
        p.last = now;
        if p.have.iter().all(|&h| h) {
            let p = self.sessions.remove(&(peer, session)).expect("present");
            return Ok(Reassembly::Done(p.data));
        }
        Ok(Reassembly::Pending)
    }

    /// The first missing offset of a session that has gone quiet, to ask for again.
    pub fn missing(&self, peer: u16, session: u8) -> Option<u16> {
        let p = self.sessions.get(&(peer, session))?;
        p.have.iter().position(|h| !h).map(|i| i as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_and_reassemble() {
        let datagram: Vec<u8> = (0..100u8).collect();
        let frames = segment(&datagram, 3, 39);
        assert_eq!(frames.len(), 3);
        let mut r = Reassembler::default();
        // Out of order: last first, after the first segment.
        assert_eq!(
            r.push(7, parse(&frames[0]).unwrap()).unwrap(),
            Reassembly::Pending
        );
        assert_eq!(
            r.push(7, parse(&frames[2]).unwrap()).unwrap(),
            Reassembly::Pending
        );
        assert_eq!(r.missing(7, 3), Some(39));
        assert_eq!(
            r.push(7, parse(&frames[1]).unwrap()).unwrap(),
            Reassembly::Done(datagram)
        );
    }

    #[test]
    fn crc_and_bounds() {
        let mut f = segment(&[1, 2, 3], 1, 39).remove(0);
        f[5] ^= 1;
        assert_eq!(parse(&f), Err(Error::Crc));
        let mut r = Reassembler::default();
        let bogus = Segment::Subsequent {
            size: 4,
            session: 1,
            offset: 3,
            data: vec![1, 2],
        };
        assert!(r.push(1, bogus).is_err());
        r.push(
            1,
            Segment::First {
                size: 4,
                session: 1,
                data: vec![1],
            },
        )
        .unwrap();
        assert_eq!(
            r.push(
                1,
                Segment::Subsequent {
                    size: 4,
                    session: 1,
                    offset: 3,
                    data: vec![1, 2]
                }
            ),
            Err(Error::OutOfBounds)
        );
        assert_eq!(
            parse(&request(2, 300)).unwrap(),
            Segment::Request {
                session: 2,
                offset: 300
            }
        );
        assert_eq!(
            parse(&complete(5)).unwrap(),
            Segment::Complete { session: 5 }
        );
    }
}
