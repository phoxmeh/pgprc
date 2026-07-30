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
    /// Shows a quick-connect button for this port in the main window's
    /// favorites row (left side, under the title bar). Toggling it just
    /// connects/disconnects the port itself, same as the Ports dialog's own
    /// Connect/Disconnect button.
    #[serde(default)]
    pub favorite: bool,
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
    /// Digipeater path, e.g. "WIDE1-1,WIDE2-1" — same comma/space-separated
    /// convention as `Beacon.via`/`PinnedSession.via`. Empty for a direct
    /// path. Picking this station from a session tab's address-book dropdown
    /// fills the tab's Via field from here, since a station usually needs
    /// the same path every time.
    #[serde(default)]
    pub via: String,
    /// Home BBS/mailbox address for this station, e.g. a Winlink RMS or
    /// packet BBS callsign — distinct from `via` (a digipeater route to
    /// reach the station directly), this is where its own mail lives.
    #[serde(default)]
    pub home_bbs: String,
}

/// A tab the user pinned: its (port, node) shell is recreated automatically
/// at the next app startup, prefilled but disconnected — the user still has
/// to press the connect (phone-handset) button. `remote` is empty for port
/// kinds with no node concept (Telnet/SSH), which use `via` as a greeting
/// line instead of a digipeater path — see `SessionTab::via_raw`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PinnedSession {
    pub port_id: String,
    pub remote: String,
    /// Digipeater path (Agwpe/Ax25RawSocket) or greeting line (Telnet/SSH).
    /// Empty for a direct path/no greeting.
    #[serde(default)]
    pub via: String,
}

/// The old (pre-split) shape of persisted scrollback for one (port, node)
/// pair — kept only so `AppConfig::load`'s one-time legacy migration can
/// parse it back out of an un-split `config.toml` and materialize it into
/// the new per-node history files under `history/`. No longer part of
/// `AppConfig` itself; nothing else in the app constructs one of these.
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
/// a human to type back. `messages` lives in its own `mailbox.toml` (see
/// `AppConfig::load`/`save`), since it's data, not a preference — only
/// `enabled`/`respond_call`/`intro_message` belong in the general
/// `config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailboxPrefs {
    #[serde(default)]
    pub enabled: bool,
    /// Only answer a connection addressed to this callsign (case-insensitive
    /// match against the destination call the connect request actually
    /// carried, not just whatever port it arrived on). Required to enable
    /// the mailbox at all -- never falls back to any other configured
    /// callsign (see `mailbox::should_answer`).
    #[serde(default)]
    pub respond_call: String,
    /// Custom greeting sent on connect, in place of the generated
    /// `mailbox::welcome_banner`. Empty falls back to the generated banner.
    #[serde(default)]
    pub intro_message: String,
    /// Sent as a one-shot unproto frame (destination "CQ") on every enabled
    /// listen port that supports unproto, every `beacon_interval_secs`,
    /// while enabled. Empty means no beacon is sent, even while enabled.
    #[serde(default)]
    pub beacon_text: String,
    #[serde(default = "default_mailbox_beacon_interval")]
    pub beacon_interval_secs: u32,
    /// Port ids to listen for unsolicited connections on. Empty means "any
    /// connect-capable port", matching this feature's original behavior
    /// before per-port filtering existed.
    #[serde(default)]
    pub listen_ports: Vec<String>,
    #[serde(skip)]
    pub messages: Vec<MailboxMessage>,
}

impl Default for MailboxPrefs {
    fn default() -> Self {
        MailboxPrefs {
            enabled: false,
            respond_call: String::new(),
            intro_message: String::new(),
            beacon_text: String::new(),
            beacon_interval_secs: default_mailbox_beacon_interval(),
            listen_ports: Vec::new(),
            messages: Vec::new(),
        }
    }
}

fn default_mailbox_beacon_interval() -> u32 {
    1200
}

