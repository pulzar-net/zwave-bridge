//! The Serial API frame decoder on arbitrary bytes, whole and split at
//! every chunk size: never panics, never returns a frame it cannot re-encode.
#![no_main]

use bytes::BytesMut;
use libfuzzer_sys::fuzz_target;
use tokio_util::codec::Decoder;
use zwave_serial::frame::{decode_all, Frame, FrameCodec};

fuzz_target!(|data: &[u8]| {
    let whole = decode_all(data);
    for f in &whole {
        if let Frame::Data(d) = f {
            assert!(d.payload.len() <= zwave_serial::frame::MAX_PAYLOAD);
            let re = decode_all(&d.encode());
            assert_eq!(re, vec![f.clone()]);
        }
    }
    // Feeding the same bytes in pieces gives the same frames.
    let step = usize::from(data.first().copied().unwrap_or(1)).max(1);
    let mut codec = FrameCodec::default();
    let mut buf = BytesMut::new();
    let mut pieces = Vec::new();
    for chunk in data.chunks(step) {
        buf.extend_from_slice(chunk);
        while let Ok(Some(f)) = codec.decode(&mut buf) {
            pieces.push(f);
        }
    }
    assert_eq!(pieces, whole);
});
