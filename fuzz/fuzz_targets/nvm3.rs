//! NVM3 images from a backup file nobody vetted.
#![no_main]

use libfuzzer_sys::fuzz_target;
use zwave_host::nvm3::{Nvm3, NvmInfo};

fuzz_target!(|data: &[u8]| {
    if let Ok(n) = Nvm3::parse(data) {
        for k in n.keys() {
            assert!(n.file(k).is_some_and(|f| f.len() <= data.len()));
        }
    }
    if let Ok(i) = NvmInfo::read(data) {
        let _ = i.restorable_onto(&i);
    }
});
