use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use pr_agwpe::client::AgwpeRunner;
use pr_ax25::{Ax25RawSocketRunner, KissRunner, KissTransport};
use pr_core::transports::ssh::SshRunner;
use pr_core::transports::telnet::TelnetRunner;
use pr_core::{spawn_port, AddressBookEntry, AppConfig, PortConfig, PortEntry, PortHandle};

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
        let now = gtk::glib::DateTime::now_local()
            .and_then(|t| t.format("%Y-%m-%d %H:%M:%S"))
            .map(|s| s.to_string())
            .unwrap_or_default();

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
        PortConfig::KissTcp { host, port, my_call } => spawn_port(KissRunner {
            transport: KissTransport::Tcp { host: host.clone(), port: *port },
            my_call: my_call.clone(),
        }),
        PortConfig::KissSerial { device, baud, my_call } => spawn_port(KissRunner {
            transport: KissTransport::Serial { device: device.clone(), baud: *baud },
            my_call: my_call.clone(),
        }),
    }
}

pub fn find_entry(config: &AppConfig, id: &str) -> Option<PortEntry> {
    config.ports.iter().find(|p| p.id == id).cloned()
}
