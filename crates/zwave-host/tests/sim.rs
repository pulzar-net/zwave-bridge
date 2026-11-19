//! The host stack against the Serial API simulator: inclusion with S2, S0
//! and SmartStart, interviews, wake-up queues, reports, exclusion, NVM
//! backup and a restart that resumes from the store.

use std::time::Duration;
use tokio::sync::mpsc::Receiver;
use zwave_host::node::{InterviewStage, NodeStatus};
use zwave_host::{Controller, Event, GrantPolicy, HostConfig, InclusionOptions, NetworkKeys, SecurityClass, ValueId, ValueKind};
use zwave_security::provisioning::Entry;
use zwave_sim::node::NodeSpec;
use zwave_sim::{SimConfig, SimHandle, Simulator};

const T: Duration = Duration::from_secs(20);

fn sim() -> Simulator {
    Simulator::new(SimConfig::default())
        .with_node(NodeSpec::switch("switch"))
        .with_node(NodeSpec::sleeping_sensor("sleeper"))
        .with_node(NodeSpec::s2_sensor("s2", vec![SecurityClass::S2Authenticated, SecurityClass::S2Unauthenticated]))
        .with_node(NodeSpec::smartstart_sensor("smartstart"))
}

fn config(keys: NetworkKeys, dir: &std::path::Path) -> HostConfig {
    let mut c = HostConfig::new(keys, Some(dir.join("nodes.json")));
    c.request_timeout = Duration::from_secs(2);
    c.awake_idle = Duration::from_millis(300);
    c.store_delay = Duration::from_millis(50);
    c.inclusion_timeout = Duration::from_secs(10);
    c
}

async fn start(keys: NetworkKeys, dir: &std::path::Path) -> (Controller, Receiver<Event>, SimHandle) {
    let (host, dev) = tokio::io::duplex(16 * 1024);
    let h = sim().spawn(dev);
    let (c, ev) = Controller::start(host, config(keys, dir)).await.expect("start");
    (c, ev, h)
}

