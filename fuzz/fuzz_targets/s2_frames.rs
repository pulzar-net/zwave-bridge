//! S2 frames from an untrusted peer: Nonce Reports and Message
//! Encapsulations in every SPAN state.
#![no_main]

use libfuzzer_sys::fuzz_target;
use zwave_security::s2::{Encapsulation, S2Manager};
use zwave_security::SecurityClass;

fuzz_target!(|data: &[u8]| {
    let _ = Encapsulation::parse(data);
    let mut m = S2Manager::new(1, 0xc0ff_ee01);
    m.set_network_key(SecurityClass::S2Authenticated, &[7; 16]);
    m.set_peer_classes(2, &[SecurityClass::S2Authenticated]);
    // No SPAN, then a local REI.
    let _ = m.decrypt(2, data);
    let _ = m.nonce_report(2, false);
    let _ = m.decrypt(2, data);
    let _ = m.decrypt_multicast(2, data);
    let _ = m.handle_nonce_report(2, data);
    let _ = m.decrypt(2, data);
});
