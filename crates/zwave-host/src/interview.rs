//! The interview: everything the host learns about a node after inclusion
//! (and resumes after a restart).

use crate::controller::{Controller, Inbound, TxSecurity};
use crate::node::{InterviewStage, Manufacturer};
use crate::{Error, Event, NodeId};
use std::time::Duration;
use tracing::{debug, info, warn};
use zwave_cc::{
    association, basic, battery, binary_sensor, binary_switch, manufacturer_specific, meter,
    multi_channel, multilevel_sensor, multilevel_switch, notification, security, security_2,
    version, wake_up, zwave_plus_info, Command,
};
use zwave_security::s2::KeyId;
use zwave_security::SecurityClass;
use zwave_serial::{DataFrame, FunctionId as F};

/// Interview attempts before giving up until the next restart or wake-up.
const MAX_ATTEMPTS: u32 = 5;

impl Controller {
    /// Starts (or resumes) a node's interview in the background, unless one is running.
    pub fn spawn_interview(&self, node: NodeId) {
        if !self.inner.interviews.lock().expect("lock").insert(node) {
            return;
        }
        let c = self.clone();
        tokio::spawn(async move {
            let mut attempt = 0;
            loop {
                attempt += 1;
                match c.interview(node).await {
                    Ok(()) => {
                        info!(node, "interview complete");
                        c.emit(Event::InterviewCompleted { node });
                        break;
                    }
                    Err(Error::UnknownNode(_)) => break,
                    Err(e) => {
                        warn!(node, attempt, "interview failed: {e}");
                        c.emit(Event::InterviewFailed {
                            node,
                            reason: e.to_string(),
                        });
                        if attempt >= MAX_ATTEMPTS {
                            break;
                        }
                        tokio::time::sleep(Duration::from_secs(10 * u64::from(attempt))).await;
                    }
                }
            }
            c.inner.interviews.lock().expect("lock").remove(&node);
        });
    }

    /// Re-runs a node's interview from the start.
    pub fn reinterview(&self, node: NodeId) -> Result<(), Error> {
        self.update_node(node, |r| {
            r.stage = InterviewStage::ProtocolInfo;
            r.secure_ccs_known = false;
        })?;
        self.spawn_interview(node);
        Ok(())
    }

    fn stage(&self, node: NodeId) -> Result<InterviewStage, Error> {
        self.with_node(node, |r| r.stage)
            .ok_or(Error::UnknownNode(node))
    }

    fn reach(&self, node: NodeId, stage: InterviewStage) -> Result<(), Error> {
        self.update_node(node, |r| {
            if r.stage < stage {
                r.stage = stage;
            }
        })?;
        self.emit(Event::InterviewStage { node, stage });
        self.emit(Event::NodeUpdated { node });
        Ok(())
    }

    async fn interview(&self, node: NodeId) -> Result<(), Error> {
        if self.stage(node)? < InterviewStage::ProtocolInfo {
            self.fetch_protocol_info(node).await?;
            self.reach(node, InterviewStage::ProtocolInfo)?;
        }
        if self.stage(node)? < InterviewStage::NodeInfo {
            self.request_node_info(node).await?;
            self.reach(node, InterviewStage::NodeInfo)?;
        }
        if self.stage(node)? < InterviewStage::Security {
            self.interview_security(node).await?;
            self.reach(node, InterviewStage::Security)?;
        }
        if self.stage(node)? < InterviewStage::Identity {
            self.interview_identity(node).await;
            self.reach(node, InterviewStage::Identity)?;
        }
        if self.stage(node)? < InterviewStage::Versions {
            self.interview_versions(node).await;
            self.reach(node, InterviewStage::Versions)?;
        }
        if self.stage(node)? < InterviewStage::Endpoints {
            self.interview_endpoints(node).await;
            self.reach(node, InterviewStage::Endpoints)?;
        }
        if self.stage(node)? < InterviewStage::CommandClasses {
            let eps: Vec<u8> = self
                .with_node(node, |r| r.endpoints.keys().copied().collect())
                .unwrap_or_default();
            for ep in eps {
                self.interview_endpoint(node, ep).await;
            }
            self.configure_wake_up(node).await;
            self.configure_lifeline(node).await;
            self.reach(node, InterviewStage::CommandClasses)?;
        }
        if let Err(e) = self.refresh_neighbors(node).await {
            debug!(node, "neighbors: {e}");
        }
        self.reach(node, InterviewStage::Complete)?;
        Ok(())
    }

