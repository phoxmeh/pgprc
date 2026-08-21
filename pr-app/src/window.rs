use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use pr_core::{AppConfig, ConnState, ConnectionId, PortCommand, PortConfig, PortEntry, PortEvent};

use crate::about_dialog;
use crate::address_book_dialog;
use crate::app_state::{find_entry, spawn_for_config, AppState};
use crate::beacons_dialog;
use crate::dial_dialog;
use crate::direwolf::{DirewolfProcess, DirewolfState};
use crate::direwolf_dialog;
use crate::help_dialog;
use crate::incoming_beacons_dialog;
use crate::keyboard_mode_dialog;
use crate::log_view::LogView;
use crate::mailbox_dialog;
use crate::monitor_view::MonitorView;
use crate::ports_dialog;
use crate::preferences_dialog;
use crate::session_tab::{port_supports_connect, port_supports_unproto, SessionTab, TabId};

/// Prefills a newly-opened tab's shell: port + node + via/address. Used both
/// for pinned-tab restoration at startup and for the fallback tab created
/// when an unsolicited connection arrives on a connect-capable port.
pub struct TabPrefill {
    pub port_id: String,
    pub remote: String,
    pub via: String,
}

/// One tab's chip in the always-visible-while-any-tab-exists strip: pin
/// icon (left of the title, per explicit request), title label, close X.
/// Only `root` is read back later (to remove the chip on close) — the pin
/// toggle and title label are wired up once at creation and otherwise left
/// alone, kept alive by `root` itself as their GTK parent.
struct TabChip {
    root: gtk::Box,
}

/// Live handles into a tab's detached pop-out window, kept so connection-
/// state changes can update the window's phone button, send-button
/// sensitivity, and status label alongside the main window's equivalents.
struct DetachedWindow {
    window: gtk::Window,
    phone_button: gtk::Button,
    send_button: gtk::Button,
    status_label: gtk::Label,
    /// Handler ID for the pop-out window's close-request signal (the one
    /// that re-attaches the tab to the main view). Stored so `close_tab`
    /// can disconnect it before calling `window.close()`, preventing the
    /// "return to main window" logic from firing when the tab itself is
    /// being removed.
    close_handler: glib::SignalHandlerId,
}

pub struct Ui {
    pub state: Rc<AppState>,
    pub monitor: Rc<MonitorView>,
    /// Diagnostic/status noise (connect/disconnect/error/connecting, AX.25
    /// connection-state transitions) -- kept separate from `monitor`, which
    /// is packet traffic only. Toggled into view via `display_stack`.
    pub log: LogView,
    /// Swaps `paned`'s start child between `monitor.container` and
    /// `log.container` -- which stream you're looking at, not show/hide.
    display_stack: gtk::Stack,
    pub tabs: RefCell<HashMap<TabId, SessionTab>>,
    /// Content-switching stack for tab scrollbacks, keyed by `tab_id`
    /// stringified. Attached/detached from `paned`'s end child based on
    /// `tab_area_expanded` -- detaching gives Monitor the full pane (a
    /// `gtk::Paned` with no end child allocates 100% to its start child).
    tab_stack: gtk::Stack,
    /// The Monitor-or-Log/tab-content split. `display_stack` is always
    /// `start_child`; `end_child` is `Some(&tab_stack)` only while expanded.
    paned: gtk::Paned,
    /// Always visible whenever `tabs` is non-empty (regardless of
    /// expanded/minimized), sitting just above the bottom bar -- a chip per
    /// open tab plus a trailing "+" to dial another. Deliberately a custom
    /// strip (not `gtk::Notebook`'s built-in one) since it needs to stay
    /// visible even while the content pane itself is collapsed/minimized.
    tab_strip: gtk::Box,
    tab_chips: RefCell<HashMap<TabId, TabChip>>,
    /// The strip's own trailing "+" button — `add_tab_chip` inserts new
    /// chips right before it so it always stays at the far right.
    tab_strip_add_button: gtk::Button,
    /// Whether the tab content pane is currently shown (state 3) or
    /// collapsed with only the strip visible (state 2), or there simply are
    /// no tabs at all (state 1, strip hidden too).
    tab_area_expanded: Cell<bool>,
    /// The tab currently shown in `tab_stack` / reflected in the bottom
    /// bar, only meaningful while `tab_area_expanded`.
    selected_tab: Cell<Option<TabId>>,
    /// Live connection (port, id) -> the tab it's bound to.
    bound: RefCell<HashMap<(String, ConnectionId), TabId>>,
    /// A dial in progress, keyed by (port_id, remote) — remote is empty for
    /// port kinds with no node concept (Telnet/SSH), since those only ever
    /// have one connection to bind.
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
    /// Header Notifications button. Lights up (`beacon-lit` CSS class) when a
    /// new notification arrives, cleared when the dialog is opened. Turns
    /// yellow (`state-warning`) while notifications are silenced.
    notifications_button: gtk::Button,
    /// Header icon that opens the Mailbox window (left-click) or its
    /// Settings (right-click) -- green while enabled, orange (taking
    /// priority) while an unread message exists. See
    /// `Ui::refresh_mailbox_button`.
    mailbox_button: gtk::Button,
    /// Ports that have received `PortConnected` and not yet a matching
    /// `PortDisconnected` -- lets `PortError` tell a genuine connect failure
    /// (never confirmed) apart from a non-fatal error on an already-live
    /// port (e.g. a bad outgoing frame), which must not clear its favorites
    /// button or `active` entry.
    confirmed_ports: RefCell<HashSet<String>>,
    /// (port_id, connection_id) pairs that received `ConnState::Connected`,
    /// meaning the remote accepted the SABM (UA). Used to distinguish a
    /// clean disconnect from a timeout (`ConnectionClosed` without a prior
    /// `Connected` state = timeout/no-answer).
    established_conns: RefCell<HashSet<(String, ConnectionId)>>,

    // --- Shared bottom bar: Node / Via / Port (in that order), then the
    // dial/minimize button, the phone-handset connect/disconnect button
    // (visible only while a tab is selected/expanded), the message entry,
    // and Send. When no tab is expanded, Node/Via/Port define an ad-hoc
    // unproto destination + port; when one is, they read-only-display that
    // tab's own fixed identity instead (see `refresh_bottom_bar`). ---
    bottom_node_entry: gtk::Entry,
    bottom_via_entry: gtk::Entry,
    bottom_port_dropdown: gtk::DropDown,
    bottom_ports_snapshot: RefCell<Vec<PortEntry>>,
    dial_button: gtk::Button,
    phone_button: gtk::Button,
    message_entry: gtk::Entry,
    /// Stored (not just a local in `build_ui`) so `refresh_bottom_bar` can
    /// gate its sensitivity on whether the ad-hoc unproto destination is
    /// filled in.
    send_button: gtk::Button,
    /// Lines sent so far this session — navigated with Up/Down in the
    /// message entry. Duplicates (identical adjacent entries) are suppressed.
    send_history: RefCell<Vec<String>>,
    /// Current position inside `send_history` during Up/Down navigation
    /// (0 = most-recent, `len-1` = oldest). `None` while editing new text.
    history_pos: Cell<Option<usize>>,
    /// The in-progress text the user was typing before they pressed Up for
    /// the first time — restored when they press Down back past history[0].
    history_draft: RefCell<String>,
    /// The single periodic "available for keyboard-to-keyboard" beacon
    /// timer -- reset in full by `reschedule_keyboard_mode_beacon`.
    keyboard_mode_beacon_timer: RefCell<Option<glib::SourceId>>,
    /// The mailbox's own periodic availability beacon timer -- reset in
    /// full by `reschedule_mailbox_beacon`.
    mailbox_beacon_timer: RefCell<Option<glib::SourceId>>,

