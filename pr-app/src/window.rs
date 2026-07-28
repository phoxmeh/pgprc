use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use pr_core::{AppConfig, ConnState, ConnectionId, PortCommand, PortEvent};

use crate::address_book_dialog;
use crate::app_state::{find_entry, spawn_for_config, AppState};
use crate::beacons_dialog;
use crate::mailbox_dialog;
use crate::monitor_view::MonitorView;
use crate::ports_dialog;
use crate::preferences_dialog;
use crate::session_tab::{port_supports_connect, port_supports_unproto, SessionTab, TabId};

/// Prefills a newly-created tab's shell: port + node, and optionally a via
/// path and/or unproto mode. Used both for pinned-tab restoration at startup
/// and for the fallback tab created when an unsolicited connection arrives.
pub struct TabPrefill {
    pub port_id: String,
    pub remote: String,
    pub via: String,
    pub unproto: bool,
}

pub struct Ui {
    pub state: Rc<AppState>,
    pub monitor: MonitorView,
    pub notebook: gtk::Notebook,
    /// Swaps between the notebook and an empty-state placeholder with its
    /// own "+ New Tab" button — GTK hides the notebook's entire tab strip
    /// (and with it, the "+" action widget on the tab line) when it has zero
    /// pages, so that's otherwise the *only* way to create the first tab.
    notebook_stack: gtk::Stack,
    pub tabs: RefCell<HashMap<TabId, SessionTab>>,
    /// Live connection (port, id) -> the tab it's bound to.
    bound: RefCell<HashMap<(String, ConnectionId), TabId>>,
    /// A Connect click in progress, keyed by (port_id, remote) — remote is
    /// empty for port kinds with no node concept (Telnet/SSH), since those
    /// only ever have one connection to bind.
    pending: RefCell<HashMap<(String, String), TabId>>,
    next_tab_id: Cell<TabId>,
    /// Scheduled beacon timers, keyed by `Beacon.id` — reset in full by
    /// `reschedule_beacons` whenever the beacon list changes.
    beacon_timers: RefCell<HashMap<String, glib::SourceId>>,
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

    pub fn send_unproto(&self, id: &str, dest: String, via: Vec<String>, bytes: Vec<u8>) {
        if let Some(handle) = self.state.active.borrow().get(id) {
            let _ = handle.cmd_tx.send(PortCommand::SendUnproto { dest, via, bytes });
        }
    }

    /// (Re)schedule every enabled beacon from the current config, discarding
    /// any previously-scheduled timers first. Call this once at startup and
    /// again whenever the beacon list is edited/saved.
    pub fn reschedule_beacons(self: &Rc<Self>) {
        for (_, source) in self.beacon_timers.borrow_mut().drain() {
            source.remove();
        }
        for beacon in self.state.config.borrow().beacons.iter().filter(|b| b.enabled) {
            let ui = self.clone();
            let port_id = beacon.port_id.clone();
            let dest = beacon.dest.clone();
            let via: Vec<String> = beacon.via.split([',', ' ']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_uppercase).collect();
            let message = beacon.message.clone();
            let source = glib::source::timeout_add_seconds_local(beacon.interval_secs.max(1), move || {
                // Skip silently if the port isn't up — matches the existing
                // unproto-send behavior elsewhere, no need to spam the
                // Monitor every interval while a port happens to be down.
                if ui.state.is_active(&port_id) {
                    ui.send_unproto(&port_id, dest.clone(), via.clone(), message.clone().into_bytes());
                }
                glib::ControlFlow::Continue
            });
            self.beacon_timers.borrow_mut().insert(beacon.id.clone(), source);
        }
    }