    async fn request_node_info(&self, node: NodeId) -> Result<(), Error> {
        let _a = self.activity(node);
        self.wait_awake(node).await;
        let (_g, rx) = self.wait_for(node, false, |i| {
            matches!(i, Inbound::NodeInfo(_) | Inbound::NodeInfoFailed)
        });
        let r = self
            .inner
            .api
            .request(
                DataFrame::request(F::REQUEST_NODE_INFO, vec![node as u8]),
                Duration::from_secs(2),
            )
            .await?;
        if r.payload.first() != Some(&1) {
            return Err(Error::Controller("Request Node Info refused".into()));
        }
        match tokio::time::timeout(self.inner.cfg.request_timeout * 2, rx).await {
            Ok(Ok(Inbound::NodeInfo(info))) => {
                self.update_node(node, |r| {
                    for cc in info.supported {
                        r.root_mut().ccs.entry(cc).or_default();
                    }
                })?;
                Ok(())
            }
            Ok(_) => Err(Error::Timeout(node)),
            Err(_) => Err(Error::Timeout(node)),
        }
    }

    async fn interview_security(&self, node: NodeId) -> Result<(), Error> {
        let Some((s2, s0)) = self.with_node(node, |r| (r.highest_s2(), r.s0_only())) else {
            return Err(Error::UnknownNode(node));
        };
        let _a = self.activity(node);
        self.wait_awake(node).await;
        let secure: Option<Vec<u16>> = if let Some(class) = s2 {
            let (c, _) = self
                .request_secured(
                    node,
                    Command::from(security_2::CommandsSupportedGet {}).encode(),
                    TxSecurity::S2(KeyId::Class(class)),
                    self.inner.cfg.request_timeout,
                    |c, _| {
                        matches!(
                            c,
                            Command::Security2(security_2::Command::CommandsSupportedReport(_))
                        )
                    },
                )
                .await?;
            let Command::Security2(security_2::Command::CommandsSupportedReport(r)) = c else {
                unreachable!()
            };
            Some(r.command_classes)
        } else if s0 {
            let (c, _) = self
                .request_secured(
                    node,
                    Command::from(security::CommandsSupportedGet {}).encode(),
                    TxSecurity::S0 { temp: false },
                    self.inner.cfg.request_timeout,
                    |c, _| {
                        matches!(
                            c,
                            Command::Security(security::Command::CommandsSupportedReport(_))
                        )
                    },
                )
                .await?;
            let Command::Security(security::Command::CommandsSupportedReport(r)) = c else {
                unreachable!()
            };
            Some(r.command_classes)
        } else {
            None
        };
        self.update_node(node, |r| {
            if let Some(list) = &secure {
                for cc in list {
                    r.root_mut().ccs.entry(*cc).or_default().secure = true;
                }
            }
            r.secure_ccs_known = true;
        })
    }

    async fn interview_identity(&self, node: NodeId) {
        let supports = |cc| {
            self.with_node(node, |r| r.root().supports(cc))
                .unwrap_or(false)
        };
        if supports(manufacturer_specific::ID) {
            let r = self
                .req(node, 0, manufacturer_specific::Get {}, |c| match c {
                    Command::ManufacturerSpecific(manufacturer_specific::Command::Report(r)) => {
                        Some(r.clone())
                    }
                    _ => None,
                })
                .await;
            match r {
                Ok(m) => {
                    let _ = self.update_node(node, |r| {
                        r.manufacturer = Some(Manufacturer {
                            manufacturer_id: m.manufacturer_id,
                            product_type: m.product_type,
                            product_id: m.product_id,
                        })
                    });
                }
                Err(e) => warn!(node, "manufacturer: {e}"),
            }
        }
        if supports(zwave_plus_info::ID) {
            let r = self
                .req(node, 0, zwave_plus_info::Get {}, |c| match c {
                    Command::ZwavePlusInfo(zwave_plus_info::Command::Report(r)) => Some(r.clone()),
                    _ => None,
                })
                .await;
            match r {
                Ok(z) => {
                    let _ = self.update_node(node, |r| {
                        r.zwave_plus =
                            Some((z.role_type, z.node_type, z.installer_icon, z.user_icon))
                    });
                }
                Err(e) => warn!(node, "Z-Wave Plus info: {e}"),
            }
        }
    }