    pub direwolf: Rc<DirewolfProcess>,
    pub window: adw::ApplicationWindow,
    /// Pop-out windows, one per detached tab — keyed by tab id so
    /// connection-state changes can refresh them alongside the main bar.
    detached_windows: RefCell<HashMap<TabId, DetachedWindow>>,
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
        self.log.append_line(&format!("[{}] connecting ({})\u{2026}", entry.name, entry.config.kind_label()));

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
            {
                let gesture = gtk::GestureClick::new();
                gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
                let btn = button.clone();
                gesture.connect_pressed(move |_, _, _, _| {
                    btn.remove_css_class("favorite-port-failed");
                });
                button.add_controller(gesture);
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

    /// Rebuild the shared bottom bar's Port dropdown from current config —
    /// call at startup and after any Ports dialog change.
    pub fn rebuild_bottom_ports(&self) {
        let ports = self.state.config.borrow().ports.clone();
        let names: Vec<&str> = ports.iter().map(|p| p.name.as_str()).collect();
        self.bottom_port_dropdown.set_model(Some(&gtk::StringList::new(&names)));
        *self.bottom_ports_snapshot.borrow_mut() = ports;
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
                    let default_call = ui.state.config.borrow().ui.default_call.clone().unwrap_or_default();
                    let text = crate::template_vars::TemplateVars::from_config(&ui.state.config.borrow(), default_call).apply(&message);
                    ui.send_unproto(&port_id, dest.clone(), via.clone(), text.into_bytes());
                }
                glib::ControlFlow::Continue
            });
            self.beacon_timers.borrow_mut().insert(beacon.id.clone(), source);
        }
    }

    /// (Re)schedule the single "available for keyboard-to-keyboard"
    /// availability beacon, discarding any previous timer first. Call at
    /// startup and again whenever keyboard-to-keyboard settings change (the
    /// header button's own toggle, or its settings dialog's Save). No-op
    /// (stops entirely) while disabled or with empty beacon text -- matches
    /// the scheduled-beacon convention of never spamming a port that isn't
    /// even up, checked fresh on every tick rather than once at schedule
    /// time.
    pub fn reschedule_keyboard_mode_beacon(self: &Rc<Self>) {
        if let Some(source) = self.keyboard_mode_beacon_timer.borrow_mut().take() {
            source.remove();
        }
        let (enabled, beacon_text, interval_secs, listen_ports, identity) = {
            let cfg = self.state.config.borrow();
            let identity =
                crate::keyboard_mode::resolve_identity(&cfg.keyboard_mode.node_call, cfg.ui.default_call.as_deref().unwrap_or(""));
            (
                cfg.keyboard_mode.enabled,
                cfg.keyboard_mode.beacon_text.clone(),
                cfg.keyboard_mode.beacon_interval_secs,
                cfg.keyboard_mode.listen_ports.clone(),
                identity,
            )
        };
        if !enabled || beacon_text.trim().is_empty() {
            return;
        }
        let ui = self.clone();
        let source = glib::source::timeout_add_seconds_local(interval_secs.max(1), move || {
            // Re-read the live port list each tick (not just once at
            // schedule time) so a port added later is picked up under the
            // empty-list "any port" convention without needing a fresh
            // Save. Ports that don't support unproto at all (e.g. Telnet/
            // SSH) are silently skipped, same convention used everywhere
            // else in this app for backend capability gaps.
            let ports = ui.state.config.borrow().ports.clone();
            let text = crate::template_vars::TemplateVars::from_config(&ui.state.config.borrow(), identity.clone()).apply(&beacon_text);
            for port in ports.iter().filter(|p| crate::keyboard_mode::listens_on(&listen_ports, &p.id)) {
                if port_supports_unproto(&port.config) && ui.state.is_active(&port.id) {
                    ui.send_unproto(&port.id, "CQ".to_string(), Vec::new(), text.clone().into_bytes());
                }
            }
            glib::ControlFlow::Continue
        });
        *self.keyboard_mode_beacon_timer.borrow_mut() = Some(source);
    }

    /// (Re)schedule the mailbox's own "available for messages" beacon,
    /// discarding any previous timer first -- same shape as
    /// `reschedule_keyboard_mode_beacon`, just keyed off `MailboxPrefs`
    /// instead. Call at startup and again whenever the mailbox's enabled
    /// state or settings change.
    pub fn reschedule_mailbox_beacon(self: &Rc<Self>) {
        if let Some(source) = self.mailbox_beacon_timer.borrow_mut().take() {
            source.remove();
        }
        let (enabled, beacon_text, interval_secs, listen_ports, respond_call) = {
            let cfg = self.state.config.borrow();
            (
                cfg.mailbox.enabled,
                cfg.mailbox.beacon_text.clone(),
                cfg.mailbox.beacon_interval_secs,
                cfg.mailbox.listen_ports.clone(),
                cfg.mailbox.respond_call.clone(),
            )
        };
        if !enabled || beacon_text.trim().is_empty() {
            return;
        }
        let ui = self.clone();
        let source = glib::source::timeout_add_seconds_local(interval_secs.max(1), move || {
            let ports = ui.state.config.borrow().ports.clone();
            let text =
                crate::template_vars::TemplateVars::from_config(&ui.state.config.borrow(), respond_call.clone()).apply(&beacon_text);
            for port in ports.iter().filter(|p| crate::keyboard_mode::listens_on(&listen_ports, &p.id)) {
                if port_supports_unproto(&port.config) && ui.state.is_active(&port.id) {
                    ui.send_unproto(&port.id, "CQ".to_string(), Vec::new(), text.clone().into_bytes());
                }
            }
            glib::ControlFlow::Continue
        });
        *self.mailbox_beacon_timer.borrow_mut() = Some(source);
    }

    /// Create a new connected-session tab (from the dial dialog), fixed at
    /// this identity for its whole lifetime. `connect` issues the actual
    /// dial immediately; otherwise the tab opens disconnected showing
    /// history, for manual review/reconnect later ("Open Disconnected").
    pub fn add_connection_tab(self: &Rc<Self>, port: PortEntry, node: String, via_raw: String, connect: bool) -> TabId {
        let tab_id = self.next_tab_id.get();
        self.next_tab_id.set(tab_id + 1);

        let tab = SessionTab::new(port, node, via_raw, self.state.clone());
        if self.state.is_pinned(&tab.port.id, &tab.node) {
            tab.pin_toggle.set_active(true);
            *tab.pinned_identity.borrow_mut() = Some((tab.port.id.clone(), tab.node.clone()));
        }
        self.preview_history(&tab);

        let root = tab.root.clone();
        let pin_toggle = tab.pin_toggle.clone();
        let save_button = tab.save_button.clone();
        let clear_history_button = tab.clear_history_button.clone();

        self.tabs.borrow_mut().insert(tab_id, tab);

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
                        let via = tab.via_raw.clone();
                        ui.state.set_pinned(&tab.port.id, &tab.node, &via, true);
                        *tab.pinned_identity.borrow_mut() = Some((tab.port.id.clone(), tab.node.clone()));
                    } else if let Some((old_port, old_remote)) = tab.pinned_identity.borrow_mut().take() {
                        ui.state.set_pinned(&old_port, &old_remote, "", false);
                    }
                }
            });
        }
        {
            let ui = self.clone();
            save_button.connect_clicked(move |_| {
                if let Some(tab) = ui.tabs.borrow().get(&tab_id) {
                    let name = format!("{}_{}", tab.port.name, tab.node).replace([':', ' ', '/'], "_");
                    let history_dir = pr_core::AppConfig::config_dir().map(|dir| pr_core::history_dir(&dir, &tab.port.name));
                    crate::export::save_text(&ui.window, &format!("{name}.txt"), tab.full_text(), history_dir.as_deref());
                }
            });
        }
        {
            let ui = self.clone();
            clear_history_button.connect_clicked(move |_| {
                ui.confirm_clear_history(tab_id);
            });
        }

        self.tab_stack.add_named(&root, Some(&tab_id.to_string()));
        self.add_tab_chip(tab_id);
        self.select_tab(tab_id);

        if connect {
            self.connect_tab(tab_id);
        }

        tab_id
    }

    /// `"{port_index}:{node}"` — fixed for the tab's whole lifetime, since
    /// both its port and node are immutable once dialed.
    fn tab_title_text(&self, tab: &SessionTab) -> String {
        let port_index = self.bottom_ports_snapshot.borrow().iter().position(|p| p.id == tab.port.id).unwrap_or(0);
        let via = tab.via();
        if via.is_empty() {
            format!("{port_index}:{}", tab.node)
        } else {
            format!("{port_index}:{} via {}", tab.node, via.join(","))
        }
    }

    /// Build this tab's strip chip (pin left of title, close on the right)
    /// and insert it just before the strip's own trailing "+" button.
    fn add_tab_chip(self: &Rc<Self>, tab_id: TabId) {
        let Some((title_text, pin_toggle)) =
            self.tabs.borrow().get(&tab_id).map(|t| (self.tab_title_text(t), t.pin_toggle.clone()))
        else {
            return;
        };

        // Sleeker than a boxed `.frame` card per chip -- a plain row with a
        // thin straight-line separator (`.tab-chip`, see `apply_base_css`)
        // and tightly-padded flat buttons (`.tab-chip-button`), narrow
        // rather than bulky.
        let chip_root = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        chip_root.add_css_class("tab-chip");

        let click_area = gtk::Button::new();
        click_area.add_css_class("flat");
        click_area.add_css_class("tab-chip-button");
        let title_label = gtk::Label::new(Some(&title_text));
        let title_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        title_box.append(&pin_toggle);
        title_box.append(&title_label);
        click_area.set_child(Some(&title_box));
        {
            let ui = self.clone();
            click_area.connect_clicked(move |_| {
                ui.select_tab(tab_id);
            });
        }
        chip_root.append(&click_area);

        let detach_button = gtk::Button::from_icon_name("window-new-symbolic");
        detach_button.add_css_class("flat");
        detach_button.add_css_class("tab-chip-button");
        detach_button.set_tooltip_text(Some("Pop out"));
        {
            let ui = self.clone();
            detach_button.connect_clicked(move |_| {
                ui.detach_tab(tab_id);
            });
        }
        chip_root.append(&detach_button);

        let close_button = gtk::Button::from_icon_name("window-close-symbolic");
        close_button.add_css_class("flat");
        close_button.add_css_class("tab-chip-button");
        {
            let ui = self.clone();
            close_button.connect_clicked(move |_| {
                ui.close_tab(tab_id);
            });
        }
        chip_root.append(&close_button);

        // Insert right before the strip's own trailing "+" button, so new
        // chips read left-to-right in creation order with "+" staying at
        // the far right.
        let sibling = self.tab_strip_add_button.prev_sibling();
        self.tab_strip.insert_child_after(&chip_root, sibling.as_ref());
        self.tab_chips.borrow_mut().insert(tab_id, TabChip { root: chip_root });
        self.refresh_tab_strip_visibility();
    }

    /// Show the tab strip whenever at least one tab exists, matching the
    /// explicit "the tab bar will not be there when there is no active
    /// connection" requirement.
    fn refresh_tab_strip_visibility(&self) {
        self.tab_strip.set_visible(!self.tabs.borrow().is_empty());
    }

    /// Expand the tab-content pane (Monitor shrinks to ~1/4) showing
    /// `tab_id`, and refresh the bottom bar/status bar/phone button for it.
    /// If the tab is currently popped out, raises its window instead of
    /// showing it inline — clicking its chip in the strip does the same.
    pub fn select_tab(self: &Rc<Self>, tab_id: TabId) {
        // Detached tab: clicking its chip raises the pop-out window.
        if let Some(dw) = self.detached_windows.borrow().get(&tab_id) {
            dw.window.present();
            return;
        }
        if !self.tabs.borrow().contains_key(&tab_id) {
            return;
        }
        self.selected_tab.set(Some(tab_id));
        self.tab_stack.set_visible_child_name(&tab_id.to_string());
        if !self.tab_area_expanded.get() {
            self.tab_area_expanded.set(true);
            self.paned.set_end_child(Some(&self.tab_stack));
            // ~1/4 of a typical window height -- approximate on purpose,
            // the user can still drag the divider.
            self.paned.set_position(180);
        }
        self.refresh_bottom_bar();
        self.refresh_status_bar();
    }

    /// Collapse the tab-content pane back to Monitor-fills-everything,
    /// without closing or disconnecting anything — the tab strip (if any
    /// tabs remain) stays visible so they can be reselected. Triggered by
    /// the dial/minimize button when a tab is currently expanded.
    pub fn minimize_tab_area(&self) {
        self.tab_area_expanded.set(false);
        self.selected_tab.set(None);
        self.paned.set_end_child(gtk::Widget::NONE);
        self.refresh_bottom_bar();
        self.refresh_status_bar();
    }

    /// The dial/minimize button in the bottom bar: opens the dial dialog
    /// when nothing is currently expanded (covers both "no tabs at all" and
    /// "tabs exist but minimized" -- clicking it always starts a *new*
    /// dial in that state, per explicit request), or minimizes the
    /// currently-expanded tab view.
    fn on_dial_button_clicked(self: &Rc<Self>) {
        if self.tab_area_expanded.get() {
            self.minimize_tab_area();
        } else {
            dial_dialog::show(self);
        }
    }

    /// Reflects whether a tab is currently expanded/selected: swaps the
    /// dial/minimize icon, shows/hides the phone-handset button, and hides
    /// Node/Via/Port entirely (not just disables them) once a tab is
    /// selected -- they're the ad-hoc unproto compose destination, not a
    /// read-only mirror of a connected tab's already-fixed identity, so
    /// they have no reason to be visible while one is selected at all.
    fn refresh_bottom_bar(&self) {
        let tabs = self.tabs.borrow();
        match self.selected_tab.get().and_then(|id| tabs.get(&id)) {
            Some(tab) => {
                self.dial_button.set_icon_name("pan-down-symbolic");
                self.dial_button.set_tooltip_text(Some("Minimize"));
                self.phone_button.set_visible(true);
                self.refresh_phone_button(tab);
                self.bottom_node_entry.set_visible(false);
                self.bottom_via_entry.set_visible(false);
                self.bottom_port_dropdown.set_visible(false);
                self.message_entry.set_placeholder_text(Some("Type and press Enter\u{2026}"));
            }
            None => {
                self.dial_button.set_icon_name("list-add-symbolic");
                self.dial_button.set_tooltip_text(Some("Dial\u{2026}"));
                self.phone_button.set_visible(false);
                // Clear any node/via left behind by whichever tab was
                // selected before -- these fields are ad-hoc unproto entry
                // now, not a read-only mirror of a tab, so a stale callsign
                // here would otherwise look like a real destination.
                self.bottom_node_entry.set_text("");
                self.bottom_via_entry.set_text("");
                self.bottom_node_entry.set_visible(true);
                self.bottom_via_entry.set_visible(true);
                self.bottom_port_dropdown.set_visible(true);
                self.message_entry.set_placeholder_text(Some("Unproto message\u{2026}"));
            }
        }
        self.refresh_send_button_sensitivity();
    }

    /// Send is blocked in two situations: in the ad-hoc unproto mode (no tab
    /// selected) when no destination has been typed, and in the connected-tab
    /// mode when the tab is currently disconnected — there is nothing to send
    /// to yet. The "Not connected" log message path in `activate_message_entry`
    /// handles the edge case of typing fast right as a connection drops.
    fn refresh_send_button_sensitivity(&self) {
        let tabs = self.tabs.borrow();
        let sensitive = match self.selected_tab.get().and_then(|id| tabs.get(&id)) {
            Some(tab) => tab.conn_id.get().is_some(),
            None => !self.bottom_node_entry.text().trim().is_empty(),
        };
        self.send_button.set_sensitive(sensitive);
    }

    /// Recolor/relabel the phone-handset button for `tab`'s current connect
    /// state — green (ready to connect) while disconnected, red (ready to
    /// disconnect) while connected; same explicit CSS-class-swap pattern as
    /// the Direwolf button.
    fn refresh_phone_button(&self, tab: &SessionTab) {
        self.phone_button.remove_css_class("state-success");
        self.phone_button.remove_css_class("state-destructive");
        let connected = tab.conn_id.get().is_some();
        if connected {
            self.phone_button.add_css_class("state-destructive");
            self.phone_button.set_tooltip_text(Some("Disconnect"));
        } else {
            self.phone_button.add_css_class("state-success");
            self.phone_button.set_tooltip_text(Some("Connect"));
        }
    }

    /// The phone-handset button: connect or disconnect whichever tab is
    /// currently selected.
    fn on_phone_button_clicked(self: &Rc<Self>) {
        let Some(tab_id) = self.selected_tab.get() else { return };
        let is_connected = self.tabs.borrow().get(&tab_id).is_some_and(|t| t.conn_id.get().is_some());
        if is_connected {
            self.disconnect_tab(tab_id);
        } else {
            self.connect_tab(tab_id);
        }
    }

    /// Navigate send history with Up/Down in `entry`. Always consumes Up
    /// and Down so the entry never loses focus to GTK's built-in
    /// focus-chain navigation — if there is no history yet the keys are
    /// swallowed but the entry text is unchanged.
    fn navigate_send_history(&self, entry: &gtk::Entry, key: gtk::gdk::Key) -> glib::Propagation {
        match key {
            gtk::gdk::Key::Up => {
                let history = self.send_history.borrow();
                let len = history.len();
                if len > 0 {
                    let new_pos = match self.history_pos.get() {
                        None => {
                            *self.history_draft.borrow_mut() = entry.text().to_string();
                            0
                        }
                        Some(i) if i + 1 < len => i + 1,
                        Some(i) => i,
                    };
                    self.history_pos.set(Some(new_pos));
                    entry.set_text(&history[len - 1 - new_pos]);
                    entry.set_position(-1);
                }
                glib::Propagation::Stop
            }
            gtk::gdk::Key::Down => {
                let history = self.send_history.borrow();
                let len = history.len();
                if len > 0 {
                    match self.history_pos.get() {
                        None => {}
                        Some(0) => {
                            self.history_pos.set(None);
                            let draft = self.history_draft.borrow().clone();
                            entry.set_text(&draft);
                            entry.set_position(-1);
                        }
                        Some(i) => {
                            let new_pos = i - 1;
                            self.history_pos.set(Some(new_pos));
                            entry.set_text(&history[len - 1 - new_pos]);
                            entry.set_position(-1);
                        }
                    }
                }
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    fn on_message_entry_key_pressed(&self, key: gtk::gdk::Key) -> glib::Propagation {
        self.navigate_send_history(&self.message_entry.clone(), key)
    }

    /// Prompt to confirm, then permanently clear the persisted history *and*
    /// the visible scrollback for the given tab.
    fn confirm_clear_history(self: &Rc<Self>, tab_id: TabId) {
        let Some((port_id, remote)) = self.tabs.borrow().get(&tab_id).map(|tab| tab.history_key()) else {
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
                ui.state.clear_history(&port_id, &remote);
                if let Some(tab) = ui.tabs.borrow().get(&tab_id) {
                    tab.clear_text();
                }
            }
        });
    }

    /// Send whatever's in the shared message entry — shared by both
    /// pressing Enter and clicking Send. Sends into the currently selected
    /// tab's live connection if one is expanded, otherwise as an ad-hoc
    /// unproto frame using the bottom bar's own Node/Via/Port fields.
    fn activate_message_entry(self: &Rc<Self>) {
        let text = self.message_entry.text().to_string();
        if let Some(tab_id) = self.selected_tab.get() {
            if text.is_empty() {
                return;
            }
            let tabs = self.tabs.borrow();
            let Some(tab) = tabs.get(&tab_id) else { return };
            if tab.conn_id.get().is_none() {
                drop(tabs);
                self.log.append_line("Not connected \u{2014} message kept in the input field.");
                return;
            }
            self.send_tab_connected(tab, &text);
            drop(tabs);
        } else {
            // Unproto: blank frames are valid (common beacon/ping practice).
            self.send_bottom_unproto(&text);
        }
        // Save to history, suppressing adjacent duplicates and blank entries.
        if !text.is_empty() {
            let mut history = self.send_history.borrow_mut();
            if history.last().map(|s| s.as_str()) != Some(text.as_str()) {
                history.push(text);
            }
        }
        self.history_pos.set(None);
        *self.history_draft.borrow_mut() = String::new();
        self.message_entry.set_text("");
    }

    /// Send the selected tab's input over its live connection.
    fn send_tab_connected(&self, tab: &SessionTab, text: &str) {
        let mut bytes = text.to_string().into_bytes();
        bytes.extend_from_slice(line_ending(&tab.port.config));
        if let Some(conn_id) = tab.conn_id.get() {
            if let Some(handle) = self.state.active.borrow().get(&tab.port.id) {
                let _ = handle.cmd_tx.send(PortCommand::Send { id: conn_id, bytes });
            }
            // Echo every sent line into the scrollback with a `»` prefix so
            // the operator always sees what they transmitted, regardless of
            // port type. AX.25/AGWPE/KISS backends never echo back at the
            // protocol level; Telnet/SSH may echo from the remote end, but
            // the `»` prefix makes the local copy visually distinct.
            tab.append_sent_line(text);
            if port_supports_connect(&tab.port.config) {
                self.monitor.append_line(&tab.port.id, &format!("[{}] TX > {}: {text}", tab.port.name, tab.node), false);
            }
            self.refresh_status_bar();
        }
    }

    /// Send the bottom bar's message as a one-shot unconnected (UI) frame,
    /// using its own Node/Via/Port fields — not tied to any tab.
    fn send_bottom_unproto(&self, text: &str) {
        let ports = self.bottom_ports_snapshot.borrow();
        let Some(port) = ports.get(self.bottom_port_dropdown.selected() as usize).cloned() else { return };
        drop(ports);
        if !port_supports_unproto(&port.config) {
            self.log.append_line(&format!("[{}] this port doesn't support unproto sending.", port.name));
            return;
        }
        let dest = self.bottom_node_entry.text().trim().to_uppercase();
        if dest.is_empty() {
            self.log.append_line("Enter a destination before sending unproto.");
            return;
        }
        if !self.state.is_active(&port.id) {
            self.log.append_line(&format!("[{}] port not connected \u{2014} can't send unproto.", port.name));
            return;
        }
        let via: Vec<String> = self
            .bottom_via_entry
            .text()
            .split([',', ' '])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_uppercase)
            .collect();
        if let Some(handle) = self.state.active.borrow().get(&port.id) {
            let _ = handle.cmd_tx.send(PortCommand::SendUnproto {
                dest: dest.clone(),
                via: via.clone(),
                bytes: text.to_string().into_bytes(),
            });
        }
        let via_suffix = if via.is_empty() { String::new() } else { format!(" via {}", via.join(",")) };
        self.monitor.append_line(&port.id, &format!("[{}] TX unproto > {dest}{via_suffix}: {text}", port.name), true);
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
        let remote_call = tab.node.trim().to_uppercase();
        for line in tab.take_mailbox_lines(chunk) {
            let mut state_slot = tab.mailbox_state.borrow_mut();
            let Some(state) = state_slot.as_mut() else { break };
            let timestamp = crate::app_state::now_timestamp();
            let (response, close, qso_entry) = {
                let mut cfg = self.state.config.borrow_mut();
                crate::mailbox::handle_line(state, &mut cfg.mailbox.messages, &remote_call, port_id, &line, &timestamp)
            };
            if let Some(entry) = qso_entry {
                self.state.config.borrow_mut().qso_log.push(entry);
            }
            drop(state_slot);
            self.state.save_config();
            self.refresh_mailbox_button();
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

    /// Watch a live keyboard-to-keyboard tab for `/bye` (and `B`/`BYE`) from
    /// the remote station: send a sign-off, then close just this AX.25
    /// connection. The port stays up and K2K mode stays enabled — only the
    /// single connected session ends, leaving the tab disconnected as if the
    /// remote had dropped carrier.
    fn drive_k2k(&self, tab: &SessionTab, port_id: &str, conn_id: ConnectionId, chunk: &str) {
        if !tab.k2k_active.get() {
            return;
        }
        for line in tab.take_mailbox_lines(chunk) {
            if crate::keyboard_mode::is_bye(&line) {
                tab.k2k_active.set(false);
                self.send_tab_text(tab, port_id, "73, disconnecting...\n");
                if let Some(handle) = self.state.active.borrow().get(port_id) {
                    let _ = handle.cmd_tx.send(PortCommand::CloseConnection { id: conn_id });
                }
                break;
            }
        }
    }

    /// Connect the tab's port (if not already active) and, for connect-
    /// capable ports, open a session to its node; for Telnet/SSH, connect
    /// the port itself (that connection *is* the whole session) -- the
    /// greeting line (if any) is sent once `ConnectionOpened` confirms it.
    pub fn connect_tab(self: &Rc<Self>, tab_id: TabId) {
        let Some((port_id, node, via, needs_node)) = self.tabs.borrow().get(&tab_id).map(|tab| {
            (tab.port.id.clone(), tab.node.clone(), tab.via(), port_supports_connect(&tab.port.config))
        }) else {
            return;
        };

        if needs_node && node.is_empty() {
            self.log.append_line("No node set for this tab \u{2014} can't connect.");
            return;
        }

        let pending_key = if needs_node { (port_id.clone(), node.clone()) } else { (port_id.clone(), String::new()) };
        self.pending.borrow_mut().insert(pending_key, tab_id);

        if let Some(tab) = self.tabs.borrow().get(&tab_id) {
            let msg = if needs_node && !node.is_empty() {
                format!("Connecting to {node}\u{2026}")
            } else {
                "Connecting\u{2026}".to_string()
            };
            tab.append_status_line(&msg);
        }

        if !self.state.is_active(&port_id) {
            self.connect_port(&port_id);
        }
        if needs_node {
            if let Some(handle) = self.state.active.borrow().get(&port_id) {
                let _ = handle.cmd_tx.send(PortCommand::OpenConnection { remote: node, via });
            }
        }
    }

    pub fn disconnect_tab(&self, tab_id: TabId) {
        let Some((port_id, conn_id)) = self.tabs.borrow().get(&tab_id).and_then(|tab| Some((tab.port.id.clone(), tab.conn_id.get()?)))
        else {
            return;
        };
        if let Some(handle) = self.state.active.borrow().get(&port_id) {
            let _ = handle.cmd_tx.send(PortCommand::CloseConnection { id: conn_id });
        }
    }

    /// Send a line from a detached window's own message entry, mirroring
    /// `activate_message_entry` but scoped to a specific tab (not the
    /// selected one) and without touching `self.message_entry`.
    fn activate_detached_message(self: &Rc<Self>, tab_id: TabId, entry: &gtk::Entry) {
        let text = entry.text().to_string();
        if text.is_empty() {
            return;
        }
        {
            let tabs = self.tabs.borrow();
            let Some(tab) = tabs.get(&tab_id) else { return };
            if tab.conn_id.get().is_none() {
                return;
            }
            self.send_tab_connected(tab, &text);
        }
        {
            let mut history = self.send_history.borrow_mut();
            if history.last().map(|s| s.as_str()) != Some(text.as_str()) {
                history.push(text);
            }
        }
        self.history_pos.set(None);
        *self.history_draft.borrow_mut() = String::new();
        entry.set_text("");
        self.refresh_detached_tab(tab_id);
    }

    /// Update a detached window's phone button, send-button sensitivity,
    /// and status label to match the current connection state of `tab_id`.
    fn refresh_detached_tab(&self, tab_id: TabId) {
        let refs = {
            let dws = self.detached_windows.borrow();
            dws.get(&tab_id).map(|dw| (dw.phone_button.clone(), dw.send_button.clone(), dw.status_label.clone()))
        };
        let Some((phone_btn, send_btn, status_lbl)) = refs else { return };
        let tabs = self.tabs.borrow();
        if let Some(tab) = tabs.get(&tab_id) {
            let connected = tab.conn_id.get().is_some();
            phone_btn.remove_css_class("state-success");
            phone_btn.remove_css_class("state-destructive");
            if connected {
                phone_btn.add_css_class("state-destructive");
                phone_btn.set_tooltip_text(Some("Disconnect"));
            } else {
                phone_btn.add_css_class("state-success");
                phone_btn.set_tooltip_text(Some("Connect"));
            }
            send_btn.set_sensitive(connected);
            let status_text = match (connected, tab.elapsed_text()) {
                (true, Some(elapsed)) => format!("Connected to {} \u{2014} {elapsed}", tab.node),
                (true, None) => format!("Connected to {}", tab.node),
                (false, _) => "Disconnected".to_string(),
            };
            status_lbl.set_text(&status_text);
        }
    }

    /// Pop `tab_id` out into its own non-modal, non-transient window.
    /// Calling again while the window is already open just raises it.
    fn detach_tab(self: &Rc<Self>, tab_id: TabId) {
        // Raise the existing window if already detached.
        let existing = self.detached_windows.borrow().get(&tab_id).map(|dw| dw.window.clone());
        if let Some(w) = existing {
            w.present();
            return;
        }

        let (title, buffer) = {
            let tabs = self.tabs.borrow();
            let Some(tab) = tabs.get(&tab_id) else { return };
            (self.tab_title_text(tab), tab.buffer_ref().clone())
        };

        let dw = adw::Window::builder().title(&title).default_width(640).default_height(500).build();
        if let Some(app) = self.window.application() {
            app.add_window(&dw);
        }

        // Header: Save and Clear History buttons in the title bar.
        let header = adw::HeaderBar::new();
        let save_btn = gtk::Button::with_label("Save\u{2026}");
        save_btn.add_css_class("flat");
        let clear_btn = gtk::Button::with_label("Clear History\u{2026}");
        clear_btn.add_css_class("flat");
        header.pack_start(&save_btn);
        header.pack_end(&clear_btn);

        // Scrollback: a second view backed by the same buffer so both
        // the main tab and this window always show the same text.
        let text_view = gtk::TextView::builder()
            .buffer(&buffer)
            .editable(false)
            .cursor_visible(false)
            .monospace(true)
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(4)
            .bottom_margin(4)
            .left_margin(6)
            .right_margin(6)
            .build();
        text_view.add_css_class("pr-mono");
        let scrolled = gtk::ScrolledWindow::builder().child(&text_view).vexpand(true).build();

        // Bottom bar: phone (connect/disconnect) + message entry + Send.
        let phone_button = gtk::Button::from_icon_name("call-start-symbolic");
        phone_button.add_css_class("flat");
        let msg_entry = gtk::Entry::builder().hexpand(true).placeholder_text("Type and press Enter\u{2026}").build();
        let send_button = gtk::Button::with_label("Send");
        send_button.add_css_class("suggested-action");
        let bottom_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bottom_bar.set_margin_start(8);
        bottom_bar.set_margin_end(8);
        bottom_bar.set_margin_top(4);
        bottom_bar.set_margin_bottom(4);
        bottom_bar.append(&phone_button);
        bottom_bar.append(&msg_entry);
        bottom_bar.append(&send_button);

        // Status label at the very bottom.
        let status_label = gtk::Label::new(Some(""));
        status_label.set_halign(gtk::Align::Start);
        status_label.set_margin_start(8);
        status_label.set_margin_end(8);
        status_label.set_margin_bottom(6);
        status_label.add_css_class("caption");

        let inner = gtk::Box::new(gtk::Orientation::Vertical, 0);
        inner.append(&scrolled);
        inner.append(&bottom_bar);
        inner.append(&status_label);

        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&header);
        toolbar_view.set_content(Some(&inner));
        dw.set_content(Some(&toolbar_view));

        // Wire up Save.
        {
            let ui = self.clone();
            let dw_ref = dw.clone();
            save_btn.connect_clicked(move |_| {
                let tabs = ui.tabs.borrow();
                if let Some(tab) = tabs.get(&tab_id) {
                    let name = format!("{}_{}", tab.port.name, tab.node).replace([':', ' ', '/'], "_");
                    let history_dir = pr_core::AppConfig::config_dir().map(|dir| pr_core::history_dir(&dir, &tab.port.name));
                    crate::export::save_text(&dw_ref, &format!("{name}.txt"), tab.full_text(), history_dir.as_deref());
                }
            });
        }

        // Wire up Clear History (confirm dialog uses the main window as parent).
        {
            let ui = self.clone();
            clear_btn.connect_clicked(move |_| ui.confirm_clear_history(tab_id));
        }

        // Wire up phone button.
        {
            let ui = self.clone();
            phone_button.connect_clicked(move |_| {
                let is_connected = ui.tabs.borrow().get(&tab_id).is_some_and(|t| t.conn_id.get().is_some());
                if is_connected { ui.disconnect_tab(tab_id); } else { ui.connect_tab(tab_id); }
            });
        }

        // Wire up message send (entry activate + Send button click).
        {
            let ui = self.clone();
            let entry = msg_entry.clone();
            msg_entry.connect_activate(move |_| ui.activate_detached_message(tab_id, &entry));
        }
        {
            let ui = self.clone();
            let entry = msg_entry.clone();
            send_button.connect_clicked(move |_| ui.activate_detached_message(tab_id, &entry));
        }

        // Key controller: Up/Down history navigation in the message entry.
        {
            let kc = gtk::EventControllerKey::new();
            let ui = self.clone();
            let entry = msg_entry.clone();
            kc.connect_key_pressed(move |_, key, _, _| ui.navigate_send_history(&entry, key));
            msg_entry.add_controller(kc);
        }

        // Scroll to the bottom of the (already-populated) buffer once rendered.
        {
            let tv = text_view.clone();
            let buf = buffer.clone();
            glib::idle_add_local_once(move || {
                let mark = buf.create_mark(None, &buf.end_iter(), false);
                tv.scroll_mark_onscreen(&mark);
            });
        }

        // When the user closes the pop-out window, return the tab to the
        // main view by removing it from the detached map (so select_tab
        // goes inline again) then selecting it there.  The handler ID is
        // stored in DetachedWindow so close_tab can disconnect it before
        // calling window.close(), preventing this re-attach logic from
        // firing when the tab itself is being permanently removed.
        let close_handler = {
            let ui = self.clone();
            dw.connect_close_request(move |_| {
                ui.detached_windows.borrow_mut().remove(&tab_id);
                // Only re-attach if the tab still exists (not being closed).
                if ui.tabs.borrow().contains_key(&tab_id) {
                    ui.select_tab(tab_id);
                }
                glib::Propagation::Proceed
            })
        };

        self.detached_windows.borrow_mut().insert(tab_id, DetachedWindow {
            window: dw.clone().upcast(),
            phone_button,
            send_button,
            status_label,
            close_handler,
        });

        // Deselect from the main view now that it lives in its own window.
        // Find another non-detached tab to show; collapse if none.
        if self.selected_tab.get() == Some(tab_id) {
            let next = {
                let tabs = self.tabs.borrow();
                let dws = self.detached_windows.borrow();
                tabs.keys().copied().find(|&id| id != tab_id && !dws.contains_key(&id))
            };
            if let Some(next_id) = next {
                self.select_tab(next_id);
            } else {
                self.minimize_tab_area();
            }
        }

        // Set initial widget state before presenting.
        self.refresh_detached_tab(tab_id);
        dw.present();
    }

    /// Remove a tab entirely: disconnect it first if live (sends a proper
    /// disconnect over the wire rather than just dropping the UI side),
    /// unpin it, and drop its chip/content page.
    pub fn close_tab(self: &Rc<Self>, tab_id: TabId) {
        self.disconnect_tab(tab_id);

        // Close the detached pop-out window first (if any). Disconnect
        // its close-request handler before calling close() so the "return
        // to main window" logic doesn't fire — the tab is being removed,
        // not just returned. Extract the window after removing from the map
        // so the borrow is dropped before close() can fire close-request.
        let detached_window = self.detached_windows.borrow_mut().remove(&tab_id);
        if let Some(dw) = detached_window {
            dw.window.disconnect(dw.close_handler);
            dw.window.close();
        }

        if let Some(tab) = self.tabs.borrow().get(&tab_id) {
            if let Some((old_port, old_remote)) = tab.pinned_identity.borrow_mut().take() {
                self.state.set_pinned(&old_port, &old_remote, "", false);
            }
        }
        self.bound.borrow_mut().retain(|_, v| *v != tab_id);
        self.pending.borrow_mut().retain(|_, v| *v != tab_id);

        if self.selected_tab.get() == Some(tab_id) {
            self.minimize_tab_area();
        }

        // Extracted into its own `let` first (not matched directly on
        // `self.tabs.borrow_mut().remove(...)`) so the `RefMut` guard is
        // dropped before `tab_stack.remove` runs below -- a stale lesson
        // from this exact class of bug earlier in this project: an `if let`
        // scrutinee's temporaries live for the whole block, so a GTK call
        // that can re-enter and re-borrow `tabs` (as `remove_page` on the
        // old `Notebook` once did) would otherwise panic.
        let removed_tab = self.tabs.borrow_mut().remove(&tab_id);
        if let Some(tab) = removed_tab {
            self.tab_stack.remove(&tab.root);
        }
        if let Some(chip) = self.tab_chips.borrow_mut().remove(&tab_id) {
            self.tab_strip.remove(&chip.root);
        }
        self.refresh_tab_strip_visibility();
        self.refresh_status_bar();
    }


    /// Load this tab's persisted history into its scrollback right after
    /// creation (before/without connecting) — "Open Disconnected" is
    /// specifically for this kind of offline review.
    fn preview_history(&self, tab: &SessionTab) {
        let (port_id, remote) = tab.history_key();
        let history = self.state.history_for(&port_id, &remote);
        tab.load_history(&history);
    }

    fn handle_event(self: &Rc<Self>, port_id: &str, event: PortEvent) {
        match event {
            PortEvent::PortConnected => {
                self.log.append_line(&format!("[{port_id}] port connected"));
                self.confirmed_ports.borrow_mut().insert(port_id.to_string());
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
                self.log.append_line(&format!("[{port_id}] port disconnected{suffix}"));

                // Unbind (not remove — tabs persist) every tab this port had.
                let affected: Vec<(String, ConnectionId)> =
                    self.bound.borrow().keys().filter(|(pid, _)| pid == port_id).cloned().collect();
                for key in affected {
                    if let Some(tab_id) = self.bound.borrow_mut().remove(&key) {
                        if let Some(tab) = self.tabs.borrow().get(&tab_id) {
                            tab.conn_id.set(None);
                            tab.mark_disconnected();
                        }
                        if self.selected_tab.get() == Some(tab_id) {
                            if let Some(tab) = self.tabs.borrow().get(&tab_id) {
                                self.refresh_phone_button(tab);
                            }
                            self.refresh_send_button_sensitivity();
                        }
                        self.refresh_detached_tab(tab_id);
                    }
                }
                self.pending.borrow_mut().retain(|(pid, _), _| pid != port_id);
                self.refresh_favorite_button(port_id);
                self.refresh_status_bar();
            }
            PortEvent::PortError { message } => {
                self.log.append_line(&format!("[{port_id}] ERROR: {message}"));
                // Only a port that never reached `PortConnected` at all is a
                // genuine connect failure -- some backends (e.g. a bad
                // outgoing KISS frame) report a `PortError` for a non-fatal
                // problem on an already-live port, which must not clear its
                // `active` entry or favorites-bar button.
                if !self.confirmed_ports.borrow().contains(port_id) {
                    self.state.active.borrow_mut().remove(port_id);
                    self.mark_favorite_failed(port_id);
                    self.refresh_status_bar();
                }
            }
            PortEvent::Monitor { line, from, to, message } => {
                // `from`/`to`/`message` are `Some` together only for a
                // genuine received UI frame (see `PortEvent::Monitor`'s own
                // doc comment); our own sent unproto lines set them to
                // `None` instead but are tagged "[unproto TX]" in `line` --
                // both count as "unproto" for the header's "UI" filter.
                let is_unproto = from.is_some() || line.contains("[unproto TX]");
                self.monitor.append_line(port_id, &format!("[{port_id}] {line}"), is_unproto);
                if let (Some(from), Some(to), Some(message)) = (from, to, message) {
                    self.maybe_notify_directed(port_id, &from, &to, &message, &line);
                    self.maybe_detect_beacon(port_id, &from, &to, &message);
                    self.maybe_record_beacon_packet(&from, &to, &message);
                    self.maybe_detect_mail(port_id, &from, &to, &message);
                }
            }
            PortEvent::ConnectionOpened { id, label, to } => {
                let needs_node =
                    find_entry(&self.state.config.borrow(), port_id).map(|e| port_supports_connect(&e.config)).unwrap_or(false);
                let pending_key =
                    if needs_node { (port_id.to_string(), label.clone()) } else { (port_id.to_string(), String::new()) };

                let existing_tab_id = self.pending.borrow_mut().remove(&pending_key);
                let is_new_incoming = existing_tab_id.is_none();
                let tab_id = existing_tab_id.unwrap_or_else(|| {
                    find_entry(&self.state.config.borrow(), port_id)
                        .map(|port| self.add_connection_tab(port, label.clone(), String::new(), false))
                        .unwrap_or(0)
                });

                self.bound.borrow_mut().insert((port_id.to_string(), id), tab_id);
                let greeting = self.tabs.borrow().get(&tab_id).map(|t| t.via_raw.clone()).filter(|_| !needs_node);
                if let Some(tab) = self.tabs.borrow().get(&tab_id) {
                    tab.conn_id.set(Some(id));
                    tab.mark_connected();
                    tab.append_status_line("Connected");

                    if needs_node && is_new_incoming {
                        // An unsolicited connect can be auto-answered by
                        // either keyboard-to-keyboard mode (a live tab a
                        // human then types into) or the mailbox (a BBS-style
                        // command prompt) -- never both. Keyboard-to-keyboard
                        // takes priority when both would match the same
                        // connect (e.g. the mailbox's `respond_call` left
                        // blank/set to the same callsign), since it's the
                        // more "a human is actually here right now" of the
                        // two. Each has its own port allow-list (empty means
                        // "any connect-capable port", each feature's
                        // original behavior before per-port filtering
                        // existed).
                        let (k2k_enabled, k2k_identity, k2k_welcome, k2k_listen_ports) = {
                            let cfg = self.state.config.borrow();
                            let identity = crate::keyboard_mode::resolve_identity(
                                &cfg.keyboard_mode.node_call,
                                cfg.ui.default_call.as_deref().unwrap_or(""),
                            );
                            (
                                cfg.keyboard_mode.enabled,
                                identity,
                                cfg.keyboard_mode.welcome_message.clone(),
                                cfg.keyboard_mode.listen_ports.clone(),
                            )
                        };
                        let (mb_enabled, mb_respond_call, mb_intro, mb_listen_ports) = {
                            let cfg = self.state.config.borrow();
                            (
                                cfg.mailbox.enabled,
                                cfg.mailbox.respond_call.trim().to_uppercase(),
                                cfg.mailbox.intro_message.clone(),
                                cfg.mailbox.listen_ports.clone(),
                            )
                        };

                        if crate::keyboard_mode::should_answer(k2k_enabled, &k2k_identity, to.as_deref())
                            && crate::keyboard_mode::listens_on(&k2k_listen_ports, port_id)
                        {
                            tab.k2k_active.set(true);
                            tab.append_status_line("Keyboard-to-keyboard session");
                            let welcome = if k2k_welcome.trim().is_empty() {
                                crate::keyboard_mode::default_welcome(&k2k_identity)
                            } else {
                                let vars = crate::template_vars::TemplateVars::from_config(&self.state.config.borrow(), k2k_identity.clone());
                                format!("{}\n", vars.apply(k2k_welcome.trim_end()))
                            };
                            self.send_tab_text(tab, port_id, &welcome);
                        } else if crate::mailbox::should_answer(mb_enabled, &mb_respond_call, to.as_deref())
                            && crate::keyboard_mode::listens_on(&mb_listen_ports, port_id)
                        {
                            *tab.mailbox_state.borrow_mut() = Some(crate::mailbox::MailboxState::Command);
                            // `should_answer` above already guarantees
                            // `mb_respond_call` is non-empty -- the mailbox
                            // never falls back to the general Profile
                            // callsign, unlike keyboard-to-keyboard.
                            let banner = if mb_intro.trim().is_empty() {
                                crate::mailbox::welcome_banner(&mb_respond_call)
                            } else {
                                let vars = crate::template_vars::TemplateVars::from_config(&self.state.config.borrow(), mb_respond_call.clone());
                                format!("{}\n", vars.apply(mb_intro.trim_end()))
                            };
                            self.send_tab_text(tab, port_id, &banner);
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
                // Telnet/SSH "Address" field: send it verbatim as the first
                // line, exactly as if typed at the far end's own prompt --
                // no protocol assumed, maximally flexible per explicit
                // request. `greeting` is `None` for connect-capable ports
                // (Agwpe/Ax25RawSocket), where the via slot is a real
                // digipeater path instead, already consumed by `OpenConnection`.
                if let Some(greeting) = greeting.filter(|g| !g.is_empty()) {
                    self.send_tab_text_raw(port_id, id, &greeting);
                }
                if needs_node {
                    self.state.log_qso_started(port_id, &label);
                }
                if self.selected_tab.get() == Some(tab_id) {
                    self.refresh_bottom_bar();
                    self.message_entry.grab_focus();
                }
                self.refresh_status_bar();
                self.refresh_detached_tab(tab_id);
            }
            PortEvent::ConnectionClosed { id } => {
                let was_established = self.established_conns.borrow_mut().remove(&(port_id.to_string(), id));
                if let Some(tab_id) = self.bound.borrow_mut().remove(&(port_id.to_string(), id)) {
                    if let Some(tab) = self.tabs.borrow().get(&tab_id) {
                        let is_connect_port =
                            find_entry(&self.state.config.borrow(), port_id).map(|e| port_supports_connect(&e.config)).unwrap_or(false);
                        if is_connect_port {
                            self.state.log_qso_ended(port_id, &tab.node);
                        }
                        tab.conn_id.set(None);
                        tab.mark_disconnected();
                        tab.flush_pending();
                        let status_msg = if was_established { "Disconnected" } else { "Connection timed out" };
                        tab.append_status_line(status_msg);
                        *tab.mailbox_state.borrow_mut() = None;
                        tab.k2k_active.set(false);
                    }
                    if self.selected_tab.get() == Some(tab_id) {
                        self.refresh_bottom_bar();
                    }
                    self.refresh_detached_tab(tab_id);
                }
                self.refresh_status_bar();
            }
            PortEvent::ConnState { id, state } => {
                self.log.append_line(&format!("[{port_id}] connection {id}: {}", describe_state(state)));
                if state == ConnState::Connected {
                    self.established_conns.borrow_mut().insert((port_id.to_string(), id));
                }
            }
            PortEvent::Data { id, bytes } => {
                if let Some(&tab_id) = self.bound.borrow().get(&(port_id.to_string(), id)) {
                    if let Some(tab) = self.tabs.borrow().get(&tab_id) {
                        let text = String::from_utf8_lossy(&bytes).replace('\0', "");
                        tab.receive_data(&text);
                        self.drive_mailbox(tab, port_id, &text);
                        self.drive_k2k(tab, port_id, id, &text);
                    }
                    self.refresh_detached_tab(tab_id);
                }
                self.refresh_status_bar();
            }
            PortEvent::StationHeard { callsign } => {
                self.state.record_heard(&callsign);
            }
            PortEvent::NodesBroadcast { from, sender_alias, entries } => {
                self.state.record_nodes_broadcast(&from, &sender_alias, &entries);
            }
        }
    }

    /// Send a raw line directly over a known `(port_id, ConnectionId)` —
    /// used for the Telnet/SSH greeting line, which fires before any tab
    /// lookup/echo bookkeeping is relevant (the connection was *just*
    /// confirmed open).
    fn send_tab_text_raw(&self, port_id: &str, conn_id: ConnectionId, text: &str) {
        if let Some(handle) = self.state.active.borrow().get(port_id) {
            let ending = find_entry(&self.state.config.borrow(), port_id).map(|e| line_ending(&e.config)).unwrap_or(b"\n");
            let mut bytes = text.as_bytes().to_vec();
            bytes.extend_from_slice(ending);
            let _ = handle.cmd_tx.send(PortCommand::Send { id: conn_id, bytes });
        }
    }

    /// Fire a desktop notification if `to` is directed at the configured
    /// callsign. Always records the packet and lights the Notifications button;
    /// only fires a desktop alert/sound when not silenced.
    fn maybe_notify_directed(&self, port_id: &str, from: &str, to: &str, message: &str, line: &str) {
        let matcher = crate::notify::NotifyMatcher::build(&self.state.config.borrow());
        if !matcher.matches_directed(to) {
            return;
        }
        self.state.record_notified_packet(port_id, line);
        self.mark_notification_received();
        let silenced = self.state.config.borrow().notify.notifications_silenced;
        if !silenced {
            let port_name = find_entry(&self.state.config.borrow(), port_id).map(|e| e.name).unwrap_or_else(|| port_id.to_string());
            let title = format!("Packet Radio \u{2014} {port_name}");
            crate::notify::send(&self.window, &title, &format!("From: {from}\nTo: {to}\n{message}"));
            crate::notify::play_sound(&self.state.config.borrow());
        }
    }

    /// Record and (optionally) notify on a received frame matching a
    /// `BeaconMonitorRule`, tracked separately from the general directed/
    /// custom-rule notifications above — a beacon match always lights up
    /// the header button regardless of whether `notify.beacon_enabled` also
    /// fires a desktop notification for it.
    fn maybe_detect_beacon(&self, port_id: &str, from: &str, to: &str, message: &str) {
        let label = {
            let cfg = self.state.config.borrow();
            let from_base = crate::qrz::strip_ssid(from.trim()).to_uppercase();
            let mut found = None;
            for rule in cfg.beacon_rules.iter().filter(|r| r.enabled) {
                if !rule.sender.trim().is_empty() {
                    let rule_sender = crate::qrz::strip_ssid(rule.sender.trim()).to_uppercase();
                    if !rule_sender.eq_ignore_ascii_case(&from_base) {
                        continue;
                    }
                }
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
        self.mark_notification_received();

        if !self.state.config.borrow().notify.notifications_silenced {
            let port_name = find_entry(&self.state.config.borrow(), port_id).map(|e| e.name).unwrap_or_else(|| port_id.to_string());
            let title = format!("Packet Radio \u{2014} {port_name} (Destination Monitor: {label})");
            crate::notify::send(&self.window, &title, &format!("From: {from}\nTo: {to}\n{message}"));
            crate::notify::play_sound(&self.state.config.borrow());
        }
    }

    /// Log a message a station sent to the literal destination "BEACON" into
    /// that station's own address-book entry (`AddressBookEntry.recent_beacons`)
    /// — distinct from `maybe_detect_beacon` above, which is about the
    /// user's own configurable destination-pattern rules and a global
    /// cross-station log, not a per-station one.
    fn maybe_record_beacon_packet(&self, from: &str, to: &str, message: &str) {
        if to.eq_ignore_ascii_case("BEACON") {
            self.state.record_id_packet(from, message);
        }
    }

    /// Detect a MAIL-destination frame that names our callsign, indicating
    /// that another node has mail waiting for us. Records a directed
    /// notification so it appears in the Notifications dialog.
    fn maybe_detect_mail(&self, port_id: &str, from: &str, to: &str, message: &str) {
        if !to.eq_ignore_ascii_case("MAIL") {
            return;
        }
        let our_call = {
            let cfg = self.state.config.borrow();
            cfg.ui.default_call.clone().unwrap_or_default()
        };
        if our_call.is_empty() {
            return;
        }
        let our_base = crate::qrz::strip_ssid(our_call.trim()).to_uppercase();
        if !message.to_uppercase().contains(&our_base) {
            return;
        }
        let from_node = crate::qrz::strip_ssid(from.trim()).to_uppercase();
        let synthetic_line = format!("Mail waiting at {from_node} (received from {from} \u{2192} MAIL)");
        self.state.record_notified_packet(port_id, &synthetic_line);
        self.mark_notification_received();
        let silenced = self.state.config.borrow().notify.notifications_silenced;
        if !silenced {
            let port_name = find_entry(&self.state.config.borrow(), port_id).map(|e| e.name).unwrap_or_else(|| port_id.to_string());
            let title = format!("Packet Radio \u{2014} {port_name}");
            crate::notify::send(&self.window, &title, &format!("Mail waiting at {from_node}\n{message}"));
            crate::notify::play_sound(&self.state.config.borrow());
        }
    }

    /// Light up the Notifications header button — called whenever a new
    /// directed notification or destination monitor match arrives.
    pub fn mark_notification_received(&self) {
        self.notifications_button.add_css_class("beacon-lit");
    }

    /// Clear the lit state — called when the Notifications dialog is opened
    /// ("mark as seen").
    pub fn clear_notification_received(&self) {
        self.notifications_button.remove_css_class("beacon-lit");
    }

    /// Recolor the header's Mailbox button from current config/message
    /// state (see `mailbox::status_class` for the green/orange priority
    /// rule). Call after anything that could change either: the mailbox's
    /// own enable toggle, a message being saved or deleted, or at startup.
    pub fn refresh_mailbox_button(&self) {
        self.mailbox_button.remove_css_class("state-success");
        self.mailbox_button.remove_css_class("state-warning");
        let (enabled, has_unread) = {
            let cfg = self.state.config.borrow();
            (cfg.mailbox.enabled, cfg.mailbox.messages.iter().any(|m| !m.read))
        };
        if let Some(class) = crate::mailbox::status_class(enabled, has_unread) {
            self.mailbox_button.add_css_class(class);
        }
        let tooltip = if has_unread {
            "Mailbox: new message(s) \u{2014} click to open, right-click for settings"
        } else if enabled {
            "Mailbox: on \u{2014} click to open, right-click for settings"
        } else {
            "Mailbox: off \u{2014} click to open, right-click for settings"
        };
        self.mailbox_button.set_tooltip_text(Some(tooltip));
    }

    /// Refresh the status bar's connect-state (left) and packet/byte stats
    /// (right) for whichever tab is currently selected. Call this whenever
    /// the selection changes, connects/disconnects, or sends/receives —
    /// cheap enough to call liberally rather than track precisely. Also
    /// ticked once a second (see `build_ui`) so the elapsed-time display
    /// keeps counting up while a tab stays selected.
    pub fn refresh_status_bar(&self) {
        let tabs = self.tabs.borrow();
        match self.selected_tab.get().and_then(|id| tabs.get(&id)) {
            Some(tab) => {
                self.status_conn_icon.set_visible(true);
                let live = tab.conn_id.get().is_some();
                self.status_conn_icon.set_icon_name(Some(if live {
                    "network-transmit-receive-symbolic"
                } else {
                    "network-offline-symbolic"
                }));
                let text = match (live, tab.elapsed_text()) {
                    (true, Some(elapsed)) => format!("Connected to {} \u{2014} {elapsed}", tab.node),
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
    // Every colored-background state class pairs its background with the
    // matching libadwaita `*_fg_color` so the icon/text on top stays
    // readable regardless of theme -- `@success_color`/`@warning_color`/
    // `@accent_color` alone (the previous background-only rules) are meant
    // for tinting an icon/text sitting on the *default* button background,
    // not for use as a background themselves, which is what made these
    // states low-contrast before. `.state-success`/`.state-destructive` are
    // new shared classes for widgets added after this fix (the
    // keyboard-to-keyboard header button, the connect/disconnect buttons)
    // rather than renaming the existing per-widget class names below and
    // risking a missed call site.
    provider.load_from_string(
        ".pin-toggle.pin-pinned { color: @accent_color; } \
         .notify-rule-toggle.notify-rule-active { background-color: @accent_color; } \
         .direwolf-running { background-color: @success_bg_color; color: @success_fg_color; } \
         .direwolf-failed { background-color: @warning_bg_color; color: @warning_fg_color; } \
         .favorite-port-button.favorite-port-connected { background-color: @success_bg_color; color: @success_fg_color; } \
         .favorite-port-button.favorite-port-failed { background-color: @warning_bg_color; color: @warning_fg_color; } \
         .beacon-lit { background-color: @accent_bg_color; color: @accent_fg_color; } \
         .log-toggle-active { background-color: @accent_bg_color; color: @accent_fg_color; } \
         .state-success { background-color: @success_bg_color; color: @success_fg_color; } \
         .state-destructive { background-color: @error_bg_color; color: @error_fg_color; } \
         .state-warning { background-color: @warning_bg_color; color: @warning_fg_color; } \
         .tab-chip { border-right: 1px solid @borders; padding: 0 2px; } \
         .tab-chip-button { padding: 2px 6px; min-height: 0; } \
         .heard-indirect-dot { color: #E8A33D; }",
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

fn refresh_keyboard_mode_button(button: &gtk::Button, enabled: bool) {
    button.remove_css_class("state-success");
    let tooltip = if enabled {
        button.add_css_class("state-success");
        "Keyboard-to-Keyboard: on \u{2014} click to turn off, right-click for settings"
    } else {
        "Keyboard-to-Keyboard: off \u{2014} click to turn on, right-click for settings"
    };
    button.set_tooltip_text(Some(tooltip));
}

fn refresh_notifications_button(button: &gtk::Button, silenced: bool) {
    button.remove_css_class("state-warning");
    let tooltip = if silenced {
        button.add_css_class("state-warning");
        "Notifications: silenced \u{2014} right-click to unsilence"
    } else {
        "Notifications\u{2026} \u{2014} right-click to silence"
    };
    button.set_tooltip_text(Some(tooltip));
}

fn refresh_beacons_button(button: &gtk::Button, enabled: bool) {
    button.remove_css_class("state-success");
    let tooltip = if enabled {
        button.add_css_class("state-success");
        "Outgoing Beacons: on \u{2014} click to disable, right-click to configure"
    } else {
        "Outgoing Beacons: off \u{2014} click to enable, right-click to configure"
    };
    button.set_tooltip_text(Some(tooltip));
}

/// The line ending to append when sending a line of text over a connected
/// session.
///
/// - Telnet: `\r\n` — RFC 854 NVT requires CRLF; strict servers (BPQ etc.)
///   never recognize a bare LF as a completed line.
/// - SSH: `\n` — the remote PTY translates newlines; sending CR would double
///   it.
/// - AX.25/AGWPE/KISS: `\r` — BBS and node software (BPQ, TNOS, JNOS …)
///   universally uses CR as the command terminator. Sending only LF leaves
///   the command buffered at the application layer indefinitely; the AX.25
///   layer still acks the I-frame with RR, giving the false impression that
///   the command was received but ignored.
fn line_ending(config: &PortConfig) -> &'static [u8] {
    match config {
        PortConfig::Telnet { .. } => b"\r\n",
        PortConfig::Ssh { .. } => b"\n",
        _ => b"\r",
    }
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
    let show_timestamps = config.ui.show_timestamps;
    let font = config.ui.font.clone().unwrap_or_else(|| "Monospace 11".to_string());
    let autoconnect_ids: Vec<String> =
        config.ports.iter().filter(|p| p.autoconnect).map(|p| p.id.clone()).collect();
    let pinned_tabs: Vec<TabPrefill> =
        config.pinned_sessions.iter().map(|p| TabPrefill { port_id: p.port_id.clone(), remote: p.remote.clone(), via: p.via.clone() }).collect();
    apply_font(&font);
    apply_base_css();
    let state = AppState::new(config);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("PGPRC")
        .default_width(1000)
        .default_height(700)
        .build();

    let monitor = Rc::new(MonitorView::new(state.clone()));
    monitor.set_show_timestamps(show_timestamps);
    monitor.container.set_vexpand(true);

    // Every run gets its own always-on session transcript -- exactly what
    // Monitor shows, mirrored to disk regardless of the live filter. This
    // replaces the old manual "Save Monitor Log..." button entirely.
    if let Some(dir) = AppConfig::config_dir() {
        let logs_dir = dir.join("logs");
        if std::fs::create_dir_all(&logs_dir).is_ok() {
            let stamp = glib::DateTime::now_local()
                .and_then(|t| t.format("%Y-%m-%d_%H-%M-%S"))
                .map(|s| s.to_string())
                .unwrap_or_else(|_| "session".to_string());
            let path = logs_dir.join(format!("session_{stamp}.txt"));
            match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                Ok(file) => monitor.set_session_log(file),
                Err(e) => tracing::warn!("failed to open session log {path:?}: {e}"),
            }
        }
    }

    let log = LogView::new(state.clone());
    log.container.set_vexpand(true);

    // Swaps between Monitor (packet traffic) and Log (connect/disconnect/
    // error noise) -- a header toggle button drives this (see
    // `log_toggle_button` below). Distinct from the Monitor/tab-content
    // `paned` split just below: this is "which stream", that's "how much
    // room the stream gets."
    let display_stack = gtk::Stack::new();
    display_stack.add_named(&monitor.container, Some("monitor"));
    display_stack.add_named(&log.container, Some("log"));
    display_stack.set_visible_child_name("monitor");
    display_stack.set_vexpand(true);

    // Content-switching stack for tab scrollbacks -- attached/detached from
    // `paned`'s end child based on whether the tab area is expanded (see
    // `Ui::select_tab`/`minimize_tab_area`). A `gtk::Paned` with no end
    // child gives its start child (Monitor/Log) the full allocation, which
    // is exactly "Monitor at 100% with zero tabs or while minimized."
    let tab_stack = gtk::Stack::new();

    let paned = gtk::Paned::builder()
        .orientation(gtk::Orientation::Vertical)
        .start_child(&display_stack)
        .resize_start_child(true)
        .resize_end_child(true)
        .shrink_start_child(false)
        .shrink_end_child(false)
        .vexpand(true)
        .build();

    // Always visible whenever any tab exists (regardless of expanded state)
    // -- deliberately a plain Box, not `gtk::Notebook`'s built-in tab strip,
    // since it needs to stay visible even while the content pane itself is
    // collapsed. Sits just above the bottom bar, per explicit request.
    let tab_strip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    tab_strip.set_margin_start(8);
    tab_strip.set_margin_end(8);
    tab_strip.set_margin_top(2);
    tab_strip.set_margin_bottom(2);
    tab_strip.set_visible(false);
    let tab_strip_add_button = gtk::Button::from_icon_name("list-add-symbolic");
    tab_strip_add_button.add_css_class("flat");
    tab_strip_add_button.set_tooltip_text(Some("Dial\u{2026}"));
    tab_strip.append(&tab_strip_add_button);

    // --- Shared bottom bar: Node / Via / Port (in that order), then the
    // dial/minimize button, the phone-handset connect/disconnect button,
    // the message entry, and Send. ---
    let bottom_node_entry = gtk::Entry::builder().placeholder_text("Node").width_chars(10).build();
    crate::ports_dialog::force_uppercase(&bottom_node_entry);
    let bottom_via_entry = gtk::Entry::builder().placeholder_text("Via (optional)").width_chars(12).build();
    crate::ports_dialog::force_uppercase(&bottom_via_entry);
    let bottom_port_dropdown = gtk::DropDown::builder().build();
    let dial_button = gtk::Button::from_icon_name("list-add-symbolic");
    dial_button.add_css_class("flat");
    dial_button.set_tooltip_text(Some("Dial\u{2026}"));
    let phone_button = gtk::Button::from_icon_name("call-start-symbolic");
    phone_button.add_css_class("flat");
    phone_button.set_visible(false);
    let message_entry = gtk::Entry::builder().hexpand(true).placeholder_text("Unproto message\u{2026}").build();
    let send_button = gtk::Button::with_label("Send");
    send_button.add_css_class("suggested-action");

    let bottom_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    bottom_bar.set_margin_start(8);
    bottom_bar.set_margin_end(8);
    bottom_bar.set_margin_top(4);
    bottom_bar.set_margin_bottom(6);
    bottom_bar.append(&dial_button);
    bottom_bar.append(&bottom_node_entry);
    bottom_bar.append(&bottom_via_entry);
    bottom_bar.append(&bottom_port_dropdown);
    bottom_bar.append(&phone_button);
    bottom_bar.append(&message_entry);
    bottom_bar.append(&send_button);

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

    // Quick-connect row for favorite-flagged ports -- lives in the header's
    // title row now (left side, left of the right-side buttons), not its
    // own row -- see `Ui::rebuild_favorites_bar`.
    let favorites_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    favorites_bar.set_halign(gtk::Align::Start);

    let content_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content_box.append(&paned);
    content_box.append(&tab_strip);
    content_box.append(&bottom_bar);
    content_box.append(&status_bar);

    let toolbar_view = adw::ToolbarView::new();

    // A plain custom title bar instead of `adw::HeaderBar`, built on
    // `gtk::Overlay`: the base layer is header_start/header_end spanning the
    // full width edge-to-edge, and the title floats on top as an overlay
    // child centered (via halign/valign) in the *overlay's own* full
    // allocation -- independent of how lopsided header_start/header_end are.
    // `gtk::CenterBox` was tried first but did not equalize start/end space
    // as its docs suggest in this GTK version; Overlay is a guaranteed-exact
    // center regardless. Wrapped in `WindowHandle` to keep click-drag-to-move
    // and double-click-to-maximize, and `WindowControls` restores the
    // minimize/maximize/close buttons `HeaderBar` provided automatically.
    let header_start = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header_start.set_halign(gtk::Align::Start);
    header_start.set_valign(gtk::Align::Center);
    let header_end = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header_end.set_halign(gtk::Align::End);
    header_end.set_valign(gtk::Align::Center);
    let header_title = gtk::Label::new(Some("PGPRC"));
    header_title.add_css_class("title");
    header_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    header_title.set_halign(gtk::Align::Center);
    header_title.set_valign(gtk::Align::Center);

    let header_sides = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header_sides.set_hexpand(true);
    header_sides.append(&header_start);
    let header_sides_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    header_sides_spacer.set_hexpand(true);
    header_sides.append(&header_sides_spacer);
    header_sides.append(&header_end);

    let header_row = gtk::Overlay::new();
    header_row.set_margin_start(6);
    header_row.set_margin_end(6);
    header_row.set_margin_top(6);
    header_row.set_margin_bottom(6);
    header_row.add_css_class("titlebar");
    header_row.set_child(Some(&header_sides));
    header_row.add_overlay(&header_title);
    let header_handle = gtk::WindowHandle::builder().child(&header_row).build();

    // Created here (rather than alongside its click handler below) so it can
    // be stored on `Ui` itself -- `maybe_detect_beacon`/`maybe_notify_directed`
    // need to light it up from anywhere event handling happens.
    let notifications_button = gtk::Button::from_icon_name("notifications-symbolic");

    // Same reasoning as `beacon_button`: created here so it can be stored on
    // `Ui` and recolored from `refresh_mailbox_button` wherever config or
    // the message list changes (its own click, the Mailbox window's Enable
    // button, a message arriving/being deleted), not just from this setup
    // code.
    let mailbox_button = gtk::Button::from_icon_name("mail-unread-symbolic");

    let ui = Rc::new(Ui {
        state,
        monitor,
        log,
        display_stack,
        tabs: RefCell::new(HashMap::new()),
        tab_stack,
        paned,
        tab_strip,
        tab_chips: RefCell::new(HashMap::new()),
        tab_strip_add_button: tab_strip_add_button.clone(),
        tab_area_expanded: Cell::new(false),
        selected_tab: Cell::new(None),
        bound: RefCell::new(HashMap::new()),
        pending: RefCell::new(HashMap::new()),
        next_tab_id: Cell::new(0),
        beacon_timers: RefCell::new(HashMap::new()),
        status_conn_icon,
        status_conn_label,
        status_stats_label,
        favorites_bar,
        favorite_buttons: RefCell::new(HashMap::new()),
        notifications_button: notifications_button.clone(),
        mailbox_button: mailbox_button.clone(),
        confirmed_ports: RefCell::new(HashSet::new()),
        established_conns: RefCell::new(HashSet::new()),
        bottom_node_entry,
        bottom_via_entry,
        bottom_port_dropdown,
        bottom_ports_snapshot: RefCell::new(Vec::new()),
        dial_button: dial_button.clone(),
        phone_button: phone_button.clone(),
        message_entry: message_entry.clone(),
        send_button: send_button.clone(),
        send_history: RefCell::new(Vec::new()),
        history_pos: Cell::new(None),
        history_draft: RefCell::new(String::new()),
        keyboard_mode_beacon_timer: RefCell::new(None),
        mailbox_beacon_timer: RefCell::new(None),
        direwolf: DirewolfProcess::new(),
        window: window.clone(),
        detached_windows: RefCell::new(HashMap::new()),
    });
    ui.rebuild_favorites_bar();
    ui.rebuild_bottom_ports();
    ui.monitor.rebuild_port_filter();

    {
        let ui = ui.clone();
        tab_strip_add_button.connect_clicked(move |_| {
            dial_dialog::show(&ui);
        });
    }
    {
        let ui = ui.clone();
        dial_button.connect_clicked(move |_| {
            ui.on_dial_button_clicked();
        });
    }
    {
        let ui = ui.clone();
        phone_button.connect_clicked(move |_| {
            ui.on_phone_button_clicked();
        });
    }
    {
        let ui = ui.clone();
        message_entry.connect_activate(move |_| {
            ui.activate_message_entry();
        });
    }
    {
        let ui = ui.clone();
        send_button.connect_clicked(move |_| {
            ui.activate_message_entry();
        });
    }
    {
        let key_controller = gtk::EventControllerKey::new();
        let ui = ui.clone();
        key_controller.connect_key_pressed(move |_, key, _, _| ui.on_message_entry_key_pressed(key));
        message_entry.add_controller(key_controller);
    }
    {
        // Only meaningful in the ad-hoc unproto compose mode (no tab
        // selected) -- `refresh_bottom_bar` re-asserts sensitivity whenever
        // that mode changes, so a stale disabled state doesn't leak into a
        // freshly-selected tab.
        let ui = ui.clone();
        let bottom_node_entry = ui.bottom_node_entry.clone();
        bottom_node_entry.connect_changed(move |_| {
            ui.refresh_send_button_sensitivity();
        });
    }

    {
        let ui = ui.clone();
        let filter_entry = ui.monitor.filter_entry.clone();
        filter_entry.connect_changed(move |entry| {
            ui.monitor.set_filter(&entry.text());
        });
    }
    {
        let ui = ui.clone();
        let unproto_only_switch = ui.monitor.unproto_only_switch.clone();
        unproto_only_switch.connect_active_notify(move |sw| {
            ui.monitor.set_unproto_only(sw.is_active());
        });
    }

    ui.refresh_status_bar();
    ui.refresh_bottom_bar();
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
    // against whatever descendant widget currently has focus, whereas a
    // manual bubble-phase key handler can lose the race against a focused
    // widget's own default key handling. Escape-closing dialogs is handled
    // separately, per dialog window, in `ports_dialog::dialog_window`.
    let shortcuts = gtk::ShortcutController::new();
    shortcuts.set_scope(gtk::ShortcutScope::Global);
    add_shortcut(&shortcuts, "<Control>n", &ui, |ui| {
        dial_dialog::show(ui);
    });
    add_shortcut(&shortcuts, "<Control>w", &ui, |ui| {
        if let Some(tab_id) = ui.selected_tab.get() {
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

    // Mailbox/Notifications/Beacons/Address Book each have their own header
    // button now, so they're deliberately left out of this menu rather than
    // duplicated in both.
    type MenuAction = fn(&Rc<Ui>);
    let menu_items: [(&str, MenuAction); 3] = [
        ("Ports\u{2026}", |ui| ports_dialog::show(ui)),
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
    // frequent-use action as Direwolf/Incoming Beacons/Notified Packets, so
    // it lives in the header instead of behind the hamburger menu.
    // Left-click opens the Mailbox window; right-click jumps straight to
    // its Settings, same interaction shape as Direwolf/keyboard-to-keyboard.
    mailbox_button.add_css_class("flat");
    ui.refresh_mailbox_button();
    {
        let ui = ui.clone();
        mailbox_button.connect_clicked(move |_| {
            mailbox_dialog::show(&ui);
        });
    }
    {
        let ui = ui.clone();
        let gesture = gtk::GestureClick::new();
        gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
        gesture.connect_pressed(move |_, _, _, _| {
            mailbox_dialog::show_settings(&ui, &ui.window);
        });
        mailbox_button.add_controller(gesture);
    }
    header_start.append(&mailbox_button);

    // Incoming keyboard-to-keyboard mode -- placed left of the Direwolf
    // button, same interaction shape: left-click toggles enabled/disabled
    // (green via `state-success` while on), right-click opens its settings
    // (welcome message, availability beacon, listen ports).
    let keyboard_mode_button = gtk::Button::from_icon_name("input-keyboard-symbolic");
    keyboard_mode_button.add_css_class("flat");
    refresh_keyboard_mode_button(&keyboard_mode_button, ui.state.config.borrow().keyboard_mode.enabled);
    {
        let ui = ui.clone();
        let button_for_recolor = keyboard_mode_button.clone();
        keyboard_mode_button.connect_clicked(move |_| {
            let enabled = {
                let mut cfg = ui.state.config.borrow_mut();
                cfg.keyboard_mode.enabled = !cfg.keyboard_mode.enabled;
                cfg.keyboard_mode.enabled
            };
            ui.state.save_config();
            refresh_keyboard_mode_button(&button_for_recolor, enabled);
            ui.reschedule_keyboard_mode_beacon();
        });
    }
    {
        let ui = ui.clone();
        let gesture = gtk::GestureClick::new();
        gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
        gesture.connect_pressed(move |_, _, _, _| {
            keyboard_mode_dialog::show_settings(&ui);
        });
        keyboard_mode_button.add_controller(gesture);
    }
    // Address book: icon-only, lives at the left edge of header_end so it's
    // adjacent to the keyboard-to-keyboard button. Clicking opens the dialog
    // directly (no right-click action needed).
    let address_book_button = gtk::Button::from_icon_name("x-office-address-book-symbolic");
    address_book_button.add_css_class("flat");
    address_book_button.set_tooltip_text(Some("Address Book\u{2026}"));
    {
        let ui = ui.clone();
        address_book_button.connect_clicked(move |_| {
            address_book_dialog::show(&ui);
        });
    }
    header_end.append(&address_book_button);
    header_end.append(&keyboard_mode_button);

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
    header_end.append(&direwolf_button);

    // Notifications button: left-click opens the unified Notifications dialog;
    // right-click silences/unsilences (turns yellow while silenced). Lights up
    // via `beacon-lit` when a new notification arrives.
    notifications_button.add_css_class("flat");
    refresh_notifications_button(&notifications_button, ui.state.config.borrow().notify.notifications_silenced);
    {
        let ui = ui.clone();
        notifications_button.connect_clicked(move |_| {
            incoming_beacons_dialog::show(&ui);
        });
    }
    {
        let ui = ui.clone();
        let btn = notifications_button.clone();
        let gesture = gtk::GestureClick::new();
        gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
        gesture.connect_pressed(move |_, _, _, _| {
            let silenced = {
                let mut cfg = ui.state.config.borrow_mut();
                cfg.notify.notifications_silenced = !cfg.notify.notifications_silenced;
                cfg.notify.notifications_silenced
            };
            ui.state.save_config();
            refresh_notifications_button(&btn, silenced);
        });
        notifications_button.add_controller(gesture);
    }
    header_start.append(&notifications_button);

    // Outgoing beacons toggle: left-click enables/disables all scheduled
    // outgoing beacons at once (green while active); right-click opens the
    // Beacons dialog for configuring individual beacon entries.
    let beacons_button = gtk::Button::from_icon_name("audio-speakers-symbolic");
    beacons_button.add_css_class("flat");
    refresh_beacons_button(&beacons_button, ui.state.config.borrow().beacon_prefs.enabled);
    {
        let ui = ui.clone();
        let btn = beacons_button.clone();
        beacons_button.connect_clicked(move |_| {
            let enabled = {
                let mut cfg = ui.state.config.borrow_mut();
                cfg.beacon_prefs.enabled = !cfg.beacon_prefs.enabled;
                cfg.beacon_prefs.enabled
            };
            ui.state.save_config();
            refresh_beacons_button(&btn, enabled);
        });
    }
    {
        let ui = ui.clone();
        let gesture = gtk::GestureClick::new();
        gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
        gesture.connect_pressed(move |_, _, _, _| {
            beacons_dialog::show(&ui);
        });
        beacons_button.add_controller(gesture);
    }
    header_start.append(&beacons_button);

    // Second toolbar row: filter controls + favorites bar. Placed below the
    // main title row so the title bar stays uncluttered.
    let filter_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    filter_bar.set_margin_start(6);
    filter_bar.set_margin_end(6);
    filter_bar.set_margin_top(2);
    filter_bar.set_margin_bottom(4);
    filter_bar.append(&ui.monitor.filter_entry);
    filter_bar.append(&ui.monitor.port_filter_button);
    let ui_label = gtk::Label::new(Some("UI"));
    ui_label.add_css_class("dim-label");
    filter_bar.append(&ui_label);
    filter_bar.append(&ui.monitor.unproto_only_switch);
    let filter_bar_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    filter_bar_spacer.set_hexpand(true);
    filter_bar.append(&filter_bar_spacer);
    filter_bar.append(&ui.favorites_bar);
    let filter_bar_handle = gtk::WindowHandle::builder().child(&filter_bar).build();

    let log_toggle_button = gtk::ToggleButton::builder().icon_name("utilities-terminal-symbolic").build();
    log_toggle_button.add_css_class("flat");
    log_toggle_button.set_tooltip_text(Some("Show Log"));
    {
        let display_stack = ui.display_stack.clone();
        log_toggle_button.connect_toggled(move |button| {
            let showing_log = button.is_active();
            display_stack.set_visible_child_name(if showing_log { "log" } else { "monitor" });
            if showing_log {
                button.add_css_class("log-toggle-active");
                button.set_tooltip_text(Some("Viewing Log \u{2014} click to show Monitor"));
            } else {
                button.remove_css_class("log-toggle-active");
                button.set_tooltip_text(Some("Show Log"));
            }
        });
    }
    header_end.append(&log_toggle_button);
    header_end.append(&gtk::WindowControls::new(gtk::PackType::End));

    toolbar_view.add_top_bar(&header_handle);
    toolbar_view.add_top_bar(&filter_bar_handle);
    toolbar_view.set_content(Some(&content_box));
    window.set_content(Some(&toolbar_view));

    window.present();

    // Pinned tabs are recreated as disconnected shells; they never
    // auto-connect, even if their port also has autoconnect enabled.
    for prefill in pinned_tabs {
        if let Some(port) = find_entry(&ui.state.config.borrow(), &prefill.port_id) {
            ui.add_connection_tab(port, prefill.remote, prefill.via, false);
        }
    }
    ui.minimize_tab_area();

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
    ui.reschedule_keyboard_mode_beacon();
    ui.reschedule_mailbox_beacon();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_endings_by_backend() {
        assert_eq!(line_ending(&PortConfig::Telnet { host: "bpq.example".to_string(), port: 8010 }), b"\r\n");
        assert_eq!(
            line_ending(&PortConfig::Ssh { host: "h".to_string(), port: 22, user: "u".to_string() }),
            b"\n"
        );
        // AX.25/AGWPE/KISS: BBS/node software expects \r as the command terminator.
        assert_eq!(line_ending(&PortConfig::Ax25RawSocket { device: "ax0".to_string() }), b"\r");
        assert_eq!(
            line_ending(&PortConfig::Agwpe {
                host: "127.0.0.1".to_string(),
                port: 8000,
                radio_port: 0,
                my_call: "N0CALL".to_string(),
                login: None,
            }),
            b"\r"
        );
        assert_eq!(
            line_ending(&PortConfig::KissTcp {
                host: "127.0.0.1".to_string(),
                port: 8001,
                my_call: "N0CALL".to_string(),
                kiss_params: pr_core::KissParams::default(),
                kiss_arq: pr_core::KissArqParams::default(),
            }),
            b"\r"
        );
        assert_eq!(
            line_ending(&PortConfig::KissBle {
                address: "AA:BB:CC:DD:EE:FF".to_string(),
                name: None,
                my_call: "N0CALL".to_string(),
                kiss_params: pr_core::KissParams::default(),
                kiss_arq: pr_core::KissArqParams::default(),
            }),
            b"\r"
        );
    }
}
