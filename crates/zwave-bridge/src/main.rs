//! `zwave-bridge`: a Z-Wave controller on MQTT.
//!
//! ```text
//! zwave-bridge [--config /etc/zwave-bridge/config.toml]
//! zwave-bridge probe <port>        say what controller answers on a port
//! zwave-bridge --version
//! ```

use anyhow::{Context, Result};
use std::path::PathBuf;
use zwave_bridge::config::Config;

const HELP: &str =
    "zwave-bridge [--config FILE]   run the bridge (default /etc/zwave-bridge/config.toml)
zwave-bridge probe PORT        identify the Z-Wave controller on PORT (a device or tcp://host:port)
zwave-bridge --version";

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let mut config = PathBuf::from("/etc/zwave-bridge/config.toml");
    let mut probe: Option<String> = None;
    let mut args = lexopt::Parser::from_env();
    while let Some(arg) = args.next()? {
        use lexopt::prelude::*;
        match arg {
            Short('c') | Long("config") => config = args.value()?.into(),
            Long("version") | Short('V') => {
                println!("zwave-bridge {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            Long("help") | Short('h') => {
                println!("{HELP}");
                return Ok(());
            }
            Value(v) if v == "probe" => probe = Some(args.value()?.string()?),
            other => return Err(other.unexpected().into()),
        }
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        if let Some(port) = probe {
            return match zwave_serial::probe(&port).await {
                Ok(Some(id)) => {
                    println!(
                        "Z-Wave controller: firmware {}, manufacturer {:#06x}, product {:#06x}:{:#06x}{}",
                        id.firmware,
                        id.manufacturer_id,
                        id.product_type,
                        id.product_id,
                        id.known_name().map(|n| format!(" ({n})")).unwrap_or_default()
                    );
                    Ok(())
                }
                Ok(None) => {
                    println!("no Z-Wave controller answered on {port}");
                    std::process::exit(1);
                }
                Err(e) => Err(anyhow::anyhow!("{port}: {e}")),
            };
        }
        let text = std::fs::read_to_string(&config).with_context(|| format!("reading {}", config.display()))?;
        let cfg = Config::parse(&text).map_err(|e| anyhow::anyhow!("{}: {e}", config.display()))?;
        let port = zwave_serial::port::open(&cfg.serial.port).await.with_context(|| format!("opening {}", cfg.serial.port))?;
        let shutdown = async {
            #[cfg(unix)]
            {
                let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal handler");
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            #[cfg(not(unix))]
            let _ = tokio::signal::ctrl_c().await;
        };
        zwave_bridge::run(cfg, port, shutdown).await
    })
}
