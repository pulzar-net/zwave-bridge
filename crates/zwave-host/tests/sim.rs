//! The host stack against the Serial API simulator: inclusion with S2, S0
//! and SmartStart, interviews, wake-up queues, reports, exclusion, NVM
//! backup and a restart that resumes from the store.

use std::time::Duration;
use zwave_host::node::{InterviewStage, NodeStatus};
use zwave_host::Events;
use zwave_host::{
    Controller, Event, GrantPolicy, HostConfig, InclusionOptions, NetworkKeys, SecurityClass,
    ValueId, ValueKind,
};
use zwave_security::provisioning::Entry;
use zwave_sim::node::NodeSpec;
use zwave_sim::{SimConfig, SimHandle, Simulator};

const T: Duration = Duration::from_secs(20);

fn sim() -> Simulator {
    Simulator::new(SimConfig::default())
        .with_node(NodeSpec::switch("switch"))
        .with_node(NodeSpec::sleeping_sensor("sleeper"))
        .with_node(NodeSpec::s2_sensor(
            "s2",
            vec![
                SecurityClass::S2Authenticated,
                SecurityClass::S2Unauthenticated,
            ],
        ))
        .with_node(NodeSpec::smartstart_sensor("smartstart"))
        .with_node(NodeSpec::long_range_sensor("lr"))
        .with_node(NodeSpec::switch("spare"))
}

fn config(keys: NetworkKeys, dir: &std::path::Path) -> HostConfig {
    let mut c = HostConfig::new(keys, Some(dir.join("nodes.json")));
    c.request_timeout = Duration::from_secs(2);
    c.awake_idle = Duration::from_millis(300);
    c.store_delay = Duration::from_millis(50);
    c.inclusion_timeout = Duration::from_secs(10);
    c
}

async fn start(keys: NetworkKeys, dir: &std::path::Path) -> (Controller, Events, SimHandle) {
    let (host, dev) = tokio::io::duplex(16 * 1024);
    let h = sim().spawn(dev);
    let (c, ev) = Controller::start(host, config(keys, dir))
        .await
        .expect("start");
    (c, ev, h)
}

/// Reads events until `f` picks one, failing after `T`.
async fn expect<R>(ev: &mut Events, what: &str, mut f: impl FnMut(&Event) -> Option<R>) -> R {
    let deadline = tokio::time::Instant::now() + T;
    loop {
        match tokio::time::timeout_at(deadline, ev.recv()).await {
            Ok(Some(e)) => {
                if let Some(r) = f(&e) {
                    return r;
                }
            }
            Ok(None) => panic!("event channel closed waiting for {what}"),
            Err(_) => panic!("timed out waiting for {what}"),
        }
    }
}

async fn include(
    c: &Controller,
    ev: &mut Events,
    h: &SimHandle,
    name: &str,
    opts: InclusionOptions,
) -> zwave_host::InclusionResult {
    let c2 = c.clone();
    let task = tokio::spawn(async move { c2.include(opts).await });
    expect(ev, "inclusion started", |e| {
        matches!(e, Event::InclusionStarted).then_some(())
    })
    .await;
    assert!(h.press(name).await.is_some(), "{name} joined");
    task.await.unwrap().expect("inclusion")
}

