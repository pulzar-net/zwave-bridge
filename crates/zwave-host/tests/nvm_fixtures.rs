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

/// Every real 700 series backup converts to the 800 series layout and back
/// with its protocol files intact; the published 800 image, which has no
/// protocol version file, is refused both ways.
#[test]
fn real_backups_convert_between_series() {
    use zwave_host::nvm3::*;
    let Ok(dir) = std::env::var("ZWAVE_JS_NVM_DIR") else {
        assert!(std::env::var("ZWAVE_JS_NVM_REQUIRED").as_deref() != Ok("1"));
        return;
    };
    let bin = std::path::Path::new(&dir).join("nvm_700_binary");
    let read = |n: &str| std::fs::read(bin.join(format!("{n}.bin"))).unwrap();
    let target_800 = write_volume(
        IMAGE_SIZE_800,
        8192,
        &[
            (APPLICATION_VERSION_FILE_800, vec![0, 23, 7, 0]),
            (RF_CONFIG_FILE, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]),
        ],
    )
    .unwrap();
    let target_700 = read("ctrlr_backup_700_7.19.1");
    let protocol = |n: &Nvm3| -> Vec<(u32, Vec<u8>)> {
        n.keys()
            .into_iter()
            .filter(|k| (0x50000..0x60000).contains(k) && *k != APPLICATION_VERSION_FILE_700)
            .map(|k| (k, n.file(k).unwrap().to_vec()))
            .collect()
    };
    let mut seen = 0;
    for e in std::fs::read_dir(&bin).unwrap() {
        let name = e.unwrap().file_name().into_string().unwrap();
        if !name.starts_with("ctrlr_backup_700_") {
            continue;
        }
        seen += 1;
        let image = std::fs::read(bin.join(&name)).unwrap();
        let src = Nvm3::parse(&image).unwrap();
        let to800 = convert(&image, &target_800).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(to800.len(), IMAGE_SIZE_800, "{name}");
        let n8 = Nvm3::parse(&to800).unwrap();
        assert!(n8.shared, "{name}");
        assert_eq!(protocol(&n8), protocol(&src), "{name}: protocol files");
        assert_eq!(
            n8.file(RF_CONFIG_FILE),
            Some(&[1, 2, 3, 4, 5, 6, 7, 8, 9][..]),
            "{name}: the target's RF config"
        );
        assert_eq!(n8.file(APPLICATION_VERSION_FILE_700), None, "{name}");
        let back = convert(&to800, &target_700).unwrap_or_else(|e| panic!("{name} back: {e}"));
        assert_eq!(back.len(), target_700.len(), "{name}");
        let n7 = Nvm3::parse(&back).unwrap();
        assert!(!n7.shared, "{name}");
        assert_eq!(protocol(&n7), protocol(&src), "{name}: protocol files back");
        assert_eq!(
            NvmInfo::read(&back).unwrap().home_id,
            NvmInfo::read(&image).unwrap().home_id,
            "{name}"
        );
    }
    assert!(seen >= 10, "fixtures missing");
    let real_800 = read("ctrlr_backup_800_7.23.0");
    assert!(convert(&real_800, &target_700)
        .unwrap_err()
        .contains("cannot be told"));
    assert!(convert(&read("ctrlr_backup_700_7.19.1"), &real_800).is_err());
}
