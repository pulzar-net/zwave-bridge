//! The `zwave-sim` binary as the README's quick start uses it: a host on
//! its TCP port, and the nodes pressed by typing on its stdin.

use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use zwave_host::{Controller, Event, HostConfig, InclusionOptions, NetworkKeys};

async fn next(ev: &mut zwave_host::Events, what: &str, mut f: impl FnMut(&Event) -> bool) {
    tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(e) = ev.recv().await {
            if f(&e) {
                return;
            }
        }
        panic!("events ended before {what}");
    })
    .await
    .unwrap_or_else(|_| panic!("no {what}"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inclusion_over_tcp_with_presses_on_stdin() {
    let sim = tokio::process::Command::new(env!("CARGO_BIN_EXE_zwave-sim"))
        .arg("127.0.0.1:0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let mut sim = match sim {
        Ok(s) => s,
        // Cross-compiled tests run under qemu-user, which cannot exec the
        // target's binary as a child; native runs cover this test.
        Err(e) if e.raw_os_error() == Some(8) => {
            eprintln!("zwave-sim binary not executable here ({e}); skipped");
            return;
        }
        Err(e) => panic!("zwave-sim did not start: {e}"),
    };
    let mut stdin = sim.stdin.take().unwrap();
    let mut out = BufReader::new(sim.stdout.take().unwrap()).lines();
    let addr = loop {
        let line = out
            .next_line()
            .await
            .unwrap()
            .expect("the simulator's address");
        if let Some(a) = line.strip_prefix("listening on tcp://") {
            break a.to_string();
        }
    };
    tokio::spawn(async move { while let Ok(Some(_)) = out.next_line().await {} });

    let dir = tempfile::tempdir().unwrap();
    let port = zwave_serial::port::open(&format!("tcp://{addr}"))
        .await
        .unwrap();
    let cfg = HostConfig::new(
        NetworkKeys::generate().unwrap(),
        Some(dir.path().join("nodes.json")),
    );
    let (c, mut ev) = Controller::start(port, cfg).await.unwrap();

    let c2 = c.clone();
    let inclusion = tokio::spawn(async move {
        c2.include(InclusionOptions {
            grant: zwave_host::GrantPolicy::Requested,
            ..Default::default()
        })
        .await
    });
    next(&mut ev, "inclusion mode", |e| {
        matches!(e, Event::InclusionStarted)
    })
    .await;
    stdin.write_all(b"press switch\n").await.unwrap();
    // The quick start's stdin ends after its commands; the simulator keeps serving.
    drop(stdin);
    next(&mut ev, "the node", |e| {
        matches!(e, Event::NodeAdded { .. })
    })
    .await;
    let r = tokio::time::timeout(Duration::from_secs(20), inclusion)
        .await
        .expect("inclusion finishes")
        .unwrap()
        .expect("included");
    next(
        &mut ev,
        "the interview",
        |e| matches!(e, Event::InterviewCompleted { node } if *node == r.node),
    )
    .await;
    c.close().unwrap();
}
