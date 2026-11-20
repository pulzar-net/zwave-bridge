//! Simulated end nodes: their command classes, their side of S0 and S2,
//! sleeping and SmartStart.

use std::collections::BTreeMap;
use zwave_cc::{
    association, basic, battery, binary_switch, configuration, firmware_update_md,
    manufacturer_specific, meter, multilevel_sensor, notification, security, security_2,
    supervision, version, wake_up, zwave_plus_info, Command, Value,
};
use zwave_security::dsk::Dsk;
use zwave_security::kex::{bootstrap_keys, KeyPair, ECDH_CURVE25519, KEX_SCHEME_1};
use zwave_security::s0::{S0Manager, S0Payload, TEMP_KEY};
use zwave_security::s2::{EncryptOptions, KeyId, S2Manager};
use zwave_security::SecurityClass;
use zwave_serial::message::{NodeInfo, NodeProtocolInfo};

/// How a simulated node is secured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Security {
    /// No security.
    None,
    /// Security 0.
    S0,
    /// Security 2, requesting these classes.
    S2(Vec<SecurityClass>),
}

/// What a simulated node is.
#[derive(Debug, Clone)]
pub struct NodeSpec {
    /// A name to address it by.
    pub name: String,
    /// Always listening, or sleeping.
    pub listening: bool,
    /// Security.
    pub security: Security,
    /// Joins by SmartStart instead of a button press.
    pub smartstart: bool,
    /// A Z-Wave Long Range node (SmartStart only, 16-bit node id).
    pub long_range: bool,
    /// Generic device class.
    pub generic: u8,
    /// Specific device class.
    pub specific: u8,
    /// Command classes and their versions.
    pub ccs: Vec<(u16, u8)>,
    /// Command classes only available with security (subset of `ccs`).
    pub secure_only: Vec<u16>,
    /// Product id reported by Manufacturer Specific.
    pub product_id: u16,
    /// The node's private key (S2), fixed so tests can derive its DSK.
    pub private_key: [u8; 32],
}

impl NodeSpec {
    /// A mains-powered switch with an energy meter, no security.
    pub fn switch(name: &str) -> Self {
        NodeSpec {
            name: name.into(),
            listening: true,
            security: Security::None,
            smartstart: false,
            long_range: false,
            generic: 0x10,
            specific: 0x01,
            ccs: vec![
                (0x5e, 2),
                (0x25, 2),
                (0x32, 3),
                (0x72, 2),
                (0x86, 3),
                (0x85, 2),
                (0x6c, 1),
                (0x20, 2),
                (0x70, 4),
                (0x7a, 5),
            ],
            secure_only: vec![],
            product_id: 1,
            private_key: [0x11; 32],
        }
    }

    /// A battery multisensor that sleeps between wake-ups, with S0.
    pub fn sleeping_sensor(name: &str) -> Self {
        NodeSpec {
            name: name.into(),
            listening: false,
            security: Security::S0,
            smartstart: false,
            long_range: false,
            generic: 0x21,
            specific: 0x01,
            ccs: vec![
                (0x5e, 2),
                (0x98, 1),
                (0x31, 11),
                (0x80, 1),
                (0x84, 2),
                (0x72, 2),
                (0x86, 3),
                (0x85, 2),
            ],
            secure_only: vec![0x31, 0x80, 0x84, 0x72, 0x86, 0x85],
            product_id: 2,
            private_key: [0x22; 32],
        }
    }

    /// A mains-powered multisensor (temperature, humidity, motion) with S2.
    pub fn s2_sensor(name: &str, requested: Vec<SecurityClass>) -> Self {
        NodeSpec {
            name: name.into(),
            listening: true,
            security: Security::S2(requested),
            smartstart: false,
            long_range: false,
            generic: 0x21,
            specific: 0x01,
            ccs: vec![
                (0x5e, 2),
                (0x9f, 1),
                (0x6c, 2),
                (0x55, 2),
                (0x31, 11),
                (0x71, 8),
                (0x72, 2),
                (0x86, 3),
                (0x85, 2),
                (0x7a, 5),
            ],
            secure_only: vec![0x31, 0x71, 0x72, 0x86, 0x85, 0x7a],
            product_id: 3,
            private_key: [0x33; 32],
        }
    }

