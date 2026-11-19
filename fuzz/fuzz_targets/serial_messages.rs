//! Every typed Serial API parser on arbitrary payloads.
#![no_main]

use libfuzzer_sys::fuzz_target;
use zwave_serial::message::*;

fuzz_target!(|data: &[u8]| {
    let Some((&which, p)) = data.split_first() else { return };
    let len = 1 + usize::from(which & 1);
    match which >> 1 {
        0 => {
            if let Ok(c) = SerialApiCapabilities::parse(p) {
                let _ = c.supports(zwave_serial::FunctionId(p.first().copied().unwrap_or(0)));
            }
        }
        1 => {
            let _ = InitData::parse(p);
        }
        2 => {
            let _ = ControllerVersion::parse(p);
        }
        3 => {
            let _ = ControllerId::parse(p, len);
        }
        4 => {
            let _ = NodeProtocolInfo::parse(p);
        }
        5 => {
            if let Ok(n) = NodeInfo::parse(p) {
                assert_eq!(NodeInfo::parse(&n.encode()).unwrap().supported, n.supported);
            }
        }
        6 => {
            let _ = ApplicationCommand::parse(p, len);
        }
        7 => {
            let _ = ApplicationCommand::parse_bridge(p, len);
        }
        8 => {
            let _ = ApplicationUpdate::parse(p, len);
        }
        9 => {
            let _ = SendDataResult::parse(p);
        }
        10 => {
            let _ = TxReport::parse(p);
        }
        11 => {
            let _ = AddNodeStatus::parse(p, len);
        }
        12 => {
            let _ = RemoveNodeStatus::parse(p, len);
        }
        13 => {
            let _ = NvmResponse::parse(p, which & 1 == 1);
        }
        14 => {
            let _ = parse_routing_info(p);
            let _ = parse_cc_list(p);
            let _ = parse_node_mask(p, 1);
        }
        _ => {}
    }
});