    async fn interview_versions(&self, node: NodeId) {
        let Some(ccs) = self.with_node(node, |r| r.root().ccs.keys().copied().collect::<Vec<_>>())
        else {
            return;
        };
        if !ccs.contains(&version::ID) {
            return;
        }
        let r = self
            .req(node, 0, version::Get {}, |c| match c {
                Command::Version(version::Command::Report(r)) => Some(r.clone()),
                _ => None,
            })
            .await;
        if let Ok(v) = &r {
            let _ = self.update_node(node, |r| {
                r.firmware = Some(format!("{}.{}", v.firmware_version, v.firmware_sub_version))
            });
        }
        for cc in ccs.into_iter().filter(|&c| c <= 0xff) {
            let r = self
                .req(
                    node,
                    0,
                    version::CommandClassGet {
                        requested_cc: cc as u8,
                    },
                    move |c| match c {
                        Command::Version(version::Command::CommandClassReport(r))
                            if u16::from(r.requested_cc) == cc =>
                        {
                            Some(r.version)
                        }
                        _ => None,
                    },
                )
                .await;
            match r {
                Ok(v) => {
                    let _ = self.update_node(node, |r| {
                        if v == 0 {
                            // Version 0: not actually supported.
                            r.root_mut().ccs.remove(&cc);
                        } else if let Some(s) = r.root_mut().ccs.get_mut(&cc) {
                            s.version = v;
                        }
                    });
                }
                Err(e) => debug!(node, cc, "version: {e}"),
            }
        }
    }

    async fn interview_endpoints(&self, node: NodeId) {
        if !self
            .with_node(node, |r| r.root().supports(multi_channel::ID))
            .unwrap_or(false)
        {
            return;
        }
        let r = self
            .req(node, 0, multi_channel::EndPointGet {}, |c| match c {
                Command::MultiChannel(multi_channel::Command::EndPointReport(r)) => {
                    Some(r.individual_end_points)
                }
                _ => None,
            })
            .await;
        let Ok(count) = r else { return };
        for ep in 1..=count.min(127) {
            let r = self
                .req(
                    node,
                    0,
                    multi_channel::CapabilityGet { end_point: ep },
                    move |c| match c {
                        Command::MultiChannel(multi_channel::Command::CapabilityReport(r))
                            if r.end_point == ep =>
                        {
                            Some(r.clone())
                        }
                        _ => None,
                    },
                )
                .await;
            match r {
                Ok(cap) => {
                    let _ = self.update_node(node, |r| {
                        let versions: Vec<(u16, crate::node::CcSupport)> =
                            r.root().ccs.iter().map(|(k, v)| (*k, *v)).collect();
                        let e = r.endpoints.entry(ep).or_default();
                        e.generic = cap.generic;
                        e.specific = cap.specific;
                        for cc in cap.command_classes {
                            let root = versions
                                .iter()
                                .find(|(k, _)| *k == cc)
                                .map(|(_, v)| *v)
                                .unwrap_or_default();
                            e.ccs.insert(cc, root);
                        }
                    });
                }
                Err(e) => warn!(node, ep, "endpoint capability: {e}"),
            }
        }
    }

