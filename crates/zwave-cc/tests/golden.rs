//! Known frames, as devices send them, parsed and re-encoded.

use zwave_cc::*;

fn rt(frame: &[u8]) -> Command {
    let c = Command::parse(frame).unwrap_or_else(|e| panic!("{frame:02x?}: {e}"));
    assert_eq!(c.encode(), frame, "{} re-encodes", c.name());
    c
}

#[test]
fn multilevel_sensor() {
    // Air temperature 22.5 °C, precision 1, 2 bytes.
    let Command::MultilevelSensor(multilevel_sensor::Command::Report(r)) =
        rt(&[0x31, 0x05, 0x01, 0x22, 0x00, 0xe1])
    else {
        panic!()
    };
    assert_eq!(
        (r.sensor_type, r.value.scale, r.value.to_f64()),
        (1, 0, 22.5)
    );
    // Negative values: -5.0 °F, 1 byte, precision 1.
    let Command::MultilevelSensor(multilevel_sensor::Command::Report(r)) =
        rt(&[0x31, 0x05, 0x01, 0x29, 0xce])
    else {
        panic!()
    };
    assert_eq!((r.value.scale, r.value.to_f64()), (1, -5.0));
    // Supported sensors bitmask: air temperature (1), humidity (5), luminance (3).
    let Command::MultilevelSensor(multilevel_sensor::Command::SupportedSensorReport(s)) =
        rt(&[0x31, 0x02, 0b0001_0101])
    else {
        panic!()
    };
    assert_eq!(s.sensor_types, vec![1, 3, 5]);
    // Get with and without the v5 fields.
    rt(&[0x31, 0x04]);
    let Command::MultilevelSensor(multilevel_sensor::Command::Get(g)) =
        rt(&[0x31, 0x04, 0x05, 0x08])
    else {
        panic!()
    };
    assert_eq!((g.sensor_type, g.scale), (Some(5), Some(1)));
    // A size of 3 is invalid.
    assert!(Command::parse(&[0x31, 0x05, 0x01, 0x23, 0, 0, 0]).is_err());
    assert!(Command::parse(&[0x31, 0x05, 0x01, 0x22, 0x00]).is_err());
}

#[test]
fn meter() {
    // Electric, 1.234 kWh (precision 3, scale 0, size 2), delta time 300 s, previous 1.000 kWh.
    let f = [0x32, 0x02, 0x21, 0x62, 0x04, 0xd2, 0x01, 0x2c, 0x03, 0xe8];
    let Command::Meter(meter::Command::Report(r)) = rt(&f) else {
        panic!()
    };
    assert_eq!(
        (
            r.meter_type,
            r.rate_type,
            r.value.to_f64(),
            r.delta_time,
            r.previous_value
        ),
        (1, 1, 1.234, Some(300), Some(1000))
    );
    // v1 report and a zero delta time (no previous value) followed by scale2.
    rt(&[0x32, 0x02, 0x01, 0x09, 0x7f]);
    let Command::Meter(meter::Command::Report(r)) =
        rt(&[0x32, 0x02, 0xa1, 0x39, 0x10, 0x00, 0x00, 0x01])
    else {
        panic!()
    };
    assert_eq!(
        (
            r.scale_bit2,
            r.value.scale,
            r.delta_time,
            r.previous_value,
            r.scale2
        ),
        (true, 3, Some(0), None, Some(1))
    );
}

#[test]
fn notification() {
    // Home security, motion detected, v8.
    let Command::Notification(notification::Command::Report(r)) =
        rt(&[0x71, 0x05, 0, 0, 0, 0xff, 0x07, 0x08, 0x00])
    else {
        panic!()
    };
    assert_eq!(
        (r.notification_type, r.event, r.params.len()),
        (Some(7), Some(8), 0)
    );
    // With event parameters and a sequence number.
    let Command::Notification(notification::Command::Report(r)) = rt(&[
        0x71, 0x05, 0, 0, 0, 0xff, 0x06, 0x05, 0x82, 0x63, 0x03, 0x2a,
    ]) else {
        panic!()
    };
    assert_eq!(
        (r.sequence, r.params, r.sequence_number),
        (Some(true), vec![0x63, 0x03], Some(0x2a))
    );
    // v1 alarm report: two bytes only.
    let Command::Notification(notification::Command::Report(r)) = rt(&[0x71, 0x05, 0x02, 0xff])
    else {
        panic!()
    };
    assert_eq!(
        (r.v1_alarm_type, r.v1_alarm_level, r.status),
        (2, 0xff, None)
    );
    // Supported types bitmask with its length in the low bits.
    let Command::Notification(notification::Command::SupportedReport(s)) =
        rt(&[0x71, 0x08, 0x01, 0b1000_0000])
    else {
        panic!()
    };
    assert_eq!(s.types, vec![7]);
    // A length byte longer than the payload is an error, not a panic.
    assert!(Command::parse(&[0x71, 0x05, 0, 0, 0, 0xff, 0x06, 0x05, 0x1f, 0x63]).is_err());
}

