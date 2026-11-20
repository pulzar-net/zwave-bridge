//! `zwave-sim`: a simulated Z-Wave controller on a TCP port, with a switch,
//! a sleeping S0 sensor, an S2 sensor and a SmartStart sensor. Point a host
//! at `tcp://127.0.0.1:<port>`.
//!
//! Type on stdin to act on the nodes: `press <name>`, `wake <name>`,
//! `power <name>`, `temp <name> <celsius>`, `motion <name>`, `dsk <name>`.
//! The simulator keeps running when stdin ends; stop it with a signal.

use tokio::io::AsyncBufReadExt;
use zwave_security::SecurityClass;
use zwave_sim::node::NodeSpec;
use zwave_sim::{SimConfig, Simulator};

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let mut args = std::env::args().skip(1);
    let addr = match args.next().as_deref() {
        Some("--version") => {
            println!("zwave-sim {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some(a) => a.to_string(),
        None => "127.0.0.1:4545".to_string(),
    };
    let specs = vec![
        NodeSpec::switch("switch"),
        NodeSpec::sleeping_sensor("sleeper"),
        NodeSpec::s2_sensor(
            "s2",
            vec![
                SecurityClass::S2Authenticated,
                SecurityClass::S2Unauthenticated,
            ],
        ),
        NodeSpec::smartstart_sensor("smartstart"),
    ];
    for s in &specs {
        println!("{:<11} DSK {}", s.name, s.dsk());
    }
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("listening on tcp://{}", listener.local_addr()?);
    let (sock, peer) = listener.accept().await?;
    println!("host connected from {peer}");
    let mut sim = Simulator::new(SimConfig::default());
    for s in specs.clone() {
        sim = sim.with_node(s);
    }
    let h = sim.spawn(sock);
    let h2 = h.clone();
    tokio::spawn(async move {
        loop {
            if let Ok((sock, peer)) = listener.accept().await {
                println!("host reconnected from {peer}");
                h2.attach(sock).await;
            }
        }
    });
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.as_slice() {
            ["press", n] => println!("{:?}", h.press(n).await),
            ["wake", n] => println!("{}", h.wake_up(n).await),
            ["power", n] => println!("{}", h.power_on(n).await),
            ["temp", n, v] => println!("{}", h.set_sensor(n, 1, v.parse().unwrap_or(20.0)).await),
            ["motion", n] => println!("{}", h.motion(n).await),
            ["dsk", n] => match specs.iter().find(|s| s.name == *n) {
                Some(s) => println!("{}", s.dsk()),
                None => println!("no node {n}"),
            },
            _ => println!("press|wake|power|motion <name>, temp <name> <value>, dsk <name>"),
        }
    }
    // Stdin ended (a script's pipe or FIFO closed): keep serving the host
    // until killed, as the controller it stands in for would.
    eprintln!("stdin closed; still serving the host");
    std::future::pending::<()>().await;
    Ok(())
}
