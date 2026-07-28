use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use pr_agwpe::client::AgwpeRunner;
use pr_ax25::{Ax25RawSocketRunner, KissRunner, KissTransport};
use pr_core::transports::ssh::SshRunner;
use pr_core::transports::telnet::TelnetRunner;
use pr_core::{
    spawn_port, AddressBookEntry, AppConfig, NotifiedPacket, PinnedSession, PortConfig, PortEntry, PortHandle,
    QsoLogEntry,
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
            }
            None => cfg.address_book.push(AddressBookEntry {
                callsign,
                name: None,
                alias: None,
                location: None,
                notes: None,
                last_heard: Some(now),
                heard_count: 1,
            }),
        }
        drop(cfg);
        self.save_config();
    }

    pub fn is_pinned(&self, port_id: &str, remote: &str, unproto: bool) -> bool {
        self.config
            .borrow()
            .pinned_sessions
            .iter()
            .any(|p| p.port_id == port_id && p.remote == remote && p.unproto == unproto)
    }

    /// Pin or unpin a (port, node, mode) tab so its shell (port + node
    /// prefilled, disconnected) is recreated automatically at the next app
    /// startup. Unconditionally replaces any existing entry for the same
    /// (port_id, remote, unproto) with the current `via`, so editing a
    /// pinned tab's via path while pinned keeps it in sync.
    pub fn set_pinned(&self, port_id: &str, remote: &str, unproto: bool, via: &str, pinned: bool) {
        let mut cfg = self.config.borrow_mut();
        cfg.pinned_sessions.retain(|p| !(p.port_id == port_id && p.remote == remote && p.unproto == unproto));
        if pinned {
            cfg.pinned_sessions.push(PinnedSession {
                port_id: port_id.to_string(),
                remote: remote.to_string(),
                via: via.to_string(),
                unproto,
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

    /// `unproto` keeps connected-mode session history separate from unproto
    /// traffic history to the same (port, remote) — they're unrelated
    /// conversations that happen to share a destination callsign. Backed by
    /// a plain-text file under `history/<port>/`, not `AppConfig` — see
    /// `history_path`.
    pub fn history_for(&self, port_id: &str, remote: &str, unproto: bool) -> Vec<String> {
        let cfg = self.config.borrow();
        let Some(path) = history_path(&cfg, port_id, remote, unproto) else {
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

    /// Permanently delete the persisted history for one (port, node, mode) —
    /// used by the tab's "Clear History" action.
    pub fn clear_history(&self, port_id: &str, remote: &str, unproto: bool) {
        let cfg = self.config.borrow();
        let Some(path) = history_path(&cfg, port_id, remote, unproto) else {
            return;
        };
        drop(cfg);
        if let Err(e) = std::fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("failed to remove history file {}: {e}", path.display());
            }
        }
    }

    /// Append one completed line to a (port, node, mode)'s persisted
    /// history file. Unlike the old in-config storage, this is an unbounded
    /// archive — `history_for` applies the line-count cap at read time
    /// instead.
    pub fn append_history_line(&self, port_id: &str, remote: &str, unproto: bool, line: &str) {
        let cfg = self.config.borrow();
        let Some(path) = history_path(&cfg, port_id, remote, unproto) else {
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
        PortConfig::KissTcp { host, port, my_call, kiss_params } => spawn_port(KissRunner {
            transport: KissTransport::Tcp { host: host.clone(), port: *port },
            my_call: my_call.clone(),
            params: kiss_params.clone(),
        }),
        PortConfig::KissSerial { device, baud, my_call, kiss_params } => spawn_port(KissRunner {
            transport: KissTransport::Serial { device: device.clone(), baud: *baud },
            my_call: my_call.clone(),
            params: kiss_params.clone(),
        }),
    }
}

pub fn find_entry(config: &AppConfig, id: &str) -> Option<PortEntry> {
    config.ports.iter().find(|p| p.id == id).cloned()
}

/// Resolve the on-disk path for one (port, node, mode)'s history file —
/// `None` only if the config directory itself can't be determined.
fn history_path(config: &AppConfig, port_id: &str, remote: &str, unproto: bool) -> Option<std::path::PathBuf> {
    let dir = AppConfig::config_dir()?;
    let port_name = find_entry(config, port_id).map(|p| p.name).unwrap_or_else(|| port_id.to_string());
    Some(pr_core::history_file_path(&dir, &port_name, remote, unproto))
}

pub(crate) fn now_timestamp() -> String {
    gtk::glib::DateTime::now_local()
        .and_then(|t| t.format("%Y-%m-%d %H:%M:%S"))
        .map(|s| s.to_string())
        .unwrap_or_default()
}