    async fn interview_endpoint(&self, node: NodeId, ep: u8) {
        let Some(e) = self
            .with_node(node, |r| r.endpoints.get(&ep).cloned())
            .flatten()
        else {
            return;
        };
        let has = |cc| e.supports(cc);
        if has(multilevel_sensor::ID) {
            if e.version(multilevel_sensor::ID) >= 5 {
                let types = self
                    .req(
                        node,
                        ep,
                        multilevel_sensor::SupportedSensorGet {},
                        |c| match c {
                            Command::MultilevelSensor(
                                multilevel_sensor::Command::SupportedSensorReport(r),
                            ) => Some(r.sensor_types.clone()),
                            _ => None,
                        },
                    )
                    .await
                    .unwrap_or_default();
                for t in types.into_iter().filter_map(|t| u8::try_from(t).ok()) {
                    let scales = self
                        .req(
                            node,
                            ep,
                            multilevel_sensor::SupportedScaleGet { sensor_type: t },
                            move |c| match c {
                                Command::MultilevelSensor(
                                    multilevel_sensor::Command::SupportedScaleReport(r),
                                ) if r.sensor_type == t => Some(r.scales),
                                _ => None,
                            },
                        )
                        .await
                        .unwrap_or(1);
                    let list: Vec<u8> = (0..4).filter(|b| scales & (1 << b) != 0).collect();
                    let first = list.first().copied().unwrap_or(0);
                    let _ = self.update_node(node, |r| {
                        r.endpoints
                            .entry(ep)
                            .or_default()
                            .caps
                            .sensors
                            .insert(t, list.clone());
                    });
                    let _ = self
                        .req(node, ep, multilevel_sensor::Get { sensor_type: Some(t), scale: Some(first) }, move |c| {
                            matches!(c, Command::MultilevelSensor(multilevel_sensor::Command::Report(r)) if r.sensor_type == t).then_some(())
                        })
                        .await;
                }
            } else {
                let r = self
                    .req(
                        node,
                        ep,
                        multilevel_sensor::Get {
                            sensor_type: None,
                            scale: None,
                        },
                        |c| match c {
                            Command::MultilevelSensor(multilevel_sensor::Command::Report(r)) => {
                                Some((r.sensor_type, r.value.scale))
                            }
                            _ => None,
                        },
                    )
                    .await;
                if let Ok((t, s)) = r {
                    let _ = self.update_node(node, |r| {
                        r.endpoints
                            .entry(ep)
                            .or_default()
                            .caps
                            .sensors
                            .insert(t, vec![s]);
                    });
                }
            }
        }
        if has(meter::ID) {
            let v = e.version(meter::ID);
            let scales: Vec<u8> = if v >= 2 {
                match self
                    .req(node, ep, meter::SupportedGet {}, |c| match c {
                        Command::Meter(meter::Command::SupportedReport(r)) => Some(r.clone()),
                        _ => None,
                    })
                    .await
                {
                    Ok(s) => {
                        let list: Vec<u8> = (0..7).filter(|b| s.scales & (1 << b) != 0).collect();
                        let _ = self.update_node(node, |r| {
                            r.endpoints.entry(ep).or_default().caps.meter =
                                Some((s.meter_type, list.clone()))
                        });
                        list
                    }
                    Err(_) => vec![],
                }
            } else {
                vec![0]
            };
            for s in scales {
                let get = if v >= 2 {
                    meter::Get {
                        rate_type: Some(if v >= 4 { 1 } else { 0 }),
                        scale: Some(s),
                        scale2: None,
                    }
                } else {
                    meter::Get::default()
                };
                let _ = self
                    .req(node, ep, get, |c| {
                        matches!(c, Command::Meter(meter::Command::Report(_))).then_some(())
                    })
                    .await;
            }
        }
        if has(notification::ID) && e.version(notification::ID) >= 3 {
            let types = self
                .req(node, ep, notification::SupportedGet {}, |c| match c {
                    Command::Notification(notification::Command::SupportedReport(r)) => {
                        Some(r.types.clone())
                    }
                    _ => None,
                })
                .await
                .unwrap_or_default();
            for t in types
                .into_iter()
                .filter_map(|t| u8::try_from(t).ok())
                .filter(|&t| t > 0)
            {
                let events = self
                    .req(
                        node,
                        ep,
                        notification::EventSupportedGet {
                            notification_type: t,
                        },
                        move |c| match c {
                            Command::Notification(notification::Command::EventSupportedReport(
                                r,
                            )) if r.notification_type == t => Some(r.events.clone()),
                            _ => None,
                        },
                    )
                    .await
                    .unwrap_or_default();
                let events: Vec<u8> = events
                    .into_iter()
                    .filter_map(|e| u8::try_from(e).ok())
                    .collect();
                let _ = self.update_node(node, |r| {
                    r.endpoints
                        .entry(ep)
                        .or_default()
                        .caps
                        .notifications
                        .insert(t, events.clone());
                });
            }
        }
        if has(binary_sensor::ID) {
            let types: Vec<u8> = if e.version(binary_sensor::ID) >= 2 {
                self.req(node, ep, binary_sensor::SupportedGet {}, |c| match c {
                    Command::BinarySensor(binary_sensor::Command::SupportedReport(r)) => {
                        Some(r.sensor_types.clone())
                    }
                    _ => None,
                })
                .await
                .unwrap_or_default()
                .into_iter()
                .filter_map(|t| u8::try_from(t).ok())
                .filter(|&t| t > 0)
                .collect()
            } else {
                vec![]
            };
            let _ = self.update_node(node, |r| {
                r.endpoints.entry(ep).or_default().caps.binary_sensors = types.clone()
            });
            if types.is_empty() {
                let _ = self
                    .req(node, ep, binary_sensor::Get { sensor_type: None }, |c| {
                        matches!(c, Command::BinarySensor(binary_sensor::Command::Report(_)))
                            .then_some(())
                    })
                    .await;
            }
            for t in types {
                let _ = self
                    .req(
                        node,
                        ep,
                        binary_sensor::Get {
                            sensor_type: Some(t),
                        },
                        |c| {
                            matches!(c, Command::BinarySensor(binary_sensor::Command::Report(_)))
                                .then_some(())
                        },
                    )
                    .await;
            }
        }
        if has(multilevel_switch::ID) {
            if e.version(multilevel_switch::ID) >= 3 {
                if let Ok(t) = self
                    .req(node, ep, multilevel_switch::SupportedGet {}, |c| match c {
                        Command::MultilevelSwitch(multilevel_switch::Command::SupportedReport(
                            r,
                        )) => Some(r.primary_switch_type),
                        _ => None,
                    })
                    .await
                {
                    let _ = self.update_node(node, |r| {
                        r.endpoints.entry(ep).or_default().caps.switch_type = Some(t)
                    });
                }
            }
            let _ = self
                .req(node, ep, multilevel_switch::Get {}, |c| {
                    matches!(
                        c,
                        Command::MultilevelSwitch(multilevel_switch::Command::Report(_))
                    )
                    .then_some(())
                })
                .await;
        }
        if has(binary_switch::ID) {
            let _ = self
                .req(node, ep, binary_switch::Get {}, |c| {
                    matches!(c, Command::BinarySwitch(binary_switch::Command::Report(_)))
                        .then_some(())
                })
                .await;
        }
        let actuator_or_sensor = [
            binary_switch::ID,
            multilevel_switch::ID,
            multilevel_sensor::ID,
            binary_sensor::ID,
            notification::ID,
            meter::ID,
        ]
        .iter()
        .any(|&c| has(c));
        if has(basic::ID) && !actuator_or_sensor {
            let _ = self
                .req(node, ep, basic::Get {}, |c| {
                    matches!(c, Command::Basic(basic::Command::Report(_))).then_some(())
                })
                .await;
        }
        if ep == 0 && has(battery::ID) {
            let _ = self
                .req(node, 0, battery::Get {}, |c| {
                    matches!(c, Command::Battery(battery::Command::Report(_))).then_some(())
                })
                .await;
        }
    }

