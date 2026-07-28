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
        #[serde(default)]
        kiss_params: KissParams,
    },
    /// KISS TNC on a serial/USB port.
    KissSerial {
        device: String,
        baud: u32,
        my_call: String,
        #[serde(default)]
        kiss_params: KissParams,
    },
}

/// Optional TNC transmit parameters sent as KISS command frames right after
/// connecting. `None` (the default for every field) means "leave the TNC's
/// own default alone" — existing configs behave exactly as before.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KissParams {
    /// Units of 10ms, e.g. `30` = 300ms.
    #[serde(default)]
    pub tx_delay: Option<u8>,
    /// 0-255, per the KISS spec's persistence algorithm.
    #[serde(default)]
    pub persistence: Option<u8>,
    /// Units of 10ms.
    #[serde(default)]
    pub slot_time: Option<u8>,
    #[serde(default)]
    pub full_duplex: Option<bool>,
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

/// A message left in the personal packet mailbox, addressed to a callsign.
/// Local store-and-forward only — not compatible with real Winlink/RMS
/// network infrastructure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailboxMessage {
    pub id: u64,
    pub to: String,
    pub from: String,
    pub subject: String,
    pub body: String,
    pub timestamp: String,
    #[serde(default)]
    pub read: bool,
}

/// Personal packet mailbox preferences. Off by default: when enabled, any
/// unsolicited incoming connection on a connect-capable port is answered
/// automatically by a small BBS-style command prompt instead of waiting for
/// a human to type back.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MailboxPrefs {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub messages: Vec<MailboxMessage>,
}

/// Desktop notification preferences. Off by default, like the mailbox —
/// firing OS notifications is a side effect the user should opt into. Which
/// destinations actually raise a notification (beyond the built-in
/// "directed to my callsign"/incoming-connection checks) is driven by each
/// `HighlightRule.notify` flag, not a separate rule list — one set of
/// destination rules covers both highlighting and notifications.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NotifyPrefs {
    #[serde(default)]
    pub enabled: bool,
}

/// A packet whose destination triggered a desktop notification, kept for
/// later review since the OS notification itself is transient — these are
/// often bulletins/nets worth revisiting, not just a one-time alert.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifiedPacket {
    pub id: u64,
    pub port_id: String,
    /// The exact text shown in the notification body, so it can be
    /// re-highlighted identically to how it first appeared in the Monitor.
    pub line: String,
    pub timestamp: String,
}

/// One real connected-mode QSO, logged for ADIF export. Distinct from the
/// address book's "heard" tracking, which includes any monitored traffic,
/// not just two-way contacts we actually opened/received a connection for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QsoLogEntry {
    pub callsign: String,
    pub port_id: String,
    /// UTC-ish local timestamp, "YYYY-MM-DD HH:MM:SS" (matches
    /// `AddressBookEntry.last_heard`'s formatting).
    pub started: String,
    #[serde(default)]
    pub ended: Option<String>,
}

/// A beacon that fires automatically on an interval while its port is
/// connected — the scheduled counterpart to the one-shot "Send Beacon"
/// action, using the exact same unproto send path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Beacon {
    pub id: String,
    pub port_id: String,
    pub dest: String,
    /// Digipeater path, e.g. "WIDE1-1,WIDE2-1". Empty for a direct path.
    #[serde(default)]
    pub via: String,
    pub message: String,
    pub interval_secs: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgwpeLogin {
    pub username: String,
    pub password: String,
}

/// A user-defined destination-address rule: any line containing a token
/// matching `pattern` gets that span colored, and — when `notify` is set —
/// a frame whose destination exactly matches also raises a desktop
/// notification (subject to `NotifyPrefs.enabled`). One rule list drives
/// both features, since "addresses I want to highlight" and "addresses I
/// want to be notified about" are the same underlying concept. Seeded by
/// default with common traffic keywords (CQ, BEACON, IDENT); users add more
/// of these for their own nets/bulletins/watched callsigns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HighlightRule {
    pub label: String,
    /// Case-insensitive. Literal destination addresses/keywords separated
    /// by `,` or `|`, e.g. `"CQ, WIDE1-1"`.
    pub pattern: String,
    /// A CSS-style color, e.g. `"#FFD700"`.
    pub color: String,
    /// Also raise a desktop notification when a frame's destination
    /// exactly matches this rule.
    #[serde(default)]
    pub notify: bool,
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
    /// Color for callsign tokens matching `UiPrefs.default_call` (the
    /// user's own station) — takes priority over `known_callsign_color`,
    /// since "traffic mentioning me" is more actionable than "a station I
    /// happen to know".
    #[serde(default = "default_my_call_color")]
    pub my_call_color: String,
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

fn default_my_call_color() -> String {
    "#FF5555".to_string()
}

fn default_ax25_command_color() -> String {
    "#C586C0".to_string()
}

fn default_rules() -> Vec<HighlightRule> {
    vec![
        HighlightRule { label: "CQ".to_string(), pattern: "CQ".to_string(), color: "#FFD700".to_string(), notify: false, enabled: true },
        HighlightRule {
            label: "BEACON/IDENT".to_string(),
            pattern: "BEACON,IDENT".to_string(),
            color: "#FF8C00".to_string(),
            notify: false,
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
            my_call_color: default_my_call_color(),
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
    /// Max raw lines the Monitor view keeps around for re-rendering when the
    /// filter changes. Separate from `history_lines` since the Monitor is a
    /// single global stream, not per-node.
    #[serde(default = "default_monitor_buffer_lines")]
    pub monitor_buffer_lines: u32,
}

fn default_history_lines() -> u32 {
    1000
}

fn default_monitor_buffer_lines() -> u32 {
    5000
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
            monitor_buffer_lines: default_monitor_buffer_lines(),
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
    #[serde(default)]
    pub beacons: Vec<Beacon>,
    #[serde(default)]
    pub qso_log: Vec<QsoLogEntry>,
    #[serde(default)]
    pub mailbox: MailboxPrefs,
    #[serde(default)]
    pub notify: NotifyPrefs,
    #[serde(default)]
    pub notified_packets: Vec<NotifiedPacket>,
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
