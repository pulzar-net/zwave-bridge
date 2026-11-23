//! Home Assistant MQTT discovery configs for values.

use crate::api::{DiscoveryConfig, DiscoveryDevice};
use zwave_host::{NodeRecord, ValueKind, ValueUpdate};

/// A discovery config: the topic to publish it on and its body.
pub struct Discovery {
    /// `<prefix>/<component>/<node id>/<object id>/config`.
    pub topic: String,
    /// The config.
    pub config: DiscoveryConfig,
}
fn object_id(key: &str) -> String {
    key.replace('/', "_")
}

fn sensor_class(property: &str) -> Option<&'static str> {
    let p = property;
    Some(match p {
        _ if p.ends_with("temperature") => "temperature",
        "humidity" => "humidity",
        "illuminance" => "illuminance",
        "power" => "power",
        "voltage" => "voltage",
        "current" => "current",
        "carbon_dioxide_co2_level" => "carbon_dioxide",
        "carbon_monoxide_co_level" => "carbon_monoxide",
        "atmospheric_pressure" | "barometric_pressure" => "pressure",
        "ultraviolet" => "uv_index",
        "particulate_matter_2_5" => "pm25",
        "particulate_matter_10" => "pm10",
        "volatile_organic_compound_level" => "volatile_organic_compounds",
        _ => return None,
    })
}

fn binary_class(property: &str) -> Option<&'static str> {
    let p = property;
    Some(match p {
        _ if p.contains("motion") => "motion",
        _ if p.contains("door") || p.contains("window") || p == "access_control_door_state" => {
            "door"
        }
        _ if p.contains("water") || p.contains("leak") => "moisture",
        _ if p.starts_with("smoke") => "smoke",
        _ if p.starts_with("co_alarm") || p == "co" => "carbon_monoxide",
        _ if p.contains("tamper") || p.contains("cover") => "tamper",
        _ if p.starts_with("heat") => "heat",
        "low" => "battery",
        _ => return None,
    })
}

/// The Home Assistant unit for a Z-Wave unit symbol.
fn ha_unit(unit: &str) -> &str {
    match unit {
        "Lux" => "lx",
        other => other,
    }
}

