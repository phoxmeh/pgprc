use std::path::PathBuf;

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

/// A single configured port: a physical/network connection to a TNC or host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortEntry {
    pub id: String,
    pub name: String,
    pub config: PortConfig,
    #[serde(default)]
    pub autoconnect: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum PortConfig {
    Telnet {
        host: String,
        port: u16,
    },
    Ssh {
        host: String,
        port: u16,
        user: String,
    },
    Agwpe {
        host: String,
        port: u16,
        radio_port: u8,
        my_call: String,
        #[serde(default)]
        login: Option<AgwpeLogin>,
    },
    Ax25RawSocket {
        device: String,
    },
    /// KISS TNC reachable over TCP (e.g. Direwolf's/UZ7HO's raw KISS port).
    /// Unconnected (UI/beacon) traffic only — connected-mode AX.25 over bare
    /// KISS would require reimplementing the modulus-8 ARQ state machine,
    /// which AGWPE and AF_AX25 raw sockets otherwise offload for us.
    KissTcp {
        host: String,
        port: u16,
        my_call: String,
    },
    /// KISS TNC on a serial/USB port.
    KissSerial {
        device: String,
        baud: u32,
        my_call: String,
    },
}

impl PortConfig {
    pub fn kind_label(&self) -> &'static str {
        match self {
            PortConfig::Telnet { .. } => "Telnet",
            PortConfig::Ssh { .. } => "SSH",
            PortConfig::Agwpe { .. } => "AGWPE",
            PortConfig::Ax25RawSocket { .. } => "AX.25 raw socket",
            PortConfig::KissTcp { .. } => "KISS (TCP)",
            PortConfig::KissSerial { .. } => "KISS (Serial)",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgwpeLogin {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiPrefs {
    #[serde(default = "default_true")]
    pub show_monitor: bool,
    #[serde(default)]
    pub font: Option<String>,
}

fn default_true() -> bool {
    true
}

impl Default for UiPrefs {
    fn default() -> Self {
        UiPrefs {
            show_monitor: true,
            font: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub ports: Vec<PortEntry>,
    #[serde(default)]
    pub ui: UiPrefs,
}

impl AppConfig {
    pub fn config_path() -> Option<PathBuf> {
        ProjectDirs::from("net", "packetradio", "packet-radio")
            .map(|dirs| dirs.config_dir().join("config.toml"))
    }

    pub fn load() -> anyhow::Result<AppConfig> {
        let Some(path) = Self::config_path() else {
            return Ok(AppConfig::default());
        };
        if !path.exists() {
            return Ok(AppConfig::default());
        }
        let text = std::fs::read_to_string(&path)?;
        Ok(toml::from_str(&text)?)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::config_path()
            .ok_or_else(|| anyhow::anyhow!("could not determine config directory"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(&path, text)?;
        Ok(())
    }
}