/// Reads events until `f` picks one, failing after `T`.
async fn expect<R>(ev: &mut Receiver<Event>, what: &str, mut f: impl FnMut(&Event) -> Option<R>) -> R {
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

async fn include(c: &Controller, ev: &mut Receiver<Event>, h: &SimHandle, name: &str, opts: InclusionOptions) -> zwave_host::InclusionResult {
    let c2 = c.clone();
    let task = tokio::spawn(async move { c2.include(opts).await });
    expect(ev, "inclusion started", |e| matches!(e, Event::InclusionStarted).then_some(())).await;
    assert!(h.press(name).await.is_some(), "{name} joined");
    task.await.unwrap().expect("inclusion")
}

async fn interviewed(ev: &mut Receiver<Event>, node: u16) {
    expect(ev, "interview completed", |e| matches!(e, Event::InterviewCompleted { node: n } if *n == node).then_some(())).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s2_inclusion_with_pin_interview_and_reports() {
    let dir = tempfile::tempdir().unwrap();
    let (c, mut ev, h) = start(NetworkKeys::generate().unwrap(), dir.path()).await;
    expect(&mut ev, "ready", |e| matches!(e, Event::Ready(_)).then_some(())).await;
    assert!(c.info().sis, "the controller made itself SIS");

    let c2 = c.clone();
    let task = tokio::spawn(async move { c2.include(InclusionOptions::default()).await });
    expect(&mut ev, "inclusion started", |e| matches!(e, Event::InclusionStarted).then_some(())).await;
    h.press("s2").await.unwrap();
    let requested = expect(&mut ev, "grant request", |e| match e {
        Event::GrantSecurityClasses { requested, .. } => Some(requested.clone()),
        _ => None,
    })
    .await;
    assert_eq!(requested, vec![SecurityClass::S2Authenticated, SecurityClass::S2Unauthenticated]);
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
    assert_eq!(r.security, vec![SecurityClass::S2Authenticated, SecurityClass::S2Unauthenticated]);
    assert_eq!(h.granted("s2").await.len(), 2);
    let node = r.node;
    interviewed(&mut ev, node).await;

    let rec = c.node(node).unwrap();
    assert_eq!(rec.stage, InterviewStage::Complete);
    assert!(rec.secure_ccs_known);
    assert!(rec.root().ccs[&0x31].secure, "multilevel sensor is secure-only");
    assert_eq!(rec.root().version(0x31), 11);
    assert_eq!(rec.manufacturer.unwrap().product_id, 3);
    assert_eq!(rec.root().caps.sensors.keys().copied().collect::<Vec<_>>(), vec![1, 5]);
    assert_eq!(rec.root().caps.notifications.get(&7), Some(&vec![8]));
    assert!(!rec.neighbors.is_empty());
    assert_eq!(h.inspect("s2", |n| n.lifeline.clone()).await.unwrap(), vec![1]);

    // An S2-encrypted report from the node.
    assert!(h.set_sensor("s2", 1, 23.4).await);
    let v = expect(&mut ev, "temperature", |e| match e {
        Event::Value(u) if u.id.node == node && u.id.property == "air_temperature" && u.value == ValueKind::Number(23.4) => Some(u.clone()),
        _ => None,
    })
    .await;
    assert_eq!(v.unit, Some("°C"));
    assert!(h.motion("s2").await);
    expect(&mut ev, "motion", |e| match e {
        Event::Value(u) if u.id.property == "home_security_motion_sensor_status" => matches!(u.value, ValueKind::State { id: 8, .. }).then_some(()),
        _ => None,
    })
    .await;
    // The insecure path is refused for a secure-only command class.
    let sent_plain = h.inspect("s2", |n| n.received.iter().any(|f| f.first() == Some(&0x31) && f.get(1) == Some(&0x04))).await.unwrap();
    assert!(sent_plain, "the node got its sensor Gets (decrypted)");

    let topo = c.topology();
    assert!(topo.nodes.iter().any(|n| n.id == node && n.last_rssi.is_some()));
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
    expect(&mut ev, "asleep", |e| matches!(e, Event::NodeStatus { node: n, status: NodeStatus::Asleep } if *n == node).then_some(())).await;
    assert!(!h.inspect("sleeper", |n| n.awake).await.unwrap());

    // A request to the sleeping node waits for its wake-up.
    let c2 = c.clone();
    let pending = tokio::spawn(async move {
        c2.request(node, 0, zwave_cc::battery::Get {}.into(), |c| matches!(c, zwave_cc::Command::Battery(_))).await
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!pending.is_finished(), "queued until the node wakes");
    assert!(h.wake_up("sleeper").await);
    let reply = tokio::time::timeout(T, pending).await.unwrap().unwrap().unwrap();
    assert!(matches!(reply, zwave_cc::Command::Battery(zwave_cc::battery::Command::Report(r)) if r.level == 87));
    expect(&mut ev, "asleep again", |e| matches!(e, Event::NodeStatus { node: n, status: NodeStatus::Asleep } if *n == node).then_some(())).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn switch_set_supervision_and_exclusion() {
    let dir = tempfile::tempdir().unwrap();
    let (c, mut ev, h) = start(NetworkKeys::generate().unwrap(), dir.path()).await;
    let r = include(&c, &mut ev, &h, "switch", InclusionOptions { grant: GrantPolicy::Requested, ..Default::default() }).await;
    assert!(r.security.is_empty());
    let node = r.node;
    interviewed(&mut ev, node).await;
    let rec = c.node(node).unwrap();
    assert_eq!(rec.root().caps.meter, Some((1, vec![0, 2])));

    let id = ValueId { node, endpoint: 0, cc: 0x25, property: "current".into() };
    c.set_value(&id, &ValueKind::Bool(true)).await.unwrap();
    assert!(h.inspect("switch", |n| n.switch_on).await.unwrap());
    expect(&mut ev, "switch on", |e| matches!(e, Event::Value(u) if u.id == id && u.value == ValueKind::Bool(true)).then_some(())).await;

    let c2 = c.clone();
    let task = tokio::spawn(async move { c2.exclude(None).await });
    expect(&mut ev, "exclusion started", |e| matches!(e, Event::ExclusionStarted).then_some(())).await;
    assert_eq!(h.press("switch").await, Some(node));
    assert_eq!(task.await.unwrap().unwrap(), Some(node));
    expect(&mut ev, "removed", |e| matches!(e, Event::NodeRemoved { node: n } if *n == node).then_some(())).await;
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
        Event::SecurityBootstrapped { node, classes } if classes == &vec![SecurityClass::S2Authenticated] => Some(*node),
        _ => None,
    })
    .await;
    interviewed(&mut ev, node).await;
    assert_eq!(c.provisioning_list()[0].node_id, Some(node));
    assert_eq!(c.node(node).unwrap().dsk.as_deref(), Some(spec.dsk().to_string().as_str()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nvm_backup_store_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let keys = NetworkKeys::generate().unwrap();
    let (host, dev) = tokio::io::duplex(16 * 1024);
    let h = sim().spawn(dev);
    let (c, mut ev) = Controller::start(host, config(keys.clone(), dir.path())).await.unwrap();
    let r = include(&c, &mut ev, &h, "s2", InclusionOptions { grant: GrantPolicy::Only(vec![SecurityClass::S2Unauthenticated]), ..Default::default() }).await;
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
    let (c2, mut ev2) = Controller::start(host2, config(keys, dir.path())).await.unwrap();
    let rec = c2.node(r.node).unwrap();
    assert_eq!(rec.stage, InterviewStage::Complete);
    assert_eq!(rec.security, vec![SecurityClass::S2Unauthenticated]);
    assert!(h.set_sensor("s2", 5, 55.0).await);
    expect(&mut ev2, "humidity after restart", |e| match e {
        Event::Value(u) if u.id.property == "humidity" && u.value == ValueKind::Number(55.0) => Some(()),
        _ => None,
    })
    .await;
}