/// Incoming keyboard-to-keyboard mode: when enabled, an unsolicited
/// connection addressed to `node_call` opens a normal live session tab with
/// a welcome message, for a human to type into directly -- unlike the
/// mailbox, there's no command parser, and the tab behaves exactly like any
/// manually-dialed one. Also drives a periodic "available for keyboard
/// chat" beacon while enabled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyboardModePrefs {
    #[serde(default)]
    pub enabled: bool,
    /// The callsign this mode answers as -- deliberately independent of
    /// both `UiPrefs.default_call` and the mailbox's own `respond_call`, so
    /// the two auto-responders can run on different SSIDs of the same
    /// station without one intercepting connects meant for the other.
    /// Empty falls back to `UiPrefs.default_call`, for anyone who hasn't
    /// set it explicitly. Note this only filters *incoming* connects and
    /// picks the identity used in the welcome message -- the beacon and any
    /// connected-mode replies still go out under whichever port sends
    /// them, since AGWPE/AX.25 backends use one fixed callsign per port
    /// (`PortConfig::Agwpe.my_call` / raw-socket's bound local call) for
    /// every frame on it, not a per-feature override. For the beacon/
    /// replies to genuinely appear as this callsign on the air, point
    /// "Listen On" at a port whose own configured callsign is this one.
    #[serde(default)]
    pub node_call: String,
    /// Sent on connect, in place of the generated
    /// `keyboard_mode::default_welcome`. Empty falls back to the generated
    /// greeting.
    #[serde(default)]
    pub welcome_message: String,
    /// Sent as a one-shot unproto frame (destination "CQ") on every enabled
    /// listen port that supports unproto, every `beacon_interval_secs`,
    /// while enabled. Empty means no beacon is sent, even while enabled.
    #[serde(default)]
    pub beacon_text: String,
    #[serde(default = "default_k2k_beacon_interval")]
    pub beacon_interval_secs: u32,
    /// Port ids to listen for unsolicited connections on (and to beacon
    /// over). Empty means "any connect-capable port".
    #[serde(default)]
    pub listen_ports: Vec<String>,
}

impl Default for KeyboardModePrefs {
    fn default() -> Self {
        KeyboardModePrefs {
            enabled: false,
            node_call: String::new(),
            welcome_message: String::new(),
            beacon_text: String::new(),
            beacon_interval_secs: default_k2k_beacon_interval(),
            listen_ports: Vec::new(),
        }
    }
}

fn default_k2k_beacon_interval() -> u32 {
    600
}

/// Preferences for optionally launching Direwolf as a managed child process
/// (this app's own dev/test rig runs against it, and plenty of users run it
/// standalone anyway). Entirely separate from `PortEntry`/`PortConfig` —
/// this only owns the OS process; a port still has to be added and
/// connected normally to actually talk to it over AGWPE/KISS.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DirewolfPrefs {
    /// Start Direwolf automatically when this app starts, mirroring a
    /// port's own `autoconnect`.
    #[serde(default)]
    pub auto_start: bool,
    /// Raw `direwolf.conf` text, written out to a managed file and passed
    /// as `-c` when this app launches Direwolf itself. Kept as one plain
    /// text blob rather than a structured form — Direwolf's config format
    /// is large and varied, and a full parser isn't worth building here.
    #[serde(default)]
    pub config_text: String,
}

/// Desktop notification preferences. Off by default, like the mailbox —
/// firing OS notifications is a side effect the user should opt into. Three
/// independent toggles since these are genuinely different kinds of traffic
/// a user may want to track separately: an incoming connection or a frame
/// directed at your own callsign, a frame matching a user `HighlightRule`
/// with its `notify` flag set, and a frame matching a `BeaconMonitorRule`
/// (tracked in the Incoming Beacons list, not user-destination traffic).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NotifyPrefs {
    #[serde(default)]
    pub directed_enabled: bool,
    #[serde(default)]
    pub custom_enabled: bool,
    #[serde(default)]
    pub beacon_enabled: bool,
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

/// Global on/off switch for every scheduled outgoing beacon at once,
/// independent of each `Beacon.enabled` flag — lives in general
/// `config.toml` (like `mailbox.enabled`/`notify.*`), while the beacon list
/// itself stays in its own `beacons.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeaconPrefs {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for BeaconPrefs {
    fn default() -> Self {
        BeaconPrefs { enabled: true }
    }
}

