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

/// A known station: either entered manually, or auto-created/updated the
/// first time we hear that callsign on any port (see `PortEvent::StationHeard`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressBookEntry {
    /// Primary key, e.g. "KD3BFP-9". Always stored uppercase.
    pub callsign: String,
    #[serde(default)]
    pub name: Option<String>,
    /// A node/BBS alias, e.g. "WL2K" or a digipeater/BBS system name —
    /// distinct from the operator's personal name.
    #[serde(default)]
    pub alias: Option<String>,
    /// Free-text location (city/state, grid square, whatever's useful).
    #[serde(default)]
    pub location: Option<String>,
    /// Free-form, potentially multi-line notes about this station.
    #[serde(default)]
    pub notes: Option<String>,
    /// Local time of the most recent time this callsign was heard, formatted
    /// for display (e.g. "2026-07-27 20:34:21"). `None` for manually-added
    /// entries that haven't actually been heard yet.
    #[serde(default)]
    pub last_heard: Option<String>,
    #[serde(default)]
    pub heard_count: u32,
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
    /// A font description string like `"Monospace 11"`: everything but a
    /// trailing numeric token is the family name, the trailing number (if
    /// present) is the point size.
    #[serde(default)]
    pub font: Option<String>,
    #[serde(default = "default_true")]
    pub show_timestamps: bool,
    /// Pre-fills the "My Callsign" field when adding a new AGWPE/KISS port.
    #[serde(default)]
    pub default_call: Option<String>,
    /// QRZ.com XML API credentials, for address book "Lookup QRZ". Stored in
    /// plain text like the AGWPE login fields already are — same tradeoff,
    /// not a new one.
    #[serde(default)]
    pub qrz_username: Option<String>,
    #[serde(default)]
    pub qrz_password: Option<String>,
}

fn default_true() -> bool {
    true
}

impl Default for UiPrefs {
    fn default() -> Self {
        UiPrefs {
            show_monitor: true,
            font: None,
            show_timestamps: true,
            default_call: None,
            qrz_username: None,
            qrz_password: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub ports: Vec<PortEntry>,
    #[serde(default)]
    pub ui: UiPrefs,
    #[serde(default)]
    pub address_book: Vec<AddressBookEntry>,
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