    /// Create a new, disconnected session tab — optionally prefilled — and
    /// return its id.
    pub fn add_tab(self: &Rc<Self>, prefill: Option<TabPrefill>) -> TabId {
        let tab_id = self.next_tab_id.get();
        self.next_tab_id.set(tab_id + 1);

        let ports = self.state.config.borrow().ports.clone();
        let address_book = self.state.config.borrow().address_book.clone();
        let tab = SessionTab::new(ports, address_book, self.state.clone());

        if let Some(prefill) = &prefill {
            if let Some(idx) = tab.available_ports.iter().position(|p| p.id == prefill.port_id) {
                tab.port_dropdown.set_selected(idx as u32);
            }
            tab.node_entry.set_text(&prefill.remote);
            tab.via_entry.set_text(&prefill.via);
            tab.unproto_toggle.set_active(prefill.unproto);
            if self.state.is_pinned(&prefill.port_id, &prefill.remote, prefill.unproto) {
                tab.pin_toggle.set_active(true);
                *tab.pinned_identity.borrow_mut() = Some((prefill.port_id.clone(), prefill.remote.clone(), prefill.unproto));
            }
        }

        // Clone out the specific widgets we need after moving `tab` into the map.
        let root = tab.root.clone();
        let tab_label = tab.tab_label.clone();
        let pin_toggle = tab.pin_toggle.clone();
        let port_dropdown = tab.port_dropdown.clone();
        let node_entry = tab.node_entry.clone();
        let via_entry = tab.via_entry.clone();
        let address_book_dropdown = tab.address_book_dropdown.clone();
        let unproto_toggle = tab.unproto_toggle.clone();
        let connect_button = tab.connect_button.clone();
        let disconnect_button = tab.disconnect_button.clone();
        let save_button = tab.save_button.clone();
        let clear_history_button = tab.clear_history_button.clone();
        let input_entry = tab.input_entry.clone();
        let send_input_button = tab.send_input_button.clone();

        self.tabs.borrow_mut().insert(tab_id, tab);
        if let Some(tab) = self.tabs.borrow().get(&tab_id) {
            self.update_node_visibility(tab);
            self.update_mode_controls(tab);
            self.update_tab_title(tab);
            self.preview_history(tab);
        }

        {
            let ui = self.clone();
            port_dropdown.connect_selected_notify(move |_| {
                if let Some(tab) = ui.tabs.borrow().get(&tab_id) {
                    ui.update_node_visibility(tab);
                    ui.update_mode_controls(tab);
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
            via_entry.connect_changed(move |_| {
                if let Some(tab) = ui.tabs.borrow().get(&tab_id) {
                    ui.sync_pin(tab);
                }
            });
        }
        {
            let ui = self.clone();
            address_book_dropdown.connect_selected_notify(move |dropdown| {
                if let Some(tab) = ui.tabs.borrow().get(&tab_id) {
                    if let Some(callsign) = tab.selected_address_book_call() {
                        tab.node_entry.set_text(callsign);
                        dropdown.set_selected(0);
                        ui.refresh_tab_for_node_entry(&tab.node_entry);
                    }
                }
            });
        }
        {
            let ui = self.clone();
            unproto_toggle.connect_toggled(move |_| {
                if let Some(tab) = ui.tabs.borrow().get(&tab_id) {
                    ui.update_mode_controls(tab);
                    ui.update_tab_title(tab);
                    ui.preview_history(tab);
                    ui.sync_pin(tab);
                }
            });
        }
        {
            let ui = self.clone();
            pin_toggle.connect_toggled(move |btn| {
                // Toggled explicitly (rather than relying on a `:checked`
                // CSS pseudo-class) so the pinned color is unambiguous and
                // doesn't depend on how a given GTK theme styles toggle
                // buttons.
                if btn.is_active() {
                    btn.add_css_class("pin-pinned");
                } else {
                    btn.remove_css_class("pin-pinned");
                }
                if let Some(tab) = ui.tabs.borrow().get(&tab_id) {
                    if btn.is_active() {
                        ui.sync_pin(tab);
                    } else if let Some((old_port, old_remote, old_unproto)) = tab.pinned_identity.borrow_mut().take() {
                        ui.state.set_pinned(&old_port, &old_remote, old_unproto, "", false);
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
            save_button.connect_clicked(move |_| {
                if let Some(tab) = ui.tabs.borrow().get(&tab_id) {
                    let name = tab.tab_label.text().to_string().replace([':', ' ', '/'], "_");
                    crate::export::save_text(&ui.window, &format!("{name}.txt"), tab.full_text());
                }
            });
        }
        {
            let ui = self.clone();
            clear_history_button.connect_clicked(move |_| {
                ui.confirm_clear_history(tab_id);
            });
        }
        {
            let ui = self.clone();
            let entry_for_button = input_entry.clone();
            input_entry.connect_activate(move |entry| {
                ui.activate_input(tab_id, entry);
            });
            let ui = self.clone();
            send_input_button.connect_clicked(move |_| {
                ui.activate_input(tab_id, &entry_for_button);
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
        self.update_notebook_stack();

        tab_id
    }

    /// Show the notebook once it has at least one page, otherwise the
    /// empty-state placeholder (see `notebook_stack`'s doc comment).
    fn update_notebook_stack(&self) {
        let name = if self.notebook.n_pages() == 0 { "empty" } else { "notebook" };
        self.notebook_stack.set_visible_child_name(name);
    }

    /// Prompt to confirm, then permanently clear the persisted history *and*
    /// the visible scrollback for whatever (port, node, mode) the tab is
    /// currently showing.
    fn confirm_clear_history(self: &Rc<Self>, tab_id: TabId) {
        let Some((port_id, remote, unproto)) = self.tabs.borrow().get(&tab_id).and_then(|tab| tab.history_key())
        else {
            self.monitor.append_line("Nothing to clear \u{2014} select a port and node first.");
            return;
        };

        let dialog = adw::AlertDialog::builder()
            .heading("Clear History?")
            .body(format!("This permanently deletes the saved history for {remote}. This can't be undone."))
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("clear", "Clear");
        dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let ui = self.clone();
        dialog.choose(&self.window, gtk::gio::Cancellable::NONE, move |response| {
            if response == "clear" {
                ui.state.clear_history(&port_id, &remote, unproto);
                if let Some(tab) = ui.tabs.borrow().get(&tab_id) {
                    tab.clear_text();
                }
            }
        });
    }

    /// Send whatever's currently in a tab's input entry — shared by both
    /// pressing Enter and clicking the Send button next to it.
    fn activate_input(self: &Rc<Self>, tab_id: TabId, entry: &gtk::Entry) {
        let text = entry.text().to_string();
        if let Some(tab) = self.tabs.borrow().get(&tab_id) {
            if tab.unproto_toggle.is_active() {
                self.send_tab_unproto(tab, &text);
            } else {
                self.send_tab_connected(tab, &text);
            }
        }
        entry.set_text("");
    }

    /// Send the tab's connected-mode input over its live connection.
    fn send_tab_connected(&self, tab: &SessionTab, text: &str) {
        let mut bytes = text.to_string().into_bytes();
        bytes.push(b'\n');
        if let (Some(conn_id), Some(port)) = (tab.conn_id.get(), tab.selected_port()) {
            if let Some(handle) = self.state.active.borrow().get(&port.id) {
                let _ = handle.cmd_tx.send(PortCommand::Send { id: conn_id, bytes });
            }
            // Connected-mode AX.25/AGWPE backends don't echo our own
            // transmissions back, so log what we sent ourselves. Telnet/SSH
            // already get a remote echo from the far end, so don't double
            // it up there.
            if port_supports_connect(&port.config) {
                let port_name = port.name.clone();
                let remote = tab.node_entry.text().to_string();
                tab.append_sent_line(text);
                self.monitor.append_line(&format!("[{port_name}] TX > {remote}: {text}"));
            }
        }
    }

    /// Send the tab's input as a one-shot unconnected (UI) frame — the tab's
    /// own destination/via fields, not tied to any live connection.
    fn send_tab_unproto(&self, tab: &SessionTab, text: &str) {
        let Some(port) = tab.selected_port() else { return };
        let port_id = port.id.clone();
        let port_name = port.name.clone();
        let dest = tab.node_entry.text().trim().to_uppercase();
        if dest.is_empty() {
            self.monitor.append_line("Enter a destination before sending unproto.");
            return;
        }
        if !self.state.is_active(&port_id) {
            self.monitor.append_line(&format!("[{port_name}] port not connected \u{2014} can't send unproto."));
            return;
        }
        let via = tab.via();
        if let Some(handle) = self.state.active.borrow().get(&port_id) {
            let _ = handle.cmd_tx.send(PortCommand::SendUnproto {
                dest: dest.clone(),
                via: via.clone(),
                bytes: text.to_string().into_bytes(),
            });
        }
        tab.append_sent_line(text);
        let via_suffix = if via.is_empty() { String::new() } else { format!(" via {}", via.join(",")) };
        self.monitor.append_line(&format!("[{port_name}] TX unproto > {dest}{via_suffix}: {text}"));
    }

    /// Send arbitrary text over a tab's live connection on the mailbox's
    /// behalf — the same wire path `send_tab_connected` uses for a human's
    /// typed line, but callable with pre-built (possibly multi-line) text.
    fn send_tab_text(&self, tab: &SessionTab, port_id: &str, text: &str) {
        if let Some(conn_id) = tab.conn_id.get() {
            if let Some(handle) = self.state.active.borrow().get(port_id) {
                let _ = handle.cmd_tx.send(PortCommand::Send { id: conn_id, bytes: text.as_bytes().to_vec() });
            }
        }
        tab.append_sent_line(text);
    }

    /// Feed a chunk of received bytes to a mailbox-driven tab's command
    /// parser, one completed line at a time, sending back whatever response
    /// each line produces. No-op for tabs not currently mailbox-driven.
    fn drive_mailbox(&self, tab: &SessionTab, port_id: &str, chunk: &str) {
        if tab.mailbox_state.borrow().is_none() {
            return;
        }
        let remote_call = tab.node_entry.text().trim().to_uppercase();
        for line in tab.take_mailbox_lines(chunk) {
            let mut state_slot = tab.mailbox_state.borrow_mut();
            let Some(state) = state_slot.as_mut() else { break };
            let mut cfg = self.state.config.borrow_mut();
            let timestamp = crate::app_state::now_timestamp();
            let (response, close) = crate::mailbox::handle_line(state, &mut cfg.mailbox.messages, &remote_call, &line, &timestamp);
            drop(cfg);
            drop(state_slot);
            self.state.save_config();
            if !response.is_empty() {
                self.send_tab_text(tab, port_id, &response);
            }
            if close {
                *tab.mailbox_state.borrow_mut() = None;
                if let (Some(conn_id), Some(handle)) = (tab.conn_id.get(), self.state.active.borrow().get(port_id)) {
                    let _ = handle.cmd_tx.send(PortCommand::CloseConnection { id: conn_id });
                }
                break;
            }
        }
    }

    /// Connect the tab's selected port (if not already active) and, for
    /// connect-capable ports, open a session to the entered node. No-op if
    /// the tab is in Unproto mode.
    pub fn connect_tab(self: &Rc<Self>, tab_id: TabId) {
        let Some((port_id, remote, via, needs_node)) = self.tabs.borrow().get(&tab_id).and_then(|tab| {
            if tab.unproto_toggle.is_active() {
                return None;
            }
            let port = tab.selected_port()?;
            Some((port.id.clone(), tab.node_entry.text().trim().to_uppercase(), tab.via(), port_supports_connect(&port.config)))
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
                let _ = handle.cmd_tx.send(PortCommand::OpenConnection { remote, via });
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
            if let Some((old_port, old_remote, old_unproto)) = tab.pinned_identity.borrow_mut().take() {
                self.state.set_pinned(&old_port, &old_remote, old_unproto, "", false);
            }
        }
        self.bound.borrow_mut().retain(|_, v| *v != tab_id);
        self.pending.borrow_mut().retain(|_, v| *v != tab_id);

        if let Some(tab) = self.tabs.borrow_mut().remove(&tab_id) {
            if let Some(page) = self.notebook.page_num(&tab.root) {
                self.notebook.remove_page(Some(page));
            }
        }
        self.update_notebook_stack();
    }

    /// Called after something outside the tab's own signal handlers sets its
    /// node text programmatically (the address-book dropdown) — the normal
    /// `connect_changed`/focus-out wiring wouldn't otherwise fire for that.
    pub fn refresh_tab_for_node_entry(&self, node_entry: &gtk::Entry) {
        if let Some(tab) = self.tabs.borrow().values().find(|t| &t.node_entry == node_entry) {
            self.update_tab_title(tab);
            self.preview_history(tab);
            self.sync_pin(tab);
        }
    }

    /// Shows/hides the node/via/unproto row for ports with no node concept
    /// at all (Telnet/SSH), and forces the Unproto toggle on/off + locked
    /// for ports that only support one of connect/unproto.
    fn update_node_visibility(&self, tab: &SessionTab) {
        let Some(port) = tab.selected_port() else {
            tab.node_row.set_visible(false);
            return;
        };
        let can_connect = port_supports_connect(&port.config);
        let can_unproto = port_supports_unproto(&port.config);
        tab.node_row.set_visible(can_connect || can_unproto);
        if !can_unproto {
            tab.unproto_toggle.set_active(false);
            tab.unproto_toggle.set_sensitive(false);
        } else if !can_connect {
            tab.unproto_toggle.set_active(true);
            tab.unproto_toggle.set_sensitive(false);
        } else {
            tab.unproto_toggle.set_sensitive(true);
        }
    }

    /// Refreshes Connect/Disconnect/input sensitivity for the tab's current
    /// Unproto state. In Unproto mode, Connect/Disconnect are always
    /// disabled (there's no per-tab connection to manage) and the input
    /// entry tracks whether the underlying port is currently active; outside
    /// Unproto mode this just restores the normal connected-mode sensitivity.
    fn update_mode_controls(&self, tab: &SessionTab) {
        if tab.unproto_toggle.is_active() {
            tab.connect_button.set_sensitive(false);
            tab.disconnect_button.set_sensitive(false);
            tab.port_dropdown.set_sensitive(true);
            tab.node_entry.set_sensitive(true);
            tab.via_entry.set_sensitive(true);
            let active = tab.selected_port().map(|p| self.state.is_active(&p.id)).unwrap_or(false);
            tab.input_entry.set_sensitive(active);
            tab.send_input_button.set_sensitive(active);
        } else {
            tab.set_connected(tab.conn_id.get().is_some());
        }
    }

    fn update_tab_title(&self, tab: &SessionTab) {
        let port_name = tab.selected_port().map(|p| p.name.as_str()).unwrap_or("(no port)");
        let remote = tab.node_entry.text();
        let mut title = if remote.trim().is_empty() { port_name.to_string() } else { format!("{port_name}: {remote}") };
        if tab.unproto_toggle.is_active() {
            title.push_str(" (unproto)");
        }
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
        let history = self.state.history_for(&port.id, &remote, tab.unproto_toggle.is_active());
        tab.load_history(&history);
    }

    /// Keep a pinned tab's persisted (port, node, mode) identity — and its
    /// via path — in sync as the user edits its fields, unpinning the stale
    /// identity in the process.
    fn sync_pin(&self, tab: &SessionTab) {
        if !tab.pin_toggle.is_active() {
            return;
        }
        let Some(port) = tab.selected_port() else { return };
        let remote = tab.node_entry.text().trim().to_uppercase();
        if remote.is_empty() {
            return;
        }
        let unproto = tab.unproto_toggle.is_active();
        let via = tab.via_entry.text().trim().to_uppercase();
        let new_id = (port.id.clone(), remote, unproto);
        let mut current = tab.pinned_identity.borrow_mut();
        if let Some(old) = current.as_ref() {
            if old != &new_id {
                self.state.set_pinned(&old.0, &old.1, old.2, "", false);
            }
        }
        self.state.set_pinned(&new_id.0, &new_id.1, new_id.2, &via, true);
        *current = Some(new_id);
    }

    fn handle_event(self: &Rc<Self>, port_id: &str, event: PortEvent) {
        match event {
            PortEvent::PortConnected => {
                self.monitor.append_line(&format!("[{port_id}] port connected"));
                self.refresh_unproto_tabs_for_port(port_id);
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
                self.refresh_unproto_tabs_for_port(port_id);
            }
            PortEvent::PortError { message } => {
                self.monitor.append_line(&format!("[{port_id}] ERROR: {message}"));
            }
            PortEvent::Monitor { line } => {
                self.monitor.append_line(&format!("[{port_id}] {line}"));
            }
            PortEvent::ConnectionOpened { id, label } => {
                let needs_node =
                    find_entry(&self.state.config.borrow(), port_id).map(|e| port_supports_connect(&e.config)).unwrap_or(false);
                let pending_key =
                    if needs_node { (port_id.to_string(), label.clone()) } else { (port_id.to_string(), String::new()) };

                let existing_tab_id = self.pending.borrow_mut().remove(&pending_key);
                let is_new_incoming = existing_tab_id.is_none();
                let tab_id = existing_tab_id.unwrap_or_else(|| {
                    self.add_tab(Some(TabPrefill {
                        port_id: port_id.to_string(),
                        remote: label.clone(),
                        via: String::new(),
                        unproto: false,
                    }))
                });

                self.bound.borrow_mut().insert((port_id.to_string(), id), tab_id);
                if let Some(tab) = self.tabs.borrow().get(&tab_id) {
                    tab.conn_id.set(Some(id));
                    tab.set_connected(true);
                    if needs_node && tab.node_entry.text().is_empty() {
                        tab.node_entry.set_text(&label);
                    }
                    self.update_tab_title(tab);

                    // An unsolicited connect while the mailbox is enabled is
                    // answered automatically instead of waiting for a human
                    // to type back.
                    if needs_node && is_new_incoming && self.state.config.borrow().mailbox.enabled {
                        *tab.mailbox_state.borrow_mut() = Some(crate::mailbox::MailboxState::Command);
                        let my_call = self.state.config.borrow().ui.default_call.clone().unwrap_or_else(|| "MAILBOX".to_string());
                        self.send_tab_text(tab, port_id, &crate::mailbox::welcome_banner(&my_call));
                    }
                }
                if needs_node {
                    self.state.log_qso_started(port_id, &label);
                }
            }
            PortEvent::ConnectionClosed { id } => {
                if let Some(tab_id) = self.bound.borrow_mut().remove(&(port_id.to_string(), id)) {
                    if let Some(tab) = self.tabs.borrow().get(&tab_id) {
                        let is_connect_port =
                            find_entry(&self.state.config.borrow(), port_id).map(|e| port_supports_connect(&e.config)).unwrap_or(false);
                        if is_connect_port {
                            self.state.log_qso_ended(port_id, &tab.node_entry.text());
                        }
                        tab.conn_id.set(None);
                        tab.set_connected(false);
                        tab.flush_pending();
                        *tab.mailbox_state.borrow_mut() = None;
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
                        tab.receive_data(&text);
                        self.drive_mailbox(tab, port_id, &text);
                    }
                }
            }
            PortEvent::StationHeard { callsign } => {
                self.state.record_heard(&callsign);
            }
        }
    }

    /// Unproto tabs have no live `ConnectionId` to key off of, so their
    /// input sensitivity has to be refreshed explicitly whenever the
    /// underlying port connects or disconnects.
    fn refresh_unproto_tabs_for_port(&self, port_id: &str) {
        for tab in self.tabs.borrow().values() {
            if tab.unproto_toggle.is_active() && tab.selected_port().is_some_and(|p| p.id == port_id) {
                self.update_mode_controls(tab);
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

/// One-time static CSS for widgets whose styling doesn't depend on user
/// preferences (unlike `apply_font`, which is reapplied whenever the font
/// setting changes) — currently just the pin toggle's checked-state tint.
fn apply_base_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(".pin-toggle.pin-pinned { color: @accent_color; }");
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
    let pinned_tabs: Vec<TabPrefill> = config
        .pinned_sessions
        .iter()
        .map(|p| TabPrefill { port_id: p.port_id.clone(), remote: p.remote.clone(), via: p.via.clone(), unproto: p.unproto })
        .collect();
    apply_font(&font);
    apply_base_css();
    let state = AppState::new(config);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Packet Radio")
        .default_width(1000)
        .default_height(700)
        .build();

    let monitor = MonitorView::new(state.clone());
    monitor.set_show_timestamps(show_timestamps);
    let notebook = gtk::Notebook::builder().vexpand(true).hexpand(true).scrollable(true).build();

    // GTK hides the notebook's entire tab strip (and the "+" action widget
    // on it) when it has zero pages, so an empty state with its own
    // "+ New Tab" button stands in until the first tab exists.
    let empty_state = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Center)
        .vexpand(true)
        .hexpand(true)
        .build();
    let empty_label = gtk::Label::new(Some("No tabs open"));
    empty_label.add_css_class("dim-label");
    empty_state.append(&empty_label);
    let empty_new_tab_button = gtk::Button::with_label("+ New Tab");
    empty_new_tab_button.add_css_class("pill");
    empty_new_tab_button.add_css_class("suggested-action");
    empty_state.append(&empty_new_tab_button);

    let notebook_stack = gtk::Stack::new();
    notebook_stack.add_named(&empty_state, Some("empty"));
    notebook_stack.add_named(&notebook, Some("notebook"));
    notebook_stack.set_visible_child_name("empty");

    let paned = gtk::Paned::builder()
        .orientation(gtk::Orientation::Vertical)
        .start_child(&monitor.container)
        .end_child(&notebook_stack)
        .resize_start_child(true)
        .resize_end_child(true)
        .shrink_start_child(false)
        .shrink_end_child(false)
        .position(220)
        .build();
    paned.set_visible(true);
    monitor.container.set_visible(show_monitor);

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();

    let ui = Rc::new(Ui {
        state,
        monitor,
        notebook,
        notebook_stack,
        tabs: RefCell::new(HashMap::new()),
        bound: RefCell::new(HashMap::new()),
        pending: RefCell::new(HashMap::new()),
        next_tab_id: Cell::new(0),
        beacon_timers: RefCell::new(HashMap::new()),
        window: window.clone(),
    });

    {
        let ui = ui.clone();
        let filter_entry = ui.monitor.filter_entry.clone();
        filter_entry.connect_changed(move |entry| {
            ui.monitor.set_filter(&entry.text());
        });
    }

    // A plain "+" button on the notebook's own tab bar creates a new,
    // disconnected session tab: pick a port, a node (if the port supports
    // one), then Connect explicitly.
    let new_tab_button = gtk::Button::from_icon_name("list-add-symbolic");
    new_tab_button.add_css_class("flat");
    new_tab_button.set_tooltip_text(Some("New Tab"));
    {
        let ui = ui.clone();
        new_tab_button.connect_clicked(move |_| {
            ui.add_tab(None);
        });
    }
    ui.notebook.set_action_widget(&new_tab_button, gtk::PackType::End);
    {
        let ui = ui.clone();
        empty_new_tab_button.connect_clicked(move |_| {
            ui.add_tab(None);
        });
    }

    // A single hamburger menu holds the less-frequently-used management
    // dialogs, instead of a header button apiece. Placed first so it's the
    // leftmost header control.
    let menu_button = gtk::MenuButton::builder().icon_name("open-menu-symbolic").tooltip_text("Menu").build();
    let menu_popover = gtk::Popover::new();
    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    menu_box.set_margin_top(4);
    menu_box.set_margin_bottom(4);
    menu_box.set_margin_start(4);
    menu_box.set_margin_end(4);
    menu_popover.set_child(Some(&menu_box));
    menu_button.set_popover(Some(&menu_popover));

    // Mailbox has its own header button now (next to Send Beacon), so it's
    // deliberately left out of this menu rather than duplicated in both.
    type MenuAction = fn(&Rc<Ui>);
    let menu_items: [(&str, MenuAction); 4] = [
        ("Ports\u{2026}", |ui| ports_dialog::show(ui)),
        ("Address Book\u{2026}", |ui| address_book_dialog::show(ui)),
        ("Beacons\u{2026}", |ui| beacons_dialog::show(ui)),
        ("Preferences\u{2026}", |ui| preferences_dialog::show(ui)),
    ];
    for (label, open) in menu_items {
        let item_button = gtk::Button::with_label(label);
        item_button.add_css_class("flat");
        if let Some(l) = item_button.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
            l.set_halign(gtk::Align::Start);
        }
        {
            let ui = ui.clone();
            let menu_popover = menu_popover.clone();
            item_button.connect_clicked(move |_| {
                menu_popover.popdown();
                open(&ui);
            });
        }
        menu_box.append(&item_button);
    }
    header.pack_start(&menu_button);

    // Quick-access icon button for the Mailbox dialog, same tier of
    // frequent-use action as "Send Beacon...", so it lives in the header
    // instead of behind the hamburger menu.
    let mailbox_button = gtk::Button::from_icon_name("mail-unread-symbolic");
    mailbox_button.add_css_class("flat");
    mailbox_button.set_tooltip_text(Some("Mailbox"));
    {
        let ui = ui.clone();
        mailbox_button.connect_clicked(move |_| {
            mailbox_dialog::show(&ui);
        });
    }
    header.pack_start(&mailbox_button);

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

    // "Save Monitor Log\u{2026}" exports the Monitor's current (filtered)
    // view — packed at the end, next to the Monitor toggle it relates to.
    let save_monitor_button = gtk::Button::with_label("Save Monitor Log\u{2026}");
    {
        let ui = ui.clone();
        save_monitor_button.connect_clicked(move |_| {
            crate::export::save_text(&ui.window, "monitor.txt", ui.monitor.full_text());
        });
    }
    header.pack_end(&save_monitor_button);

    let monitor_toggle = gtk::ToggleButton::builder().label("Monitor").active(show_monitor).build();
    {
        let ui = ui.clone();
        monitor_toggle.connect_toggled(move |btn| {
            ui.monitor.container.set_visible(btn.is_active());
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
    for prefill in pinned_tabs {
        ui.add_tab(Some(prefill));
    }

    for id in autoconnect_ids {
        ui.connect_port(&id);
    }

    ui.reschedule_beacons();
}
