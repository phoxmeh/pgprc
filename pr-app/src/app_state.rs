use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use pr_agwpe::client::AgwpeRunner;
use pr_ax25::{Ax25RawSocketRunner, KissRunner, KissTransport};
use pr_core::transports::ssh::SshRunner;
use pr_core::transports::telnet::TelnetRunner;
use pr_core::{
    spawn_port, AddressBookEntry, AppConfig, IncomingBeacon, NotifiedPacket, PinnedSession, PortConfig, PortEntry,
    PortHandle, QsoLogEntry,
};

pub struct AppState {
    pub config: RefCell<AppConfig>,
    /// Port entry id -> live handle, present only while that port is connected.
    pub active: RefCell<HashMap<String, PortHandle>>,
    /// Cached QRZ.com session key, valid for ~24h server-side. Deliberately
    /// not persisted: it's a live auth token, not configuration.
    pub qrz_session: RefCell<Option<String>>,
}

impl AppState {
    pub fn new(config: AppConfig) -> Rc<Self> {
        Rc::new(AppState {
            config: RefCell::new(config),
            active: RefCell::new(HashMap::new()),
            qrz_session: RefCell::new(None),
        })
    }

    pub fn is_active(&self, id: &str) -> bool {
        self.active.borrow().contains_key(id)
    }

    pub fn save_config(&self) {
        if let Err(e) = self.config.borrow().save() {
            tracing::warn!("failed to save config: {e}");
        }
    }

    /// Record that `callsign` was just heard: bump its entry's `last_heard`
    /// timestamp and `heard_count`, creating the entry if this is the first
    /// time. Manually-entered name/notes on an existing entry are preserved.
    pub fn record_heard(&self, callsign: &str) {
        let callsign = callsign.trim().to_uppercase();
        if callsign.is_empty() {
            return;
        }
        let now = now_timestamp();

        let mut cfg = self.config.borrow_mut();
        match cfg.address_book.iter_mut().find(|e| e.callsign == callsign) {
            Some(entry) => {
                entry.last_heard = Some(now);
                entry.heard_count += 1;
                entry.heard_direct = true;
            }
            None => cfg.address_book.push(AddressBookEntry {
                callsign,
                name: None,
                alias: None,
                location: None,
                notes: None,
                last_heard: Some(now),
                heard_count: 1,
                via: String::new(),
                home_bbs: String::new(),
                heard_direct: true,
                recent_beacons: Vec::new(),
            }),
        }
        drop(cfg);
        self.save_config();
    }

    /// Record entries seen in a NET/ROM NODES routing broadcast from `from`
    /// (which we did directly hear, hence also gets a direct-hear bump via
    /// `record_heard`): each listed destination is added/refreshed as an
    /// *indirect* sighting — we only know about it because `from` mentioned
    /// it, not because we heard it ourselves — unless it's already known
    /// directly, in which case only its `last_heard` is refreshed.
    /// `heard_count` is direct-hear telemetry only and is never touched
    /// here. Aliases are backfilled only when currently empty, so a
    /// user-set alias (or one learned from an earlier, still-accurate
    /// broadcast) is never clobbered.
    pub fn record_nodes_broadcast(&self, from: &str, sender_alias: &str, entries: &[pr_core::NodesBroadcastEntry]) {
        let from = from.trim().to_uppercase();
        if from.is_empty() {
            return;
        }
        self.record_heard(&from);

        let now = now_timestamp();
        let mut cfg = self.config.borrow_mut();

        if !sender_alias.is_empty() {
            if let Some(entry) = cfg.address_book.iter_mut().find(|e| e.callsign == from) {
                if entry.alias.as_deref().unwrap_or("").is_empty() {
                    entry.alias = Some(sender_alias.to_string());
                }
            }
        }

        for entry in entries {
            let callsign = entry.callsign.trim().to_uppercase();
            if callsign.is_empty() {
                continue;
            }
            let alias = entry.alias.trim();
            match cfg.address_book.iter_mut().find(|e| e.callsign == callsign) {
                Some(existing) => {
                    existing.last_heard = Some(now.clone());
                    if !existing.heard_direct && !alias.is_empty() && existing.alias.as_deref().unwrap_or("").is_empty() {
                        existing.alias = Some(alias.to_string());
                    }
                }
                None => cfg.address_book.push(AddressBookEntry {
                    callsign,
                    name: None,
                    alias: if alias.is_empty() { None } else { Some(alias.to_string()) },
                    location: None,
                    notes: None,
                    last_heard: Some(now.clone()),
                    heard_count: 0,
                    via: String::new(),
                    home_bbs: String::new(),
                    heard_direct: false,
                    recent_beacons: Vec::new(),
                }),
            }
        }
        drop(cfg);
        self.save_config();
    }

