use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc;

use adw::prelude::*;
use gtk::glib;

use pr_core::{AppConfig, ConnState, ConnectionId, PortCommand, PortEvent};

use crate::address_book_dialog;
use crate::app_state::{find_entry, spawn_for_config, AppState};
use crate::connection_view::ConnectionTab;
use crate::monitor_view::MonitorView;
use crate::ports_dialog;
use crate::preferences_dialog;

pub struct Ui {
    pub state: Rc<AppState>,
    pub monitor: MonitorView,
    pub notebook: gtk::Notebook,
    pub tabs: RefCell<HashMap<(String, ConnectionId), ConnectionTab>>,
    pub window: adw::ApplicationWindow,
}

impl Ui {
    pub fn connect_port(self: &Rc<Self>, id: &str) {
        if self.state.is_active(id) {
            return;
        }
        let entry = match find_entry(&self.state.config.borrow(), id) {
            Some(e) => e,
            None => return,
        };
        self.monitor
            .append_line(&format!("[{}] connecting ({})\u{2026}", entry.name, entry.config.kind_label()));

        let handle = spawn_for_config(&entry.config);
        let cmd_tx = handle.cmd_tx.clone();
        let events = handle.events.clone();
        self.state.active.borrow_mut().insert(id.to_string(), handle);

        // Reopen any sessions the user pinned on this port (including at
        // startup, for an autoconnect port). Commands queue on the port's
        // channel even before its background thread reaches its command
        // loop, so sending these immediately is safe.
        let pinned_remotes: Vec<String> = self
            .state
            .config
            .borrow()
            .pinned_sessions
            .iter()
            .filter(|p| p.port_id == id)
            .map(|p| p.remote.clone())
            .collect();
        for remote in pinned_remotes {
            let _ = cmd_tx.send(PortCommand::OpenConnection { remote });
        }

        let ui = self.clone();
        let port_id = id.to_string();
        glib::spawn_future_local(async move {
            while let Ok(event) = events.recv().await {
                ui.handle_event(&port_id, &cmd_tx, event);
            }
            ui.state.active.borrow_mut().remove(&port_id);
        });
    }

    pub fn disconnect_port(&self, id: &str) {
        if let Some(handle) = self.state.active.borrow().get(id) {
            let _ = handle.cmd_tx.send(PortCommand::Disconnect);
        }
    }

    pub fn open_connection(&self, id: &str, remote: String) {
        if let Some(handle) = self.state.active.borrow().get(id) {
            let _ = handle.cmd_tx.send(PortCommand::OpenConnection { remote });
        }
    }

    pub fn send_unproto(&self, id: &str, dest: String, bytes: Vec<u8>) {
        if let Some(handle) = self.state.active.borrow().get(id) {
            let _ = handle.cmd_tx.send(PortCommand::SendUnproto { dest, bytes });
        }
    }

    /// Tab label with a title plus Pin/Close controls: Pin persists the
    /// (port, remote) pair so the session reopens automatically next time
    /// this port connects; Close ends just this one connection.
    fn build_tab_label(self: &Rc<Self>, port_id: &str, conn_id: ConnectionId, remote: &str, cmd_tx: &mpsc::Sender<PortCommand>) -> gtk::Box {
        let tab_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);

        let title = gtk::Label::new(Some(&format!("{port_id}: {remote}")));
        tab_box.append(&title);

        let pin_toggle = gtk::ToggleButton::builder().label("Pin").active(self.state.is_pinned(port_id, remote)).build();
        pin_toggle.add_css_class("flat");
        {
            let ui = self.clone();
            let port_id = port_id.to_string();
            let remote = remote.to_string();
            pin_toggle.connect_toggled(move |btn| {
                ui.state.set_pinned(&port_id, &remote, btn.is_active());
            });
        }
        tab_box.append(&pin_toggle);

        let close_button = gtk::Button::with_label("\u{2715}");
        close_button.add_css_class("flat");
        {
            let cmd_tx = cmd_tx.clone();
            close_button.connect_clicked(move |_| {
                let _ = cmd_tx.send(PortCommand::CloseConnection { id: conn_id });
            });
        }
        tab_box.append(&close_button);

