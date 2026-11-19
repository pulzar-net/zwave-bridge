//! Values: what reports mean, as numbers with units, independent of the
//! command class that carried them.

use crate::NodeId;
use zwave_cc::{basic, battery, binary_sensor, binary_switch, configuration, meter, multilevel_sensor, multilevel_switch, notification, tables, Command};

/// Identifies one value of one node.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ValueId {
    /// The node.
    pub node: NodeId,
    /// The endpoint (0 is the root device).
    pub endpoint: u8,
    /// The command class it belongs to.
    pub cc: u16,
    /// A stable snake-case property: `air_temperature`, `electric_kwh`,
    /// `home_security_motion_sensor_status`, `level`, `current`.
    pub property: String,
}

impl ValueId {
    /// `<cc name>/<property>`, or with an endpoint `<cc name>/<property>/<endpoint>`.
    pub fn key(&self) -> String {
        let cc = zwave_cc::cc_info(self.cc).map(|c| c.name.to_string()).unwrap_or_else(|| format!("{:#06x}", self.cc));
        if self.endpoint == 0 {
            format!("{cc}/{}", self.property)
        } else {
            format!("{cc}/{}/{}", self.property, self.endpoint)
        }
    }
}

/// A value.
#[derive(Debug, Clone, PartialEq)]
pub enum ValueKind {
    /// On/off, open/closed, detected/idle.
    Bool(bool),
    /// A measurement.
    Number(f64),
    /// A level, 0 to 99 (0xFF for "on at the last level").
    Level(u8),
    /// A notification state or event: its id and label.
    State {
        /// Event id (0 is idle).
        id: u8,
        /// Its label.
        label: String,
    },
}

/// A value as reported.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueUpdate {
    /// Which value.
    pub id: ValueId,
    /// The value.
    pub value: ValueKind,
    /// The unit symbol, if any.
    pub unit: Option<&'static str>,
    /// A label for people.
    pub label: String,
    /// Whether the value can be set (switches, levels).
    pub writable: bool,
}

