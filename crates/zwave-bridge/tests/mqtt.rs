//! End to end: the bridge on the controller simulator and a real Mosquitto
//! broker. An S2 node is included over MQTT (keys granted and PIN typed
//! through request topics), interviewed, and its sensor report arrives on
//! its value topic with a Home Assistant discovery config.
//!
//! Needs `mosquitto` on PATH (or `MOSQUITTO=/path/to/mosquitto`). Without it
//! the test is skipped, unless `ZWAVE_BRIDGE_REQUIRE_MQTT=1` (CI sets it).

use rumqttc::{AsyncClient, Event as MqttEvent, MqttOptions, Packet, QoS};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;
use zwave_bridge::config::Config;
use zwave_security::SecurityClass;
use zwave_sim::node::NodeSpec;
use zwave_sim::{SimConfig, Simulator};

fn mosquitto() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("MOSQUITTO") {
        return Some(p.into());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .chain(
            ["/usr/sbin", "/opt/homebrew/sbin", "/usr/local/sbin"]
                .iter()
                .map(Into::into),
        )
        .map(|d| d.join("mosquitto"))
        .find(|p| p.is_file())
}

struct Broker {
    child: tokio::process::Child,
    port: u16,
}

impl Drop for Broker {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

async fn broker(dir: &std::path::Path) -> Option<Broker> {
    let bin = mosquitto()?;
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let conf = dir.join("mosquitto.conf");
    std::fs::write(
        &conf,
        format!("listener {port} 127.0.0.1\nallow_anonymous true\npersistence false\n"),
    )
    .unwrap();
    let child = tokio::process::Command::new(bin)
        .arg("-c")
        .arg(&conf)
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return Some(Broker { child, port });
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    None
}

async fn next(
    rx: &mut mpsc::Receiver<(String, Vec<u8>)>,
    what: &str,
    mut f: impl FnMut(&str, &[u8]) -> bool,
) -> (String, Vec<u8>) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some((t, p))) => {
                if f(&t, &p) {
                    return (t, p);
                }
            }
            _ => panic!("timed out waiting for {what}"),
        }
    }
}