        tab_box
    }

    fn handle_event(self: &Rc<Self>, port_id: &str, cmd_tx: &mpsc::Sender<PortCommand>, event: PortEvent) {
        match event {
            PortEvent::PortConnected => {
                self.monitor.append_line(&format!("[{port_id}] port connected"));
            }
            PortEvent::PortDisconnected { reason } => {
                let suffix = reason.map(|r| format!(": {r}")).unwrap_or_default();
                self.monitor.append_line(&format!("[{port_id}] port disconnected{suffix}"));
                // The port thread is gone, so any of its connection tabs are
                // now stale (they'll never get a ConnectionClosed event) —
                // close them, or reconnecting would leave duplicate tabs.
                let stale: Vec<ConnectionId> = self
                    .tabs
                    .borrow()
                    .keys()
                    .filter(|(pid, _)| pid == port_id)
                    .map(|(_, id)| *id)
                    .collect();
                for id in stale {
                    if let Some(tab) = self.tabs.borrow_mut().remove(&(port_id.to_string(), id)) {
                        if let Some(page) = self.notebook.page_num(&tab.root) {
                            self.notebook.remove_page(Some(page));
                        }
                    }
                }
            }
            PortEvent::PortError { message } => {
                self.monitor.append_line(&format!("[{port_id}] ERROR: {message}"));
            }
            PortEvent::Monitor { line } => {
                self.monitor.append_line(&format!("[{port_id}] {line}"));
            }
            PortEvent::ConnectionOpened { id, label } => {
                let tab = ConnectionTab::new();
                let cmd_tx2 = cmd_tx.clone();
                tab.entry.connect_activate(move |entry| {
                    let mut bytes = entry.text().to_string().into_bytes();
                    bytes.push(b'\n');
                    let _ = cmd_tx2.send(PortCommand::Send { id, bytes });
                    entry.set_text("");
                });

                let tab_label = self.build_tab_label(port_id, id, &label, cmd_tx);
                self.notebook.append_page(&tab.root, Some(&tab_label));
                self.notebook.set_tab_reorderable(&tab.root, true);
                let page_idx = self.notebook.page_num(&tab.root);
                self.notebook.set_current_page(page_idx);
                self.tabs.borrow_mut().insert((port_id.to_string(), id), tab);
            }
            PortEvent::ConnectionClosed { id } => {
                let key = (port_id.to_string(), id);
                if let Some(tab) = self.tabs.borrow_mut().remove(&key) {
                    if let Some(page) = self.notebook.page_num(&tab.root) {
                        self.notebook.remove_page(Some(page));
                    }
                }
            }
            PortEvent::ConnState { id, state } => {
                self.monitor
                    .append_line(&format!("[{port_id}] connection {id}: {}", describe_state(state)));
            }
            PortEvent::Data { id, bytes } => {
                let key = (port_id.to_string(), id);
                if let Some(tab) = self.tabs.borrow().get(&key) {
                    tab.append_text(&String::from_utf8_lossy(&bytes));
                }
            }
            PortEvent::StationHeard { callsign } => {
                self.state.record_heard(&callsign);
            }
        }
    }
}

/// Apply a font description string (e.g. `"Monospace 11"`) to the Monitor
/// and Connection scrollback views via a CSS provider. GTK4 gives widgets no
/// direct "set font" API anymore, only CSS, and `TextView`'s content lives
/// on an internal `text` child node, so the rule targets both it and the
/// widget itself to make sure the font actually takes effect.
pub fn apply_font(font_desc: &str) {
    let (family, size) = parse_font_desc(font_desc);
    let escaped_family = family.replace('"', "");
    let css = format!(".pr-mono, .pr-mono text {{ font-family: \"{escaped_family}\"; font-size: {size}pt; }}");
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&css);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
}

fn parse_font_desc(desc: &str) -> (String, u32) {
    let desc = desc.trim();
    if desc.is_empty() {
        return ("Monospace".to_string(), 11);
    }
    if let Some(idx) = desc.rfind(' ') {
        let (family, size_str) = desc.split_at(idx);
        if let Ok(size) = size_str.trim().parse::<u32>() {
            return (family.to_string(), size);
        }
    }
    (desc.to_string(), 11)
}