/// A user-defined rule for detecting "this looks like a beacon" among
/// incoming UI frames, tracked in the Incoming Beacons list. Uses a real
/// regex against the frame's destination — unlike `HighlightRule`'s comma/
/// pipe literal list — since beacon destination formats vary widely (not
/// just literal tokens like "CQ"/"BEACON").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeaconMonitorRule {
    pub id: String,
    pub label: String,
    pub pattern: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// One received frame that matched a `BeaconMonitorRule`, kept for later
/// review in the Incoming Beacons dialog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingBeacon {
    pub id: u64,
    pub port_id: String,
    pub from: String,
    pub to: String,
    pub message: String,
    pub timestamp: String,
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
    /// Lives in its own `rules.toml` (see `AppConfig::load`/`save`) — a
    /// user-managed list, not a preference toggle, so it gets the same
    /// "own file" treatment as ports/address book/etc.
    #[serde(skip)]
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
    /// A font description string like `"Monospace 11"`: everything but a
    /// trailing numeric token is the family name, the trailing number (if
    /// present) is the point size.
    #[serde(default)]
    pub font: Option<String>,
    #[serde(default = "default_true")]
    pub show_timestamps: bool,
    /// Pre-fills the "My Callsign" field when adding a new AGWPE/KISS port.
    /// Also shown as "Callsign" in Preferences' Profile section.
    #[serde(default)]
    pub default_call: Option<String>,
    /// Operator's own name, shown in Preferences' Profile section.
    #[serde(default)]
    pub name: Option<String>,
    /// Free-text location (city/state, grid square, ...), shown in
    /// Preferences' Profile section. Used by the `$$LOC` template
    /// variable in automated message text.
    #[serde(default)]
    pub location: Option<String>,
    /// Home BBS/mailbox address, shown in Preferences' Profile section.
    #[serde(default)]
    pub home_bbs: Option<String>,
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
    /// Max lines kept in a connected tab's *live* scrollback display before
    /// the oldest are trimmed from the GTK buffer — a memory-sanity cap
    /// only. The on-disk history file this feeds is never trimmed, so the
    /// full session is always recoverable there even once the live display
    /// has dropped its earliest lines.
    #[serde(default = "default_tab_buffer_max_lines")]
    pub tab_buffer_max_lines: u32,
}

fn default_history_lines() -> u32 {
    1000
}

fn default_monitor_buffer_lines() -> u32 {
    5000
}

fn default_tab_buffer_max_lines() -> u32 {
    25000
}

fn default_true() -> bool {
    true
}

impl Default for UiPrefs {
    fn default() -> Self {
        UiPrefs {
            font: None,
            show_timestamps: true,
            default_call: None,
            name: None,
            location: None,
            home_bbs: None,
            qrz_username: None,
            qrz_password: None,
            history_lines: default_history_lines(),
            monitor_buffer_lines: default_monitor_buffer_lines(),
            tab_buffer_max_lines: default_tab_buffer_max_lines(),
        }
    }
}