    async fn configure_wake_up(&self, node: NodeId) {
        let Some((has, v)) = self.with_node(node, |r| {
            (
                r.root().supports(wake_up::ID),
                r.root().version(wake_up::ID),
            )
        }) else {
            return;
        };
        if !has {
            return;
        }
        let mut want = self.inner.cfg.wake_up_interval;
        if v >= 2 {
            if let Ok(c) = self
                .req(node, 0, wake_up::IntervalCapabilitiesGet {}, |c| match c {
                    Command::WakeUp(wake_up::Command::IntervalCapabilitiesReport(r)) => {
                        Some(r.clone())
                    }
                    _ => None,
                })
                .await
            {
                want = want.clamp(c.minimum, c.maximum.max(c.minimum));
                if let Some(steps) = (want - c.minimum).checked_div(c.step) {
                    want = c.minimum + steps * c.step;
                }
            }
        }
        let own = self.info().node_id as u8;
        if let Err(e) = self
            .send(
                node,
                0,
                &wake_up::IntervalSet {
                    seconds: want,
                    node_id: own,
                }
                .into(),
            )
            .await
        {
            warn!(node, "wake-up interval: {e}");
            return;
        }
        if let Ok(r) = self
            .req(node, 0, wake_up::IntervalGet {}, |c| match c {
                Command::WakeUp(wake_up::Command::IntervalReport(r)) => Some(r.clone()),
                _ => None,
            })
            .await
        {
            let _ = self.update_node(node, |rec| rec.wake_up_interval = Some(r.seconds));
        }
    }

    async fn configure_lifeline(&self, node: NodeId) {
        if !self
            .with_node(node, |r| r.root().supports(association::ID))
            .unwrap_or(false)
        {
            return;
        }
        let groups = self
            .req(node, 0, association::GroupingsGet {}, |c| match c {
                Command::Association(association::Command::GroupingsReport(r)) => Some(r.groups),
                _ => None,
            })
            .await
            .unwrap_or(0);
        if groups == 0 {
            return;
        }
        let own = self.info().node_id as u8;
        if let Err(e) = self
            .send(
                node,
                0,
                &association::Set {
                    group: 1,
                    nodes: vec![own],
                }
                .into(),
            )
            .await
        {
            warn!(node, "lifeline: {e}");
        }
    }

    /// The classes a node was granted, for callers that need them.
    pub fn security_classes(&self, node: NodeId) -> Vec<SecurityClass> {
        self.with_node(node, |r| r.security.clone())
            .unwrap_or_default()
    }
}