/// The discovery config for a value, or `None` when it should not appear in Home Assistant.
pub fn for_value(
    prefix: &str,
    base: &str,
    home_id: u32,
    node: &NodeRecord,
    device_cfg: Option<&zwave_host::device_config::DeviceConfig>,
    v: &ValueUpdate,
) -> Option<Discovery> {
    let key = v.id.key();
    let state_topic = format!("{base}/nodes/{}/{key}", v.id.node);
    let device_id = format!("zwave_{home_id:08x}_{}", v.id.node);
    let unique_id = format!("{device_id}_{}", object_id(&key));
    let mut device = DiscoveryDevice {
        identifiers: vec![device_id.clone()],
        name: node
            .name
            .clone()
            .unwrap_or_else(|| format!("Z-Wave node {}", v.id.node)),
        via_device: format!("zwave_{home_id:08x}_controller"),
        ..Default::default()
    };
    if let Some(m) = node.manufacturer {
        device.manufacturer = Some(format!("{:#06x}", m.manufacturer_id));
        device.model = Some(format!("{:#06x}:{:#06x}", m.product_type, m.product_id));
    }
    if let Some(d) = device_cfg {
        if node.name.is_none() {
            device.name = format!("{} {}", d.manufacturer, d.label).trim().to_string();
        }
        device.manufacturer = Some(d.manufacturer.clone());
        device.model = Some(d.label.clone());
    }
    device.sw_version = node.firmware.clone();
    device.suggested_area = node.location.clone();
    let mut c = DiscoveryConfig {
        name: v.label.clone(),
        unique_id: unique_id.clone(),
        object_id: unique_id,
        state_topic: state_topic.clone(),
        availability_topic: format!("{base}/bridge/state"),
        device,
        ..Default::default()
    };
    let s = |t: &str| Some(t.to_string());
    let set = format!("{state_topic}/set");
    let component = match (v.id.cc, &v.value) {
        (0x25, _) => {
            c.command_topic = Some(set);
            c.value_template = s("{{ 'ON' if value_json.value else 'OFF' }}");
            c.payload_on = s("ON");
            c.payload_off = s("OFF");
            "switch"
        }
        (0x26, _) => {
            c.command_topic = Some(set.clone());
            c.state_value_template = s("{{ 'ON' if value_json.value > 0 else 'OFF' }}");
            c.brightness_state_topic = Some(state_topic);
            c.brightness_value_template = s("{{ [value_json.value, 99] | min }}");
            c.brightness_command_topic = Some(set);
            c.brightness_scale = Some(99);
            c.on_command_type = s("brightness");
            c.payload_on = s("ON");
            c.payload_off = s("OFF");
            "light"
        }
        (0x20, _) | (0x70, _) => return None,
        (_, ValueKind::Bool(_)) => {
            c.value_template = s("{{ 'ON' if value_json.value else 'OFF' }}");
            c.device_class = binary_class(&v.id.property).map(String::from);
            if v.id.cc == 0x80 {
                c.entity_category = s("diagnostic");
            }
            "binary_sensor"
        }
        (0x71, ValueKind::State { .. }) => match binary_class(&v.id.property) {
            Some(dc) => {
                c.value_template = s("{{ 'ON' if value_json.value != 0 else 'OFF' }}");
                c.device_class = s(dc);
                "binary_sensor"
            }
            None => {
                c.value_template = s("{{ value_json.state }}");
                "sensor"
            }
        },
        (_, ValueKind::Number(_) | ValueKind::Level(_)) => {
            c.value_template = s("{{ value_json.value }}");
            c.unit_of_measurement = v.unit.map(|u| ha_unit(u).to_string());
            if v.id.cc == 0x32 {
                c.device_class = match v.unit {
                    Some("kWh") => {
                        c.state_class = s("total_increasing");
                        s("energy")
                    }
                    Some("W") => s("power"),
                    Some("V") => s("voltage"),
                    Some("A") => s("current"),
                    Some("m³") => s("gas"),
                    _ => None,
                };
                if c.state_class.is_none() {
                    c.state_class = s("measurement");
                }
            } else if v.id.cc == 0x80 {
                c.device_class = s("battery");
                c.entity_category = s("diagnostic");
                c.state_class = s("measurement");
            } else {
                c.device_class = sensor_class(&v.id.property).map(String::from);
                c.state_class = s("measurement");
            }
            "sensor"
        }
        _ => return None,
    };
    Some(Discovery {
        topic: format!(
            "{prefix}/{component}/{device_id}/{}/config",
            object_id(&key)
        ),
        config: c,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zwave_host::ValueId;

    fn upd(cc: u16, property: &str, value: ValueKind, unit: Option<&'static str>) -> ValueUpdate {
        ValueUpdate {
            id: ValueId {
                node: 4,
                endpoint: 0,
                cc,
                property: property.into(),
            },
            value,
            unit,
            label: "L".into(),
            writable: false,
            rssi: None,
        }
    }

    #[test]
    fn components() {
        let n = NodeRecord::new(4);
        let d = for_value(
            "homeassistant",
            "zwave",
            0xc0ffee01,
            &n,
            None,
            &upd(0x31, "air_temperature", ValueKind::Number(1.0), Some("°C")),
        )
        .unwrap();
        assert_eq!(
            d.topic,
            "homeassistant/sensor/zwave_c0ffee01_4/multilevel_sensor_air_temperature/config"
        );
        assert_eq!(d.config.device_class.as_deref(), Some("temperature"));
        assert_eq!(
            d.config.state_topic,
            "zwave/nodes/4/multilevel_sensor/air_temperature"
        );
        let d = for_value(
            "ha",
            "zwave",
            1,
            &n,
            None,
            &upd(
                0x71,
                "home_security_motion_sensor_status",
                ValueKind::State {
                    id: 8,
                    label: "x".into(),
                },
                None,
            ),
        )
        .unwrap();
        assert!(d.topic.starts_with("ha/binary_sensor/"));
        assert_eq!(d.config.device_class.as_deref(), Some("motion"));
        let d = for_value(
            "ha",
            "zwave",
            1,
            &n,
            None,
            &upd(0x25, "current", ValueKind::Bool(true), None),
        )
        .unwrap();
        assert_eq!(
            d.config.command_topic.as_deref(),
            Some("zwave/nodes/4/binary_switch/current/set")
        );
        let d = for_value(
            "ha",
            "zwave",
            1,
            &n,
            None,
            &upd(0x32, "electric_kwh", ValueKind::Number(1.0), Some("kWh")),
        )
        .unwrap();
        assert_eq!(d.config.state_class.as_deref(), Some("total_increasing"));
        assert!(for_value(
            "ha",
            "zwave",
            1,
            &n,
            None,
            &upd(0x70, "parameter_1", ValueKind::Number(1.0), None)
        )
        .is_none());
    }
}
