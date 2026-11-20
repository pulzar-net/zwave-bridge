#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod config;
pub mod discovery;

use anyhow::{Context, Result};
use config::Config;
use rumqttc::{AsyncClient, Event as MqttEvent, LastWill, MqttOptions, Packet, QoS};
use serde_json::{json, Value as Json};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use zwave_host::{
    Controller, Event, GrantPolicy, HostConfig, InclusionOptions, NetworkKeys, NodeStatus,
    SecurityClass, ValueId, ValueKind,
};
use zwave_security::provisioning::Entry;
use zwave_serial::port::Port;

/// Loads `keys.json` from `dir`, creating it with fresh keys (mode 0600) on first start.
pub fn load_or_create_keys(dir: &Path) -> Result<NetworkKeys> {
    let path = dir.join("keys.json");
    match std::fs::read(&path) {
        Ok(b) => {
            let mut keys: NetworkKeys = serde_json::from_slice(&b)
                .with_context(|| format!("reading {}", path.display()))?;
            // Keys made before Long Range support get Long Range keys of their own, once.
            if keys.fill_long_range().map_err(|e| anyhow::anyhow!("{e}"))? {
                let tmp = path.with_extension("json.tmp");
                write_private(&tmp, &serde_json::to_vec_pretty(&keys)?)?;
                std::fs::rename(&tmp, &path)?;
                info!("added Long Range keys to {}", path.display());
            }
            Ok(keys)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            let keys = NetworkKeys::generate().map_err(|e| anyhow::anyhow!("{e}"))?;
            let json = serde_json::to_vec_pretty(&keys)?;
            let tmp = path.with_extension("json.tmp");
            write_private(&tmp, &json)?;
            std::fs::rename(&tmp, &path)?;
            info!("generated network keys in {}", path.display());
            Ok(keys)
        }
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

#[cfg(unix)]
fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(data)?;
    f.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    std::fs::write(path, data)?;
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn status_name(s: NodeStatus) -> &'static str {
    match s {
        NodeStatus::Unknown => "unknown",
        NodeStatus::Alive => "alive",
        NodeStatus::Awake => "awake",
        NodeStatus::Asleep => "asleep",
        NodeStatus::Dead => "dead",
    }
}

fn class_names(c: &[SecurityClass]) -> Vec<&'static str> {
    c.iter().map(|c| c.name()).collect()
}

fn parse_classes(v: &Json) -> Vec<SecurityClass> {
    let names: Vec<String> = serde_json::from_value(v.clone()).unwrap_or_default();
    SecurityClass::HIGHEST_FIRST
        .into_iter()
        .filter(|c| names.iter().any(|n| n == c.name()))
        .collect()
}

enum FromMqtt {
    Connected,
    Message(String, Vec<u8>),
}

struct Bridge {
    cfg: Config,
    restart: std::sync::Arc<tokio::sync::Notify>,
    library: Option<std::sync::Arc<zwave_host::device_config::Library>>,
    devices: HashMap<u16, std::sync::Arc<zwave_host::device_config::DeviceConfig>>,
    c: Controller,
    mqtt: AsyncClient,
    base: String,
    discovered: HashMap<u16, HashSet<String>>,
    responses: mpsc::Sender<(String, Json)>,
    /// The `transaction` of the request being handled, echoed in its response.
    transaction: Option<Json>,
}

