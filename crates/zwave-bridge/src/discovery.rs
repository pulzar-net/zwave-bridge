//! Home Assistant MQTT discovery configs for values.

use serde_json::{json, Value as Json};
use zwave_host::{NodeRecord, ValueKind, ValueUpdate};

/// A discovery config: the topic to publish it on and its body.
pub struct Discovery {
    /// `<prefix>/<component>/<node id>/<object id>/config`.
    pub topic: String,
    /// The config.
    pub config: Json,
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
    v: &ValueUpdate,
) -> Option<Discovery> {
    let key = v.id.key();
    let state_topic = format!("{base}/nodes/{}/{key}", v.id.node);
    let device_id = format!("zwave_{home_id:08x}_{}", v.id.node);
    let unique_id = format!("{device_id}_{}", object_id(&key));
    let mut device = json!({
        "identifiers": [device_id],
        "name": node.name.clone().unwrap_or_else(|| format!("Z-Wave node {}", v.id.node)),
        "via_device": format!("zwave_{home_id:08x}_controller"),
    });
    if let Some(m) = node.manufacturer {
        device["manufacturer"] = json!(format!("{:#06x}", m.manufacturer_id));
        device["model"] = json!(format!("{:#06x}:{:#06x}", m.product_type, m.product_id));
    }
    if let Some(fw) = &node.firmware {
        device["sw_version"] = json!(fw);
    }
    if let Some(loc) = &node.location {
        device["suggested_area"] = json!(loc);
    }
    let mut c = json!({
        "name": v.label,
        "unique_id": unique_id,
        "object_id": unique_id,
        "state_topic": state_topic,
        "availability_topic": format!("{base}/bridge/state"),
        "device": device,
    });
    let set = format!("{state_topic}/set");
    let component = match (v.id.cc, &v.value) {
        (0x25, _) => {
            c["command_topic"] = json!(set);
            c["value_template"] = json!("{{ 'ON' if value_json.value else 'OFF' }}");
            c["payload_on"] = json!("ON");
            c["payload_off"] = json!("OFF");
            "switch"
        }
        (0x26, _) => {
            c["command_topic"] = json!(set);
            c["state_value_template"] = json!("{{ 'ON' if value_json.value > 0 else 'OFF' }}");
            c["brightness_state_topic"] = json!(state_topic);
            c["brightness_value_template"] = json!("{{ [value_json.value, 99] | min }}");
            c["brightness_command_topic"] = json!(set);
            c["brightness_scale"] = json!(99);
            c["on_command_type"] = json!("brightness");
            c["payload_on"] = json!("ON");
            c["payload_off"] = json!("OFF");
            "light"
        }
        (0x20, _) | (0x70, _) => return None,
        (_, ValueKind::Bool(_)) => {
            c["value_template"] = json!("{{ 'ON' if value_json.value else 'OFF' }}");
            if let Some(dc) = binary_class(&v.id.property) {
                c["device_class"] = json!(dc);
            }
            if v.id.cc == 0x80 {
                c["entity_category"] = json!("diagnostic");
            }
            "binary_sensor"
        }
        (0x71, ValueKind::State { .. }) => match binary_class(&v.id.property) {
            Some(dc) => {
                c["value_template"] = json!("{{ 'ON' if value_json.value != 0 else 'OFF' }}");
                c["device_class"] = json!(dc);
                "binary_sensor"
            }
            None => {
                c["value_template"] = json!("{{ value_json.label }}");
                "sensor"
            }
        },
        (_, ValueKind::Number(_) | ValueKind::Level(_)) => {
            c["value_template"] = json!("{{ value_json.value }}");
            if let Some(u) = v.unit {
                c["unit_of_measurement"] = json!(ha_unit(u));
            }
            if v.id.cc == 0x32 {
                match v.unit {
                    Some("kWh") => {
                        c["device_class"] = json!("energy");
                        c["state_class"] = json!("total_increasing");
                    }
                    Some("W") => c["device_class"] = json!("power"),
                    Some("V") => c["device_class"] = json!("voltage"),
                    Some("A") => c["device_class"] = json!("current"),
                    Some("m³") => c["device_class"] = json!("gas"),
                    _ => {}
                }
                if c.get("state_class").is_none() {
                    c["state_class"] = json!("measurement");
                }
            } else if v.id.cc == 0x80 {
                c["device_class"] = json!("battery");
                c["entity_category"] = json!("diagnostic");
                c["state_class"] = json!("measurement");
            } else {
                if let Some(dc) = sensor_class(&v.id.property) {
                    c["device_class"] = json!(dc);
                }
                c["state_class"] = json!("measurement");
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
            &upd(0x31, "air_temperature", ValueKind::Number(1.0), Some("°C")),
        )
        .unwrap();
        assert_eq!(
            d.topic,
            "homeassistant/sensor/zwave_c0ffee01_4/multilevel_sensor_air_temperature/config"
        );
        assert_eq!(d.config["device_class"], "temperature");
        assert_eq!(
            d.config["state_topic"],
            "zwave/nodes/4/multilevel_sensor/air_temperature"
        );
        let d = for_value(
            "ha",
            "zwave",
            1,
            &n,
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
        assert_eq!(d.config["device_class"], "motion");
        let d = for_value(
            "ha",
            "zwave",
            1,
            &n,
            &upd(0x25, "current", ValueKind::Bool(true), None),
        )
        .unwrap();
        assert_eq!(
            d.config["command_topic"],
            "zwave/nodes/4/binary_switch/current/set"
        );
        let d = for_value(
            "ha",
            "zwave",
            1,
            &n,
            &upd(0x32, "electric_kwh", ValueKind::Number(1.0), Some("kWh")),
        )
        .unwrap();
        assert_eq!(d.config["state_class"], "total_increasing");
        assert!(for_value(
            "ha",
            "zwave",
            1,
            &n,
            &upd(0x70, "parameter_1", ValueKind::Number(1.0), None)
        )
        .is_none());
    }
}