#[test]
fn supervision_and_multi_channel_nest() {
    let inner = [0x25, 0x01, 0xff];
    let mut f = vec![0x6c, 0x01, 0x85, inner.len() as u8];
    f.extend_from_slice(&inner);
    let Command::Supervision(supervision::Command::Get(g)) = rt(&f) else {
        panic!()
    };
    assert_eq!((g.status_updates, g.session_id), (true, 5));
    assert!(matches!(
        Command::parse(&g.command).unwrap(),
        Command::BinarySwitch(_)
    ));
    let Command::MultiChannel(multi_channel::Command::CommandEncapsulation(e)) =
        rt(&[0x60, 0x0d, 0x02, 0x00, 0x31, 0x05, 0x01, 0x22, 0x00, 0xe1])
    else {
        panic!()
    };
    assert_eq!((e.source_end_point, e.destination_end_point), (2, 0));
    assert!(matches!(
        Command::parse(&e.command).unwrap(),
        Command::MultilevelSensor(_)
    ));
    let Command::MultiChannel(multi_channel::Command::CapabilityReport(c)) =
        rt(&[0x60, 0x0a, 0x01, 0x10, 0x01, 0x5e, 0x25, 0x86])
    else {
        panic!()
    };
    assert_eq!(c.command_classes, vec![0x5e, 0x25, 0x86]);
}

#[test]
fn configuration_sizes() {
    let Command::Configuration(configuration::Command::Report(r)) =
        rt(&[0x70, 0x06, 0x0d, 0x02, 0xff, 0xfe])
    else {
        panic!()
    };
    assert_eq!((r.parameter, r.size, r.value), (13, 2, -2));
    rt(&[0x70, 0x04, 0x03, 0x84, 0x00, 0x01, 0x51, 0x80]);
    assert!(Command::parse(&[0x70, 0x06, 0x0d, 0x03, 0, 0, 0]).is_err());
    // Properties with size 0: no min/max/default.
    let Command::Configuration(configuration::Command::PropertiesReport(p)) =
        rt(&[0x70, 0x0f, 0x00, 0x05, 0x00, 0x00, 0x06])
    else {
        panic!()
    };
    assert_eq!((p.size, p.next_parameter), (0, 6));
}

#[test]
fn security_and_device_info() {
    let Command::Security2(security_2::Command::KexReport(k)) =
        rt(&[0x9f, 0x05, 0x00, 0x02, 0x01, 0x83])
    else {
        panic!()
    };
    assert_eq!((k.echo, k.schemes, k.curves, k.keys), (false, 2, 1, 0x83));
    let Command::Security2(security_2::Command::NonceReport(n)) = rt(&[
        0x9f, 0x02, 0x11, 0x01, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
    ]) else {
        panic!()
    };
    assert_eq!((n.sos, n.mos, n.receiver_ei.len()), (true, false, 16));
    let Command::ManufacturerSpecific(manufacturer_specific::Command::Report(m)) =
        rt(&[0x72, 0x05, 0x02, 0x7a, 0x00, 0x03, 0x00, 0x87])
    else {
        panic!()
    };
    assert_eq!(
        (m.manufacturer_id, m.product_type, m.product_id),
        (0x027a, 3, 0x87)
    );
    let Command::Version(version::Command::Report(v)) =
        rt(&[0x86, 0x12, 0x03, 0x07, 0x12, 0x01, 0x02, 0x01, 0x00])
    else {
        panic!()
    };
    assert_eq!(
        (v.firmware_version, v.hardware_version, v.firmware_targets),
        (1, Some(1), Some(0))
    );
    let Command::WakeUp(wake_up::Command::IntervalReport(w)) =
        rt(&[0x84, 0x06, 0x00, 0x0e, 0x10, 0x01])
    else {
        panic!()
    };
    assert_eq!((w.seconds, w.node_id), (3600, 1));
    let Command::Battery(battery::Command::Report(b)) = rt(&[0x80, 0x03, 0x5a]) else {
        panic!()
    };
    assert_eq!(b.level, 90);
    let Command::Association(association::Command::Report(a)) =
        rt(&[0x85, 0x03, 0x01, 0x05, 0x00, 0x01])
    else {
        panic!()
    };
    assert_eq!(a.nodes, vec![1]);
}