fn snake(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

fn unit_key(unit: &str, label: &str) -> String {
    let s = snake(if unit.is_empty() { label } else { unit });
    if s.is_empty() {
        "value".into()
    } else {
        s
    }
}

/// The values a command carries. Commands that carry none give an empty list.
pub fn from_command(node: NodeId, endpoint: u8, cmd: &Command) -> Vec<ValueUpdate> {
    let id = |cc: u16, property: &str| ValueId { node, endpoint, cc, property: property.to_string() };
    let mut out = Vec::new();
    match cmd {
        Command::Basic(basic::Command::Report(r)) if r.current <= 99 || r.current == 0xff => out.push(ValueUpdate {
            id: id(basic::ID, "current"),
            value: ValueKind::Level(r.current),
            unit: None,
            label: "Basic".into(),
            writable: true,
        }),
        Command::BinarySwitch(binary_switch::Command::Report(r)) if r.current != 0xfe => out.push(ValueUpdate {
            id: id(binary_switch::ID, "current"),
            value: ValueKind::Bool(r.current != 0),
            unit: None,
            label: "Switch".into(),
            writable: true,
        }),
        Command::MultilevelSwitch(multilevel_switch::Command::Report(r)) if r.current != 0xfe => out.push(ValueUpdate {
            id: id(multilevel_switch::ID, "current"),
            value: ValueKind::Level(r.current),
            unit: None,
            label: "Level".into(),
            writable: true,
        }),
        Command::MultilevelSensor(multilevel_sensor::Command::Report(r)) => {
            let t = tables::sensor_type(r.sensor_type);
            let scale = t.and_then(|t| t.scale(r.value.scale));
            let name = t.map(|t| t.name.to_string()).unwrap_or_else(|| format!("sensor_{}", r.sensor_type));
            out.push(ValueUpdate {
                id: id(multilevel_sensor::ID, &name),
                value: ValueKind::Number(r.value.to_f64()),
                unit: scale.map(|s| s.unit).filter(|u| !u.is_empty()),
                label: t.map(|t| t.label.to_string()).unwrap_or(name),
                writable: false,
            });
        }
        Command::Meter(meter::Command::Report(r)) => {
            let mut scale = r.value.scale | if r.scale_bit2 { 0x04 } else { 0 };
            if scale == 7 {
                if let Some(s2) = r.scale2 {
                    scale = 8 + s2;
                }
            }
            let t = tables::meter_type(r.meter_type);
            let s = t.and_then(|t| t.scale(scale));
            let tname = t.map(|t| t.name.to_string()).unwrap_or_else(|| format!("meter_{}", r.meter_type));
            let skey = s.map(|s| unit_key(s.unit, s.label)).unwrap_or_else(|| format!("scale_{scale}"));
            out.push(ValueUpdate {
                id: id(meter::ID, &format!("{tname}_{skey}")),
                value: ValueKind::Number(r.value.to_f64()),
                unit: s.map(|s| s.unit).filter(|u| !u.is_empty()),
                label: format!("{} {}", t.map(|t| t.label).unwrap_or("Meter"), s.map(|s| s.label).unwrap_or("")).trim().to_string(),
                writable: false,
            });
        }
        Command::Notification(notification::Command::Report(r)) => {
            if let (Some(ty), Some(event)) = (r.notification_type, r.event) {
                let t = tables::notification_type(ty);
                let tname = t.map(|t| t.name.to_string()).unwrap_or_else(|| format!("notification_{ty}"));
                let ev = t.and_then(|t| t.event(event));
                // Idle (event 0) clears the variable named in the first parameter byte.
                let variable = if event == 0 {
                    r.params.first().and_then(|&p| t.and_then(|t| t.event(p))).map(|e| e.variable)
                } else {
                    ev.map(|e| e.variable)
                };
                let (prop, label) = match variable {
                    Some(v) if !v.is_empty() => (format!("{tname}_{}", snake(v)), format!("{} {}", t.map(|t| t.label).unwrap_or(""), v)),
                    _ => (format!("{tname}_event"), format!("{} event", t.map(|t| t.label).unwrap_or("Notification"))),
                };
                out.push(ValueUpdate {
                    id: id(notification::ID, &prop),
                    value: ValueKind::State {
                        id: event,
                        label: if event == 0 { "Idle".into() } else { ev.map(|e| e.label.to_string()).unwrap_or_else(|| format!("Event {event}")) },
                    },
                    unit: None,
                    label: label.trim().to_string(),
                    writable: false,
                });
            } else {
                out.push(ValueUpdate {
                    id: id(notification::ID, &format!("alarm_{}", r.v1_alarm_type)),
                    value: ValueKind::Number(f64::from(r.v1_alarm_level)),
                    unit: None,
                    label: format!("Alarm {}", r.v1_alarm_type),
                    writable: false,
                });
            }
        }
        Command::Battery(battery::Command::Report(r)) => {
            out.push(ValueUpdate {
                id: id(battery::ID, "level"),
                value: ValueKind::Number(if r.level == 0xff { 0.0 } else { f64::from(r.level.min(100)) }),
                unit: Some("%"),
                label: "Battery".into(),
                writable: false,
            });
            out.push(ValueUpdate {
                id: id(battery::ID, "low"),
                value: ValueKind::Bool(r.level == 0xff),
                unit: None,
                label: "Battery low".into(),
                writable: false,
            });
        }
        Command::BinarySensor(binary_sensor::Command::Report(r)) => {
            let name = match r.sensor_type {
                Some(1) => "general_purpose",
                Some(2) => "smoke",
                Some(3) => "co",
                Some(4) => "co2",
                Some(5) => "heat",
                Some(6) => "water",
                Some(7) => "freeze",
                Some(8) => "tamper",
                Some(9) => "aux",
                Some(10) => "door_window",
                Some(11) => "tilt",
                Some(12) => "motion",
                Some(13) => "glass_break",
                _ => "any",
            };
            out.push(ValueUpdate {
                id: id(binary_sensor::ID, name),
                value: ValueKind::Bool(r.value != 0),
                unit: None,
                label: format!("Binary sensor {}", name.replace('_', " ")),
                writable: false,
            });
        }
        Command::Configuration(configuration::Command::Report(r)) => out.push(ValueUpdate {
            id: id(configuration::ID, &format!("parameter_{}", r.parameter)),
            value: ValueKind::Number(f64::from(r.value)),
            unit: None,
            label: format!("Parameter {}", r.parameter),
            writable: true,
        }),
        _ => {}
    }
    out
}

/// The command that sets a writable value, if there is one.
pub fn set_command(id: &ValueId, value: &ValueKind) -> Option<Command> {
    let level = |v: &ValueKind| -> Option<u8> {
        Some(match v {
            ValueKind::Bool(b) => if *b { 0xff } else { 0 },
            ValueKind::Level(l) => *l,
            ValueKind::Number(n) if (0.0..=99.0).contains(n) => *n as u8,
            ValueKind::Number(n) if *n == 255.0 => 0xff,
            _ => return None,
        })
    };
    match (id.cc, id.property.as_str()) {
        (basic::ID, "current") => Some(basic::Set { value: level(value)? }.into()),
        (binary_switch::ID, "current") => Some(binary_switch::Set { target: level(value)?.min(1) * 0xff, duration: None }.into()),
        (multilevel_switch::ID, "current") => Some(multilevel_switch::Set { value: level(value)?, duration: None }.into()),
        _ => None,
    }
}

/// The Get that refreshes a value after it was set.
pub fn get_command(id: &ValueId) -> Option<Command> {
    match id.cc {
        basic::ID => Some(basic::Get {}.into()),
        binary_switch::ID => Some(binary_switch::Get {}.into()),
        multilevel_switch::ID => Some(multilevel_switch::Get {}.into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensor_and_meter_values() {
        let c = Command::parse(&[0x31, 0x05, 0x01, 0x22, 0x00, 0xe1]).unwrap();
        let v = from_command(4, 0, &c);
        assert_eq!(v[0].id.property, "air_temperature");
        assert_eq!(v[0].unit, Some("°C"));
        assert_eq!(v[0].value, ValueKind::Number(22.5));
        assert_eq!(v[0].id.key(), "multilevel_sensor/air_temperature");
        let c = Command::parse(&[0x32, 0x02, 0x21, 0x62, 0x04, 0xd2]).unwrap();
        assert_eq!(from_command(4, 2, &c)[0].id.key(), "meter/electric_kwh/2");
    }

    #[test]
    fn notification_states() {
        let motion = Command::parse(&[0x71, 0x05, 0, 0, 0, 0xff, 0x07, 0x08, 0x00]).unwrap();
        let v = &from_command(3, 0, &motion)[0];
        assert_eq!(v.id.property, "home_security_motion_sensor_status");
        assert_eq!(v.value, ValueKind::State { id: 8, label: "Motion detection".into() });
        let idle = Command::parse(&[0x71, 0x05, 0, 0, 0, 0xff, 0x07, 0x00, 0x01, 0x08]).unwrap();
        let v = &from_command(3, 0, &idle)[0];
        assert_eq!(v.id.property, "home_security_motion_sensor_status");
        assert!(matches!(v.value, ValueKind::State { id: 0, .. }));
    }

    #[test]
    fn set_commands() {
        let id = ValueId { node: 2, endpoint: 0, cc: 0x25, property: "current".into() };
        assert_eq!(set_command(&id, &ValueKind::Bool(true)).unwrap().encode(), vec![0x25, 0x01, 0xff]);
        let id = ValueId { node: 2, endpoint: 0, cc: 0x26, property: "current".into() };
        assert_eq!(set_command(&id, &ValueKind::Number(42.0)).unwrap().encode(), vec![0x26, 0x01, 42]);
        let id = ValueId { node: 2, endpoint: 0, cc: 0x31, property: "air_temperature".into() };
        assert!(set_command(&id, &ValueKind::Number(1.0)).is_none());
    }
}
