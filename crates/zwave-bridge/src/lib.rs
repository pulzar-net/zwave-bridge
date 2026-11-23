#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod api;
pub mod config;
pub mod discovery;

use anyhow::{Context, Result};
use config::Config;
use rumqttc::{AsyncClient, Event as MqttEvent, LastWill, MqttOptions, Packet, QoS};
use serde::Serialize;
use serde_json::{json, Value as Json};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use zwave_host::{
    Controller, Event, GrantPolicy, HostConfig, InclusionOptions, NetworkKeys, ValueId, ValueKind,
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
    responses: mpsc::Sender<(String, Vec<u8>)>,
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
        api::BridgeState::payload(api::Availability::Offline),
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
    let (rtx, mut rrx) = mpsc::channel::<(String, Vec<u8>)>(64);
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
                b.publish_raw(&format!("{}/bridge/response/{action}", b.base), body, false).await;
            },
            _ = &mut shutdown => break,
            _ = restart.notified() => {
                // The controller restarted with a restored memory: start over with a fresh host.
                tokio::time::sleep(Duration::from_secs(1)).await;
                b.publish_raw(&format!("{}/bridge/state", b.base), api::BridgeState::payload(api::Availability::Offline), true).await;
                let _ = b.mqtt.disconnect().await;
                return Err(anyhow::anyhow!("controller memory restored; restart zwave-bridge"));
            }
        }
    }
    b.publish_raw(
        &format!("{}/bridge/state", b.base),
        api::BridgeState::payload(api::Availability::Offline),
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

    /// Publishes one of the [`api`] payloads as JSON.
    async fn publish(&self, topic: &str, body: &impl Serialize, retain: bool) {
        match serde_json::to_vec(body) {
            Ok(b) => self.publish_raw(topic, b, retain).await,
            Err(e) => warn!("serialize {topic}: {e}"),
        }
    }

    /// Publishes the response to the request being handled.
    async fn respond(&self, action: &str, body: &impl Serialize) {
        self.publish(
            &format!("{}/bridge/response/{action}", self.base),
            body,
            false,
        )
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
            api::BridgeState::payload(api::Availability::Online),
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
        let body = api::BridgeInfo {
            home_id: format!("{:08x}", i.home_id),
            node_id: i.node_id,
            library_version: i.library_version,
            library_type: i.library_type,
            firmware: i.firmware,
            manufacturer_id: i.manufacturer_id,
            product_type: i.product_type,
            product_id: i.product_id,
            chip_type: i.chip_type,
            sis: i.sis,
            region: i.region,
            dsk: self.c.dsk().to_string(),
            nodes: i.nodes,
            version: env!("CARGO_PKG_VERSION").to_string(),
        };
        self.publish(&format!("{}/bridge/info", self.base), &body, true)
            .await;
    }

    async fn publish_node(&self, node: u16) {
        let Some(rec) = self.c.node(node) else { return };
        let body = api::Node::new(
            &rec,
            self.c.status(node),
            self.devices.get(&node).map(|d| d.as_ref()),
        );
        self.publish(&format!("{}/nodes/{node}", self.base), &body, true)
            .await;
        self.publish_raw(
            &format!("{}/nodes/{node}/status", self.base),
            body.status.as_str().as_bytes().to_vec(),
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
        let params = api::NodeParameters(d.params.iter().map(api::Parameter::from).collect());
        self.publish(
            &format!("{}/nodes/{node}/parameters", self.base),
            &params,
            true,
        )
        .await;
    }

    async fn event(&self, body: api::BridgeEvent) {
        self.publish(&format!("{}/bridge/event", self.base), &body, false)
            .await;
    }

    async fn on_event(&mut self, e: Event) {
        match e {
            Event::Ready(_) => self.publish_info().await,
            Event::Value(v) => {
                let key = v.id.key();
                let topic = format!("{}/nodes/{}/{key}", self.base, v.id.node);
                let body = api::ValueMessage::new(&v, now_ms());
                if self.cfg.homeassistant.enabled
                    && !self
                        .discovered
                        .get(&v.id.node)
                        .is_some_and(|s| s.contains(&key))
                {
                    if let Some(rec) = self.c.node(v.id.node) {
                        if let Some(d) = discovery::for_value(
                            &self.cfg.homeassistant.discovery_prefix,
                            &self.base,
                            self.c.info().home_id,
                            &rec,
                            self.devices.get(&v.id.node).map(|d| d.as_ref()),
                            &v,
                        ) {
                            self.publish(&d.topic, &d.config, true).await;
                        }
                        self.discovered.entry(v.id.node).or_default().insert(key);
                    }
                }
                self.publish(&topic, &body, true).await;
            }
            Event::NodeAdded { node } => {
                self.event(api::BridgeEvent::NodeAdded { node }).await;
                self.publish_node(node).await;
            }
            Event::NodeRemoved { node } => {
                self.event(api::BridgeEvent::NodeRemoved { node }).await;
                self.publish_raw(&format!("{}/nodes/{node}", self.base), vec![], true)
                    .await;
                self.publish_raw(&format!("{}/nodes/{node}/status", self.base), vec![], true)
                    .await;
                self.discovered.remove(&node);
            }
            Event::NodeUpdated { node } | Event::NodeStatus { node, .. } => {
                self.publish_node(node).await
            }
            Event::InterviewCompleted { node } => {
                self.load_device(node);
                self.publish_parameters(node).await;
                self.event(api::BridgeEvent::InterviewCompleted { node })
                    .await;
                self.publish_node(node).await;
            }
            Event::InterviewFailed { node, reason } => {
                self.event(api::BridgeEvent::InterviewFailed { node, reason })
                    .await
            }
            Event::InterviewStage { node, stage } => {
                self.event(api::BridgeEvent::InterviewStage {
                    node,
                    stage: stage.into(),
                })
                .await
            }
            Event::InclusionStarted => self.event(api::BridgeEvent::InclusionStarted).await,
            Event::InclusionStopped => self.event(api::BridgeEvent::InclusionStopped).await,
            Event::ExclusionStarted => self.event(api::BridgeEvent::ExclusionStarted).await,
            Event::ExclusionStopped => self.event(api::BridgeEvent::ExclusionStopped).await,
            Event::GrantSecurityClasses { node, requested } => {
                self.event(api::BridgeEvent::GrantSecurityClasses {
                    node,
                    requested: api::classes(&requested),
                })
                .await
            }
            Event::ValidateDsk { node, dsk } => {
                self.event(api::BridgeEvent::ValidateDsk { node, dsk })
                    .await
            }
            Event::SecurityBootstrapped { node, classes } => {
                self.event(api::BridgeEvent::SecurityBootstrapped {
                    node,
                    classes: api::classes(&classes),
                })
                .await
            }
            Event::SecurityFailed { node, reason } => {
                self.event(api::BridgeEvent::SecurityFailed { node, reason })
                    .await
            }
            Event::Route {
                node,
                repeaters,
                rssi,
            } => {
                let body = api::Route {
                    repeaters,
                    rssi,
                    time: now_ms(),
                };
                self.publish(&format!("{}/nodes/{node}/route", self.base), &body, true)
                    .await
            }
            Event::Command {
                node,
                endpoint,
                command,
            } => {
                let body = api::NodeCommand {
                    endpoint,
                    cc: command.cc(),
                    command: command.name().to_string(),
                    frame: hex(&command.encode()),
                };
                self.publish(&format!("{}/nodes/{node}/command", self.base), &body, false)
                    .await;
            }
            Event::FirmwareUpdateProgress { node, sent, total } => {
                self.event(api::BridgeEvent::FirmwareUpdateProgress { node, sent, total })
                    .await
            }
            Event::FirmwareUpdateFinished {
                node,
                status,
                success,
            } => {
                self.event(api::BridgeEvent::FirmwareUpdateFinished {
                    node,
                    status,
                    success,
                })
                .await
            }
            Event::ProvisioningChanged { entries } => {
                self.event(api::BridgeEvent::ProvisioningChanged {
                    entries: api::provisioning(&entries),
                })
                .await
            }
        }
    }

    async fn on_message(&mut self, topic: &str, payload: &[u8]) {
        let Some(rest) = topic.strip_prefix(&format!("{}/", self.base)) else {
            return;
        };
        if let Some(action) = rest.strip_prefix("bridge/request/") {
            self.request(action, payload).await;
        } else if let Some(path) = rest
            .strip_prefix("nodes/")
            .and_then(|p| p.strip_suffix("/set"))
        {
            self.set(path, payload);
        }
    }

    /// Publishes a response once `fut` gives it, without holding up the bridge.
    fn respond_later<T: Serialize + Send + 'static>(
        &self,
        action: &str,
        fut: impl std::future::Future<Output = T> + Send + 'static,
    ) {
        let tx = self.responses.clone();
        let action = action.to_string();
        tokio::spawn(async move {
            match serde_json::to_vec(&fut.await) {
                Ok(body) => {
                    let _ = tx.send((action, body)).await;
                }
                Err(e) => warn!("serialize the {action} response: {e}"),
            }
        });
    }

    async fn request(&mut self, action: &str, payload: &[u8]) {
        let args: Json = if payload.is_empty() {
            json!({})
        } else {
            match serde_json::from_slice(payload) {
                Ok(j) => j,
                Err(e) => {
                    let body =
                        api::ErrorResponse::new(Err(format!("{action}: not JSON: {e}")), None);
                    return self.respond(action, &body).await;
                }
            }
        };
        let transaction = args.get("transaction").cloned();
        debug!(%action, %args, "request");
        self.handle_request(action, args, transaction).await;
    }

    async fn handle_request(&mut self, action: &str, args: Json, t: Option<Json>) {
        // The request's arguments as its type in `api`, or an error response.
        macro_rules! args {
            ($ty:ty) => {
                match serde_json::from_value::<$ty>(args) {
                    Ok(a) => a,
                    Err(e) => {
                        let body = api::ErrorResponse::new(Err(format!("{action}: {e}")), t);
                        return self.respond(action, &body).await;
                    }
                }
            };
        }
        let c = self.c.clone();
        let err = |e: zwave_host::Error| e.to_string();
        match action {
            "include" => {
                let a = args!(api::IncludeRequest);
                let insecure = a.strategy == Some(api::InclusionStrategy::Insecure);
                let grant = match (a.strategy, &a.classes) {
                    (Some(api::InclusionStrategy::Requested), _) => GrantPolicy::Requested,
                    (Some(api::InclusionStrategy::Insecure), _) => GrantPolicy::Only(vec![]),
                    (None, Some(classes)) => GrantPolicy::Only(api::host_classes(classes)),
                    (None, None) => GrantPolicy::Ask,
                };
                let opts = InclusionOptions {
                    insecure,
                    pin: a
                        .pin
                        .as_deref()
                        .and_then(|p| zwave_security::dsk::parse_pin(p).ok()),
                    grant,
                    timeout: a.timeout.map(Duration::from_secs),
                };
                self.respond_later(action, async move {
                    let r = c
                        .include(opts)
                        .await
                        .map_err(err)
                        .map(|r| api::IncludeData {
                            node: r.node,
                            security: api::classes(&r.security),
                            security_error: r.security_error,
                        });
                    api::IncludeResponse::new(r, t)
                });
            }
            "exclude" => {
                let a = args!(api::ExcludeRequest);
                let timeout = a.timeout.map(Duration::from_secs);
                self.respond_later(action, async move {
                    let r = c
                        .exclude(timeout)
                        .await
                        .map_err(err)
                        .map(|node| api::ExcludeData { node });
                    api::ExcludeResponse::new(r, t)
                });
            }
            "stop" => {
                args!(api::StopRequest);
                let body = api::StopResponse {
                    ok: c.stop_inclusion(),
                    data: None,
                    error: None,
                    transaction: t,
                };
                self.respond(action, &body).await;
            }
            "grant" => {
                let a = args!(api::GrantRequest);
                let body = api::GrantResponse {
                    ok: c.grant_security_classes(api::host_classes(&a.classes)),
                    data: None,
                    error: None,
                    transaction: t,
                };
                self.respond(action, &body).await;
            }
            "validate_dsk" => {
                let a = args!(api::ValidateDskRequest);
                let pin = a
                    .pin
                    .as_deref()
                    .and_then(|p| zwave_security::dsk::parse_pin(p).ok());
                let body = api::ValidateDskResponse {
                    ok: c.validate_dsk(pin),
                    data: None,
                    error: None,
                    transaction: t,
                };
                self.respond(action, &body).await;
            }
            "provision" => {
                let a = args!(api::ProvisionRequest);
                let entry = if let Some(qr) = &a.qr {
                    zwave_security::qr::parse(qr)
                        .map(|q| Entry::from_qr(&q))
                        .map_err(|e| e.to_string())
                } else if let Some(dsk) = &a.dsk {
                    dsk.parse()
                        .map_err(|e: zwave_security::Error| e.to_string())
                        .map(|dsk| Entry {
                            dsk,
                            granted: api::host_classes(a.classes.as_deref().unwrap_or(&[
                                api::SecurityClass::S2Authenticated,
                                api::SecurityClass::S2Unauthenticated,
                            ])),
                            status: Default::default(),
                            node_id: None,
                            name: a.name.clone(),
                        })
                } else {
                    Err("give \"qr\" or \"dsk\"".to_string())
                };
                self.respond_later(action, async move {
                    let r = match entry {
                        Ok(e) => {
                            let dsk = e.dsk.to_string();
                            c.provision(e)
                                .await
                                .map_err(err)
                                .map(|_| api::DskData { dsk })
                        }
                        Err(e) => Err(e),
                    };
                    api::ProvisionResponse::new(r, t)
                });
            }
            "unprovision" => {
                let a = args!(api::UnprovisionRequest);
                let dsk = a.dsk.as_deref().and_then(|d| d.parse().ok());
                self.respond_later(action, async move {
                    let r = match dsk {
                        Some(d) => Ok(api::UnprovisionData {
                            removed: c.unprovision(&d).await.is_some(),
                        }),
                        None => Err("give \"dsk\"".to_string()),
                    };
                    api::UnprovisionResponse::new(r, t)
                });
            }
            "provisioning_list" => {
                args!(api::ProvisioningListRequest);
                let entries = api::provisioning(&c.provisioning_list());
                let body = api::ProvisioningListResponse::new(
                    Ok(api::ProvisioningListData { entries }),
                    t,
                );
                self.respond(action, &body).await;
            }
            "backup" => {
                args!(api::BackupRequest);
                let dir = self.cfg.store.dir.clone();
                self.respond_later(action, async move {
                    let r = match c.backup_nvm().await {
                        Ok(image) => {
                            let path = dir.join(format!("nvm-{}.bin", now_ms() / 1000));
                            std::fs::write(&path, &image)
                                .map(|_| api::BackupData {
                                    file: path.display().to_string(),
                                    bytes: image.len(),
                                })
                                .map_err(|e| e.to_string())
                        }
                        Err(e) => Err(e.to_string()),
                    };
                    api::BackupResponse::new(r, t)
                });
            }
            "reinterview" => {
                let a = args!(api::ReinterviewRequest);
                let r = a
                    .node
                    .ok_or("give \"node\"".to_string())
                    .and_then(|n| c.reinterview(n).map_err(err))
                    .map(|_| api::Empty {});
                self.respond(action, &api::ReinterviewResponse::new(r, t))
                    .await;
            }
            "remove_failed" => {
                let a = args!(api::RemoveFailedRequest);
                self.respond_later(action, async move {
                    let r = match a.node {
                        Some(n) => c
                            .remove_failed_node(n)
                            .await
                            .map_err(err)
                            .map(|_| api::NodeData { node: n }),
                        None => Err("give \"node\"".to_string()),
                    };
                    api::RemoveFailedResponse::new(r, t)
                });
            }
            "rename" => {
                let a = args!(api::RenameRequest);
                let r = a
                    .node
                    .ok_or("give \"node\"".to_string())
                    .and_then(|n| c.set_name(n, a.name, a.location).map_err(err))
                    .map(|_| api::Empty {});
                self.respond(action, &api::RenameResponse::new(r, t)).await;
            }
            "restore" => {
                // A backup the bridge wrote (or the operator put) in the store directory.
                let a = args!(api::RestoreRequest);
                let dir = self.cfg.store.dir.clone();
                let restarted = self.restart.clone();
                self.respond_later(action, async move {
                    let r = async {
                        let file = a
                            .file
                            .filter(|f| {
                                !f.contains('/') && !f.contains('\\') && !f.starts_with('.')
                            })
                            .ok_or(
                                "\"file\" is a backup's name in the store directory".to_string(),
                            )?;
                        let image =
                            std::fs::read(dir.join(&file)).map_err(|e| format!("{file}: {e}"))?;
                        c.restore_nvm(&image).await.map_err(err)?;
                        restarted.notify_one();
                        Ok(api::RestoreData {
                            file,
                            restart: true,
                        })
                    };
                    api::RestoreResponse::new(r.await, t)
                });
            }
            "firmware_update" => {
                // Only files the operator put in <store>/firmware, never a path from the network.
                let a = args!(api::FirmwareUpdateRequest);
                let dir = self.cfg.store.dir.join("firmware");
                self.respond_later(action, async move {
                    let r = async {
                        let (Some(n), Some(file)) = (a.node, a.file) else {
                            return Err("give \"node\" and \"file\"".to_string());
                        };
                        if file.contains('/') || file.contains('\\') || file.starts_with('.') {
                            return Err("\"file\" is a name in the firmware directory".into());
                        }
                        let bytes =
                            std::fs::read(dir.join(&file)).map_err(|e| format!("{file}: {e}"))?;
                        let image = zwave_host::firmware::read_image(&file, &bytes)
                            .map_err(|e| format!("{file}: {e}"))?;
                        let r = c
                            .update_firmware(n, &image, a.target.unwrap_or(0))
                            .await
                            .map_err(err)?;
                        Ok(api::FirmwareUpdateData {
                            node: n,
                            status: r.status,
                            success: r.success(),
                            wait_time: r.wait_time,
                            activated: r.activated,
                        })
                    };
                    api::FirmwareUpdateResponse::new(r.await, t)
                });
            }
            "configure" => {
                let a = args!(api::ConfigureRequest);
                let device = a.node.and_then(|n| self.devices.get(&n).cloned());
                let settings: Vec<(String, i64)> = a
                    .settings
                    .iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_i64()?)))
                    .collect();
                self.respond_later(action, async move {
                    let r = match (a.node, device) {
                        (Some(n), Some(d)) => c
                            .configure(n, &d, &settings)
                            .await
                            .map_err(err)
                            .map(|_| api::NodeData { node: n }),
                        (Some(n), None) => Err(format!("no device file for node {n}")),
                        _ => Err("give \"node\" and \"settings\"".to_string()),
                    };
                    api::ConfigureResponse::new(r, t)
                });
            }
            "topology" => {
                args!(api::TopologyRequest);
                self.respond_later(action, async move {
                    for n in c.nodes() {
                        let _ = c.refresh_neighbors(n.id).await;
                    }
                    api::TopologyResponse::new(Ok(c.topology().into()), t)
                });
            }
            other => {
                let body = api::ErrorResponse::new(Err(format!("unknown request {other}")), t);
                self.respond(other, &body).await;
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
        let switch = cc == 0x25;
        let value = match api::SetRequest::parse(payload).value() {
            api::SetValue::Bool(b) => Some(ValueKind::Bool(*b)),
            api::SetValue::Number(f) if switch => Some(ValueKind::Bool(*f != 0.0)),
            api::SetValue::Number(f) => Some(ValueKind::Level(f.clamp(0.0, 255.0) as u8)),
            api::SetValue::Text(w) => match w.to_ascii_uppercase().as_str() {
                "ON" | "TRUE" if switch => Some(ValueKind::Bool(true)),
                "ON" | "TRUE" => Some(ValueKind::Level(0xff)),
                "OFF" | "FALSE" if switch => Some(ValueKind::Bool(false)),
                "OFF" | "FALSE" => Some(ValueKind::Level(0)),
                _ => None,
            },
        };
        let Some(value) = value else {
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
