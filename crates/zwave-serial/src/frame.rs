//! Serial API frames.
//!
//! A data frame is `SOF LEN TYPE FUNC PAYLOAD.. CHECKSUM`, where `LEN` counts
//! the bytes after itself (type, function, payload and checksum) and the
//! checksum is `0xFF` XOR every byte from `LEN` to the end of the payload.
//! Single-byte frames acknowledge (`ACK`), reject (`NAK`) or signal a
//! collision (`CAN`).

use bytes::{Buf, BufMut, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

/// Start of frame.
pub const SOF: u8 = 0x01;
/// Acknowledge.
pub const ACK: u8 = 0x06;
/// Negative acknowledge (bad checksum or timeout on the receiver).
pub const NAK: u8 = 0x15;
/// Cancel: the receiver was sending a frame itself.
pub const CAN: u8 = 0x18;

/// Longest payload a frame can carry: LEN is one byte and counts type, function and checksum too.
pub const MAX_PAYLOAD: usize = 255 - 3;

/// Request (host to controller, or an unsolicited controller frame) or response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameType {
    /// `0x00`: a request, a callback or an unsolicited frame.
    Request,
    /// `0x01`: the controller's response to a request.
    Response,
}

/// A data frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataFrame {
    /// Request or response.
    pub kind: FrameType,
    /// Function id.
    pub function: u8,
    /// Payload, at most [`MAX_PAYLOAD`] bytes.
    pub payload: Vec<u8>,
}

impl DataFrame {
    /// A request frame.
    pub fn request(function: impl Into<u8>, payload: Vec<u8>) -> Self {
        DataFrame {
            kind: FrameType::Request,
            function: function.into(),
            payload,
        }
    }

    /// A response frame.
    pub fn response(function: impl Into<u8>, payload: Vec<u8>) -> Self {
        DataFrame {
            kind: FrameType::Response,
            function: function.into(),
            payload,
        }
    }

    /// Serializes the frame. Panics if the payload exceeds [`MAX_PAYLOAD`],
    /// which every builder in this crate rules out.
    pub fn encode(&self) -> Vec<u8> {
        assert!(self.payload.len() <= MAX_PAYLOAD, "Serial API payload too long");
        let len = (self.payload.len() + 3) as u8;
        let mut v = Vec::with_capacity(self.payload.len() + 5);
        v.push(SOF);
        v.push(len);
        v.push(match self.kind {
            FrameType::Request => 0x00,
            FrameType::Response => 0x01,
        });
        v.push(self.function);
        v.extend_from_slice(&self.payload);
        v.push(checksum(&v[1..]));
        v
    }
}

/// The Serial API checksum over `LEN..=last payload byte`.
pub fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0xff, |acc, b| acc ^ b)
}

/// One unit on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// ACK.
    Ack,
    /// NAK.
    Nak,
    /// CAN.
    Can,
    /// A data frame with a valid checksum.
    Data(DataFrame),
    /// A data frame whose checksum or type byte was wrong; answer with NAK.
    Invalid,
}

impl Frame {
    /// Serializes the frame (`Invalid` encodes as nothing).
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Frame::Ack => vec![ACK],
            Frame::Nak => vec![NAK],
            Frame::Can => vec![CAN],
            Frame::Data(d) => d.encode(),
            Frame::Invalid => vec![],
        }
    }
}

/// Streaming codec for Serial API frames.
///
/// Bytes that cannot start a frame are skipped. A data frame whose length
/// byte is below the minimum is skipped as noise. A frame that is still
/// incomplete is kept until more bytes arrive; the caller's read timeout
/// decides when a partial frame is abandoned (see [`FrameCodec::reset`]).
#[derive(Debug, Default, Clone)]
pub struct FrameCodec {
    skipped: usize,
}

impl FrameCodec {
    /// Bytes skipped as noise so far.
    pub fn skipped(&self) -> usize {
        self.skipped
    }

    /// Drops a partial frame from `buf` (the receive timeout expired mid-frame).
    pub fn reset(&mut self, buf: &mut BytesMut) {
        self.skipped += buf.len();
        buf.clear();
    }
}

impl Decoder for FrameCodec {
    type Item = Frame;
    type Error = std::io::Error;

    fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<Frame>, std::io::Error> {
        loop {
            let Some(&first) = buf.first() else {
                return Ok(None);
            };
            match first {
                ACK => {
                    buf.advance(1);
                    return Ok(Some(Frame::Ack));
                }
                NAK => {
                    buf.advance(1);
                    return Ok(Some(Frame::Nak));
                }
                CAN => {
                    buf.advance(1);
                    return Ok(Some(Frame::Can));
                }
                SOF => {
                    let Some(&len) = buf.get(1) else {
                        return Ok(None);
                    };
                    let len = len as usize;
                    if len < 3 {
                        // Not a frame; resynchronize on the next byte.
                        buf.advance(1);
                        self.skipped += 1;
                        continue;
                    }
                    if buf.len() < len + 2 {
                        buf.reserve(len + 2 - buf.len());
                        return Ok(None);
                    }
                    let raw = buf.split_to(len + 2);
                    let kind = match raw[2] {
                        0x00 => FrameType::Request,
                        0x01 => FrameType::Response,
                        _ => return Ok(Some(Frame::Invalid)),
                    };
                    if checksum(&raw[1..len + 1]) != raw[len + 1] {
                        return Ok(Some(Frame::Invalid));
                    }
                    return Ok(Some(Frame::Data(DataFrame {
                        kind,
                        function: raw[3],
                        payload: raw[4..len + 1].to_vec(),
                    })));
                }
                _ => {
                    buf.advance(1);
                    self.skipped += 1;
                }
            }
        }
    }
}

impl Encoder<Frame> for FrameCodec {
    type Error = std::io::Error;

    fn encode(&mut self, item: Frame, dst: &mut BytesMut) -> Result<(), std::io::Error> {
        dst.put_slice(&item.encode());
        Ok(())
    }
}

/// Decodes every complete frame in `bytes` (for tests and fuzzing).
pub fn decode_all(bytes: &[u8]) -> Vec<Frame> {
    let mut codec = FrameCodec::default();
    let mut buf = BytesMut::from(bytes);
    let mut out = Vec::new();
    while let Ok(Some(f)) = codec.decode(&mut buf) {
        out.push(f);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_frames() {
        // GetSerialApiCapabilities request, as every host sends it.
        let f = DataFrame::request(0x07u8, vec![]);
        assert_eq!(f.encode(), vec![0x01, 0x03, 0x00, 0x07, 0xfb]);
        // GetControllerId response: home id 0xC0FFEE01, node 1.
        let r = DataFrame::response(0x20u8, vec![0xc0, 0xff, 0xee, 0x01, 0x01]);
        let enc = r.encode();
        assert_eq!(decode_all(&enc), vec![Frame::Data(r)]);
    }

    #[test]
    fn noise_partial_and_bad_checksum() {
        let good = DataFrame::request(0x04u8, vec![0x00, 0x02, 0x03, 0x25, 0x03, 0xff]).encode();
        let mut bad = good.clone();
        let last = bad.len() - 1;
        bad[last] ^= 0xff;
        let mut stream = vec![0x42, 0x00, SOF, 0x02];
        stream.extend_from_slice(&[ACK, NAK, CAN]);
        stream.extend_from_slice(&bad);
        stream.extend_from_slice(&good);
        let frames = decode_all(&stream);
        assert_eq!(frames.len(), 5);
        assert_eq!(frames[0], Frame::Ack);
        assert_eq!(frames[3], Frame::Invalid);
        assert!(matches!(frames[4], Frame::Data(_)));

        // A partial frame waits for more bytes.
        let mut codec = FrameCodec::default();
        let mut buf = BytesMut::from(&good[..4]);
        assert_eq!(codec.decode(&mut buf).unwrap(), None);
        buf.extend_from_slice(&good[4..]);
        assert!(matches!(codec.decode(&mut buf).unwrap(), Some(Frame::Data(_))));
    }

    #[test]
    fn bad_type_byte_is_invalid() {
        let mut raw = DataFrame::request(0x04u8, vec![1]).encode();
        raw[2] = 0x07;
        let n = raw.len() - 1;
        raw[n] = checksum(&raw[1..n]);
        assert_eq!(decode_all(&raw), vec![Frame::Invalid]);
    }
}
