//! The bridge's configuration file (TOML).

use serde::Deserialize;
use std::path::PathBuf;

/// The whole configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The controller.
    pub serial: Serial,
    /// Where state lives.
    #[serde(default)]
    pub store: Store,
    /// The broker.
    #[serde(default)]
    pub mqtt: Mqtt,
    /// Home Assistant discovery.
    #[serde(default)]
    pub homeassistant: HomeAssistant,
    /// Network behavior.
    #[serde(default)]
    pub network: Network,
}

/// The controller's port.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Serial {
    /// A serial device (`/dev/ttyUSB0`, `/dev/serial/by-id/...`) or `tcp://host:port`.
    pub port: String,
}

/// State on disk.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Store {
    /// Directory for `nodes.json`, `keys.json` and NVM backups.
    #[serde(default = "default_dir")]
    pub dir: PathBuf,
}

fn default_dir() -> PathBuf {
    PathBuf::from("/var/lib/zwave-bridge")
}

impl Default for Store {
    fn default() -> Self {
        Store { dir: default_dir() }
    }
}

/// The MQTT broker.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mqtt {
    /// Host.
    #[serde(default = "default_host")]
    pub host: String,
    /// Port.
    #[serde(default = "default_port")]
    pub port: u16,
    /// Client id.
    #[serde(default = "default_client_id")]
    pub client_id: String,
    /// User name.
    #[serde(default)]
    pub username: Option<String>,
    /// Password.
    #[serde(default)]
    pub password: Option<String>,
    /// A file holding the password (read at start), instead of `password`.
    #[serde(default)]
    pub password_file: Option<PathBuf>,
    /// Topic prefix.
    #[serde(default = "default_base")]
    pub base_topic: String,
    /// Keep-alive, seconds.
    #[serde(default = "default_keep_alive")]
    pub keep_alive: u64,
}

fn default_host() -> String {
    "127.0.0.1".into()
}
fn default_port() -> u16 {
    1883
}
fn default_client_id() -> String {
    "zwave-bridge".into()
}
fn default_base() -> String {
    "zwave".into()
}
fn default_keep_alive() -> u64 {
    30
}

impl Default for Mqtt {
    fn default() -> Self {
        Mqtt {
            host: default_host(),
            port: default_port(),
            client_id: default_client_id(),
            username: None,
            password: None,
            password_file: None,
            base_topic: default_base(),
            keep_alive: default_keep_alive(),
        }
    }
}

/// Home Assistant MQTT discovery.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeAssistant {
    /// Publish discovery configs.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Discovery prefix.
    #[serde(default = "default_prefix")]
    pub discovery_prefix: String,
}

fn yes() -> bool {
    true
}
fn default_prefix() -> String {
    "homeassistant".into()
}

impl Default for HomeAssistant {
    fn default() -> Self {
        HomeAssistant {
            enabled: true,
            discovery_prefix: default_prefix(),
        }
    }
}

/// Network behavior.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Network {
    /// Wake-up interval configured on sleeping nodes, seconds.
    #[serde(default = "default_wake")]
    pub wake_up_interval: u32,
    /// How long inclusion and exclusion wait for a node, seconds.
    #[serde(default = "default_inclusion")]
    pub inclusion_timeout: u64,
}

fn default_wake() -> u32 {
    3600
}
fn default_inclusion() -> u64 {
    60
}

impl Default for Network {
    fn default() -> Self {
        Network {
            wake_up_interval: default_wake(),
            inclusion_timeout: default_inclusion(),
        }
    }
}

impl Config {
    /// Parses a configuration file's text.
    pub fn parse(text: &str) -> Result<Config, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_and_full() {
        let c = Config::parse("[serial]\nport = \"/dev/ttyUSB0\"\n").unwrap();
        assert_eq!(c.mqtt.base_topic, "zwave");
        assert!(c.homeassistant.enabled);
        let c = Config::parse(
            "[serial]\nport = \"tcp://127.0.0.1:4545\"\n[store]\ndir = \"/tmp/z\"\n[mqtt]\nhost = \"broker\"\nport = 1884\nbase_topic = \"home/zwave\"\n[homeassistant]\nenabled = false\n[network]\nwake_up_interval = 7200\n",
        )
        .unwrap();
        assert_eq!(
            (
                c.mqtt.port,
                c.network.wake_up_interval,
                c.homeassistant.enabled
            ),
            (1884, 7200, false)
        );
        assert!(Config::parse("[serial]\nport = \"x\"\nbogus = 1\n").is_err());
    }
}