    /// Record one message a station sent to destination "BEACON", keeping
    /// only the last 5 *unique* texts (a repeat moves back to the front
    /// with a fresh timestamp instead of adding a duplicate). No-op for an
    /// empty message.
    pub fn record_beacon_packet(&self, from: &str, message: &str) {
        let from = from.trim().to_uppercase();
        if from.is_empty() || message.is_empty() {
            return;
        }
        let now = now_timestamp();

        let mut cfg = self.config.borrow_mut();
        let entry = match cfg.address_book.iter_mut().find(|e| e.callsign == from) {
            Some(entry) => entry,
            None => {
                cfg.address_book.push(AddressBookEntry {
                    callsign: from.clone(),
                    name: None,
                    alias: None,
                    location: None,
                    notes: None,
                    last_heard: None,
                    heard_count: 0,
                    via: String::new(),
                    home_bbs: String::new(),
                    heard_direct: false,
                    recent_beacons: Vec::new(),
                });
                cfg.address_book.iter_mut().find(|e| e.callsign == from).expect("just pushed")
            }
        };
        entry.recent_beacons.retain(|b| b.text != message);
        entry.recent_beacons.insert(0, pr_core::BeaconPacketLogEntry { text: message.to_string(), when: now });
        entry.recent_beacons.truncate(5);
        drop(cfg);
        self.save_config();
    }

    pub fn is_pinned(&self, port_id: &str, remote: &str) -> bool {
        self.config.borrow().pinned_sessions.iter().any(|p| p.port_id == port_id && p.remote == remote)
    }

    /// Pin or unpin a (port, node) tab so its shell (port + node prefilled,
    /// disconnected) is recreated automatically at the next app startup.
    /// Unconditionally replaces any existing entry for the same
    /// (port_id, remote) with the current `via`, so editing a pinned tab's
    /// via path while pinned keeps it in sync.
    pub fn set_pinned(&self, port_id: &str, remote: &str, via: &str, pinned: bool) {
        let mut cfg = self.config.borrow_mut();
        cfg.pinned_sessions.retain(|p| !(p.port_id == port_id && p.remote == remote));
        if pinned {
            cfg.pinned_sessions.push(PinnedSession {
                port_id: port_id.to_string(),
                remote: remote.to_string(),
                via: via.to_string(),
            });
        }
        drop(cfg);
        self.save_config();
    }

    /// Log the start of a real connected-mode QSO, for ADIF export — call
    /// only for `port_supports_connect` ports (Telnet/SSH aren't ham-radio
    /// contacts, KISS has no connected mode).
    pub fn log_qso_started(&self, port_id: &str, callsign: &str) {
        let callsign = callsign.trim().to_uppercase();
        if callsign.is_empty() {
            return;
        }
        let mut cfg = self.config.borrow_mut();
        cfg.qso_log.push(QsoLogEntry { callsign, port_id: port_id.to_string(), started: now_timestamp(), ended: None });
        drop(cfg);
        self.save_config();
    }

    /// Fill in the `ended` timestamp of the most recent still-open QSO log
    /// entry for this (port, callsign).
    pub fn log_qso_ended(&self, port_id: &str, callsign: &str) {
        let callsign = callsign.trim().to_uppercase();
        let mut cfg = self.config.borrow_mut();
        if let Some(entry) =
            cfg.qso_log.iter_mut().rev().find(|e| e.port_id == port_id && e.callsign == callsign && e.ended.is_none())
        {
            entry.ended = Some(now_timestamp());
        }
        drop(cfg);
        self.save_config();
    }

