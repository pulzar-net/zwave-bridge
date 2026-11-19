//! S0 frames from an untrusted peer.
#![no_main]

use libfuzzer_sys::fuzz_target;
use zwave_security::s0::S0Manager;

fuzz_target!(|data: &[u8]| {
    let mut m = S0Manager::new(1, &[9; 16]);
    let _ = m.handle_nonce_report(2, data);
    // Issue a nonce whose id the input may name.
    let _ = m.nonce_report(2);
    let _ = m.decapsulate(2, data, false);
    let _ = m.decapsulate(2, data, true);
});
