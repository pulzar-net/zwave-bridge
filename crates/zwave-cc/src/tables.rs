//! The multilevel sensor, meter and notification types assigned by the
//! Z-Wave Alliance, generated from `defs/tables/*.toml`.

/// A scale (unit) of a sensor or meter type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scale {
    /// Scale id.
    pub id: u8,
    /// Name.
    pub label: &'static str,
    /// Unit symbol (empty when unitless).
    pub unit: &'static str,
}

/// A multilevel sensor type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensorType {
    /// Sensor type id.
    pub id: u8,
    /// Snake-case name, e.g. `air_temperature`.
    pub name: &'static str,
    /// Label.
    pub label: &'static str,
    /// Scales.
    pub scales: &'static [Scale],
}

/// A meter type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeterType {
    /// Meter type id.
    pub id: u8,
    /// Snake-case name, e.g. `electric`.
    pub name: &'static str,
    /// Label.
    pub label: &'static str,
    /// Scales.
    pub scales: &'static [Scale],
}

/// A notification type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationType {
    /// Notification type id.
    pub id: u8,
    /// Snake-case name, e.g. `home_security`.
    pub name: &'static str,
    /// Label.
    pub label: &'static str,
    /// Events and states.
    pub events: &'static [NotificationEvent],
}

/// One notification event or state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationEvent {
    /// Event id.
    pub id: u8,
    /// Label.
    pub label: &'static str,
    /// The state variable this event sets (e.g. "Motion sensor status");
    /// empty for one-shot events.
    pub variable: &'static str,
    /// Whether the variable returns to idle (event 0) on its own.
    pub idle: bool,
}

impl SensorType {
    /// A scale by id.
    pub fn scale(&self, id: u8) -> Option<&'static Scale> {
        self.scales.iter().find(|s| s.id == id)
    }
}

impl MeterType {
    /// A scale by id.
    pub fn scale(&self, id: u8) -> Option<&'static Scale> {
        self.scales.iter().find(|s| s.id == id)
    }
}

impl NotificationType {
    /// An event by id.
    pub fn event(&self, id: u8) -> Option<&'static NotificationEvent> {
        self.events.iter().find(|e| e.id == id)
    }
}

include!(concat!(env!("OUT_DIR"), "/tables.rs"));

/// A multilevel sensor type by id.
pub fn sensor_type(id: u8) -> Option<&'static SensorType> {
    SENSOR_TYPES
        .binary_search_by_key(&id, |t| t.id)
        .ok()
        .map(|i| &SENSOR_TYPES[i])
}

/// A meter type by id.
pub fn meter_type(id: u8) -> Option<&'static MeterType> {
    METER_TYPES
        .binary_search_by_key(&id, |t| t.id)
        .ok()
        .map(|i| &METER_TYPES[i])
}

/// A notification type by id.
pub fn notification_type(id: u8) -> Option<&'static NotificationType> {
    NOTIFICATION_TYPES
        .binary_search_by_key(&id, |t| t.id)
        .ok()
        .map(|i| &NOTIFICATION_TYPES[i])
}
