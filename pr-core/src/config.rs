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

/// A tab the user pinned: its (port, node) shell is recreated automatically
/// at the next app startup, prefilled but disconnected — the user still has
/// to press Connect. `remote` is empty for port kinds with no node concept
/// (Telnet/SSH).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PinnedSession {
    pub port_id: String,
    pub remote: String,
    /// Digipeater path, e.g. "WIDE1-1,WIDE2-1". Empty for a direct path.
    #[serde(default)]
    pub via: String,
    /// True if this tab sends unconnected (UI) traffic to `remote` instead
    /// of opening a connected-mode session.
    #[serde(default)]
    pub unproto: bool,
}

/// Persisted scrollback for one (port, node) pair, so reconnecting to a
/// station you've talked to before restores context. Trimmed to
/// `UiPrefs.history_lines` lines on save. Kept separate from connected-mode
/// history for the same (port, remote) via `unproto`, since the two are
/// unrelated conversations that happen to share a destination callsign.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NodeHistory {
    pub port_id: String,
    pub remote: String,
    #[serde(default)]
    pub unproto: bool,
    #[serde(default)]
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgwpeLogin {
    pub username: String,
    pub password: String,
}

/// A user-defined highlight: any line containing text matching `pattern`
/// gets that span colored. Seeded by default with common traffic keywords
/// (CQ, BEACON, IDENT); users add more of these for their own nets/bulletins
/// — there's no separate mechanism for "custom" rules, just more entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HighlightRule {
    pub label: String,
    /// Case-insensitive. Literal keywords separated by `,` or `|` unless
    /// `regex` is set, in which case it's used as a regex directly.
    pub pattern: String,
    #[serde(default)]
    pub regex: bool,
    /// A CSS-style color, e.g. `"#FFD700"`.
    pub color: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Monitor/session scrollback highlighting preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HighlightPrefs {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Color for AX.25-style callsign tokens not in the address book.
    #[serde(default = "default_callsign_color")]
    pub callsign_color: String,
    /// Color for callsign tokens matching an address book entry.
    #[serde(default = "default_known_callsign_color")]
    pub known_callsign_color: String,
    /// Color for the bracketed frame/command tag on monitor lines, e.g.
    /// `[UI]`, `[SABM]`, `[I N(S)=1 N(R)=0]`.
    #[serde(default = "default_ax25_command_color")]
    pub ax25_command_color: String,
    #[serde(default = "default_rules")]
    pub rules: Vec<HighlightRule>,
}

fn default_callsign_color() -> String {
    "#4FC1FF".to_string()
}

fn default_known_callsign_color() -> String {
    "#B5CEA8".to_string()
}

fn default_ax25_command_color() -> String {
    "#C586C0".to_string()
}

fn default_rules() -> Vec<HighlightRule> {
    vec![
        HighlightRule { label: "CQ".to_string(), pattern: "CQ".to_string(), regex: false, color: "#FFD700".to_string(), enabled: true },
        HighlightRule {
            label: "BEACON/IDENT".to_string(),
            pattern: "BEACON,IDENT".to_string(),
            regex: false,
            color: "#FF8C00".to_string(),
            enabled: true,
        },
    ]
}

impl Default for HighlightPrefs {
    fn default() -> Self {
        HighlightPrefs {
            enabled: true,
            callsign_color: default_callsign_color(),
            known_callsign_color: default_known_callsign_color(),
            ax25_command_color: default_ax25_command_color(),
            rules: default_rules(),
        }
    }
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
    /// Max lines of scrollback kept per (port, node) in `NodeHistory`.
    #[serde(default = "default_history_lines")]
    pub history_lines: u32,
}

fn default_history_lines() -> u32 {
    1000
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
            history_lines: default_history_lines(),
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
    #[serde(default)]
    pub pinned_sessions: Vec<PinnedSession>,
    #[serde(default)]
    pub node_history: Vec<NodeHistory>,
    #[serde(default)]
    pub highlighting: HighlightPrefs,
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