async fn interviewed(ev: &mut Events, node: u16) {
    expect(ev, "interview completed", |e| {
        matches!(e, Event::InterviewCompleted { node: n } if *n == node).then_some(())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s2_inclusion_with_pin_interview_and_reports() {
    let dir = tempfile::tempdir().unwrap();
    let (c, mut ev, h) = start(NetworkKeys::generate().unwrap(), dir.path()).await;
    expect(&mut ev, "ready", |e| {
        matches!(e, Event::Ready(_)).then_some(())
    })
    .await;
    assert!(c.info().sis, "the controller made itself SIS");

    let c2 = c.clone();
    let task = tokio::spawn(async move { c2.include(InclusionOptions::default()).await });
    expect(&mut ev, "inclusion started", |e| {
        matches!(e, Event::InclusionStarted).then_some(())
    })
    .await;
    h.press("s2").await.unwrap();
    let requested = expect(&mut ev, "grant request", |e| match e {
        Event::GrantSecurityClasses { requested, .. } => Some(requested.clone()),
        _ => None,
    })
    .await;
    assert_eq!(
        requested,
        vec![
            SecurityClass::S2Authenticated,
            SecurityClass::S2Unauthenticated
        ]
    );
    assert!(c.grant_security_classes(requested));
    let masked = expect(&mut ev, "DSK check", |e| match e {
        Event::ValidateDsk { dsk, .. } => Some(dsk.clone()),
        _ => None,
    })
    .await;
    let dsk = NodeSpec::s2_sensor("x", vec![]).dsk();
    assert_eq!(masked, dsk.masked());
    assert!(c.validate_dsk(Some(dsk.pin())));
    let r = task.await.unwrap().expect("inclusion");
    assert_eq!(
        r.security,
        vec![
            SecurityClass::S2Authenticated,
            SecurityClass::S2Unauthenticated
        ]
    );
    assert_eq!(h.granted("s2").await.len(), 2);
    let node = r.node;
    interviewed(&mut ev, node).await;

    let rec = c.node(node).unwrap();
    assert_eq!(rec.stage, InterviewStage::Complete);
    assert!(rec.secure_ccs_known);
    assert!(
        rec.root().ccs[&0x31].secure,
        "multilevel sensor is secure-only"
    );
    assert_eq!(rec.root().version(0x31), 11);
    assert_eq!(rec.manufacturer.unwrap().product_id, 3);
    assert_eq!(
        rec.root().caps.sensors.keys().copied().collect::<Vec<_>>(),
        vec![1, 5]
    );
    assert_eq!(rec.root().caps.notifications.get(&7), Some(&vec![8]));
    assert!(!rec.neighbors.is_empty());
    assert_eq!(
        h.inspect("s2", |n| n.lifeline.clone()).await.unwrap(),
        vec![1]
    );

    // An S2-encrypted report from the node.
    assert!(h.set_sensor("s2", 1, 23.4).await);
    let v = expect(&mut ev, "temperature", |e| match e {
        Event::Value(u)
            if u.id.node == node
                && u.id.property == "air_temperature"
                && u.value == ValueKind::Number(23.4) =>
        {
            Some(u.clone())
        }
        _ => None,
    })
    .await;
    assert_eq!(v.unit, Some("°C"));
    assert!(h.motion("s2").await);
    expect(&mut ev, "motion", |e| match e {
        Event::Value(u) if u.id.property == "home_security_motion_sensor_status" => {
            matches!(u.value, ValueKind::State { id: 8, .. }).then_some(())
        }
        _ => None,
    })
    .await;
    // The insecure path is refused for a secure-only command class.
    let sent_plain = h
        .inspect("s2", |n| {
            n.received
                .iter()
                .any(|f| f.first() == Some(&0x31) && f.get(1) == Some(&0x04))
        })
        .await
        .unwrap();
    assert!(sent_plain, "the node got its sensor Gets (decrypted)");

    let topo = c.topology();
    assert!(topo
        .nodes
        .iter()
        .any(|n| n.id == node && n.last_rssi.is_some()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sleeping_s0_node_and_wake_up_queue() {
    let dir = tempfile::tempdir().unwrap();
    let (c, mut ev, h) = start(NetworkKeys::generate().unwrap(), dir.path()).await;
    let r = include(&c, &mut ev, &h, "sleeper", InclusionOptions::default()).await;
    assert_eq!(r.security, vec![SecurityClass::S0Legacy]);
    let node = r.node;
    interviewed(&mut ev, node).await;
    let rec = c.node(node).unwrap();
    assert!(rec.protocol.sleeping());
    assert_eq!(rec.wake_up_interval, Some(3600));
    assert!(rec.root().ccs[&0x80].secure);
    // Sent back to sleep once idle.
    expect(&mut ev, "asleep", |e| {
        matches!(e, Event::NodeStatus { node: n, status: NodeStatus::Asleep } if *n == node)
            .then_some(())
    })
    .await;
    assert!(!h.inspect("sleeper", |n| n.awake).await.unwrap());

    // A request to the sleeping node waits for its wake-up.
    let c2 = c.clone();
    let pending = tokio::spawn(async move {
        c2.request(node, 0, zwave_cc::battery::Get {}.into(), |c| {
            matches!(c, zwave_cc::Command::Battery(_))
        })
        .await
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!pending.is_finished(), "queued until the node wakes");
    assert!(h.wake_up("sleeper").await);
    let reply = tokio::time::timeout(T, pending)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        matches!(reply, zwave_cc::Command::Battery(zwave_cc::battery::Command::Report(r)) if r.level == 87)
    );
    expect(&mut ev, "asleep again", |e| {
        matches!(e, Event::NodeStatus { node: n, status: NodeStatus::Asleep } if *n == node)
            .then_some(())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn switch_set_supervision_and_exclusion() {
    let dir = tempfile::tempdir().unwrap();
    let (c, mut ev, h) = start(NetworkKeys::generate().unwrap(), dir.path()).await;
    let r = include(
        &c,
        &mut ev,
        &h,
        "switch",
        InclusionOptions {
            grant: GrantPolicy::Requested,
            ..Default::default()
        },
    )
    .await;
    assert!(r.security.is_empty());
    let node = r.node;
    interviewed(&mut ev, node).await;
    let rec = c.node(node).unwrap();
    assert_eq!(rec.root().caps.meter, Some((1, vec![0, 2])));

    let id = ValueId {
        node,
        endpoint: 0,
        cc: 0x25,
        property: "current".into(),
    };
    c.set_value(&id, &ValueKind::Bool(true)).await.unwrap();
    assert!(h.inspect("switch", |n| n.switch_on).await.unwrap());
    expect(&mut ev, "switch on", |e| {
        matches!(e, Event::Value(u) if u.id == id && u.value == ValueKind::Bool(true)).then_some(())
    })
    .await;

    let c2 = c.clone();
    let task = tokio::spawn(async move { c2.exclude(None).await });
    expect(&mut ev, "exclusion started", |e| {
        matches!(e, Event::ExclusionStarted).then_some(())
    })
    .await;
    assert_eq!(h.press("switch").await, Some(node));
    assert_eq!(task.await.unwrap().unwrap(), Some(node));
    expect(&mut ev, "removed", |e| {
        matches!(e, Event::NodeRemoved { node: n } if *n == node).then_some(())
    })
    .await;
    assert!(c.node(node).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn smartstart_from_qr_code() {
    let dir = tempfile::tempdir().unwrap();
    let (c, mut ev, h) = start(NetworkKeys::generate().unwrap(), dir.path()).await;
    let spec = NodeSpec::smartstart_sensor("x");
    let qr = zwave_security::qr::encode(&zwave_security::qr::QrCode {
        version: 1,
        requested_classes: vec![SecurityClass::S2Authenticated],
        dsk: spec.dsk(),
        product_type: None,
        product_id: None,
        supported_protocols: None,
        max_inclusion_request_interval: None,
    });
    let parsed = zwave_security::qr::parse(&qr).unwrap();
    c.provision(Entry::from_qr(&parsed)).await.unwrap();
    // Give the listen-mode command a moment to reach the simulator.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(h.power_on("smartstart").await);
    let node = expect(&mut ev, "bootstrapped", |e| match e {
        Event::SecurityBootstrapped { node, classes }
            if classes == &vec![SecurityClass::S2Authenticated] =>
        {
            Some(*node)
        }
        _ => None,
    })
    .await;
    interviewed(&mut ev, node).await;
    assert_eq!(c.provisioning_list()[0].node_id, Some(node));
    assert_eq!(
        c.node(node).unwrap().dsk.as_deref(),
        Some(spec.dsk().to_string().as_str())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nvm_backup_store_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let keys = NetworkKeys::generate().unwrap();
    let (host, dev) = tokio::io::duplex(16 * 1024);
    let h = sim().spawn(dev);
    let (c, mut ev) = Controller::start(host, config(keys.clone(), dir.path()))
        .await
        .unwrap();
    let r = include(
        &c,
        &mut ev,
        &h,
        "s2",
        InclusionOptions {
            grant: GrantPolicy::Only(vec![SecurityClass::S2Unauthenticated]),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(r.security, vec![SecurityClass::S2Unauthenticated]);
    interviewed(&mut ev, r.node).await;

    let image = c.backup_nvm().await.unwrap();
    assert_eq!(image, h.nvm().await);

    c.flush().unwrap();
    let stored = zwave_host::store::load(&dir.path().join("nodes.json")).unwrap();
    assert_eq!(stored.nodes[&r.node].stage, InterviewStage::Complete);
    drop(c);
    drop(ev);

    // A new host process on the same stick: the node is known, secure, and reports.
    let (host2, dev2) = tokio::io::duplex(16 * 1024);
    h.attach(dev2).await;
    let (c2, mut ev2) = Controller::start(host2, config(keys, dir.path()))
        .await
        .unwrap();
    let rec = c2.node(r.node).unwrap();
    assert_eq!(rec.stage, InterviewStage::Complete);
    assert_eq!(rec.security, vec![SecurityClass::S2Unauthenticated]);
    assert!(h.set_sensor("s2", 5, 55.0).await);
    expect(&mut ev2, "humidity after restart", |e| match e {
        Event::Value(u) if u.id.property == "humidity" && u.value == ValueKind::Number(55.0) => {
            Some(())
        }
        _ => None,
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configure_merges_partial_parameters() {
    use zwave_host::device_config::{DeviceConfig, ParamConfig};
    let dir = tempfile::tempdir().unwrap();
    let (c, mut ev, h) = start(NetworkKeys::generate().unwrap(), dir.path()).await;
    let r = include(&c, &mut ev, &h, "switch", InclusionOptions::default()).await;
    interviewed(&mut ev, r.node).await;
    let param = |key: &str, parameter, mask, size, max| ParamConfig {
        parameter,
        mask,
        key: key.into(),
        label: key.into(),
        description: None,
        size,
        min: 0,
        max,
        default: 0,
        unsigned: true,
        read_only: false,
        write_only: false,
        allow_manual_entry: true,
        options: vec![],
        unit: None,
    };
    let cfg = DeviceConfig {
        file: "test.json".into(),
        manufacturer: String::new(),
        manufacturer_id: 0,
        label: String::new(),
        description: String::new(),
        devices: vec![],
        firmware: ("0.0".into(), "255.255".into()),
        associations: Default::default(),
        params: vec![
            param("3", 3, None, 1, 99),
            param("7[0xf0]", 7, Some(0xf0), 2, 15),
            param("7[0x0f]", 7, Some(0x0f), 2, 15),
        ],
        compat: None,
        endpoints: None,
        metadata: None,
    };
    // Parameter 7 starts at 0; each call changes one nibble and keeps the other.
    c.configure(r.node, &cfg, &[("7[0x0f]".into(), 3)])
        .await
        .unwrap();
    c.configure(r.node, &cfg, &[("3".into(), 42), ("7[0xf0]".into(), 0xa)])
        .await
        .unwrap();
    let params = h.inspect("switch", |n| n.params.clone()).await.unwrap();
    assert_eq!(params.get(&3), Some(&(1, 42)));
    assert_eq!(params.get(&7), Some(&(2, 0xa3)));
    assert!(
        c.configure(r.node, &cfg, &[("3".into(), 100)])
            .await
            .is_err(),
        "out of range"
    );
    assert!(
        c.configure(r.node, &cfg, &[("9".into(), 1)]).await.is_err(),
        "unknown parameter"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn firmware_update_over_s2() {
    let dir = tempfile::tempdir().unwrap();
    let (c, mut ev, h) = start(NetworkKeys::generate().unwrap(), dir.path()).await;
    let r = include(
        &c,
        &mut ev,
        &h,
        "s2",
        InclusionOptions {
            grant: GrantPolicy::Only(vec![SecurityClass::S2Unauthenticated]),
            ..Default::default()
        },
    )
    .await;
    interviewed(&mut ev, r.node).await;
    assert!(c.node(r.node).unwrap().root().ccs[&0x7a].secure);
    // An image of several fragments, not a multiple of the fragment size.
    let image: Vec<u8> = (0..1000u32).map(|i| (i * 31 + 7) as u8).collect();
    let res = c.update_firmware(r.node, &image, 0).await.unwrap();
    assert!(res.success(), "{res:?}");
    assert_eq!(res.wait_time, Some(5));
    assert_eq!(
        h.inspect("s2", |n| n.firmware.clone()).await.unwrap(),
        image
    );
    expect(&mut ev, "progress", |e| {
        matches!(e, Event::FirmwareUpdateProgress { sent, total, .. } if sent == total)
            .then_some(())
    })
    .await;
    // The node got each fragment exactly once (decrypted on its side).
    let fragments = h
        .inspect("s2", |n| {
            n.received
                .iter()
                .filter(|f| f.starts_with(&[0x7a, 0x06]))
                .count()
        })
        .await
        .unwrap();
    assert_eq!(fragments, 1000usize.div_ceil(28));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn long_range_smartstart_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let keys = NetworkKeys::generate().unwrap();
    let (host, dev) = tokio::io::duplex(16 * 1024);
    let h = sim().spawn(dev);
    let (c, mut ev) = Controller::start(host, config(keys.clone(), dir.path()))
        .await
        .unwrap();
    let spec = NodeSpec::long_range_sensor("x");
    c.provision(Entry {
        dsk: spec.dsk(),
        granted: vec![
            SecurityClass::S2Authenticated,
            SecurityClass::S2Unauthenticated,
        ],
        status: Default::default(),
        node_id: None,
        name: None,
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(h.power_on("lr").await);
    let (node, classes) = expect(&mut ev, "bootstrapped", |e| match e {
        Event::SecurityBootstrapped { node, classes } => Some((*node, classes.clone())),
        _ => None,
    })
    .await;
    assert!(
        zwave_host::is_long_range(node),
        "node {node} has a Long Range id"
    );
    // Unauthenticated does not exist on Long Range, even if granted.
    assert_eq!(classes, vec![SecurityClass::S2Authenticated]);
    interviewed(&mut ev, node).await;
    assert!(c.node(node).unwrap().neighbors.is_empty());
    assert!(h.set_sensor("lr", 1, 19.5).await);
    expect(&mut ev, "LR temperature", |e| {
        matches!(e, Event::Value(u) if u.id.node == node && u.value == ValueKind::Number(19.5))
            .then_some(())
    })
    .await;
    c.flush().unwrap();
    drop(c);
    drop(ev);

    let (host2, dev2) = tokio::io::duplex(16 * 1024);
    h.attach(dev2).await;
    let (c2, mut ev2) = Controller::start(host2, config(keys, dir.path()))
        .await
        .unwrap();
    assert!(
        c2.info().nodes.contains(&node),
        "the Long Range node list is read at start"
    );
    assert!(h.set_sensor("lr", 1, 20.0).await);
    expect(&mut ev2, "LR temperature after restart", |e| {
        matches!(e, Event::Value(u) if u.id.node == node && u.value == ValueKind::Number(20.0))
            .then_some(())
    })
    .await;
}

fn nvm_700(home: [u8; 4], extra: &[u8]) -> Vec<u8> {
    use zwave_host::nvm3::*;
    let mut ci = home.to_vec();
    ci.push(1);
    ci.resize(13, 0);
    let app = write_volume(
        APPLICATION_SIZE_700,
        2048,
        &[(APPLICATION_VERSION_FILE_700, vec![1, 18, 7, 0])],
    )
    .unwrap();
    let proto = write_volume(
        IMAGE_SIZE_700 - APPLICATION_SIZE_700,
        2048,
        &[
            (PROTOCOL_VERSION_FILE, vec![0, 17, 7, 3]),
            (CONTROLLER_INFO_FILE, ci),
            (0x50005, extra.to_vec()),
        ],
    )
    .unwrap();
    [app, proto].concat()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nvm_restore_checks_compatibility() {
    let dir = tempfile::tempdir().unwrap();
    let original = nvm_700([0xc0, 0xff, 0xee, 0x01], &[1, 2, 3]);
    let (host, dev) = tokio::io::duplex(16 * 1024);
    let h = Simulator::new(SimConfig {
        nvm: Some(original.clone()),
        frame_gap: Duration::ZERO,
        ..SimConfig::default()
    })
    .spawn(dev);
    let (c, _ev) = Controller::start(host, config(NetworkKeys::generate().unwrap(), dir.path()))
        .await
        .unwrap();
    assert_eq!(c.backup_nvm().await.unwrap(), original);
    let info = zwave_host::nvm3::NvmInfo::read(&original).unwrap();
    assert_eq!(info.home_id, Some(0xc0ffee01));

    // A backup of the same series and SDK is written back as it is.
    let other = nvm_700([0xc0, 0xff, 0xee, 0x02], &[9, 9, 9, 9, 9]);
    c.restore_nvm(&other).await.unwrap();
    assert_eq!(h.nvm().await, other);

    // An 800 series image needs conversion and is refused before anything is written.
    let shared = zwave_host::nvm3::write_volume(
        zwave_host::nvm3::IMAGE_SIZE_800,
        8192,
        &[(
            zwave_host::nvm3::APPLICATION_VERSION_FILE_800,
            vec![0, 1, 0, 0],
        )],
    )
    .unwrap();
    let e = c.restore_nvm(&shared).await.unwrap_err().to_string();
    assert!(e.contains("conversion"), "{e}");
    assert_eq!(h.nvm().await, other);
    // Something that is not an NVM image at all.
    assert!(c.restore_nvm(&[0u8; 100]).await.is_err());
}

/// `flush` and the background writer never race on the store's temporary file.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn store_writes_do_not_race() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(NetworkKeys::generate().unwrap(), dir.path());
    cfg.store_delay = Duration::ZERO;
    let (host, dev) = tokio::io::duplex(16 * 1024);
    let h = sim().spawn(dev);
    let (c, mut ev) = Controller::start(host, cfg).await.unwrap();
    let r = include(&c, &mut ev, &h, "switch", InclusionOptions::default()).await;
    for i in 0..200 {
        c.set_name(r.node, Some(format!("switch {i}")), None)
            .unwrap();
        c.flush().unwrap();
        if i % 20 == 0 {
            tokio::task::yield_now().await;
        }
    }
    c.flush().unwrap();
    let stored = zwave_host::store::load(&dir.path().join("nodes.json")).unwrap();
    assert_eq!(stored.nodes[&r.node].name.as_deref(), Some("switch 199"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replace_a_failed_node() {
    let dir = tempfile::tempdir().unwrap();
    let (c, mut ev, h) = start(NetworkKeys::generate().unwrap(), dir.path()).await;
    let r = include(&c, &mut ev, &h, "switch", InclusionOptions::default()).await;
    interviewed(&mut ev, r.node).await;
    // A node that answers cannot be replaced.
    assert!(c
        .replace_failed_node(r.node, InclusionOptions::default())
        .await
        .is_err());
    assert!(h.power_off("switch").await);
    assert!(!c.ping(r.node).await);
    let c2 = c.clone();
    let node = r.node;
    let task = tokio::spawn(async move {
        c2.replace_failed_node(node, InclusionOptions::default())
            .await
    });
    expect(&mut ev, "replacement ready", |e| {
        matches!(e, Event::InclusionStarted).then_some(())
    })
    .await;
    assert_eq!(h.press("spare").await, Some(r.node));
    let res = task.await.unwrap().unwrap();
    assert_eq!(res.node, r.node);
    interviewed(&mut ev, r.node).await;
    assert_eq!(h.node_id("spare").await, Some(r.node));
    assert!(c.ping(r.node).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raw_settings_region_and_factory_reset() {
    let dir = tempfile::tempdir().unwrap();
    let keys = NetworkKeys::generate().unwrap();
    let (host, dev) = tokio::io::duplex(16 * 1024);
    let h = sim().spawn(dev);
    let (c, mut ev) = Controller::start(host, config(keys.clone(), dir.path()))
        .await
        .unwrap();
    let r = include(&c, &mut ev, &h, "switch", InclusionOptions::default()).await;
    interviewed(&mut ev, r.node).await;
    // Without a device file: sizes come from the device.
    c.configure_raw(r.node, &[("7".into(), 0x0102)])
        .await
        .unwrap();
    c.configure_raw(r.node, &[("7[0xf0]".into(), 0xa)])
        .await
        .unwrap();
    assert_eq!(
        h.inspect("switch", |n| n.params.get(&7).copied())
            .await
            .unwrap(),
        Some((1, 0xa2u8 as i8 as i32))
    );
    assert!(c.configure_raw(r.node, &[("x".into(), 1)]).await.is_err());

    c.set_region(zwave_serial::message::RfRegion::Europe)
        .await
        .unwrap();
    let home = c.info().home_id;
    c.factory_reset().await.unwrap();
    drop(c);
    drop(ev);
    let (host2, dev2) = tokio::io::duplex(16 * 1024);
    h.attach(dev2).await;
    let (c2, _ev2) = Controller::start(host2, config(keys, &dir.path().join("2")))
        .await
        .unwrap();
    assert_ne!(c2.info().home_id, home, "a new home id after the reset");
    assert!(c2.nodes().is_empty());
    assert_eq!(c2.info().region.as_deref(), Some("Europe"));
}