#[test]
fn unknown_and_edge_frames() {
    assert_eq!(Command::parse(&[0x00]).unwrap(), Command::NoOperation);
    assert_eq!(Command::NoOperation.encode(), vec![0x00]);
    let Command::Unknown(u) = rt(&[0x5b, 0x03, 0x01, 0x80, 0x01]) else {
        panic!()
    };
    assert_eq!((u.cc, u.command), (0x5b, 0x03));
    let Command::Unknown(u) = rt(&[0xf1, 0x00, 0x01, 0xaa]) else {
        panic!()
    };
    assert_eq!(u.cc, 0xf100);
    assert!(Command::parse(&[]).is_err());
    assert!(Command::parse(&[0x20]).is_err());
    // An unknown command of a known class.
    assert!(matches!(rt(&[0x20, 0x7e, 0x01]), Command::Unknown(_)));
}

#[test]
fn default_commands_roundtrip() {
    // Every command's default value encodes to something that parses back to itself.
    let samples: Vec<Command> = vec![
        basic::Set::default().into(),
        basic::Get::default().into(),
        binary_switch::Report::default().into(),
        multilevel_switch::StartLevelChange::default().into(),
        multilevel_sensor::Report::default().into(),
        meter::Report::default().into(),
        notification::Report::default().into(),
        battery::Report::default().into(),
        wake_up::IntervalCapabilitiesReport::default().into(),
        configuration::Set {
            parameter: 1,
            default: false,
            size: 4,
            value: -7,
        }
        .into(),
        association::Set {
            group: 1,
            nodes: vec![1, 2],
        }
        .into(),
        version::ZwaveSoftwareReport::default().into(),
        security_2::TransferEnd {
            key_verified: true,
            key_request_complete: false,
        }
        .into(),
        supervision::Report {
            more_status_updates: false,
            wake_up_request: false,
            session_id: 9,
            status: 0xff,
            duration: Some(0),
        }
        .into(),
        zwave_plus_info::Report::default().into(),
        multi_channel::EndPointReport::default().into(),
    ];
    for c in samples {
        let e = c.encode();
        assert_eq!(Command::parse(&e).unwrap(), c, "{}", c.name());
    }
}

#[test]
fn tables() {
    let t = tables::sensor_type(1).unwrap();
    assert_eq!(
        (t.name, t.scale(0).unwrap().unit),
        ("air_temperature", "°C")
    );
    assert_eq!(tables::sensor_type(5).unwrap().name, "humidity");
    assert_eq!(tables::meter_type(1).unwrap().scale(2).unwrap().unit, "W");
    let n = tables::notification_type(7).unwrap();
    assert_eq!(n.name, "home_security");
    assert_eq!(n.event(8).unwrap().variable, "Motion sensor status");
    assert!(tables::sensor_type(0).is_none());
    assert!(cc_info(0x9f).unwrap().title.contains("Security 2"));
}

#[test]
fn values_and_durations() {
    let v = Value::from_f64(-12.34, 2, 1);
    assert_eq!((v.size, v.raw), (2, -1234));
    assert_eq!(Value::from_f64(1e9, 0, 0).size, 4);
    assert_eq!(
        decode_duration(0x80),
        Some(std::time::Duration::from_secs(60))
    );
    assert_eq!(decode_duration(0xfe), None);
    assert_eq!(
        encode_duration(Some(std::time::Duration::from_secs(90))),
        90
    );
    assert_eq!(
        encode_duration(Some(std::time::Duration::from_secs(200))),
        0x83
    );
    assert_eq!(encode_duration(None), 0xff);
}
