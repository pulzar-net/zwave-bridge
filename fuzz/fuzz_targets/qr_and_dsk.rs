//! QR codes, DSKs and PINs as typed or scanned text.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else { return };
    if let Ok(q) = zwave_security::qr::parse(s) {
        // A parsed code re-encodes to one that parses the same (TLV order may differ).
        let again = zwave_security::qr::parse(&zwave_security::qr::encode(&q)).expect("re-encoded");
        assert_eq!(again.dsk, q.dsk);
    }
    if let Ok(d) = s.parse::<zwave_security::dsk::Dsk>() {
        assert_eq!(d.to_string().parse::<zwave_security::dsk::Dsk>().unwrap(), d);
    }
    let _ = zwave_security::dsk::parse_pin(s);
});