fn js(p: &[u8]) -> Value {
    serde_json::from_slice(p).unwrap_or(Value::Null)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn include_s2_node_over_mqtt_and_get_a_sensor_report() {
    let dir = tempfile::tempdir().unwrap();
    let Some(b) = broker(dir.path()).await else {
        if std::env::var("ZWAVE_BRIDGE_REQUIRE_MQTT").as_deref() == Ok("1") {
            panic!("mosquitto is required but was not found");
        }
        eprintln!("skipped: no mosquitto");
        return;
    };

    // The test's own MQTT client.
    let mut opts = MqttOptions::new("test", "127.0.0.1", b.port);
    opts.set_keep_alive(Duration::from_secs(10));
    opts.set_max_packet_size(256 * 1024, 256 * 1024);
    let (client, mut el) = AsyncClient::new(opts, 64);
    let (tx, mut rx) = mpsc::channel(1024);
    tokio::spawn(async move {
        loop {
            match el.poll().await {
                Ok(MqttEvent::Incoming(Packet::Publish(p))) => {
                    let _ = tx.send((p.topic, p.payload.to_vec())).await;
                }
                Ok(_) => {}
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
    });
    client.subscribe("zwave/#", QoS::AtLeastOnce).await.unwrap();
    client
        .subscribe("homeassistant/#", QoS::AtLeastOnce)
        .await
        .unwrap();

    // The simulator and the bridge.
    let spec = NodeSpec::s2_sensor(
        "s2",
        vec![
            SecurityClass::S2Authenticated,
            SecurityClass::S2Unauthenticated,
        ],
    );
    let pin = spec.dsk().pin();
    let (host, dev) = tokio::io::duplex(16 * 1024);
    let sim = Simulator::new(SimConfig::default())
        .with_node(spec)
        .spawn(dev);
    let cfg = Config::parse(&format!(
        "[serial]\nport = \"sim\"\n[store]\ndir = {:?}\n[mqtt]\nport = {}\nclient_id = \"bridge\"\n",
        dir.path().join("state"),
        b.port
    ))
    .unwrap();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let bridge = tokio::spawn(zwave_bridge::run(cfg, host, async {
        let _ = stop_rx.await;
    }));

    next(&mut rx, "online", |t, p| {
        t == "zwave/bridge/state" && p == b"online"
    })
    .await;
    let (_, info) = next(&mut rx, "info", |t, _| t == "zwave/bridge/info").await;
    assert_eq!(js(&info)["home_id"], "c0ffee01");

    let publish = |topic: &str, body: Value| {
        let c = client.clone();
        let topic = topic.to_string();
        async move {
            c.publish(topic, QoS::AtLeastOnce, false, body.to_string())
                .await
                .unwrap()
        }
    };
    publish("zwave/bridge/request/include", json!({})).await;
    next(&mut rx, "inclusion started", |t, p| {
        t == "zwave/bridge/event" && js(p)["event"] == "inclusion_started"
    })
    .await;
    let node = sim.press("s2").await.expect("joined");
    let (_, grant) = next(&mut rx, "grant prompt", |t, p| {
        t == "zwave/bridge/event" && js(p)["event"] == "grant_security_classes"
    })
    .await;
    let requested = js(&grant)["requested"].clone();
    assert_eq!(requested, json!(["s2_authenticated", "s2_unauthenticated"]));
    publish(
        "zwave/bridge/request/grant",
        json!({ "classes": requested }),
    )
    .await;
    let (_, dsk) = next(&mut rx, "DSK prompt", |t, p| {
        t == "zwave/bridge/event" && js(p)["event"] == "validate_dsk"
    })
    .await;
    assert!(js(&dsk)["dsk"].as_str().unwrap().starts_with("xxxxx-"));
    publish(
        "zwave/bridge/request/validate_dsk",
        json!({ "pin": format!("{pin:05}") }),
    )
    .await;
    let (_, res) = next(&mut rx, "include response", |t, _| {
        t == "zwave/bridge/response/include"
    })
    .await;
    let res = js(&res);
    assert_eq!(res["ok"], true, "{res}");
    assert_eq!(res["node"], node);
    assert_eq!(
        res["security"],
        json!(["s2_authenticated", "s2_unauthenticated"])
    );
    next(&mut rx, "interview", |t, p| {
        t == "zwave/bridge/event"
            && js(p)["event"] == "interview_completed"
            && js(p)["node"] == node
    })
    .await;
    let (_, rec) = next(&mut rx, "node record", |t, p| {
        t == format!("zwave/nodes/{node}") && js(p)["stage"] == "complete"
    })
    .await;
    assert_eq!(
        js(&rec)["security"],
        json!(["s2_authenticated", "s2_unauthenticated"])
    );

    // The node reports a new temperature, S2-encrypted, to its lifeline.
    assert!(sim.set_sensor("s2", 1, 24.6).await);
    let topic = format!("zwave/nodes/{node}/multilevel_sensor/air_temperature");
    let (_, v) = next(&mut rx, "temperature value", |t, p| {
        t == topic && js(p)["value"] == json!(24.6)
    })
    .await;
    assert_eq!(js(&v)["unit"], "°C");
    let disc = format!(
        "homeassistant/sensor/zwave_c0ffee01_{node}/multilevel_sensor_air_temperature/config"
    );
    // Discovery is published before the first value, so it is already in the stream or retained.
    client
        .subscribe(disc.clone(), QoS::AtLeastOnce)
        .await
        .unwrap();
    let (_, d) = next(&mut rx, "discovery config", |t, _| t == disc).await;
    let d = js(&d);
    assert_eq!(d["device_class"], "temperature");
    assert_eq!(d["state_topic"], topic);
    assert_eq!(d["availability_topic"], "zwave/bridge/state");

    let _ = stop_tx.send(());
    bridge.await.unwrap().unwrap();
    assert!(dir.path().join("state/keys.json").is_file());
    assert!(dir.path().join("state/nodes.json").is_file());
}
