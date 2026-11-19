//! Every generated command class parser: arbitrary frames parse or fail
//! without panicking, and whatever parses re-encodes to something that
//! parses back to the same command.
#![no_main]

use libfuzzer_sys::fuzz_target;
use zwave_cc::{crc16, Command};

fuzz_target!(|data: &[u8]| {
    if let Ok(c) = Command::parse(data) {
        let e = c.encode();
        let again = Command::parse(&e).expect("re-encoded command parses");
        assert_eq!(again, c, "{}", c.name());
    }
    let _ = crc16::decapsulate(data);
    let _ = zwave_cc::decode_cc_list(data);
    let _ = zwave_cc::decode_bitmask(data, 1);
});