    /// An S2 node that joins by SmartStart.
    pub fn smartstart_sensor(name: &str) -> Self {
        let mut s = Self::s2_sensor(name, vec![SecurityClass::S2Authenticated]);
        s.smartstart = true;
        s.product_id = 4;
        s.private_key = [0x44; 32];
        s
    }

    /// A Long Range sensor that joins by SmartStart with S2 Authenticated.
    pub fn long_range_sensor(name: &str) -> Self {
        let mut s = Self::s2_sensor(name, vec![SecurityClass::S2Authenticated]);
        s.smartstart = true;
        s.long_range = true;
        s.product_id = 5;
        s.private_key = [0x55; 32];
        s
    }

    /// The node's DSK.
    pub fn dsk(&self) -> Dsk {
        Dsk::from_public_key(KeyPair::from_private(self.private_key).public())
    }

    fn version_of(&self, cc: u16) -> u8 {
        self.ccs
            .iter()
            .find(|(c, _)| *c == cc)
            .map(|(_, v)| *v)
            .unwrap_or(0)
    }

    /// The node information frame: insecure command classes only, as nodes send it.
    pub fn nif(&self) -> NodeInfo {
        NodeInfo {
            basic: 0x04,
            generic: self.generic,
            specific: self.specific,
            supported: self
                .ccs
                .iter()
                .map(|(c, _)| *c)
                .filter(|c| !self.secure_only.contains(c))
                .collect(),
            controlled: vec![],
        }
    }