fn describe_state(state: ConnState) -> &'static str {
    match state {
        ConnState::Connecting => "connecting",
        ConnState::Connected => "connected",
        ConnState::Disconnecting => "disconnecting",
        ConnState::Disconnected => "disconnected",
    }
}

pub fn build_ui(app: &adw::Application) {
    let config = AppConfig::load().unwrap_or_else(|e| {
        tracing::warn!("failed to load config, starting fresh: {e}");
        AppConfig::default()
    });
    let show_monitor = config.ui.show_monitor;
    let show_timestamps = config.ui.show_timestamps;
    let font = config.ui.font.clone().unwrap_or_else(|| "Monospace 11".to_string());
    let autoconnect_ids: Vec<String> =
        config.ports.iter().filter(|p| p.autoconnect).map(|p| p.id.clone()).collect();
    apply_font(&font);
    let state = AppState::new(config);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Packet Radio")
        .default_width(1000)
        .default_height(700)
        .build();

    let monitor = MonitorView::new();
    monitor.set_show_timestamps(show_timestamps);
    let notebook = gtk::Notebook::builder().vexpand(true).hexpand(true).scrollable(true).build();

    let paned = gtk::Paned::builder()
        .orientation(gtk::Orientation::Vertical)
        .start_child(&monitor.widget)
        .end_child(&notebook)
        .resize_start_child(true)
        .resize_end_child(true)
        .shrink_start_child(false)
        .shrink_end_child(false)
        .position(220)
        .build();
    paned.set_visible(true);
    monitor.widget.set_visible(show_monitor);

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();

    let ui = Rc::new(Ui {
        state,
        monitor,
        notebook,
        tabs: RefCell::new(HashMap::new()),
        window: window.clone(),
    });

    // "Ports\u{2026}" button opens the Port Manager.
    let ports_button = gtk::Button::with_label("Ports\u{2026}");
    {
        let ui = ui.clone();
        ports_button.connect_clicked(move |_| {
            ports_dialog::show(&ui);
        });
    }
    header.pack_start(&ports_button);

    // "New Connection\u{2026}" opens a connection to a remote station over
    // an already-connected AGWPE/AX.25 port.
    let new_conn_button = gtk::Button::with_label("New Connection\u{2026}");
    {
        let ui = ui.clone();
        new_conn_button.connect_clicked(move |_| {
            ports_dialog::show_new_connection(&ui);
        });
    }
    header.pack_start(&new_conn_button);

    // "Send Beacon\u{2026}" sends a one-shot unconnected (UI) frame over an
    // already-connected AGWPE port.
    let beacon_button = gtk::Button::with_label("Send Beacon\u{2026}");
    {
        let ui = ui.clone();
        beacon_button.connect_clicked(move |_| {
            ports_dialog::show_send_unproto(&ui);
        });
    }
    header.pack_start(&beacon_button);

    // "Address Book\u{2026}" lists stations heard automatically plus manual entries.
    let address_book_button = gtk::Button::with_label("Address Book\u{2026}");
    {
        let ui = ui.clone();
        address_book_button.connect_clicked(move |_| {
            address_book_dialog::show(&ui);
        });
    }
    header.pack_start(&address_book_button);

    // "Preferences\u{2026}" opens font/timestamp/default-callsign settings.
    let prefs_button = gtk::Button::with_label("Preferences\u{2026}");
    {
        let ui = ui.clone();
        prefs_button.connect_clicked(move |_| {
            preferences_dialog::show(&ui);
        });
    }
    header.pack_start(&prefs_button);

    let monitor_toggle = gtk::ToggleButton::builder().label("Monitor").active(show_monitor).build();
    {
        let ui = ui.clone();
        monitor_toggle.connect_toggled(move |btn| {
            ui.monitor.widget.set_visible(btn.is_active());
            ui.state.config.borrow_mut().ui.show_monitor = btn.is_active();
            ui.state.save_config();
        });
    }
    header.pack_end(&monitor_toggle);

    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&paned));
    window.set_content(Some(&toolbar_view));

    window.present();

    for id in autoconnect_ids {
        ui.connect_port(&id);
    }
}
