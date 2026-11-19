//! The memory budget: a host with 200 interviewed nodes stays under 16 MB
//! resident (measured as this whole test process, simulator included, on
//! x86-64 Linux where the kernel reports the peak).

use std::time::Duration;
use zwave_host::{Controller, Event, HostConfig, NetworkKeys};
use zwave_sim::node::NodeSpec;
use zwave_sim::{SimConfig, Simulator};

// Under qemu the kernel would report the emulator's memory, so only natively.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn peak_rss_kib() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    s.lines()
        .find(|l| l.starts_with("VmHWM:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn peak_rss_kib() -> Option<u64> {
    None
}

const N: u16 = 200;

#[test]
fn two_hundred_nodes_fit_the_budget() {
    // One thread, as the bridge runs.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let mut sim = Simulator::new(SimConfig {
            frame_gap: Duration::ZERO,
            ..SimConfig::default()
        });
        for i in 0..N {
            sim = sim.with_joined_node(NodeSpec::switch(&format!("switch{i}")));
        }
        let (host, dev) = tokio::io::duplex(64 * 1024);
        sim.spawn(dev);
        let mut cfg = HostConfig::new(NetworkKeys::generate().unwrap(), None);
        cfg.request_timeout = Duration::from_secs(5);
        let (c, mut ev) = Controller::start(host, cfg).await.unwrap();
        let mut done = 0;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
        while done < N {
            match tokio::time::timeout_at(deadline, ev.recv()).await {
                Ok(Some(Event::InterviewCompleted { .. })) => done += 1,
                Ok(Some(Event::InterviewFailed { node, reason })) => {
                    eprintln!("failed {node}: {reason}")
                }
                Ok(Some(_)) => {}
                _ => panic!("only {done} of {N} interviews completed"),
            }
        }
        assert_eq!(c.nodes().len(), N as usize);
        assert!(c
            .nodes()
            .iter()
            .all(|n| n.stage == zwave_host::node::InterviewStage::Complete));
    });
    if let Some(kib) = peak_rss_kib() {
        eprintln!("peak RSS with 200 nodes: {kib} KiB");
        assert!(
            kib < 16 * 1024,
            "peak RSS {kib} KiB is over the 16 MiB budget"
        );
    }
}
