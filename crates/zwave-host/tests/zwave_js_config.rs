//! The device config loader against the real `@zwave-js/config` package:
//! every device file resolves and evaluates for every device it lists, at
//! its lowest and highest firmware. Point `ZWAVE_JS_CONFIG_DIR` at the
//! package's `config/devices` directory; CI downloads the latest release.
//! Without it the tests are skipped, unless `ZWAVE_JS_CONFIG_REQUIRED=1`.

use zwave_host::device_config::{plan_settings, DeviceId, DirSource, Library};

fn library() -> Option<Library> {
    match std::env::var("ZWAVE_JS_CONFIG_DIR") {
        Ok(d) => Some(Library::new(DirSource::new(d))),
        Err(_) => {
            assert!(
                std::env::var("ZWAVE_JS_CONFIG_REQUIRED").as_deref() != Ok("1"),
                "ZWAVE_JS_CONFIG_DIR is required"
            );
            eprintln!("skipped: ZWAVE_JS_CONFIG_DIR not set");
            None
        }
    }
}

#[test]
fn every_device_file_loads() {
    let Some(lib) = library() else { return };
    let index = lib.index();
    let mut failures = Vec::new();
    let mut loaded = 0;
    let mut params = 0;
    for e in &index {
        let e = match e {
            Ok(e) => e,
            Err(err) => {
                failures.push(err.to_string());
                continue;
            }
        };
        for &(pt, pid) in &e.devices {
            for fw in [&e.firmware.0, &e.firmware.1] {
                let id = DeviceId {
                    manufacturer_id: e.manufacturer_id,
                    product_type: pt,
                    product_id: pid,
                    firmware_version: fw.clone(),
                    sdk_version: Some("7.18.0".into()),
                };
                match lib.load(&e.path, &id) {
                    Ok(c) => {
                        loaded += 1;
                        params += c.params.len();
                        for p in &c.params {
                            if p.size == 0 && !p.read_only {
                                failures
                                    .push(format!("{}: parameter {} has no size", e.path, p.key));
                            }
                        }
                    }
                    Err(err) => failures.push(err.to_string()),
                }
            }
        }
    }
    failures.sort();
    failures.dedup();
    eprintln!(
        "{} files, {loaded} evaluations, {params} parameters, {} failures",
        index.len(),
        failures.len()
    );
    for f in failures.iter().take(30) {
        eprintln!("  {f}");
    }
    assert!(
        index.len() > 1000,
        "the directory does not look like config/devices"
    );
    assert!(failures.is_empty());
}

#[test]
fn zooz_zse40_by_firmware() {
    let Some(lib) = library() else { return };
    let id = |fw: &str| DeviceId {
        manufacturer_id: 0x027a,
        product_type: 0x2021,
        product_id: 0x2101,
        firmware_version: fw.into(),
        sdk_version: None,
    };
    let new = lib
        .lookup(&id("32.32"))
        .unwrap()
        .expect("ZSE40 is in the library");
    assert_eq!(new.label, "ZSE40 700");
    assert_eq!(new.lifeline(), Some(1));
    let p5 = new.param("5").unwrap();
    assert_eq!(
        (p5.min, p5.max, p5.unit.as_deref()),
        (15, 255, Some("seconds"))
    );
    // The first hardware revision's firmware has a different range and unit.
    let old = lib.lookup(&id("16.9")).unwrap().unwrap();
    assert_eq!(old.label, "ZSE40");
    assert_eq!(old.param("5").unwrap().unit.as_deref(), Some("minutes"));
    // Parameter 1 comes from the master template.
    assert!(!new.param("1").unwrap().options.is_empty());
    let w = plan_settings(&new, &[("2".into(), 20)], &|_| None).unwrap();
    assert_eq!(w[0].command().encode(), vec![0x70, 0x04, 0x02, 0x01, 20]);
}

/// The platform delivers one device's files as a map keyed by path; the
/// loader reads that the same as the directory.
#[test]
fn map_source_matches_directory() {
    let Ok(dir) = std::env::var("ZWAVE_JS_CONFIG_DIR") else {
        return;
    };
    let read = |p: &str| std::fs::read_to_string(std::path::Path::new(&dir).join(p)).unwrap();
    let files = ["0x027a/zse40.json", "templates/master_template.json"];
    let map = files.iter().map(|p| (p.to_string(), read(p))).collect();
    let from_map = Library::new(zwave_host::device_config::MapSource::new(map));
    let from_dir = Library::new(DirSource::new(&dir));
    let id = DeviceId {
        manufacturer_id: 0x027a,
        product_type: 0x2021,
        product_id: 0x2101,
        firmware_version: "32.32".into(),
        sdk_version: None,
    };
    assert_eq!(
        from_map.lookup(&id).unwrap(),
        from_dir.load("0x027a/zse40.json", &id).ok()
    );
}