    /// Protocol information as the controller reports it.
    pub fn protocol_info(&self) -> NodeProtocolInfo {
        NodeProtocolInfo {
            listening: self.listening,
            routing: self.listening,
            flirs_1000: false,
            flirs_250: false,
            beaming: true,
            controller: false,
            security: !matches!(self.security, Security::None),
            protocol_version: 3,
            speeds: 0b111,
            basic: 0x04,
            generic: self.generic,
            specific: self.specific,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sec {
    Plain,
    S2(KeyId),
    S0,
}

struct Kex {
    granted: Vec<SecurityClass>,
    controller_pub: Option<[u8; 32]>,
    requested_report: security_2::KexReport,
    pending: Vec<SecurityClass>,
    current: Option<SecurityClass>,
}

/// A simulated node's state.
pub struct SimNode {
    /// What it is.
    pub spec: NodeSpec,
    /// Its node id once included.
    pub id: Option<u16>,
    /// Awake (sleeping nodes).
    pub awake: bool,
    /// Classes it holds keys for after inclusion.
    pub granted: Vec<SecurityClass>,
    /// Sensor readings by type.
    pub sensors: BTreeMap<u8, f64>,
    /// Switch state.
    pub switch_on: bool,
    /// Lifeline (association group 1).
    pub lifeline: Vec<u8>,
    /// Configured wake-up interval.
    pub wake_interval: u32,
    /// The installed firmware image.
    pub firmware: Vec<u8>,
    fw_rx: Option<(u16, u16, Vec<u8>, u16)>,
    /// Configuration parameters: size and value.
    pub params: BTreeMap<u8, (u8, i32)>,
    /// Every command received, decrypted, for tests to inspect.
    pub received: Vec<Vec<u8>>,
    keypair: KeyPair,
    s2: S2Manager,
    s0: S0Manager,
    s0_keyed: bool,
    kex: Option<Kex>,
    pending_s2: Vec<(Vec<u8>, KeyId)>,
    pending_s0: Vec<Vec<u8>>,
    last_s2: Option<(Vec<u8>, KeyId)>,
}

const CTRL: u16 = 1;

impl SimNode {
    /// A node out of any network.
    pub fn new(spec: NodeSpec) -> Self {
        let keypair = KeyPair::from_private(spec.private_key);
        let mut sensors = BTreeMap::new();
        if spec.version_of(0x31) > 0 {
            sensors.insert(1, 22.5);
            sensors.insert(5, 41.0);
        }
        SimNode {
            spec,
            id: None,
            awake: true,
            granted: vec![],
            sensors,
            switch_on: false,
            lifeline: vec![],
            wake_interval: 3600,
            firmware: b"simulated firmware 1.2".to_vec(),
            fw_rx: None,
            params: BTreeMap::new(),
            received: vec![],
            keypair,
            s2: S2Manager::new(0, 0),
            s0: S0Manager::new(0, &TEMP_KEY),
            s0_keyed: false,
            kex: None,
            pending_s2: vec![],
            pending_s0: vec![],
            last_s2: None,
        }
    }

    /// Joins a network as `id`.
    pub fn join(&mut self, id: u16, home_id: u32) {
        self.id = Some(id);
        self.s2 = S2Manager::new(id, home_id);
        self.s0 = S0Manager::new(id, &TEMP_KEY);
        self.s0_keyed = false;
        self.granted.clear();
        self.lifeline.clear();
        self.awake = true;
    }

    /// Leaves the network (excluded or reset).
    pub fn leave(&mut self) {
        self.id = None;
        self.granted.clear();
        self.kex = None;
    }

    /// Whether frames reach it now.
    pub fn reachable(&self) -> bool {
        self.id.is_some() && (self.spec.listening || self.awake)
    }

    fn highest(&self) -> Sec {
        if let Some(c) = self.granted.iter().copied().filter(|c| c.is_s2()).max() {
            Sec::S2(KeyId::Class(c))
        } else if self.granted.contains(&SecurityClass::S0Legacy) {
            Sec::S0
        } else {
            Sec::Plain
        }
    }

    fn send(&mut self, plaintext: Vec<u8>, sec: Sec) -> Vec<Vec<u8>> {
        match sec {
            Sec::Plain => vec![plaintext],
            Sec::S2(key) => match self.s2.encrypt(
                CTRL,
                &plaintext,
                &EncryptOptions {
                    key: Some(key),
                    ..Default::default()
                },
            ) {
                Ok(f) => {
                    self.last_s2 = Some((plaintext, key));
                    vec![f]
                }
                Err(_) => {
                    self.pending_s2.push((plaintext, key));
                    vec![self.s2.nonce_get(CTRL).expect("rng")]
                }
            },
            Sec::S0 => {
                self.pending_s0.push(plaintext);
                if self.pending_s0.len() == 1 {
                    vec![S0Manager::nonce_get()]
                } else {
                    vec![]
                }
            }
        }
    }

    /// Something the node sends on its own (a report to its lifeline).
    pub fn unsolicited(&mut self, cmd: Command) -> Vec<Vec<u8>> {
        if self.id.is_none() {
            return vec![];
        }
        let sec = self.highest();
        self.send(cmd.encode(), sec)
    }

    /// A Wake Up Notification.
    pub fn wake(&mut self) -> Vec<Vec<u8>> {
        self.awake = true;
        self.unsolicited(wake_up::Notification {}.into())
    }

    /// Handles a frame from the controller; returns frames to send back.
    pub fn receive(&mut self, frame: &[u8]) -> Vec<Vec<u8>> {
        match (frame.first(), frame.get(1)) {
            (Some(0x9f), Some(0x01)) => vec![self.s2.nonce_report(CTRL, false).expect("rng")],
            (Some(0x9f), Some(0x02)) => {
                if self.s2.handle_nonce_report(CTRL, frame).is_err() {
                    return vec![];
                }
                let mut pending = std::mem::take(&mut self.pending_s2);
                if pending.is_empty() {
                    // Our last frame did not decrypt: send it again on the new SPAN.
                    pending.extend(self.last_s2.take());
                }
                let mut out = vec![];
                for (p, key) in pending {
                    match self.s2.encrypt(
                        CTRL,
                        &p,
                        &EncryptOptions {
                            key: Some(key),
                            ..Default::default()
                        },
                    ) {
                        Ok(f) => out.push(f),
                        Err(_) => self.pending_s2.push((p, key)),
                    }
                }
                out
            }
            (Some(0x9f), Some(0x03)) => match self.s2.decrypt(CTRL, frame) {
                Ok(d) => self.application(&d.command, Sec::S2(d.key)),
                Err(zwave_security::Error::Duplicate) => vec![],
                Err(_) => vec![self.s2.nonce_report(CTRL, false).expect("rng")],
            },
            (Some(0x98), Some(0x40)) => vec![self.s0.nonce_report(CTRL).expect("rng")],
            (Some(0x98), Some(0x80)) => {
                if self.s0.handle_nonce_report(CTRL, frame).is_err() || self.pending_s0.is_empty() {
                    return vec![];
                }
                let p = self.pending_s0.remove(0);
                let temp = !self.s0_keyed;
                let mut out = vec![self.s0.encapsulate(CTRL, &p, temp).expect("fits")];
                if !self.pending_s0.is_empty() {
                    out.push(S0Manager::nonce_get());
                }
                out
            }
            (Some(0x98), Some(0x81 | 0xc1)) => {
                let temp = !self.s0_keyed;
                match self.s0.decapsulate(CTRL, frame, temp) {
                    Ok(S0Payload::Complete(cmd)) => self.application(&cmd, Sec::S0),
                    _ => vec![],
                }
            }
            _ => self.application(frame, Sec::Plain),
        }
    }

    fn supports(&self, cc: u16, sec: Sec) -> bool {
        self.spec.version_of(cc) > 0
            && (sec != Sec::Plain
                || !self.spec.secure_only.contains(&cc)
                || self.granted.is_empty())
    }

    fn application(&mut self, frame: &[u8], sec: Sec) -> Vec<Vec<u8>> {
        self.received.push(frame.to_vec());
        if frame.starts_with(&[0x7a, 0x06]) && self.supports(0x7a, sec) {
            return self.firmware_fragment(frame, sec);
        }
        if frame.starts_with(&[0x7a, 0x03]) && self.supports(0x7a, sec) {
            if let Ok(Command::FirmwareUpdateMd(firmware_update_md::Command::RequestGet(r))) =
                Command::parse(frame)
            {
                let size = r.fragment_size.unwrap_or(32);
                self.fw_rx = Some((r.checksum, size, Vec::new(), 1));
                let mut out = self.send(
                    Command::from(firmware_update_md::RequestReport {
                        status: 0xff,
                        resume: Some(false),
                        non_secure: Some(false),
                    })
                    .encode(),
                    sec,
                );
                out.extend(
                    self.send(
                        Command::from(firmware_update_md::Get {
                            count: 2,
                            report: 1,
                        })
                        .encode(),
                        sec,
                    ),
                );
                return out;
            }
        }
        let Ok(cmd) = Command::parse(frame) else {
            return vec![];
        };
        if let Command::Supervision(supervision::Command::Get(g)) = &cmd {
            let mut out = self.application(&g.command, sec);
            let rep: Command = supervision::Report {
                more_status_updates: false,
                wake_up_request: false,
                session_id: g.session_id,
                status: 0xff,
                duration: Some(0),
            }
            .into();
            out.extend(self.send(rep.encode(), sec));
            return out;
        }
        let reply: Option<Command> = match cmd {
            Command::Security2(c) => return self.kex(c, sec),
            Command::Security(c) => return self.s0_kex(c, sec),
            _ if !self.supports(cmd.cc(), sec) => None,
            Command::Version(version::Command::Get(_)) => Some(
                version::Report {
                    library_type: 3,
                    protocol_version: 7,
                    protocol_sub_version: 18,
                    firmware_version: 1,
                    firmware_sub_version: 2,
                    hardware_version: Some(1),
                    firmware_targets: Some(0),
                    target_versions: vec![],
                }
                .into(),
            ),
            Command::Version(version::Command::CommandClassGet(g)) => Some(
                version::CommandClassReport {
                    requested_cc: g.requested_cc,
                    version: self.spec.version_of(u16::from(g.requested_cc)),
                }
                .into(),
            ),
            Command::ManufacturerSpecific(manufacturer_specific::Command::Get(_)) => Some(
                manufacturer_specific::Report {
                    manufacturer_id: 0xfffe,
                    product_type: 0x5349,
                    product_id: self.spec.product_id,
                }
                .into(),
            ),
            Command::ZwavePlusInfo(zwave_plus_info::Command::Get(_)) => Some(
                zwave_plus_info::Report {
                    zwave_plus_version: 2,
                    role_type: if self.spec.listening { 5 } else { 6 },
                    node_type: 0,
                    installer_icon: 0x0c07,
                    user_icon: 0x0c07,
                }
                .into(),
            ),
            Command::MultilevelSensor(multilevel_sensor::Command::SupportedSensorGet(_)) => Some(
                multilevel_sensor::SupportedSensorReport {
                    sensor_types: self.sensors.keys().map(|&k| u16::from(k)).collect(),
                }
                .into(),
            ),
            Command::MultilevelSensor(multilevel_sensor::Command::SupportedScaleGet(g)) => Some(
                multilevel_sensor::SupportedScaleReport {
                    sensor_type: g.sensor_type,
                    scales: 0b01,
                }
                .into(),
            ),
            Command::MultilevelSensor(multilevel_sensor::Command::Get(g)) => {
                let t = g.sensor_type.unwrap_or(1);
                self.sensors.get(&t).map(|v| {
                    multilevel_sensor::Report {
                        sensor_type: t,
                        value: Value::from_f64(*v, 1, 0),
                    }
                    .into()
                })
            }
            Command::Notification(notification::Command::SupportedGet(_)) => Some(
                notification::SupportedReport {
                    v1_alarm: false,
                    types: vec![7],
                }
                .into(),
            ),
            Command::Notification(notification::Command::EventSupportedGet(g)) => Some(
                notification::EventSupportedReport {
                    notification_type: g.notification_type,
                    events: vec![8],
                }
                .into(),
            ),
            Command::Battery(battery::Command::Get(_)) => Some(
                battery::Report {
                    level: 87,
                    ..Default::default()
                }
                .into(),
            ),
            Command::WakeUp(wake_up::Command::IntervalCapabilitiesGet(_)) => Some(
                wake_up::IntervalCapabilitiesReport {
                    minimum: 300,
                    maximum: 86_400,
                    default: 3600,
                    step: 60,
                    on_demand: Some(false),
                }
                .into(),
            ),
            Command::WakeUp(wake_up::Command::IntervalSet(s)) => {
                self.wake_interval = s.seconds;
                None
            }
            Command::WakeUp(wake_up::Command::IntervalGet(_)) => Some(
                wake_up::IntervalReport {
                    seconds: self.wake_interval,
                    node_id: CTRL as u8,
                }
                .into(),
            ),
            Command::WakeUp(wake_up::Command::NoMoreInformation(_)) => {
                self.awake = false;
                None
            }
            Command::Association(association::Command::GroupingsGet(_)) => {
                Some(association::GroupingsReport { groups: 1 }.into())
            }
            Command::Association(association::Command::Set(s)) => {
                if s.group == 1 {
                    self.lifeline = s.nodes;
                }
                None
            }
            Command::Association(association::Command::Get(g)) => Some(
                association::Report {
                    group: g.group,
                    max_nodes: 5,
                    reports_to_follow: 0,
                    nodes: self.lifeline.clone(),
                }
                .into(),
            ),
            Command::BinarySwitch(binary_switch::Command::Get(_)) => Some(self.switch_report()),
            Command::BinarySwitch(binary_switch::Command::Set(s)) => {
                self.switch_on = s.target != 0;
                None
            }
            Command::Basic(basic::Command::Set(s)) => {
                self.switch_on = s.value != 0;
                None
            }
            Command::FirmwareUpdateMd(firmware_update_md::Command::MetaDataGet(_)) => Some(
                firmware_update_md::MetaDataReport {
                    manufacturer_id: 0xfffe,
                    firmware_id: 0x0001,
                    checksum: zwave_cc::crc16::checksum(&self.firmware),
                    upgradable: Some(0xff),
                    additional_targets: Some(0),
                    max_fragment_size: Some(36),
                    rest: vec![],
                }
                .into(),
            ),
            Command::Configuration(configuration::Command::Set(s)) => {
                self.params.insert(s.parameter, (s.size, s.value));
                None
            }
            Command::Configuration(configuration::Command::Get(g)) => {
                let (size, value) = self.params.get(&g.parameter).copied().unwrap_or((1, 0));
                Some(
                    configuration::Report {
                        parameter: g.parameter,
                        size,
                        value,
                    }
                    .into(),
                )
            }
            Command::Basic(basic::Command::Get(_)) => Some(
                basic::Report {
                    current: if self.switch_on { 0xff } else { 0 },
                    target: None,
                    duration: None,
                }
                .into(),
            ),
            Command::Meter(meter::Command::SupportedGet(_)) => Some(
                meter::SupportedReport {
                    meter_reset: true,
                    rate_type: 1,
                    meter_type: 1,
                    more_scale_types: false,
                    scales: 0b101,
                    extra_scales: vec![],
                }
                .into(),
            ),
            Command::Meter(meter::Command::Get(g)) => {
                let scale = g.scale.unwrap_or(0);
                let v = if scale == 2 { 12.5 } else { 1.234 };
                Some(
                    meter::Report {
                        scale_bit2: scale & 4 != 0,
                        rate_type: 1,
                        meter_type: 1,
                        value: Value {
                            scale: scale & 3,
                            ..Value::from_f64(v, 3, 0)
                        },
                        delta_time: Some(0),
                        previous_value: None,
                        scale2: None,
                    }
                    .into(),
                )
            }
            _ => None,
        };
        match reply {
            Some(r) => self.send(r.encode(), sec),
            None => vec![],
        }
    }

    fn firmware_fragment(&mut self, frame: &[u8], sec: Sec) -> Vec<Vec<u8>> {
        let Some((checksum, _size, data, next)) = self.fw_rx.as_mut() else {
            return vec![];
        };
        if frame.len() < 6 {
            return vec![];
        }
        let (body, crc) = frame.split_at(frame.len() - 2);
        if zwave_cc::crc16::checksum(body) != u16::from_be_bytes([crc[0], crc[1]]) {
            // A corrupt fragment: ask for it again.
            let again = *next;
            return self.send(
                Command::from(firmware_update_md::Get {
                    count: 1,
                    report: again,
                })
                .encode(),
                sec,
            );
        }
        let header = u16::from_be_bytes([body[2], body[3]]);
        let (number, last) = (header & 0x7fff, header & 0x8000 != 0);
        if number != *next {
            return vec![];
        }
        data.extend_from_slice(&body[4..]);
        *next += 1;
        if last {
            let ok = zwave_cc::crc16::checksum(data) == *checksum;
            let image = std::mem::take(data);
            self.fw_rx = None;
            if ok {
                self.firmware = image;
            }
            let status = if ok { 0xff } else { 0x00 };
            return self.send(
                Command::from(firmware_update_md::StatusReport {
                    status,
                    wait_time: Some(5),
                })
                .encode(),
                sec,
            );
        }
        if (*next - 1) % 2 == 0 {
            let n = *next;
            return self.send(
                Command::from(firmware_update_md::Get {
                    count: 2,
                    report: n,
                })
                .encode(),
                sec,
            );
        }
        vec![]
    }

    fn switch_report(&self) -> Command {
        let v = if self.switch_on { 0xff } else { 0 };
        binary_switch::Report {
            current: v,
            target: Some(v),
            duration: Some(0),
        }
        .into()
    }

    /// A multilevel sensor report for `sensor_type`, as the node would send it.
    pub fn sensor_report(&self, sensor_type: u8) -> Option<Command> {
        self.sensors.get(&sensor_type).map(|v| {
            multilevel_sensor::Report {
                sensor_type,
                value: Value::from_f64(*v, 1, 0),
            }
            .into()
        })
    }

    fn kex(&mut self, c: security_2::Command, sec: Sec) -> Vec<Vec<u8>> {
        let Security::S2(requested) = self.spec.security.clone() else {
            return vec![];
        };
        match c {
            security_2::Command::KexGet(_) if sec == Sec::Plain => {
                let report = security_2::KexReport {
                    request_csa: false,
                    echo: false,
                    schemes: KEX_SCHEME_1,
                    curves: ECDH_CURVE25519,
                    keys: SecurityClass::to_key_bits(&requested),
                };
                self.kex = Some(Kex {
                    granted: vec![],
                    controller_pub: None,
                    requested_report: report.clone(),
                    pending: vec![],
                    current: None,
                });
                self.send(Command::from(report).encode(), Sec::Plain)
            }
            security_2::Command::KexSet(s) if !s.echo && sec == Sec::Plain => {
                let Some(k) = self.kex.as_mut() else {
                    return vec![];
                };
                k.granted = SecurityClass::from_key_bits(s.keys);
                // Lowest first, as nodes ask for them.
                let mut pending = k.granted.clone();
                pending.reverse();
                k.pending = pending;
                let mut key = self.keypair.public().to_vec();
                if k.granted.iter().any(|c| c.needs_pin()) {
                    key[0] = 0;
                    key[1] = 0;
                }
                self.send(
                    Command::from(security_2::PublicKeyReport {
                        including_node: false,
                        public_key: key,
                    })
                    .encode(),
                    Sec::Plain,
                )
            }
            security_2::Command::PublicKeyReport(p) if p.including_node && sec == Sec::Plain => {
                let Some(k) = self.kex.as_mut() else {
                    return vec![];
                };
                let Ok(cpub) = <[u8; 32]>::try_from(p.public_key.as_slice()) else {
                    return vec![];
                };
                k.controller_pub = Some(cpub);
                let granted = SecurityClass::to_key_bits(&k.granted);
                let Ok(shared) = self.keypair.shared_secret(&cpub) else {
                    return vec![];
                };
                let tk = bootstrap_keys(&shared, &cpub, self.keypair.public());
                self.s2.begin_bootstrap(CTRL, tk);
                let echo = security_2::KexSet {
                    request_csa: false,
                    echo: true,
                    scheme: KEX_SCHEME_1,
                    curve: ECDH_CURVE25519,
                    keys: granted,
                };
                self.send(Command::from(echo).encode(), Sec::S2(KeyId::Temporary))
            }
            security_2::Command::KexReport(r) if r.echo && sec == Sec::S2(KeyId::Temporary) => {
                let Some(k) = self.kex.as_mut() else {
                    return vec![];
                };
                if r.keys != k.requested_report.keys {
                    return vec![];
                }
                self.next_key_get()
            }
            security_2::Command::NetworkKeyReport(r) if sec == Sec::S2(KeyId::Temporary) => {
                let Some(k) = self.kex.as_mut() else {
                    return vec![];
                };
                let Some(class) = SecurityClass::from_key_bit(r.granted_key) else {
                    return vec![];
                };
                let Ok(key) = <[u8; 16]>::try_from(r.network_key.as_slice()) else {
                    return vec![];
                };
                k.current = Some(class);
                self.s2.set_network_key(class, &key);
                if class == SecurityClass::S0Legacy {
                    self.s0.set_network_key(&key);
                    self.s0_keyed = true;
                }
                self.s2.set_peer_classes(CTRL, &[class]);
                self.send(
                    Command::from(security_2::NetworkKeyVerify {}).encode(),
                    Sec::S2(KeyId::Class(class)),
                )
            }
            security_2::Command::TransferEnd(t)
                if t.key_verified && sec == Sec::S2(KeyId::Temporary) =>
            {
                let Some(k) = self.kex.as_mut() else {
                    return vec![];
                };
                if let Some(c) = k.current.take() {
                    self.granted.push(c);
                }
                if k.pending.is_empty() {
                    let out = self.send(
                        Command::from(security_2::TransferEnd {
                            key_verified: false,
                            key_request_complete: true,
                        })
                        .encode(),
                        Sec::S2(KeyId::Temporary),
                    );
                    self.s2.end_bootstrap(CTRL);
                    self.s2.set_peer_classes(CTRL, &self.granted.clone());
                    self.kex = None;
                    out
                } else {
                    self.next_key_get()
                }
            }
            security_2::Command::KexFail(_) => {
                self.kex = None;
                self.s2.end_bootstrap(CTRL);
                vec![]
            }
            security_2::Command::CommandsSupportedGet(_)
                if matches!(sec, Sec::S2(KeyId::Class(_))) =>
            {
                let list = self.spec.secure_only.clone();
                self.send(
                    Command::from(security_2::CommandsSupportedReport {
                        command_classes: list,
                    })
                    .encode(),
                    sec,
                )
            }
            _ => vec![],
        }
    }

    fn next_key_get(&mut self) -> Vec<Vec<u8>> {
        let Some(k) = self.kex.as_mut() else {
            return vec![];
        };
        let Some(class) = k.pending.first().copied() else {
            return vec![];
        };
        k.pending.remove(0);
        self.send(
            Command::from(security_2::NetworkKeyGet {
                requested_key: class.key_bit(),
            })
            .encode(),
            Sec::S2(KeyId::Temporary),
        )
    }

    fn s0_kex(&mut self, c: security::Command, sec: Sec) -> Vec<Vec<u8>> {
        match c {
            security::Command::SchemeGet(_)
                if sec == Sec::Plain && self.spec.security == Security::S0 =>
            {
                self.send(
                    Command::from(security::SchemeReport { schemes: 0 }).encode(),
                    Sec::Plain,
                )
            }
            security::Command::NetworkKeySet(s) if sec == Sec::S0 && !self.s0_keyed => {
                let Ok(key) = <[u8; 16]>::try_from(s.network_key.as_slice()) else {
                    return vec![];
                };
                self.s0.set_network_key(&key);
                self.s0_keyed = true;
                self.granted = vec![SecurityClass::S0Legacy];
                self.send(
                    Command::from(security::NetworkKeyVerify {}).encode(),
                    Sec::S0,
                )
            }
            security::Command::CommandsSupportedGet(_) if sec == Sec::S0 => {
                let list = self.spec.secure_only.clone();
                self.send(
                    Command::from(security::CommandsSupportedReport {
                        reports_to_follow: 0,
                        command_classes: list,
                    })
                    .encode(),
                    Sec::S0,
                )
            }
            _ => vec![],
        }
    }
}