    /// Every tab is a two-way connection now (unproto has no per-node
    /// history of its own — see the packet_radio project memory's redesign
    /// notes). Backed by a plain-text file under `history/<port>/`, not
    /// `AppConfig` — see `history_path`.
    pub fn history_for(&self, port_id: &str, remote: &str) -> Vec<String> {
        let cfg = self.config.borrow();
        let Some(path) = history_path(&cfg, port_id, remote) else {
            return Vec::new();
        };
        let max_lines = cfg.ui.history_lines as usize;
        drop(cfg);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Vec::new();
        };
        let lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
        if lines.len() > max_lines {
            lines[lines.len() - max_lines..].to_vec()
        } else {
            lines
        }
    }

    /// Permanently delete the persisted history for one (port, node) —
    /// used by the tab's "Clear History" action.
    pub fn clear_history(&self, port_id: &str, remote: &str) {
        let cfg = self.config.borrow();
        let Some(path) = history_path(&cfg, port_id, remote) else {
            return;
        };
        drop(cfg);
        if let Err(e) = std::fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("failed to remove history file {}: {e}", path.display());
            }
        }
    }

    /// Append one completed line to a (port, node)'s persisted history
    /// file. Unlike the old in-config storage, this is an unbounded
    /// archive — `history_for` applies the line-count cap at read time
    /// instead.
    pub fn append_history_line(&self, port_id: &str, remote: &str, line: &str) {
        let cfg = self.config.borrow();
        let Some(path) = history_path(&cfg, port_id, remote) else {
            return;
        };
        drop(cfg);
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!("failed to create history dir {}: {e}", parent.display());
                return;
            }
        }
        use std::io::Write;
        match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            Ok(mut f) => {
                if let Err(e) = writeln!(f, "{line}") {
                    tracing::warn!("failed to append history line to {}: {e}", path.display());
                }
            }
            Err(e) => tracing::warn!("failed to open history file {}: {e}", path.display()),
        }
    }

    /// Record a packet whose destination triggered a desktop notification,
    /// for later review in the Notified Packets dialog — these are
    /// transient OS popups otherwise, easy to miss if you weren't looking.
    pub fn record_notified_packet(&self, port_id: &str, line: &str) {
        let mut cfg = self.config.borrow_mut();
        let id = cfg.notified_packets.iter().map(|p| p.id).max().unwrap_or(0) + 1;
        cfg.notified_packets.push(NotifiedPacket {
            id,
            port_id: port_id.to_string(),
            line: line.to_string(),
            timestamp: now_timestamp(),
        });
        drop(cfg);
        self.save_config();
    }

    /// Remove one entry from the Notified Packets list — used by its
    /// two-click (arm, then confirm) delete button.
    pub fn remove_notified_packet(&self, id: u64) {
        let mut cfg = self.config.borrow_mut();
        cfg.notified_packets.retain(|p| p.id != id);
        drop(cfg);
        self.save_config();
    }

    /// Record a received frame that matched a `BeaconMonitorRule`, for later
    /// review in the Incoming Beacons dialog.
    pub fn record_incoming_beacon(&self, port_id: &str, from: &str, to: &str, message: &str) {
        let mut cfg = self.config.borrow_mut();
        let id = cfg.incoming_beacons.iter().map(|b| b.id).max().unwrap_or(0) + 1;
        cfg.incoming_beacons.push(IncomingBeacon {
            id,
            port_id: port_id.to_string(),
            from: from.to_string(),
            to: to.to_string(),
            message: message.to_string(),
            timestamp: now_timestamp(),
        });
        drop(cfg);
        self.save_config();
    }

    /// Remove one entry from the Incoming Beacons list — used by its
    /// two-click (arm, then confirm) delete button.
    pub fn remove_incoming_beacon(&self, id: u64) {
        let mut cfg = self.config.borrow_mut();
        cfg.incoming_beacons.retain(|b| b.id != id);
        drop(cfg);
        self.save_config();
    }

    /// Permanently clear the whole Incoming Beacons list — used by its
    /// "Clear All" action, after user confirmation.
    pub fn clear_incoming_beacons(&self) {
        self.config.borrow_mut().incoming_beacons.clear();
        self.save_config();
    }
}

/// Spawn the background thread appropriate for this port's configuration.
pub fn spawn_for_config(config: &PortConfig) -> PortHandle {
    match config {
        PortConfig::Telnet { host, port } => spawn_port(TelnetRunner {
            host: host.clone(),
            port: *port,
        }),
        PortConfig::Ssh { host, port, user } => spawn_port(SshRunner {
            host: host.clone(),
            port: *port,
            user: user.clone(),
        }),
        PortConfig::Agwpe { host, port, radio_port, my_call, login } => spawn_port(AgwpeRunner {
            host: host.clone(),
            tcp_port: *port,
            radio_port: *radio_port,
            my_call: my_call.clone(),
            login: login.as_ref().map(|l| (l.username.clone(), l.password.clone())),
        }),
        PortConfig::Ax25RawSocket { device } => spawn_port(Ax25RawSocketRunner { device: device.clone() }),
        PortConfig::KissTcp { host, port, my_call, kiss_params, kiss_arq } => spawn_port(KissRunner {
            transport: KissTransport::Tcp { host: host.clone(), port: *port },
            my_call: my_call.clone(),
            params: kiss_params.clone(),
            arq: kiss_arq.clone(),
        }),
        PortConfig::KissSerial { device, baud, my_call, kiss_params, kiss_arq } => spawn_port(KissRunner {
            transport: KissTransport::Serial { device: device.clone(), baud: *baud },
            my_call: my_call.clone(),
            params: kiss_params.clone(),
            arq: kiss_arq.clone(),
        }),
    }
}

pub fn find_entry(config: &AppConfig, id: &str) -> Option<PortEntry> {
    config.ports.iter().find(|p| p.id == id).cloned()
}

/// Resolve the on-disk path for one (port, node)'s history file — `None`
/// only if the config directory itself can't be determined.
fn history_path(config: &AppConfig, port_id: &str, remote: &str) -> Option<std::path::PathBuf> {
    let dir = AppConfig::config_dir()?;
    let port_name = find_entry(config, port_id).map(|p| p.name).unwrap_or_else(|| port_id.to_string());
    Some(pr_core::history_file_path(&dir, &port_name, remote))
}

pub(crate) fn now_timestamp() -> String {
    gtk::glib::DateTime::now_local()
        .and_then(|t| t.format("%Y-%m-%d %H:%M:%S"))
        .map(|s| s.to_string())
        .unwrap_or_default()
}
