//! The NVM3 reader against real controller backups: the 700 and 800
//! series images Z-Wave JS publishes with its NVM tools (MIT), with the
//! values their JSON exports state. Point `ZWAVE_JS_NVM_DIR` at a directory
//! holding `nvm_700_binary/*.bin` and `nvm_700_json/*.json`; CI fetches
//! them. Skipped without it, unless `ZWAVE_JS_NVM_REQUIRED=1`.

use zwave_host::nvm3::{Nvm3, NvmInfo};

#[test]
fn real_backups_parse() {
    let dir = match std::env::var("ZWAVE_JS_NVM_DIR") {
        Ok(d) => std::path::PathBuf::from(d),
        Err(_) => {
            assert!(
                std::env::var("ZWAVE_JS_NVM_REQUIRED").as_deref() != Ok("1"),
                "ZWAVE_JS_NVM_DIR is required"
            );
            eprintln!("skipped: ZWAVE_JS_NVM_DIR not set");
            return;
        }
    };
    let mut checked = 0;
    let mut compared = 0;
    for e in std::fs::read_dir(dir.join("nvm_700_binary")).unwrap() {
        let path = e.unwrap().path();
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let image = std::fs::read(&path).unwrap();
        let n = Nvm3::parse(&image).unwrap_or_else(|e| panic!("{name}: {e}"));
        let info = NvmInfo::read(&image).unwrap();
        assert_eq!(info.shared, name.contains("_800_"), "{name}: volume layout");
        if info.shared {
            assert!(
                info.protocol.is_none() && info.application.is_some(),
                "{name}: 800 series version files"
            );
        } else {
            assert_eq!(
                info.protocol
                    .unwrap_or_else(|| panic!("{name}: no protocol version"))
                    .major,
                7,
                "{name}"
            );
        }
        assert!(info.home_id.is_some_and(|h| h != 0), "{name}: home id");
        assert!(n.keys().len() > 5, "{name}: files");
        // The JSON export of the same backup, where one is published.
        let json = dir.join("nvm_700_json").join(format!("{name}.json"));
        if let Ok(text) = std::fs::read_to_string(&json) {
            let j: serde_json::Value = serde_json::from_str(&text).unwrap();
            let c = &j["controller"];
            // Some exports are of other backups with the same SDK; compare only the same network.
            if c["homeId"].as_str() == Some(format!("{:#010x}", info.home_id.unwrap()).as_str()) {
                assert_eq!(
                    c["nodeId"].as_u64().unwrap(),
                    u64::from(info.node_id.unwrap()),
                    "{name}: node id"
                );
                compared += 1;
            }
        }
        checked += 1;
    }
    eprintln!("{checked} images read, {compared} compared with their JSON");
    assert!(checked >= 10 && compared >= 3);
}
