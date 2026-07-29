use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use pr_core::{AppConfig, ConnState, ConnectionId, PortCommand, PortEvent};

use crate::about_dialog;
use crate::address_book_dialog;
use crate::app_state::{find_entry, spawn_for_config, AppState};
use crate::beacons_dialog;
use crate::direwolf::{DirewolfProcess, DirewolfState};
use crate::direwolf_dialog;
use crate::help_dialog;
use crate::incoming_beacons_dialog;
use crate::mailbox_dialog;
use crate::monitor_view::MonitorView;
use crate::notified_packets_dialog;
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
    /// Bottom status bar: left side shows the currently selected tab's
    /// connect/disconnect state (icon + subtle-colored text), right side its
    /// packet/byte counters — see `refresh_status_bar`.
    status_conn_icon: gtk::Image,
    status_conn_label: gtk::Label,
    status_stats_label: gtk::Label,
    /// Left-aligned row of quick-connect buttons under the title bar, one
    /// per favorite-flagged port. Rebuilt whenever the port list changes.
    favorites_bar: gtk::Box,
    favorite_buttons: RefCell<HashMap<String, gtk::Button>>,
    /// Header icon that opens the Incoming Beacons dialog. Lights up
    /// (`beacon-lit` CSS class) when a new beacon is detected, cleared the
    /// next time the dialog is actually opened.
    beacon_button: gtk::Button,
    /// Ports that have received `PortConnected` and not yet a matching
    /// `PortDisconnected` -- lets `PortError` tell a genuine connect failure
    /// (never confirmed) apart from a non-fatal error on an already-live
    /// port (e.g. a bad outgoing frame), which must not clear its favorites
    /// button or `active` entry.
    confirmed_ports: RefCell<HashSet<String>>,
    pub direwolf: Rc<DirewolfProcess>,
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

    /// Rebuild the favorites quick-connect row from the current config's
    /// favorite-flagged ports. Call after anything that adds/removes/renames
    /// a port or changes its favorite flag (the Ports dialog).
    pub fn rebuild_favorites_bar(self: &Rc<Self>) {
        while let Some(child) = self.favorites_bar.first_child() {
            self.favorites_bar.remove(&child);
        }
        self.favorite_buttons.borrow_mut().clear();

        for port in self.state.config.borrow().ports.iter().filter(|p| p.favorite) {
            let button = gtk::Button::with_label(&port.name);
            button.add_css_class("favorite-port-button");
            if self.state.is_active(&port.id) {
                button.add_css_class("favorite-port-connected");
            }
            {
                let ui = self.clone();
                let id = port.id.clone();
                button.connect_clicked(move |_| {
                    if ui.state.is_active(&id) {
                        ui.disconnect_port(&id);
                    } else {
                        ui.connect_port(&id);
                    }
                });
            }
            self.favorites_bar.append(&button);
            self.favorite_buttons.borrow_mut().insert(port.id.clone(), button);
        }
    }

    /// Update just one favorite button's connected-state color -- cheaper
    /// than a full rebuild, called on every port connect/disconnect event.
    /// Always clears the failed-to-connect indicator too, since reaching
    /// either a connected or disconnected state supersedes it.
    fn refresh_favorite_button(&self, port_id: &str) {
        if let Some(button) = self.favorite_buttons.borrow().get(port_id) {
            button.remove_css_class("favorite-port-failed");
            if self.state.is_active(port_id) {
                button.add_css_class("favorite-port-connected");
            } else {
                button.remove_css_class("favorite-port-connected");
            }
        }
    }

    /// Mark a favorite button yellow after a genuine connect failure (the
    /// port never reached `PortConnected` at all).
    fn mark_favorite_failed(&self, port_id: &str) {
        if let Some(button) = self.favorite_buttons.borrow().get(port_id) {
            button.remove_css_class("favorite-port-connected");
            button.add_css_class("favorite-port-failed");
        }
    }

    /// (Re)schedule every enabled beacon from the current config, discarding
    /// any previously-scheduled timers first. Call this once at startup and
    /// again whenever the beacon list is edited/saved.
    pub fn reschedule_beacons(self: &Rc<Self>) {
        for (_, source) in self.beacon_timers.borrow_mut().drain() {
            source.remove();
        }
        if !self.state.config.borrow().beacon_prefs.enabled {
            // Master kill switch: stop everything regardless of each
            // beacon's own `enabled` flag, and don't reschedule until it's
            // flipped back on.
            return;
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
        let capture_toggle = tab.capture_toggle.clone();
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
            address_book_dropdown.connect_selected_notify(move |_dropdown| {
                if let Some(tab) = ui.tabs.borrow().get(&tab_id) {
                    if let Some(entry) = tab.selected_address_book_entry() {
                        tab.node_entry.set_text(&entry.callsign);
                        // A station usually needs the same digipeater path
                        // every time, so fill it in too — but only if the
                        // entry actually has one, so picking a direct-path
                        // station doesn't clobber a via the user already typed.
                        if !entry.via.trim().is_empty() {
                            tab.via_entry.set_text(&entry.via);
                        }
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
                    let history_dir = pr_core::AppConfig::config_dir()
                        .and_then(|dir| tab.selected_port().map(|p| pr_core::history_dir(&dir, &p.name)));
                    crate::export::save_text(&ui.window, &format!("{name}.txt"), tab.full_text(), history_dir.as_deref());
                }
            });
        }
        {
            let ui = self.clone();
            capture_toggle.connect_toggled(move |btn| {
                if btn.is_active() {
                    let started = ui.tabs.borrow().get(&tab_id).and_then(|tab| tab.start_capture());
                    match started {
                        Some(path) => ui.monitor.append_line(&format!("Capturing to {}", path.display())),
                        None => {
                            ui.monitor.append_line("Can't start capture \u{2014} pick a port and node first.");
                            btn.set_active(false);
                        }
                    }
                } else if let Some(tab) = ui.tabs.borrow().get(&tab_id) {
                    tab.stop_capture();
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
        self.refresh_status_bar();

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
    /// pressing Enter and clicking the Send button next to it. The input
    /// field stays editable at all times (see `SessionTab::set_connected`),
    /// but `send_input_button`'s sensitivity only gates the mouse path —
    /// pressing Enter in a focused `gtk::Entry` fires regardless of any
    /// other widget's sensitivity, so this re-checks the same "is there
    /// actually something to send this over" condition before sending or
    /// clearing the text, rather than silently discarding a composed message.
    fn activate_input(self: &Rc<Self>, tab_id: TabId, entry: &gtk::Entry) {
        let text = entry.text().to_string();
        if text.is_empty() {
            return;
        }
        let tabs = self.tabs.borrow();
        let Some(tab) = tabs.get(&tab_id) else { return };
        let ready = if tab.unproto_toggle.is_active() {
            tab.selected_port().is_some_and(|p| self.state.is_active(&p.id))
        } else {
            tab.conn_id.get().is_some()
        };
        if !ready {
            drop(tabs);
            self.monitor.append_line("Not connected \u{2014} message kept in the input field.");
            return;
        }
        if tab.unproto_toggle.is_active() {
            self.send_tab_unproto(tab, &text);
        } else {
            self.send_tab_connected(tab, &text);
        }
        drop(tabs);
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
                self.refresh_status_bar();
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
        self.refresh_status_bar();
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
        self.refresh_status_bar();
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

        // Extracted into its own `let` rather than matched directly on
        // `self.tabs.borrow_mut().remove(...)`: in an `if let`, temporaries in
        // the scrutinee live for the whole block, so the `RefMut` guard would
        // otherwise still be held while `remove_page` runs below.
        // `remove_page` can synchronously emit `switch-page` (GTK
        // recalculates the notebook's current page during removal), which
        // re-enters `refresh_status_bar` -> `current_tab_id` ->
        // `self.tabs.borrow()` -- an immutable borrow while still mutably
        // borrowed, which panics and crashes the app. This was the real cause
        // of the "close a tab, app crashes" bug.
        let removed_tab = self.tabs.borrow_mut().remove(&tab_id);
        if let Some(tab) = removed_tab {
            if let Some(page) = self.notebook.page_num(&tab.root) {
                self.notebook.remove_page(Some(page));
            }
        }
        self.update_notebook_stack();
        self.refresh_status_bar();
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
            tab.input_entry.set_sensitive(true);
            tab.send_input_button.set_sensitive(active);
        } else {
            tab.set_connected(tab.conn_id.get().is_some());
        }
    }

    /// The tab backing the notebook's currently visible page, if any.
    fn current_tab_id(&self) -> Option<TabId> {
        let current = self.notebook.current_page()?;
        self.tabs.borrow().iter().find(|(_, t)| self.notebook.page_num(&t.root) == Some(current)).map(|(id, _)| *id)
    }

    /// Refresh the status bar's connect-state (left) and packet/byte stats
    /// (right) for whichever tab is currently selected. Call this whenever
    /// the selected tab changes, connects/disconnects, or sends/receives —
    /// cheap enough to call liberally rather than track precisely. Also
    /// ticked once a second (see `build_ui`) so the elapsed-time display
    /// keeps counting up while a tab stays selected.
    ///
    /// The connect indicator reflects a genuine two-way connected-mode
    /// session to a node, not the underlying port -- and is hidden entirely
    /// for an Unproto tab, which has no such session at all.
    pub fn refresh_status_bar(&self) {
        let tabs = self.tabs.borrow();
        match self.current_tab_id().and_then(|id| tabs.get(&id)) {
            Some(tab) if tab.unproto_toggle.is_active() => {
                self.status_conn_icon.set_visible(false);
                self.status_conn_label.set_text("");
                self.status_stats_label.set_text(&tab.stats_text());
            }
            Some(tab) => {
                self.status_conn_icon.set_visible(true);
                let live = tab.conn_id.get().is_some();
                self.status_conn_icon.set_icon_name(Some(if live {
                    "network-transmit-receive-symbolic"
                } else {
                    "network-offline-symbolic"
                }));
                let text = match (live, tab.elapsed_text()) {
                    (true, Some(elapsed)) => format!("Connected to {} \u{2014} {elapsed}", tab.node_entry.text()),
                    (true, None) => "Connected".to_string(),
                    (false, _) => "Disconnected".to_string(),
                };
                self.status_conn_label.set_text(&text);
                self.status_stats_label.set_text(&tab.stats_text());
            }
            None => {
                self.status_conn_icon.set_visible(true);
                self.status_conn_icon.set_icon_name(Some("network-offline-symbolic"));
                self.status_conn_label.set_text("No tab selected");
                self.status_stats_label.set_text("");
            }
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
                self.confirmed_ports.borrow_mut().insert(port_id.to_string());
                self.refresh_unproto_tabs_for_port(port_id);
                self.refresh_favorite_button(port_id);
                self.refresh_status_bar();
            }
            PortEvent::PortDisconnected { reason } => {
                // Remove from `active` immediately rather than waiting for
                // `connect_port`'s event loop to end (which only happens once
                // the event channel itself closes, *after* this event is
                // handled) -- otherwise `is_active` still reports true for
                // every refresh below, which left e.g. the favorites-bar
                // button stuck green after a manual disconnect.
                self.state.active.borrow_mut().remove(port_id);
                self.confirmed_ports.borrow_mut().remove(port_id);

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
                            tab.mark_disconnected();
                        }
                    }
                }
                self.pending.borrow_mut().retain(|(pid, _), _| pid != port_id);
                self.refresh_unproto_tabs_for_port(port_id);
                self.refresh_favorite_button(port_id);
                self.refresh_status_bar();
            }
            PortEvent::PortError { message } => {
                self.monitor.append_line(&format!("[{port_id}] ERROR: {message}"));
                // Only a port that never reached `PortConnected` at all is a
                // genuine connect failure -- some backends (e.g. a bad
                // outgoing KISS frame) report a `PortError` for a non-fatal
                // problem on an already-live port, which must not clear its
                // `active` entry or favorites-bar button.
                if !self.confirmed_ports.borrow().contains(port_id) {
                    self.state.active.borrow_mut().remove(port_id);
                    self.refresh_unproto_tabs_for_port(port_id);
                    self.mark_favorite_failed(port_id);
                    self.refresh_status_bar();
                }
            }
            PortEvent::Monitor { line, from, to, message } => {
                self.monitor.append_line(&format!("[{port_id}] {line}"));
                if let (Some(from), Some(to), Some(message)) = (from, to, message) {
                    self.maybe_notify_directed(port_id, &from, &to, &message, &line);
                    self.maybe_detect_beacon(port_id, &from, &to, &message);
                    self.feed_unproto_tabs(port_id, &line);
                }
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
                    tab.mark_connected();
                    if needs_node && tab.node_entry.text().is_empty() {
                        tab.node_entry.set_text(&label);
                    }
                    self.update_tab_title(tab);

                    if needs_node && is_new_incoming {
                        // An unsolicited connect while the mailbox is enabled
                        // is answered automatically instead of waiting for a
                        // human to type back.
                        if self.state.config.borrow().mailbox.enabled {
                            *tab.mailbox_state.borrow_mut() = Some(crate::mailbox::MailboxState::Command);
                            let my_call = self.state.config.borrow().ui.default_call.clone().unwrap_or_else(|| "MAILBOX".to_string());
                            self.send_tab_text(tab, port_id, &crate::mailbox::welcome_banner(&my_call));
                        }
                        // An incoming connection is inherently "directed at
                        // me" (that's what accepting it means), so this
                        // doesn't need `NotifyMatcher` at all — just the
                        // feature's on/off switch.
                        if self.state.config.borrow().notify.directed_enabled {
                            let port_name = find_entry(&self.state.config.borrow(), port_id).map(|e| e.name).unwrap_or_else(|| port_id.to_string());
                            let body = format!("{label} connected to you");
                            crate::notify::send(&self.window, &format!("Packet Radio \u{2014} {port_name}"), &body);
                            self.state.record_notified_packet(port_id, &body);
                        }
                    }
                }
                if needs_node {
                    self.state.log_qso_started(port_id, &label);
                }
                self.refresh_status_bar();
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
                        tab.mark_disconnected();
                        tab.flush_pending();
                        *tab.mailbox_state.borrow_mut() = None;
                    }
                }
                self.refresh_status_bar();
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
                self.refresh_status_bar();
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

    /// Stream every observed UI (unproto) frame into any Unproto-mode tab
    /// open on the same port -- an Unproto tab has no live `ConnectionId` of
    /// its own to receive `Data` events through, so without this a reply
    /// sent back via unproto would only ever show up in the global Monitor
    /// pane, not in the tab the user is actually watching.
    fn feed_unproto_tabs(&self, port_id: &str, line: &str) {
        for tab in self.tabs.borrow().values() {
            if tab.unproto_toggle.is_active() && tab.selected_port().is_some_and(|p| p.id == port_id) {
                tab.append_monitor_line(line);
            }
        }
    }

    /// Fire a desktop notification if `to` is directed at the configured
    /// callsign or matches a Custom Rule — no-op if the relevant toggle is
    /// off or nothing matches. `line` (the full formatted Monitor text) is
    /// only used for the historical Notified Packets record, which wants
    /// the same highlighted display everything else gets; the live
    /// notification body itself uses the clean structured fields, per
    /// explicit request (from/to/message only, no frame-tag/PID metadata).
    fn maybe_notify_directed(&self, port_id: &str, from: &str, to: &str, message: &str, line: &str) {
        let matcher = crate::notify::NotifyMatcher::build(&self.state.config.borrow());
        let Some(match_kind) = matcher.match_destination(to) else { return };
        let port_name = find_entry(&self.state.config.borrow(), port_id).map(|e| e.name).unwrap_or_else(|| port_id.to_string());
        let title = match &match_kind {
            crate::notify::NotifyMatch::Directed => format!("Packet Radio \u{2014} {port_name}"),
            crate::notify::NotifyMatch::Custom(label) => format!("Packet Radio \u{2014} {port_name} ({label})"),
        };
        crate::notify::send(&self.window, &title, &format!("From: {from}\nTo: {to}\n{message}"));
        self.state.record_notified_packet(port_id, line);
    }

    /// Record and (optionally) notify on a received frame matching a
    /// `BeaconMonitorRule`, tracked separately from the general directed/
    /// custom-rule notifications above — a beacon match always lights up
    /// the header button regardless of whether `notify.beacon_enabled` also
    /// fires a desktop notification for it.
    fn maybe_detect_beacon(&self, port_id: &str, from: &str, to: &str, message: &str) {
        let label = {
            let cfg = self.state.config.borrow();
            let mut found = None;
            for rule in cfg.beacon_rules.iter().filter(|r| r.enabled) {
                if let Ok(re) = regex::RegexBuilder::new(&rule.pattern).case_insensitive(true).build() {
                    if re.is_match(to) {
                        found = Some(rule.label.clone());
                        break;
                    }
                }
            }
            found
        };
        let Some(label) = label else { return };

        self.state.record_incoming_beacon(port_id, from, to, message);
        self.mark_beacon_lit();

        if self.state.config.borrow().notify.beacon_enabled {
            let port_name = find_entry(&self.state.config.borrow(), port_id).map(|e| e.name).unwrap_or_else(|| port_id.to_string());
            let title = format!("Packet Radio \u{2014} {port_name} (Beacon: {label})");
            crate::notify::send(&self.window, &title, &format!("From: {from}\nTo: {to}\n{message}"));
        }
    }

    /// Light up the header's Incoming Beacons button — same explicit
    /// CSS-class-toggle pattern as the favorites bar/pin toggle elsewhere in
    /// this file, rather than a `:checked`-style pseudo-class.
    pub fn mark_beacon_lit(&self) {
        self.beacon_button.add_css_class("beacon-lit");
    }

    /// Clear the lit state — called when the Incoming Beacons dialog is
    /// actually opened, the simplest "mark as seen" trigger available.
    pub fn clear_beacon_lit(&self) {
        self.beacon_button.remove_css_class("beacon-lit");
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
/// setting changes): the tab pin toggle's active-state tint, and the
/// per-rule notify bell's "lit" state. The bell uses a background tint
/// rather than `color` — the `notifications-symbolic` icon doesn't recolor
/// via a plain `color` override the way single-path icons like
/// `pin-symbolic` do (confirmed empirically: a `background-color: red`
/// diagnostic rendered immediately, a `color` override on the icon itself
/// did not).
fn apply_base_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        ".pin-toggle.pin-pinned { color: @accent_color; } \
         .notify-rule-toggle.notify-rule-active { background-color: @accent_color; } \
         .direwolf-running { background-color: @success_color; } \
         .direwolf-failed { background-color: @warning_color; } \
         .favorite-port-button.favorite-port-connected { background-color: @success_color; } \
         .favorite-port-button.favorite-port-failed { background-color: @warning_color; } \
         .beacon-lit { background-color: @accent_color; }",
    );
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

/// Bind a keyboard accelerator (standard GTK format, e.g. `"<Control>n"`) on
/// `controller` to a callback taking the `Ui`. Logs and no-ops on an
/// unparseable accelerator string rather than panicking, since that's a
/// static typo in this file, not a runtime condition worth crashing over.
fn add_shortcut(controller: &gtk::ShortcutController, accel: &str, ui: &Rc<Ui>, action: impl Fn(&Rc<Ui>) + 'static) {
    let Some(trigger) = gtk::ShortcutTrigger::parse_string(accel) else {
        tracing::warn!("failed to parse shortcut accelerator {accel:?}");
        return;
    };
    let ui = ui.clone();
    let callback_action = gtk::CallbackAction::new(move |_, _| {
        action(&ui);
        glib::Propagation::Stop
    });
    controller.add_shortcut(gtk::Shortcut::new(Some(trigger), Some(callback_action)));
}

/// Reflect `direwolf`'s current state on its header button: color (via CSS
/// class — `background-color`, not `color`, since a plain `color` override
/// doesn't visibly recolor every symbolic icon in this theme, per the
/// `notifications-symbolic` gotcha noted elsewhere in this project) and
/// tooltip text.
fn refresh_direwolf_button(button: &gtk::Button, direwolf: &DirewolfProcess) {
    button.remove_css_class("direwolf-running");
    button.remove_css_class("direwolf-failed");
    let tooltip = match direwolf.state.get() {
        DirewolfState::Stopped => "Direwolf: stopped \u{2014} click to start, right-click for log/settings",
        DirewolfState::Running => {
            button.add_css_class("direwolf-running");
            "Direwolf: running \u{2014} click to stop, right-click for log/settings"
        }
        DirewolfState::FailedToStart => {
            button.add_css_class("direwolf-failed");
            "Direwolf: failed to start \u{2014} right-click for log/settings"
        }
    };
    button.set_tooltip_text(Some(tooltip));
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
        .title("PGPRC")
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
        .vexpand(true)
        .build();
    paned.set_visible(true);
    monitor.container.set_visible(show_monitor);

    // Bottom status bar: connect/disconnect state for the selected tab on
    // the left (icon + subtle-colored text), its packet/byte counters on
    // the right — see `Ui::refresh_status_bar`.
    let status_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    status_bar.set_margin_start(8);
    status_bar.set_margin_end(8);
    status_bar.set_margin_top(2);
    status_bar.set_margin_bottom(2);
    let status_conn_icon = gtk::Image::from_icon_name("network-offline-symbolic");
    status_conn_icon.add_css_class("dim-label");
    let status_conn_label = gtk::Label::new(Some("No tab selected"));
    status_conn_label.add_css_class("dim-label");
    status_conn_label.add_css_class("caption");
    status_bar.append(&status_conn_icon);
    status_bar.append(&status_conn_label);
    let status_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    status_spacer.set_hexpand(true);
    status_bar.append(&status_spacer);
    let status_stats_label = gtk::Label::new(None);
    status_stats_label.add_css_class("dim-label");
    status_stats_label.add_css_class("caption");
    status_bar.append(&status_stats_label);

    // Left-aligned quick-connect row for favorite-flagged ports, directly
    // under the title bar -- see `Ui::rebuild_favorites_bar`.
    let favorites_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    favorites_bar.set_halign(gtk::Align::Start);
    favorites_bar.set_margin_start(8);
    favorites_bar.set_margin_end(8);
    favorites_bar.set_margin_top(4);
    favorites_bar.set_margin_bottom(4);

    let content_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content_box.append(&favorites_bar);
    content_box.append(&paned);
    content_box.append(&status_bar);

    let toolbar_view = adw::ToolbarView::new();

    // A plain custom title bar instead of `adw::HeaderBar`: HeaderBar always
    // keeps its title dead-center in the *whole* bar (reserving equal space
    // on each side, regardless of how wide the packed content actually is),
    // which reads as off-center here since the left group (menu/mailbox/
    // beacon/filter) is much wider than the right group (Save Monitor Log/
    // Monitor toggle). A plain `Box` with a hexpand+center title between two
    // natural-width side boxes centers it in the *actual* leftover space
    // instead. Wrapped in `WindowHandle` to keep click-drag-to-move and
    // double-click-to-maximize, and `WindowControls` restores the
    // minimize/maximize/close buttons `HeaderBar` provided automatically.
    let header_start = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let header_end = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let header_title = gtk::Label::new(Some("PGPRC"));
    header_title.add_css_class("title");
    header_title.set_hexpand(true);
    header_title.set_halign(gtk::Align::Center);
    header_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let header_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header_row.set_margin_start(6);
    header_row.set_margin_end(6);
    header_row.set_margin_top(6);
    header_row.set_margin_bottom(6);
    header_row.add_css_class("titlebar");
    header_row.append(&header_start);
    header_row.append(&header_title);
    header_row.append(&header_end);
    let header_handle = gtk::WindowHandle::builder().child(&header_row).build();

    // Created here (rather than alongside its click handler below) so it can
    // be stored on `Ui` itself -- `maybe_detect_beacon` needs to light it up
    // from anywhere event handling happens, not just from this setup code.
    let beacon_button = gtk::Button::from_icon_name("audio-speakers-symbolic");

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
        status_conn_icon,
        status_conn_label,
        status_stats_label,
        favorites_bar,
        favorite_buttons: RefCell::new(HashMap::new()),
        beacon_button: beacon_button.clone(),
        confirmed_ports: RefCell::new(HashSet::new()),
        direwolf: DirewolfProcess::new(),
        window: window.clone(),
    });
    ui.rebuild_favorites_bar();

    {
        let ui = ui.clone();
        let filter_entry = ui.monitor.filter_entry.clone();
        filter_entry.connect_changed(move |entry| {
            ui.monitor.set_filter(&entry.text());
        });
    }

    {
        let ui = ui.clone();
        let notebook = ui.notebook.clone();
        notebook.connect_switch_page(move |_, _, _| {
            ui.refresh_status_bar();
        });
    }
    ui.refresh_status_bar();
    {
        // Ticks the status bar's elapsed-connected-time display once a
        // second; cheap enough (two label/icon updates) to run unconditionally
        // rather than starting/stopping a timer per tab.
        let ui = ui.clone();
        glib::source::timeout_add_seconds_local(1, move || {
            ui.refresh_status_bar();
            glib::ControlFlow::Continue
        });
    }

    // A starter set of keyboard shortcuts mirroring existing mouse actions.
    // `gtk::ShortcutController` (rather than a hand-rolled `EventControllerKey`
    // matching on raw modifier bits) is the GTK4-recommended way to bind
    // these: it parses standard accelerator strings and correctly resolves
    // against whatever descendant widget currently has focus (e.g. a session
    // tab's input entry) via `ShortcutScope::Global`, whereas a manual bubble-
    // phase key handler can lose the race against a focused widget's own
    // default key handling. Ctrl+Tab/Shift+Ctrl+Tab aren't included since
    // `gtk::Notebook` already binds those itself. Escape-closing dialogs is
    // handled separately, per dialog window, in `ports_dialog::dialog_window`.
    let shortcuts = gtk::ShortcutController::new();
    shortcuts.set_scope(gtk::ShortcutScope::Global);
    add_shortcut(&shortcuts, "<Control>n", &ui, |ui| {
        ui.add_tab(None);
    });
    add_shortcut(&shortcuts, "<Control>w", &ui, |ui| {
        if let Some(tab_id) = ui.current_tab_id() {
            ui.close_tab(tab_id);
        }
    });
    add_shortcut(&shortcuts, "<Control>comma", &ui, |ui| {
        preferences_dialog::show(ui);
    });
    add_shortcut(&shortcuts, "<Control>f", &ui, |ui| {
        ui.monitor.filter_entry.grab_focus();
    });
    add_shortcut(&shortcuts, "<Control>q", &ui, |ui| {
        ui.window.close();
    });
    ui.window.add_controller(shortcuts);

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

    // Mailbox/Incoming Beacons/Notified Packets each have their own header
    // button now, so they're deliberately left out of this menu rather than
    // duplicated in both.
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
    menu_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let help_about_items: [(&str, MenuAction); 2] =
        [("Help\u{2026}", |ui| help_dialog::show(ui)), ("About\u{2026}", |ui| about_dialog::show(ui))];
    for (label, open) in help_about_items {
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
    header_start.append(&menu_button);

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
    header_start.append(&mailbox_button);

    // Modem-style handset for the optional managed Direwolf process — icon
    // only, colored via CSS class to reflect state (see
    // `refresh_direwolf_button`): default = stopped, green = running,
    // yellow = failed to start. Left-click toggles start/stop; right-click
    // opens the console window (`direwolf_dialog::show_console`).
    let direwolf_button = gtk::Button::from_icon_name("call-start-symbolic");
    direwolf_button.add_css_class("flat");
    refresh_direwolf_button(&direwolf_button, &ui.direwolf);
    {
        let ui = ui.clone();
        direwolf_button.connect_clicked(move |_| {
            if ui.direwolf.is_running() {
                ui.direwolf.stop();
            } else if let Some(dir) = pr_core::AppConfig::config_dir() {
                let config_text = ui.state.config.borrow().direwolf.config_text.clone();
                ui.direwolf.start(&dir.join("direwolf.conf"), &config_text);
            }
        });
    }
    {
        let ui = ui.clone();
        let gesture = gtk::GestureClick::new();
        gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
        gesture.connect_pressed(move |_, _, _, _| {
            direwolf_dialog::show_console(&ui);
        });
        direwolf_button.add_controller(gesture);
    }
    {
        let direwolf_button = direwolf_button.clone();
        let direwolf = ui.direwolf.clone();
        ui.direwolf.add_on_change(move || {
            refresh_direwolf_button(&direwolf_button, &direwolf);
        });
    }
    header_start.append(&direwolf_button);

    // Opens the Incoming Beacons dialog -- icon-only (a loudspeaker with
    // sound waves), same tier as the Direwolf button next to it. `flat`
    // keeps its resting appearance uniform with the other header icon
    // buttons; it only stands out via `beacon-lit` when a new beacon is
    // actually detected (see `Ui::mark_beacon_lit`).
    beacon_button.add_css_class("flat");
    beacon_button.set_tooltip_text(Some("Incoming Beacons\u{2026}"));
    {
        let ui = ui.clone();
        beacon_button.connect_clicked(move |_| {
            incoming_beacons_dialog::show(&ui);
        });
    }
    header_start.append(&beacon_button);

    // Quick-access icon button for Notified Packets, same tier as Mailbox/
    // Direwolf/Incoming Beacons -- moved out of the hamburger menu so every
    // frequent-use dialog lives at the same level.
    let notified_packets_button = gtk::Button::from_icon_name("notifications-symbolic");
    notified_packets_button.add_css_class("flat");
    notified_packets_button.set_tooltip_text(Some("Notified Packets\u{2026}"));
    {
        let ui = ui.clone();
        notified_packets_button.connect_clicked(move |_| {
            notified_packets_dialog::show(&ui);
        });
    }
    header_start.append(&notified_packets_button);
    header_start.append(&ui.monitor.filter_entry);

    // "Save Monitor Log\u{2026}" exports the Monitor's current (filtered)
    // view — packed at the end, next to the Monitor toggle it relates to.
    let save_monitor_button = gtk::Button::with_label("Save Monitor Log\u{2026}");
    {
        let ui = ui.clone();
        save_monitor_button.connect_clicked(move |_| {
            let history_dir = pr_core::AppConfig::config_dir().map(|dir| dir.join("history"));
            crate::export::save_text(&ui.window, "monitor.txt", ui.monitor.full_text(), history_dir.as_deref());
        });
    }
    header_end.append(&save_monitor_button);

    let monitor_toggle = gtk::ToggleButton::builder().label("Monitor").active(show_monitor).build();
    {
        let ui = ui.clone();
        monitor_toggle.connect_toggled(move |btn| {
            ui.monitor.container.set_visible(btn.is_active());
            ui.state.config.borrow_mut().ui.show_monitor = btn.is_active();
            ui.state.save_config();
        });
    }
    header_end.append(&monitor_toggle);
    header_end.append(&gtk::WindowControls::new(gtk::PackType::End));

    toolbar_view.add_top_bar(&header_handle);
    toolbar_view.set_content(Some(&content_box));
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

    if ui.state.config.borrow().direwolf.auto_start {
        if let Some(dir) = pr_core::AppConfig::config_dir() {
            let config_text = ui.state.config.borrow().direwolf.config_text.clone();
            ui.direwolf.start(&dir.join("direwolf.conf"), &config_text);
        }
    }

    ui.reschedule_beacons();
}
