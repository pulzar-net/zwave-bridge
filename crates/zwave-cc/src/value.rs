//! Z-Wave's sized decimal values and durations.

/// A sensor, meter or battery health value: a signed integer of 1, 2 or 4
/// bytes with a decimal precision and a scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Value {
    /// Decimal places (0 to 7).
    pub precision: u8,
    /// Scale (unit) bits stored with the value (0 to 3).
    pub scale: u8,
    /// Size in bytes: 1, 2 or 4.
    pub size: u8,
    /// The integer as sent.
    pub raw: i32,
}

impl Default for Value {
    fn default() -> Self {
        Value {
            precision: 0,
            scale: 0,
            size: 1,
            raw: 0,
        }
    }
}

impl Value {
    /// The value as a float.
    pub fn to_f64(&self) -> f64 {
        f64::from(self.raw) / 10f64.powi(i32::from(self.precision))
    }

    /// Encodes `v` with `precision` decimals in the smallest size that fits.
    pub fn from_f64(v: f64, precision: u8, scale: u8) -> Value {
        let precision = precision.min(7);
        let raw = (v * 10f64.powi(i32::from(precision))).round();
        let raw = raw.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
        let size = if i8::try_from(raw).is_ok() {
            1
        } else if i16::try_from(raw).is_ok() {
            2
        } else {
            4
        };
        Value {
            precision,
            scale: scale & 0x03,
            size,
            raw,
        }
    }

    /// Appends the precision/scale/size byte and the value.
    pub fn encode(&self, w: &mut Vec<u8>) {
        w.push(((self.precision & 0x07) << 5) | ((self.scale & 0x03) << 3) | (self.size & 0x07));
        crate::write_sized(w, self.raw, self.size);
    }
}

/// Decodes a duration byte: `None` for unknown (`0xFE`) or reserved values.
/// 0 is instantly, up to `0x7F` seconds, `0x80..=0xFD` minutes.
pub fn decode_duration(b: u8) -> Option<std::time::Duration> {
    match b {
        0x00..=0x7f => Some(std::time::Duration::from_secs(u64::from(b))),
        0x80..=0xfd => Some(std::time::Duration::from_secs(60 * u64::from(b - 0x7f))),
        _ => None,
    }
}

/// Encodes a duration (rounded; capped at 126 minutes). `None` means "device default" (`0xFF`).
pub fn encode_duration(d: Option<std::time::Duration>) -> u8 {
    match d {
        None => 0xff,
        Some(d) if d.as_secs() <= 0x7f => d.as_secs() as u8,
        Some(d) => (0x7f + d.as_secs().div_ceil(60).min(126)) as u8,
    }
}
