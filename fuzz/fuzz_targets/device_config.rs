//! Device config files and `$if` conditions as delivered from outside.
#![no_main]

use libfuzzer_sys::fuzz_target;
use std::collections::BTreeMap;
use zwave_host::device_config::{evaluate, plan_settings, DeviceId, Library, MapSource};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else { return };
    let id = DeviceId { manufacturer_id: 1, product_type: 2, product_id: 3, firmware_version: "1.2".into(), sdk_version: Some("7.18".into()) };
    let _ = evaluate(text, &id);
    // The text as a device file that may import itself or a template.
    let mut files = BTreeMap::new();
    files.insert("0x0001/dev.json".to_string(), text.to_string());
    files.insert("templates/t.json".to_string(), text.to_string());
    let lib = Library::new(MapSource::new(files));
    if let Ok(cfg) = lib.load("0x0001/dev.json", &id) {
        let keys: Vec<(String, i64)> = cfg.params.iter().map(|p| (p.key.clone(), p.default)).collect();
        let _ = plan_settings(&cfg, &keys, &|_| Some(0));
    }
});
