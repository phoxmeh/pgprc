use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use pr_core::{AppConfig, ConnState, ConnectionId, PortCommand, PortEvent};

use crate::address_book_dialog;
use crate::app_state::{find_entry, spawn_for_config, AppState};
use crate::monitor_view::MonitorView;
use crate::ports_dialog;
use crate::preferences_dialog;
use crate::session_tab::{port_needs_node, SessionTab, TabId};

pub struct Ui {
    pub state: Rc<AppState>,
    pub monitor: MonitorView,
    pub notebook: gtk::Notebook,
    pub tabs: RefCell<HashMap<TabId, SessionTab>>,
    /// Live connection (port, id) -> the tab it's bound to.
    bound: RefCell<HashMap<(String, ConnectionId), TabId>>,
    /// A Connect click in progress, keyed by (port_id, remote) — remote is
    /// empty for port kinds with no node concept (Telnet/SSH/KISS), since
    /// those only ever have one connection to bind.
    pending: RefCell<HashMap<(String, String), TabId>>,
    next_tab_id: Cell<TabId>,
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
        let events = handle.events.clone();
        self.state.active.borrow_mut().insert(id.to_string(), handle);

        let ui = self.clone();
        let port_id = id.to_string();
        glib::spawn_future_local(async move {
            while let Ok(event) = events.recv().await {
                ui.handle_event(&port_id, event);
            }
            ui.state.active.borrow_mut().remove(&port_id);
        });
    }

    pub fn disconnect_port(&self, id: &str) {
        if let Some(handle) = self.state.active.borrow().get(id) {
            let _ = handle.cmd_tx.send(PortCommand::Disconnect);
        }
    }

    pub fn send_unproto(&self, id: &str, dest: String, bytes: Vec<u8>) {
        if let Some(handle) = self.state.active.borrow().get(id) {
            let _ = handle.cmd_tx.send(PortCommand::SendUnproto { dest, bytes });
        }
    }

    /// Create a new, disconnected session tab — optionally prefilled with a
    /// (port_id, remote) — and return its id.
    pub fn add_tab(self: &Rc<Self>, prefill: Option<(String, String)>) -> TabId {
        let tab_id = self.next_tab_id.get();
        self.next_tab_id.set(tab_id + 1);

        let ports = self.state.config.borrow().ports.clone();
        let tab = SessionTab::new(ports);

        if let Some((port_id, remote)) = &prefill {
            if let Some(idx) = tab.available_ports.iter().position(|p| &p.id == port_id) {
                tab.port_dropdown.set_selected(idx as u32);
            }
            tab.node_entry.set_text(remote);
            if self.state.is_pinned(port_id, remote) {
                tab.pin_toggle.set_active(true);
                *tab.pinned_identity.borrow_mut() = Some((port_id.clone(), remote.clone()));
            }
        }

        // Clone out the specific widgets we need after moving `tab` into the map.
        let root = tab.root.clone();
        let tab_label = tab.tab_label.clone();
        let pin_toggle = tab.pin_toggle.clone();
        let port_dropdown = tab.port_dropdown.clone();
        let node_entry = tab.node_entry.clone();
        let address_book_button = tab.address_book_button.clone();
        let connect_button = tab.connect_button.clone();
        let disconnect_button = tab.disconnect_button.clone();
        let input_entry = tab.input_entry.clone();

        self.tabs.borrow_mut().insert(tab_id, tab);
        if let Some(tab) = self.tabs.borrow().get(&tab_id) {
            self.update_node_visibility(tab);
            self.update_tab_title(tab);
            self.preview_history(tab);
        }

        {
            let ui = self.clone();
            port_dropdown.connect_selected_notify(move |_| {
                if let Some(tab) = ui.tabs.borrow().get(&tab_id) {
                    ui.update_node_visibility(tab);
                    ui.update_tab_title(tab);
                    ui.preview_history(tab);
                    ui.sync_pin(tab);
                }
            });
        }
        {
            let ui = self.clone();
            node_entry.connect_changed(move |_| {
                if let Some(tab) = ui.tabs.borrow().get(&tab_id) {
                    ui.update_tab_title(tab);
                    ui.sync_pin(tab);
                }
            });
        }
        {
            let ui = self.clone();
            let focus = gtk::EventControllerFocus::new();
            focus.connect_leave(move |_| {
                if let Some(tab) = ui.tabs.borrow().get(&tab_id) {
                    ui.preview_history(tab);
                }
            });
            node_entry.add_controller(focus);
        }
        {
            let ui = self.clone();
            let node_entry = node_entry.clone();
            address_book_button.connect_clicked(move |_| {
                address_book_dialog::pick(&ui, &node_entry);
            });
        }
        {
            let ui = self.clone();
            pin_toggle.connect_toggled(move |btn| {
                if let Some(tab) = ui.tabs.borrow().get(&tab_id) {
                    if btn.is_active() {
                        ui.sync_pin(tab);
                    } else if let Some((old_port, old_remote)) = tab.pinned_identity.borrow_mut().take() {
                        ui.state.set_pinned(&old_port, &old_remote, false);
                    }
                }
            });
        }
        {
            let ui = self.clone();
            connect_button.connect_clicked(move |_| {
                ui.connect_tab(tab_id);
            });
        }
        {
            let ui = self.clone();
            disconnect_button.connect_clicked(move |_| {
                ui.disconnect_tab(tab_id);
            });
        }
        {
            let ui = self.clone();
            input_entry.connect_activate(move |entry| {
                let mut bytes = entry.text().to_string().into_bytes();
                bytes.push(b'\n');
                if let Some(tab) = ui.tabs.borrow().get(&tab_id) {
                    if let (Some(conn_id), Some(port)) = (tab.conn_id.get(), tab.selected_port()) {
                        if let Some(handle) = ui.state.active.borrow().get(&port.id) {
                            let _ = handle.cmd_tx.send(PortCommand::Send { id: conn_id, bytes });
                        }
                    }
                }
                entry.set_text("");
            });
        }

        let close_button = gtk::Button::with_label("\u{2715}");
        close_button.add_css_class("flat");
        {
            let ui = self.clone();
            close_button.connect_clicked(move |_| {
                ui.close_tab(tab_id);
            });
        }

        let tab_label_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        tab_label_box.append(&tab_label);
        tab_label_box.append(&pin_toggle);
        tab_label_box.append(&close_button);

        self.notebook.append_page(&root, Some(&tab_label_box));
        self.notebook.set_tab_reorderable(&root, true);
        let page_idx = self.notebook.page_num(&root);
        self.notebook.set_current_page(page_idx);

        tab_id
    }

    /// Connect the tab's selected port (if not already active) and, for
    /// node-capable ports, open a session to the entered node.
    pub fn connect_tab(self: &Rc<Self>, tab_id: TabId) {
        let Some((port_id, remote, needs_node)) = self.tabs.borrow().get(&tab_id).and_then(|tab| {
            let port = tab.selected_port()?;
            Some((port.id.clone(), tab.node_entry.text().trim().to_uppercase(), port_needs_node(&port.config)))
        }) else {
            return;
        };

        if let Some(tab) = self.tabs.borrow().get(&tab_id) {
            self.preview_history(tab);
        }

        if needs_node && remote.is_empty() {
            self.monitor.append_line("Enter a node/callsign before connecting.");
            return;
        }

        let pending_key = if needs_node { (port_id.clone(), remote.clone()) } else { (port_id.clone(), String::new()) };
        self.pending.borrow_mut().insert(pending_key, tab_id);

        if !self.state.is_active(&port_id) {
            self.connect_port(&port_id);
        }
        if needs_node {
            if let Some(handle) = self.state.active.borrow().get(&port_id) {
                let _ = handle.cmd_tx.send(PortCommand::OpenConnection { remote });
            }
        }
    }

    pub fn disconnect_tab(&self, tab_id: TabId) {
        let Some((port_id, conn_id)) = self.tabs.borrow().get(&tab_id).and_then(|tab| Some((tab.selected_port()?.id.clone(), tab.conn_id.get()?)))
        else {
            return;
        };
        if let Some(handle) = self.state.active.borrow().get(&port_id) {
            let _ = handle.cmd_tx.send(PortCommand::CloseConnection { id: conn_id });
        }
    }

    /// Remove a tab entirely: disconnect it if live, unpin it, and drop its
    /// notebook page.
    pub fn close_tab(self: &Rc<Self>, tab_id: TabId) {
        self.disconnect_tab(tab_id);

        if let Some(tab) = self.tabs.borrow().get(&tab_id) {
            if let Some((old_port, old_remote)) = tab.pinned_identity.borrow_mut().take() {
                self.state.set_pinned(&old_port, &old_remote, false);
            }
        }
        self.bound.borrow_mut().retain(|_, v| *v != tab_id);
        self.pending.borrow_mut().retain(|_, v| *v != tab_id);

        if let Some(tab) = self.tabs.borrow_mut().remove(&tab_id) {
            if let Some(page) = self.notebook.page_num(&tab.root) {
                self.notebook.remove_page(Some(page));
            }
        }
    }

    /// Called after something outside the tab's own signal handlers sets its
    /// node text programmatically (the address-book picker) — the normal
    /// `connect_changed`/focus-out wiring wouldn't otherwise fire for that.
    pub fn refresh_tab_for_node_entry(&self, node_entry: &gtk::Entry) {
        if let Some(tab) = self.tabs.borrow().values().find(|t| &t.node_entry == node_entry) {
            self.update_tab_title(tab);
            self.preview_history(tab);
            self.sync_pin(tab);
        }
    }

    fn update_node_visibility(&self, tab: &SessionTab) {
        let needs_node = tab.selected_port().map(|p| port_needs_node(&p.config)).unwrap_or(false);
        tab.node_row.set_visible(needs_node);
    }

    fn update_tab_title(&self, tab: &SessionTab) {
        let port_name = tab.selected_port().map(|p| p.name.as_str()).unwrap_or("(no port)");
        let remote = tab.node_entry.text();
        let title = if remote.trim().is_empty() { port_name.to_string() } else { format!("{port_name}: {remote}") };
        tab.tab_label.set_text(&title);
    }

    /// Load a previous node's history into the scrollback, but only while
    /// disconnected — never clobber a live session's display.
    fn preview_history(&self, tab: &SessionTab) {
        if tab.conn_id.get().is_some() {
            return;
        }
        let Some(port) = tab.selected_port() else {
            tab.clear_text();
            return;
        };
        let remote = tab.node_entry.text().trim().to_uppercase();
        if remote.is_empty() {
            tab.clear_text();
            return;
        }
        let history = self.state.history_for(&port.id, &remote);
        tab.load_history(&history);
    }

    /// Keep a pinned tab's persisted (port, node) identity in sync as the
    /// user edits its fields, unpinning the stale identity in the process.
    fn sync_pin(&self, tab: &SessionTab) {
        if !tab.pin_toggle.is_active() {
            return;
        }
        let Some(port) = tab.selected_port() else { return };
        let remote = tab.node_entry.text().trim().to_uppercase();
        if remote.is_empty() {
            return;
        }
        let new_id = (port.id.clone(), remote);
        let mut current = tab.pinned_identity.borrow_mut();
        if current.as_ref() != Some(&new_id) {
            if let Some((old_port, old_remote)) = current.take() {
                self.state.set_pinned(&old_port, &old_remote, false);
            }
            self.state.set_pinned(&new_id.0, &new_id.1, true);
            *current = Some(new_id);
        }
    }

    fn handle_event(self: &Rc<Self>, port_id: &str, event: PortEvent) {
        match event {
            PortEvent::PortConnected => {
                self.monitor.append_line(&format!("[{port_id}] port connected"));
            }
            PortEvent::PortDisconnected { reason } => {
                let suffix = reason.map(|r| format!(": {r}")).unwrap_or_default();
                self.monitor.append_line(&format!("[{port_id}] port disconnected{suffix}"));

                // Unbind (not remove — tabs persist) every tab this port had.
                let affected: Vec<(String, ConnectionId)> =
                    self.bound.borrow().keys().filter(|(pid, _)| pid == port_id).cloned().collect();
                for key in affected {
                    if let Some(tab_id) = self.bound.borrow_mut().remove(&key) {
                        if let Some(tab) = self.tabs.borrow().get(&tab_id) {
                            tab.conn_id.set(None);
                            tab.set_connected(false);
                        }
                    }
                }
                self.pending.borrow_mut().retain(|(pid, _), _| pid != port_id);
            }
            PortEvent::PortError { message } => {
                self.monitor.append_line(&format!("[{port_id}] ERROR: {message}"));
            }
            PortEvent::Monitor { line } => {
                self.monitor.append_line(&format!("[{port_id}] {line}"));
            }
            PortEvent::ConnectionOpened { id, label } => {
                let needs_node =
                    find_entry(&self.state.config.borrow(), port_id).map(|e| port_needs_node(&e.config)).unwrap_or(false);
                let pending_key =
                    if needs_node { (port_id.to_string(), label.clone()) } else { (port_id.to_string(), String::new()) };

                let tab_id = self
                    .pending
                    .borrow_mut()
                    .remove(&pending_key)
                    .unwrap_or_else(|| self.add_tab(Some((port_id.to_string(), label.clone()))));

                self.bound.borrow_mut().insert((port_id.to_string(), id), tab_id);
                if let Some(tab) = self.tabs.borrow().get(&tab_id) {
                    tab.conn_id.set(Some(id));
                    tab.set_connected(true);
                    if needs_node && tab.node_entry.text().is_empty() {
                        tab.node_entry.set_text(&label);
                    }
                    self.update_tab_title(tab);
                }
            }
            PortEvent::ConnectionClosed { id } => {
                if let Some(tab_id) = self.bound.borrow_mut().remove(&(port_id.to_string(), id)) {
                    if let Some(tab) = self.tabs.borrow().get(&tab_id) {
                        tab.conn_id.set(None);
                        tab.set_connected(false);
                        self.flush_pending_line(tab);
                    }
                }
            }
            PortEvent::ConnState { id, state } => {
                self.monitor
                    .append_line(&format!("[{port_id}] connection {id}: {}", describe_state(state)));
            }
            PortEvent::Data { id, bytes } => {
                if let Some(&tab_id) = self.bound.borrow().get(&(port_id.to_string(), id)) {
                    if let Some(tab) = self.tabs.borrow().get(&tab_id) {
                        let text = String::from_utf8_lossy(&bytes).replace('\0', "");
                        tab.append_text(&text);
                        // Only node-capable ports have anything meaningful to
                        // preview later (see `preview_history`), so don't
                        // bother persisting history for the rest.
                        if let Some(port) = tab.selected_port().filter(|p| port_needs_node(&p.config)) {
                            let port_id = port.id.clone();
                            let remote = tab.node_entry.text().to_string();
                            let mut pending_line = tab.pending_line.borrow_mut();
                            pending_line.push_str(&text);
                            while let Some(pos) = pending_line.find('\n') {
                                let line: String = pending_line.drain(..=pos).collect();
                                self.state.append_history_line(&port_id, &remote, line.trim_end_matches(['\r', '\n']));
                            }
                        }
                    }
                }
            }
            PortEvent::StationHeard { callsign } => {
                self.state.record_heard(&callsign);
            }
        }
    }

    fn flush_pending_line(&self, tab: &SessionTab) {
        let Some(port) = tab.selected_port().filter(|p| port_needs_node(&p.config)) else { return };
        let port_id = port.id.clone();
        let remote = tab.node_entry.text().to_string();
        let mut pending_line = tab.pending_line.borrow_mut();
        if !pending_line.is_empty() {
            self.state.append_history_line(&port_id, &remote, pending_line.trim_end_matches(['\r', '\n']));
            pending_line.clear();
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
    let pinned_tabs: Vec<(String, String)> =
        config.pinned_sessions.iter().map(|p| (p.port_id.clone(), p.remote.clone())).collect();
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
        bound: RefCell::new(HashMap::new()),
        pending: RefCell::new(HashMap::new()),
        next_tab_id: Cell::new(0),
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

    // "+ New Tab" creates a disconnected session tab: pick a port, a node
    // (if the port supports one), then Connect explicitly.
    let new_tab_button = gtk::Button::with_label("+ New Tab");
    {
        let ui = ui.clone();
        new_tab_button.connect_clicked(move |_| {
            ui.add_tab(None);
        });
    }
    header.pack_start(&new_tab_button);

    // "Send Beacon\u{2026}" sends a one-shot unconnected (UI) frame over an
    // already-connected AGWPE/KISS port.
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

    // "Preferences\u{2026}" opens font/timestamp/history/default-callsign settings.
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

    // Pinned tabs are recreated as disconnected shells; they never
    // auto-connect, even if their port also has autoconnect enabled.
    for (port_id, remote) in pinned_tabs {
        ui.add_tab(Some((port_id, remote)));
    }

    for id in autoconnect_ids {
        ui.connect_port(&id);
    }
}
