//! Transport Service parsing and reassembly with arbitrary segments.
#![no_main]

use libfuzzer_sys::fuzz_target;
use zwave_cc::transport_service::{parse, Reassembler, Reassembly};

fuzz_target!(|data: &[u8]| {
    let mut r = Reassembler::default();
    // Several frames separated by their first length byte.
    let mut rest = data;
    while let Some((&n, tail)) = rest.split_first() {
        let n = usize::from(n).min(tail.len());
        let (frame, next) = tail.split_at(n);
        if let Ok(seg) = parse(frame) {
            if let Ok(Reassembly::Done(d)) = r.push(u16::from(frame.get(1).copied().unwrap_or(0) & 3), seg) {
                assert!(d.len() <= zwave_cc::transport_service::MAX_DATAGRAM);
            }
        }
        rest = next;
    }
});