/// General preferences persisted in `config.toml`. Every other data type
/// (ports, address book, QSO log, notified packets, highlight rules, pinned
/// sessions, beacons, mailbox messages) lives in its own single-purpose file
/// under the same config directory — see `AppConfig::load`/`save` — so a
/// human poking around `~/.config/packet-radio/` finds one small file per
/// concern instead of one large one. Each such field keeps its original name
/// and type here (just marked `#[serde(skip)]`) purely so the rest of the
/// app can keep reading `cfg.ports`, `cfg.address_book`, etc. unchanged.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(skip)]
    pub ports: Vec<PortEntry>,
    #[serde(default)]
    pub ui: UiPrefs,
    #[serde(skip)]
    pub address_book: Vec<AddressBookEntry>,
    #[serde(skip)]
    pub pinned_sessions: Vec<PinnedSession>,
    #[serde(default)]
    pub highlighting: HighlightPrefs,
    #[serde(skip)]
    pub beacons: Vec<Beacon>,
    #[serde(default)]
    pub beacon_prefs: BeaconPrefs,
    #[serde(skip)]
    pub beacon_rules: Vec<BeaconMonitorRule>,
    #[serde(skip)]
    pub incoming_beacons: Vec<IncomingBeacon>,
    #[serde(skip)]
    pub qso_log: Vec<QsoLogEntry>,
    #[serde(default)]
    pub mailbox: MailboxPrefs,
    #[serde(default)]
    pub keyboard_mode: KeyboardModePrefs,
    #[serde(default)]
    pub notify: NotifyPrefs,
    #[serde(skip)]
    pub notified_packets: Vec<NotifiedPacket>,
    #[serde(default)]
    pub direwolf: DirewolfPrefs,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PortsFile {
    #[serde(default)]
    ports: Vec<PortEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AddressBookFile {
    #[serde(default)]
    address_book: Vec<AddressBookEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct QsoLogFile {
    #[serde(default)]
    qso_log: Vec<QsoLogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct NotifiedPacketsFile {
    #[serde(default)]
    notified_packets: Vec<NotifiedPacket>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RulesFile {
    #[serde(default)]
    rules: Vec<HighlightRule>,
}

impl Default for RulesFile {
    fn default() -> Self {
        RulesFile { rules: default_rules() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PinnedSessionsFile {
    #[serde(default)]
    pinned_sessions: Vec<PinnedSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct BeaconsFile {
    #[serde(default)]
    beacons: Vec<Beacon>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct MailboxFile {
    #[serde(default)]
    messages: Vec<MailboxMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct BeaconRulesFile {
    #[serde(default)]
    beacon_rules: Vec<BeaconMonitorRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct IncomingBeaconsFile {
    #[serde(default)]
    incoming_beacons: Vec<IncomingBeacon>,
}

fn load_part<T: serde::de::DeserializeOwned + Default>(path: &std::path::Path) -> anyhow::Result<T> {
    if !path.exists() {
        return Ok(T::default());
    }
    let text = std::fs::read_to_string(path)?;
    Ok(toml::from_str(&text)?)
}

fn save_part<T: Serialize>(path: &std::path::Path, value: &T) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(value)?;
    std::fs::write(path, text)?;
    Ok(())
}

/// Best-effort extraction of one top-level (or `section.key`) array out of a
/// legacy, un-split `config.toml`, parsed generically. Used only by
/// `migrate_legacy_config`, once, for a file this app itself wrote — falls
/// back to an empty/default value rather than failing the whole migration if
/// a section is missing or doesn't parse.
fn extract<T: serde::de::DeserializeOwned + Default>(value: &toml::Value, key: &str) -> T {
    value.get(key).cloned().and_then(|v| v.try_into().ok()).unwrap_or_default()
}

fn extract_nested<T: serde::de::DeserializeOwned + Default>(value: &toml::Value, section: &str, key: &str) -> T {
    value
        .get(section)
        .and_then(|s| s.get(key))
        .cloned()
        .and_then(|v| v.try_into().ok())
        .unwrap_or_default()
}

impl AppConfig {
    pub fn config_dir() -> Option<PathBuf> {
        ProjectDirs::from("net", "packetradio", "packet-radio").map(|dirs| dirs.config_dir().to_path_buf())
    }

    pub fn config_path() -> Option<PathBuf> {
        Self::config_dir().map(|dir| dir.join("config.toml"))
    }

    fn ports_path(dir: &std::path::Path) -> PathBuf {
        dir.join("ports.toml")
    }
    fn address_book_path(dir: &std::path::Path) -> PathBuf {
        dir.join("address_book.toml")
    }
    fn qso_log_path(dir: &std::path::Path) -> PathBuf {
        dir.join("qso_log.toml")
    }
    fn notified_packets_path(dir: &std::path::Path) -> PathBuf {
        dir.join("notified_packets.toml")
    }
    fn rules_path(dir: &std::path::Path) -> PathBuf {
        dir.join("rules.toml")
    }
    fn pinned_sessions_path(dir: &std::path::Path) -> PathBuf {
        dir.join("pinned_sessions.toml")
    }
    fn beacons_path(dir: &std::path::Path) -> PathBuf {
        dir.join("beacons.toml")
    }
    fn mailbox_path(dir: &std::path::Path) -> PathBuf {
        dir.join("mailbox.toml")
    }
    fn beacon_rules_path(dir: &std::path::Path) -> PathBuf {
        dir.join("beacon_rules.toml")
    }
    fn incoming_beacons_path(dir: &std::path::Path) -> PathBuf {
        dir.join("incoming_beacons.toml")
    }

    /// One-time migration from the old single-`config.toml` layout: detected
    /// by `config.toml` existing but `ports.toml` not existing yet. Splits
    /// every data section out into its own file (including `node_history`,
    /// which becomes plain-text per-node files under `history/`), then
    /// overwrites `config.toml` with just the general-preferences subset.
    fn migrate_legacy_config(dir: &std::path::Path, main_path: &std::path::Path) -> anyhow::Result<()> {
        let text = std::fs::read_to_string(main_path)?;
        let value: toml::Value = toml::from_str(&text)?;

        let ports: Vec<PortEntry> = extract(&value, "ports");
        let address_book: Vec<AddressBookEntry> = extract(&value, "address_book");
        let qso_log: Vec<QsoLogEntry> = extract(&value, "qso_log");
        let notified_packets: Vec<NotifiedPacket> = extract(&value, "notified_packets");
        let rules: Vec<HighlightRule> = extract_nested(&value, "highlighting", "rules");
        let pinned_sessions: Vec<PinnedSession> = extract(&value, "pinned_sessions");
        let beacons: Vec<Beacon> = extract(&value, "beacons");
        let mailbox_messages: Vec<MailboxMessage> = extract_nested(&value, "mailbox", "messages");
        let node_history: Vec<NodeHistory> = extract(&value, "node_history");

        save_part(&Self::ports_path(dir), &PortsFile { ports: ports.clone() })?;
        save_part(&Self::address_book_path(dir), &AddressBookFile { address_book })?;
        save_part(&Self::qso_log_path(dir), &QsoLogFile { qso_log })?;
        save_part(&Self::notified_packets_path(dir), &NotifiedPacketsFile { notified_packets })?;
        save_part(&Self::rules_path(dir), &RulesFile { rules })?;
        save_part(&Self::pinned_sessions_path(dir), &PinnedSessionsFile { pinned_sessions })?;
        save_part(&Self::beacons_path(dir), &BeaconsFile { beacons })?;
        save_part(&Self::mailbox_path(dir), &MailboxFile { messages: mailbox_messages })?;

        // Unproto history has no on-disk convention any more (every tab is a
        // two-way connection now) -- old unproto-bucketed entries from this
        // legacy format simply aren't migrated forward.
        for h in node_history.iter().filter(|h| !h.unproto) {
            let port_name = ports.iter().find(|p| p.id == h.port_id).map(|p| p.name.as_str()).unwrap_or(&h.port_id);
            let path = crate::history_paths::history_file_path(dir, port_name, &h.remote);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut content = h.lines.join("\n");
            if !h.lines.is_empty() {
                content.push('\n');
            }
            std::fs::write(&path, content)?;
        }

        // Re-parsing the same legacy text through the normal typed path
        // yields exactly the general-preferences subset for free: every
        // now-`#[serde(skip)]`/removed field is simply ignored.
        let slimmed: AppConfig = toml::from_str(&text)?;
        let slimmed_text = toml::to_string_pretty(&slimmed)?;
        std::fs::write(main_path, slimmed_text)?;

        Ok(())
    }

    /// Pre-redesign unproto tabs wrote their own `_unproto.txt` history
    /// files (one per ad-hoc destination); every tab is a two-way
    /// connection now, so anything matching that suffix is permanently
    /// orphaned. Swept on every load rather than gated behind a one-time
    /// flag -- the scan is cheap and a no-op once they're gone.
    fn sweep_orphaned_unproto_history(dir: &std::path::Path) {
        let Ok(port_dirs) = std::fs::read_dir(dir.join("history")) else { return };
        for port_dir in port_dirs.flatten() {
            let Ok(files) = std::fs::read_dir(port_dir.path()) else { continue };
            for file in files.flatten() {
                if file.file_name().to_string_lossy().ends_with("_unproto.txt") {
                    let _ = std::fs::remove_file(file.path());
                }
            }
        }
    }

    pub fn load() -> anyhow::Result<AppConfig> {
        let Some(dir) = Self::config_dir() else {
            return Ok(AppConfig::default());
        };
        let main_path = dir.join("config.toml");

        if main_path.exists() && !Self::ports_path(&dir).exists() {
            Self::migrate_legacy_config(&dir, &main_path)?;
        }
        Self::sweep_orphaned_unproto_history(&dir);

        let mut cfg: AppConfig = if main_path.exists() {
            let text = std::fs::read_to_string(&main_path)?;
            toml::from_str(&text)?
        } else {
            AppConfig::default()
        };

        cfg.ports = load_part::<PortsFile>(&Self::ports_path(&dir))?.ports;
        cfg.address_book = load_part::<AddressBookFile>(&Self::address_book_path(&dir))?.address_book;
        cfg.qso_log = load_part::<QsoLogFile>(&Self::qso_log_path(&dir))?.qso_log;
        cfg.notified_packets = load_part::<NotifiedPacketsFile>(&Self::notified_packets_path(&dir))?.notified_packets;
        cfg.highlighting.rules = load_part::<RulesFile>(&Self::rules_path(&dir))?.rules;
        cfg.pinned_sessions = load_part::<PinnedSessionsFile>(&Self::pinned_sessions_path(&dir))?.pinned_sessions;
        cfg.beacons = load_part::<BeaconsFile>(&Self::beacons_path(&dir))?.beacons;
        cfg.mailbox.messages = load_part::<MailboxFile>(&Self::mailbox_path(&dir))?.messages;
        cfg.beacon_rules = load_part::<BeaconRulesFile>(&Self::beacon_rules_path(&dir))?.beacon_rules;
        cfg.incoming_beacons = load_part::<IncomingBeaconsFile>(&Self::incoming_beacons_path(&dir))?.incoming_beacons;

        Ok(cfg)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let dir = Self::config_dir().ok_or_else(|| anyhow::anyhow!("could not determine config directory"))?;
        std::fs::create_dir_all(&dir)?;

        let main_path = dir.join("config.toml");
        let text = toml::to_string_pretty(self)?;
        std::fs::write(&main_path, text)?;

        save_part(&Self::ports_path(&dir), &PortsFile { ports: self.ports.clone() })?;
        save_part(&Self::address_book_path(&dir), &AddressBookFile { address_book: self.address_book.clone() })?;
        save_part(&Self::qso_log_path(&dir), &QsoLogFile { qso_log: self.qso_log.clone() })?;
        save_part(
            &Self::notified_packets_path(&dir),
            &NotifiedPacketsFile { notified_packets: self.notified_packets.clone() },
        )?;
        save_part(&Self::rules_path(&dir), &RulesFile { rules: self.highlighting.rules.clone() })?;
        save_part(
            &Self::pinned_sessions_path(&dir),
            &PinnedSessionsFile { pinned_sessions: self.pinned_sessions.clone() },
        )?;
        save_part(&Self::beacons_path(&dir), &BeaconsFile { beacons: self.beacons.clone() })?;
        save_part(&Self::mailbox_path(&dir), &MailboxFile { messages: self.mailbox.messages.clone() })?;
        save_part(&Self::beacon_rules_path(&dir), &BeaconRulesFile { beacon_rules: self.beacon_rules.clone() })?;
        save_part(
            &Self::incoming_beacons_path(&dir),
            &IncomingBeaconsFile { incoming_beacons: self.incoming_beacons.clone() },
        )?;

        Ok(())
    }
}