/// Runs the bridge on `port` until `shutdown` completes.
pub async fn run<P: Port>(
    cfg: Config,
    port: P,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    let keys = load_or_create_keys(&cfg.store.dir)?;
    let mut hc = HostConfig::new(keys, Some(cfg.store.dir.join("nodes.json")));
    hc.wake_up_interval = cfg.network.wake_up_interval;
    hc.inclusion_timeout = Duration::from_secs(cfg.network.inclusion_timeout);
    let (c, mut events) = Controller::start(port, hc)
        .await
        .map_err(|e| anyhow::anyhow!("controller: {e}"))?;
    let info = c.info();
    info!(home = format!("{:08x}", info.home_id), node = info.node_id, version = %info.library_version, "controller ready");

    let base = cfg.mqtt.base_topic.trim_end_matches('/').to_string();
    let mut opts = MqttOptions::new(&cfg.mqtt.client_id, &cfg.mqtt.host, cfg.mqtt.port);
    opts.set_keep_alive(Duration::from_secs(cfg.mqtt.keep_alive.max(5)));
    opts.set_last_will(LastWill::new(
        format!("{base}/bridge/state"),
        "offline",
        QoS::AtLeastOnce,
        true,
    ));
    opts.set_max_packet_size(256 * 1024, 256 * 1024);
    let password = match (&cfg.mqtt.password, &cfg.mqtt.password_file) {
        (Some(p), _) => Some(p.clone()),
        (None, Some(f)) => Some(
            std::fs::read_to_string(f)
                .with_context(|| format!("reading {}", f.display()))?
                .trim_end()
                .to_string(),
        ),
        _ => None,
    };
    if let Some(u) = &cfg.mqtt.username {
        opts.set_credentials(u, password.unwrap_or_default());
    }
    let (mqtt, mut eventloop) = AsyncClient::new(opts, 256);
    let (mtx, mut mrx) = mpsc::channel::<FromMqtt>(256);
    tokio::spawn(async move {
        loop {
            match eventloop.poll().await {
                Ok(MqttEvent::Incoming(Packet::ConnAck(_))) => {
                    if mtx.send(FromMqtt::Connected).await.is_err() {
                        return;
                    }
                }
                Ok(MqttEvent::Incoming(Packet::Publish(p))) => {
                    if mtx
                        .send(FromMqtt::Message(p.topic, p.payload.to_vec()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    warn!("MQTT: {e}; reconnecting");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
    });
    let (rtx, mut rrx) = mpsc::channel::<(String, Json)>(64);
    let library = cfg.devices.dir.as_ref().map(|d| {
        info!("device library: {}", d.display());
        std::sync::Arc::new(zwave_host::device_config::Library::new(
            zwave_host::device_config::DirSource::new(d),
        ))
    });
    let restart = std::sync::Arc::new(tokio::sync::Notify::new());
    let mut b = Bridge {
        restart: restart.clone(),
        cfg,
        library,
        devices: HashMap::new(),
        c,
        mqtt,
        base,
        discovered: HashMap::new(),
        responses: rtx,
        transaction: None,
    };
    for n in b.c.nodes() {
        b.load_device(n.id);
    }
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            m = mrx.recv() => match m {
                Some(FromMqtt::Connected) => b.on_connected().await,
                Some(FromMqtt::Message(t, p)) => b.on_message(&t, &p).await,
                None => break,
            },
            e = events.recv() => match e {
                Some(e) => b.on_event(e).await,
                None => break,
            },
            r = rrx.recv() => if let Some((action, body)) = r {
                b.publish_raw(&format!("{}/bridge/response/{action}", b.base), body.to_string().into_bytes(), false).await;
            },
            _ = &mut shutdown => break,
            _ = restart.notified() => {
                // The controller restarted with a restored memory: start over with a fresh host.
                tokio::time::sleep(Duration::from_secs(1)).await;
                b.publish_raw(&format!("{}/bridge/state", b.base), b"offline".to_vec(), true).await;
                let _ = b.mqtt.disconnect().await;
                return Err(anyhow::anyhow!("controller memory restored; restart zwave-bridge"));
            }
        }
    }
    b.publish_raw(
        &format!("{}/bridge/state", b.base),
        b"offline".to_vec(),
        true,
    )
    .await;
    let _ = b.mqtt.disconnect().await;
    b.c.flush().map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

impl Bridge {
    async fn publish_raw(&self, topic: &str, payload: Vec<u8>, retain: bool) {
        if let Err(e) = self
            .mqtt
            .publish(topic, QoS::AtLeastOnce, retain, payload)
            .await
        {
            warn!("publish {topic}: {e}");
        }
    }

    async fn publish(&self, topic: &str, body: &Json, retain: bool) {
        if let (Some(t), true) = (&self.transaction, topic.contains("/bridge/response/")) {
            let mut body = body.clone();
            body["transaction"] = t.clone();
            return self
                .publish_raw(topic, body.to_string().into_bytes(), retain)
                .await;
        }
        self.publish_raw(topic, body.to_string().into_bytes(), retain)
            .await;
    }

    async fn on_connected(&mut self) {
        info!("MQTT connected");
        for t in [
            format!("{}/bridge/request/+", self.base),
            format!("{}/nodes/#", self.base),
        ] {
            if let Err(e) = self.mqtt.subscribe(t, QoS::AtLeastOnce).await {
                warn!("subscribe: {e}");
            }
        }
        self.publish_raw(
            &format!("{}/bridge/state", self.base),
            b"online".to_vec(),
            true,
        )
        .await;
        self.publish_info().await;
        for n in self.c.nodes() {
            self.publish_node(n.id).await;
        }
        self.discovered.clear();
    }

    async fn publish_info(&self) {
        let i = self.c.info();
        let body = json!({
            "home_id": format!("{:08x}", i.home_id),
            "node_id": i.node_id,
            "library_version": i.library_version,
            "library_type": i.library_type,
            "firmware": i.firmware,
            "manufacturer_id": i.manufacturer_id,
            "product_type": i.product_type,
            "product_id": i.product_id,
            "chip_type": i.chip_type,
            "sis": i.sis,
            "region": i.region,
            "dsk": self.c.dsk().to_string(),
            "nodes": i.nodes,
            "version": env!("CARGO_PKG_VERSION"),
        });
        self.publish(&format!("{}/bridge/info", self.base), &body, true)
            .await;
    }

    async fn publish_node(&self, node: u16) {
        let Some(rec) = self.c.node(node) else { return };
        let mut body = serde_json::to_value(&rec).unwrap_or(Json::Null);
        let status = status_name(self.c.status(node));
        body["status"] = json!(status);
        if let Some(d) = self.devices.get(&node) {
            body["device"] = json!({"file": d.file, "manufacturer": d.manufacturer, "label": d.label, "description": d.description});
        }
        self.publish(&format!("{}/nodes/{node}", self.base), &body, true)
            .await;
        self.publish_raw(
            &format!("{}/nodes/{node}/status", self.base),
            status.as_bytes().to_vec(),
            true,
        )
        .await;
    }

    /// Finds the node's device file in the library, if there is one.
    fn load_device(&mut self, node: u16) {
        let (Some(lib), Some(rec)) = (&self.library, self.c.node(node)) else {
            return;
        };
        let Some(id) = zwave_host::device_config::device_id(&rec) else {
            return;
        };
        match lib.lookup(&id) {
            Ok(Some(d)) => {
                info!(node, file = %d.file, "device file: {} {}", d.manufacturer, d.label);
                self.devices.insert(node, std::sync::Arc::new(d));
            }
            Ok(None) => debug!(node, "no device file"),
            Err(e) => warn!(node, "device file: {e}"),
        }
    }

    async fn publish_parameters(&self, node: u16) {
        let Some(d) = self.devices.get(&node) else {
            return;
        };
        let params: Vec<Json> = d
            .params
            .iter()
            .map(|p| {
                json!({"key": p.key, "label": p.label, "description": p.description, "size": p.size, "min": p.min, "max": p.max,
                       "default": p.default, "unsigned": p.unsigned, "read_only": p.read_only, "unit": p.unit,
                       "options": p.options.iter().map(|(v, l)| json!({"value": v, "label": l})).collect::<Vec<_>>()})
            })
            .collect();
        self.publish(
            &format!("{}/nodes/{node}/parameters", self.base),
            &json!(params),
            true,
        )
        .await;
    }

    async fn event(&self, body: Json) {
        self.publish(&format!("{}/bridge/event", self.base), &body, false)
            .await;
    }

    async fn on_event(&mut self, e: Event) {
        match e {
            Event::Ready(_) => self.publish_info().await,
            Event::Value(v) => {
                let key = v.id.key();
                let topic = format!("{}/nodes/{}/{key}", self.base, v.id.node);
                let mut body = json!({ "time": now_ms(), "label": v.label });
                match &v.value {
                    ValueKind::Bool(b) => body["value"] = json!(b),
                    ValueKind::Number(n) => body["value"] = json!(n),
                    ValueKind::Level(l) => body["value"] = json!(l),
                    ValueKind::State { id, label } => {
                        body["value"] = json!(id);
                        body["state"] = json!(label);
                    }
                }
                if let Some(u) = v.unit {
                    body["unit"] = json!(u);
                }
                if self.cfg.homeassistant.enabled && !self.discovered.get(&v.id.node).is_some_and(|s| s.contains(&key)) {
                    if let Some(rec) = self.c.node(v.id.node) {
                        if let Some(d) = discovery::for_value(&self.cfg.homeassistant.discovery_prefix, &self.base, self.c.info().home_id, &rec, self.devices.get(&v.id.node).map(|d| d.as_ref()), &v) {
                            self.publish(&d.topic, &d.config, true).await;
                        }
                        self.discovered.entry(v.id.node).or_default().insert(key);
                    }
                }
                self.publish(&topic, &body, true).await;
            }
            Event::NodeAdded { node } => {
                self.event(json!({"event": "node_added", "node": node})).await;
                self.publish_node(node).await;
            }
            Event::NodeRemoved { node } => {
                self.event(json!({"event": "node_removed", "node": node})).await;
                self.publish_raw(&format!("{}/nodes/{node}", self.base), vec![], true).await;
                self.publish_raw(&format!("{}/nodes/{node}/status", self.base), vec![], true).await;
                self.discovered.remove(&node);
            }
            Event::NodeUpdated { node } | Event::NodeStatus { node, .. } => self.publish_node(node).await,
            Event::InterviewCompleted { node } => {
                self.load_device(node);
                self.publish_parameters(node).await;
                self.event(json!({"event": "interview_completed", "node": node})).await;
                self.publish_node(node).await;
            }
            Event::InterviewFailed { node, reason } => self.event(json!({"event": "interview_failed", "node": node, "reason": reason})).await,
            Event::InterviewStage { node, stage } => self.event(json!({"event": "interview_stage", "node": node, "stage": stage})).await,
            Event::InclusionStarted => self.event(json!({"event": "inclusion_started"})).await,
            Event::InclusionStopped => self.event(json!({"event": "inclusion_stopped"})).await,
            Event::ExclusionStarted => self.event(json!({"event": "exclusion_started"})).await,
            Event::ExclusionStopped => self.event(json!({"event": "exclusion_stopped"})).await,
            Event::GrantSecurityClasses { node, requested } => {
                self.event(json!({"event": "grant_security_classes", "node": node, "requested": class_names(&requested)})).await
            }
            Event::ValidateDsk { node, dsk } => self.event(json!({"event": "validate_dsk", "node": node, "dsk": dsk})).await,
            Event::SecurityBootstrapped { node, classes } => self.event(json!({"event": "security_bootstrapped", "node": node, "classes": class_names(&classes)})).await,
            Event::SecurityFailed { node, reason } => self.event(json!({"event": "security_failed", "node": node, "reason": reason})).await,
            Event::Route { node, repeaters, rssi } => {
                self.publish(&format!("{}/nodes/{node}/route", self.base), &json!({"repeaters": repeaters, "rssi": rssi, "time": now_ms()}), true).await
            }
            Event::Command { node, endpoint, command } => {
                let body = json!({"endpoint": endpoint, "cc": command.cc(), "command": command.name(), "frame": hex(&command.encode())});
                self.publish(&format!("{}/nodes/{node}/command", self.base), &body, false).await;
            }
            Event::FirmwareUpdateProgress { node, sent, total } => {
                self.event(json!({"event": "firmware_update_progress", "node": node, "sent": sent, "total": total})).await
            }
            Event::FirmwareUpdateFinished { node, status, success } => {
                self.event(json!({"event": "firmware_update_finished", "node": node, "status": status, "success": success})).await
            }
            Event::ProvisioningChanged { entries } => self.event(json!({"event": "provisioning_changed", "entries": provisioning_json(&entries)})).await,
        }
    }

    async fn on_message(&mut self, topic: &str, payload: &[u8]) {
        let Some(rest) = topic.strip_prefix(&format!("{}/", self.base)) else {
            return;
        };
        if let Some(action) = rest.strip_prefix("bridge/request/") {
            let args: Json = if payload.is_empty() {
                json!({})
            } else {
                serde_json::from_slice(payload).unwrap_or(json!({}))
            };
            self.request(action.to_string(), args).await;
        } else if let Some(path) = rest
            .strip_prefix("nodes/")
            .and_then(|p| p.strip_suffix("/set"))
        {
            self.set(path, payload);
        }
    }

    fn respond_later(
        &self,
        action: String,
        fut: impl std::future::Future<Output = Json> + Send + 'static,
    ) {
        let tx = self.responses.clone();
        let transaction = self.transaction.clone();
        tokio::spawn(async move {
            let mut body = fut.await;
            if let Some(t) = transaction {
                body["transaction"] = t;
            }
            let _ = tx.send((action, body)).await;
        });
    }

    async fn request(&mut self, action: String, args: Json) {
        self.transaction = args.get("transaction").cloned();
        self.handle_request(action, args).await;
        self.transaction = None;
    }

    async fn handle_request(&mut self, action: String, args: Json) {
        debug!(%action, %args, "request");
        let c = self.c.clone();
        let node = args.get("node").and_then(Json::as_u64).map(|n| n as u16);
        let reply = |r: Result<Json, String>| match r {
            Ok(mut j) => {
                j["ok"] = json!(true);
                j
            }
            Err(e) => json!({"ok": false, "error": e}),
        };
        match action.as_str() {
            "include" => {
                let grant = match args.get("strategy").and_then(Json::as_str) {
                    Some("requested") => GrantPolicy::Requested,
                    Some("insecure") => GrantPolicy::Only(vec![]),
                    _ => match args.get("classes") {
                        Some(v) => GrantPolicy::Only(parse_classes(v)),
                        None => GrantPolicy::Ask,
                    },
                };
                let opts = InclusionOptions {
                    insecure: args.get("strategy").and_then(Json::as_str) == Some("insecure"),
                    pin: args
                        .get("pin")
                        .and_then(Json::as_str)
                        .and_then(|p| zwave_security::dsk::parse_pin(p).ok()),
                    grant,
                    timeout: args
                        .get("timeout")
                        .and_then(Json::as_u64)
                        .map(Duration::from_secs),
                };
                self.respond_later(action, async move {
                    reply(c.include(opts).await.map(|r| json!({"node": r.node, "security": class_names(&r.security), "security_error": r.security_error})).map_err(|e| e.to_string()))
                });
            }
            "exclude" => {
                let t = args
                    .get("timeout")
                    .and_then(Json::as_u64)
                    .map(Duration::from_secs);
                self.respond_later(action, async move {
                    reply(
                        c.exclude(t)
                            .await
                            .map(|n| json!({"node": n}))
                            .map_err(|e| e.to_string()),
                    )
                });
            }
            "stop" => {
                let body = json!({"ok": c.stop_inclusion()});
                self.publish(&format!("{}/bridge/response/stop", self.base), &body, false)
                    .await;
            }
            "grant" => {
                let ok = c.grant_security_classes(parse_classes(
                    args.get("classes").unwrap_or(&json!([])),
                ));
                self.publish(
                    &format!("{}/bridge/response/grant", self.base),
                    &json!({"ok": ok}),
                    false,
                )
                .await;
            }
            "validate_dsk" => {
                let pin = args
                    .get("pin")
                    .and_then(Json::as_str)
                    .and_then(|p| zwave_security::dsk::parse_pin(p).ok());
                let ok = c.validate_dsk(pin);
                self.publish(
                    &format!("{}/bridge/response/validate_dsk", self.base),
                    &json!({"ok": ok}),
                    false,
                )
                .await;
            }
            "provision" => {
                let entry = if let Some(qr) = args.get("qr").and_then(Json::as_str) {
                    zwave_security::qr::parse(qr)
                        .map(|q| Entry::from_qr(&q))
                        .map_err(|e| e.to_string())
                } else if let Some(dsk) = args.get("dsk").and_then(Json::as_str) {
                    dsk.parse()
                        .map_err(|e: zwave_security::Error| e.to_string())
                        .map(|dsk| Entry {
                            dsk,
                            granted: parse_classes(
                                args.get("classes")
                                    .unwrap_or(&json!(["s2_authenticated", "s2_unauthenticated"])),
                            ),
                            status: Default::default(),
                            node_id: None,
                            name: args.get("name").and_then(Json::as_str).map(String::from),
                        })
                } else {
                    Err("give \"qr\" or \"dsk\"".to_string())
                };
                self.respond_later(action, async move {
                    match entry {
                        Ok(e) => {
                            let dsk = e.dsk.to_string();
                            reply(
                                c.provision(e)
                                    .await
                                    .map(|_| json!({"dsk": dsk}))
                                    .map_err(|e| e.to_string()),
                            )
                        }
                        Err(e) => reply(Err(e)),
                    }
                });
            }
            "unprovision" => {
                let dsk = args
                    .get("dsk")
                    .and_then(Json::as_str)
                    .and_then(|d| d.parse().ok());
                self.respond_later(action, async move {
                    match dsk {
                        Some(d) => reply(Ok(json!({"removed": c.unprovision(&d).await.is_some()}))),
                        None => reply(Err("give \"dsk\"".into())),
                    }
                });
            }
            "provisioning_list" => {
                let body =
                    json!({"ok": true, "entries": provisioning_json(&c.provisioning_list())});
                self.publish(
                    &format!("{}/bridge/response/provisioning_list", self.base),
                    &body,
                    false,
                )
                .await;
            }
            "backup" => {
                let dir = self.cfg.store.dir.clone();
                self.respond_later(action, async move {
                    reply(match c.backup_nvm().await {
                        Ok(image) => {
                            let path = dir.join(format!("nvm-{}.bin", now_ms() / 1000));
                            std::fs::write(&path, &image).map(|_| json!({"file": path.display().to_string(), "bytes": image.len()})).map_err(|e| e.to_string())
                        }
                        Err(e) => Err(e.to_string()),
                    })
                });
            }
            "reinterview" => {
                let r = node
                    .ok_or("give \"node\"".to_string())
                    .and_then(|n| c.reinterview(n).map_err(|e| e.to_string()));
                self.publish(
                    &format!("{}/bridge/response/reinterview", self.base),
                    &reply(r.map(|_| json!({}))),
                    false,
                )
                .await;
            }
            "remove_failed" => {
                self.respond_later(action, async move {
                    match node {
                        Some(n) => reply(
                            c.remove_failed_node(n)
                                .await
                                .map(|_| json!({"node": n}))
                                .map_err(|e| e.to_string()),
                        ),
                        None => reply(Err("give \"node\"".into())),
                    }
                });
            }
            "rename" => {
                let name = args.get("name").and_then(Json::as_str).map(String::from);
                let location = args
                    .get("location")
                    .and_then(Json::as_str)
                    .map(String::from);
                let r = node
                    .ok_or("give \"node\"".to_string())
                    .and_then(|n| c.set_name(n, name, location).map_err(|e| e.to_string()));
                self.publish(
                    &format!("{}/bridge/response/rename", self.base),
                    &reply(r.map(|_| json!({}))),
                    false,
                )
                .await;
            }
            "restore" => {
                // A backup the bridge wrote (or the operator put) in the store directory.
                let file = args.get("file").and_then(Json::as_str).map(String::from);
                let dir = self.cfg.store.dir.clone();
                let restarted = self.restart.clone();
                self.respond_later(action, async move {
                    let Some(file) = file
                        .filter(|f| !f.contains('/') && !f.contains('\\') && !f.starts_with('.'))
                    else {
                        return reply(Err(
                            "\"file\" is a backup's name in the store directory".into()
                        ));
                    };
                    let image = match std::fs::read(dir.join(&file)) {
                        Ok(b) => b,
                        Err(e) => return reply(Err(format!("{file}: {e}"))),
                    };
                    match c.restore_nvm(&image).await {
                        Ok(()) => {
                            restarted.notify_one();
                            reply(Ok(json!({"file": file, "restart": true})))
                        }
                        Err(e) => reply(Err(e.to_string())),
                    }
                });
            }
            "firmware_update" => {
                // Only files the operator put in <store>/firmware, never a path from the network.
                let dir = self.cfg.store.dir.join("firmware");
                let file = args.get("file").and_then(Json::as_str).map(String::from);
                let target = args.get("target").and_then(Json::as_u64).unwrap_or(0) as u8;
                self.respond_later(action, async move {
                    let (Some(n), Some(file)) = (node, file) else {
                        return reply(Err("give \"node\" and \"file\"".into()));
                    };
                    if file.contains('/') || file.contains('\\') || file.starts_with('.') {
                        return reply(Err("\"file\" is a name in the firmware directory".into()));
                    }
                    let bytes = match std::fs::read(dir.join(&file)) {
                        Ok(b) => b,
                        Err(e) => return reply(Err(format!("{file}: {e}"))),
                    };
                    let image = match zwave_host::firmware::read_image(&file, &bytes) {
                        Ok(i) => i,
                        Err(e) => return reply(Err(format!("{file}: {e}"))),
                    };
                    reply(
                        c.update_firmware(n, &image, target)
                            .await
                            .map(|r| json!({"node": n, "status": r.status, "success": r.success(), "wait_time": r.wait_time, "activated": r.activated}))
                            .map_err(|e| e.to_string()),
                    )
                });
            }
            "configure" => {
                let device = node.and_then(|n| self.devices.get(&n).cloned());
                let settings: Vec<(String, i64)> = args
                    .get("settings")
                    .and_then(Json::as_object)
                    .map(|m| {
                        m.iter()
                            .filter_map(|(k, v)| Some((k.clone(), v.as_i64()?)))
                            .collect()
                    })
                    .unwrap_or_default();
                self.respond_later(action, async move {
                    match (node, device) {
                        (Some(n), Some(d)) => reply(
                            c.configure(n, &d, &settings)
                                .await
                                .map(|_| json!({"node": n}))
                                .map_err(|e| e.to_string()),
                        ),
                        (Some(n), None) => reply(Err(format!("no device file for node {n}"))),
                        _ => reply(Err("give \"node\" and \"settings\"".into())),
                    }
                });
            }
            "topology" => {
                self.respond_later(action, async move {
                    for n in c.nodes() {
                        let _ = c.refresh_neighbors(n.id).await;
                    }
                    let t = c.topology();
                    let nodes: Vec<Json> = t
                        .nodes
                        .iter()
                        .map(|n| json!({"id": n.id, "neighbors": n.neighbors, "route": n.last_route, "rssi": n.last_rssi, "listening": n.listening}))
                        .collect();
                    reply(Ok(json!({"controller": t.controller, "nodes": nodes})))
                });
            }
            other => {
                let body = json!({"ok": false, "error": format!("unknown request {other}")});
                self.publish(
                    &format!("{}/bridge/response/{other}", self.base),
                    &body,
                    false,
                )
                .await;
            }
        }
    }

    fn set(&self, path: &str, payload: &[u8]) {
        // <node>/<class>/<property>[/<endpoint>]
        let parts: Vec<&str> = path.split('/').collect();
        let (node, class, property, endpoint) = match parts.as_slice() {
            [n, c, p] => (n, c, p, "0"),
            [n, c, p, e] => (n, c, p, *e),
            _ => return,
        };
        let (Ok(node), Ok(endpoint)) = (node.parse::<u16>(), endpoint.parse::<u8>()) else {
            return;
        };
        let Some(cc) = zwave_cc::COMMAND_CLASSES
            .iter()
            .find(|c| c.name == *class)
            .map(|c| c.id)
        else {
            return;
        };
        let text = String::from_utf8_lossy(payload).trim().to_string();
        let json: Option<Json> = serde_json::from_str(&text).ok();
        let raw = match json.as_ref().and_then(|j| j.get("value")).cloned().or(json) {
            Some(Json::Bool(b)) => Some(ValueKind::Bool(b)),
            Some(Json::Number(n)) => n.as_f64().map(|f| {
                if cc == 0x25 {
                    ValueKind::Bool(f != 0.0)
                } else {
                    ValueKind::Level(f.clamp(0.0, 255.0) as u8)
                }
            }),
            _ => match text.to_ascii_uppercase().as_str() {
                "ON" | "TRUE" => Some(if cc == 0x25 {
                    ValueKind::Bool(true)
                } else {
                    ValueKind::Level(0xff)
                }),
                "OFF" | "FALSE" => Some(if cc == 0x25 {
                    ValueKind::Bool(false)
                } else {
                    ValueKind::Level(0)
                }),
                _ => None,
            },
        };
        let Some(value) = raw else {
            warn!(topic = path, "unreadable set payload");
            return;
        };
        let id = ValueId {
            node,
            endpoint,
            cc,
            property: property.to_string(),
        };
        let c = self.c.clone();
        tokio::spawn(async move {
            if let Err(e) = c.set_value(&id, &value).await {
                warn!("set {}: {e}", id.key());
            }
        });
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn provisioning_json(entries: &[Entry]) -> Json {
    Json::Array(
        entries
            .iter()
            .map(|e| json!({"dsk": e.dsk.to_string(), "classes": class_names(&e.granted), "node": e.node_id, "name": e.name, "active": e.status == Default::default()}))
            .collect(),
    )
}
