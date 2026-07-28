use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;

use pr_core::{AddressBookEntry, ConnectionId, PortConfig, PortEntry};

use crate::app_state::AppState;
use crate::highlight::{highlight_line, Highlighter, TagCache};
use crate::mailbox::MailboxState;

pub type TabId = u64;

/// True for port kinds that support opening a connected-mode session to a
/// specific remote callsign (AGWPE, AX.25 raw socket). Telnet/SSH have no
/// node concept — connecting the port *is* the whole session.
pub fn port_supports_connect(config: &PortConfig) -> bool {
    matches!(config, PortConfig::Agwpe { .. } | PortConfig::Ax25RawSocket { .. })
}

/// True for port kinds that can send one-shot unconnected (UI) traffic.
/// AX.25 raw sockets only expose connected mode (`SOCK_SEQPACKET`); a
/// separate `SOCK_DGRAM` socket would be needed for unproto and isn't
/// implemented.
pub fn port_supports_unproto(config: &PortConfig) -> bool {
    matches!(config, PortConfig::Agwpe { .. } | PortConfig::KissTcp { .. } | PortConfig::KissSerial { .. })
}

/// True for port kinds where entering a destination callsign makes sense at
/// all — either connected-mode or unproto. Gates whether the tab shows its
/// node/via/unproto row.
pub fn port_needs_node(config: &PortConfig) -> bool {
    port_supports_connect(config) || port_supports_unproto(config)
}

/// A user-managed session tab: pick a port (and, for node-capable ports, a
/// remote callsign), Connect/Disconnect explicitly. The tab persists across
/// disconnects — only its Close button removes it — so it can be reused for
/// a different node or reconnected later.
pub struct SessionTab {
    pub root: gtk::Box,
    pub tab_label: gtk::Label,
    pub pin_toggle: gtk::ToggleButton,
    pub port_dropdown: gtk::DropDown,
    /// Snapshot of configured ports at tab-creation time, in the same order
    /// as `port_dropdown`'s model, so a selection index can be resolved back
    /// to a `PortEntry`.
    pub available_ports: Vec<PortEntry>,
    pub node_row: gtk::Box,
    pub node_entry: gtk::Entry,
    /// Optional digipeater path, e.g. "WIDE1-1,WIDE2-1".
    pub via_entry: gtk::Entry,
    /// A one-shot picker: index 0 is a placeholder, indices 1.. correspond
    /// positionally to `available_address_book`. Selecting a real entry
    /// copies its callsign into `node_entry` and resets back to 0 (wired in
    /// `window.rs`, mirroring `port_dropdown`/`available_ports`). Snapshot
    /// taken at tab-creation time like the port list — reopen a new tab to
    /// see address book entries added since.
    pub address_book_dropdown: gtk::DropDown,
    available_address_book: Vec<String>,
    /// When active, this tab sends unconnected (UI) traffic to `node_entry`
    /// instead of opening a connected-mode session — Connect/Disconnect are
    /// disabled and `input_entry` sends via `PortCommand::SendUnproto`.
    pub unproto_toggle: gtk::CheckButton,
    pub connect_button: gtk::Button,
    pub disconnect_button: gtk::Button,
    pub save_button: gtk::Button,
    pub clear_history_button: gtk::Button,
    pub input_entry: gtk::Entry,
    pub send_input_button: gtk::Button,
    pub conn_id: Cell<Option<ConnectionId>>,
    /// Text received since the last completed line, for splitting arbitrary
    /// byte chunks into history lines.
    pending_line: RefCell<String>,
    /// The (port_id, remote, unproto) currently persisted as pinned for this
    /// tab, if any — lets us unpin the *old* identity when the user edits a
    /// pinned tab's port/node/mode instead of leaking an orphaned pin entry.
    pub pinned_identity: RefCell<Option<(String, String, bool)>>,
    /// Bytes actually sent/received over the wire — the only traffic stats
    /// honestly available here, since the AX.25 ARQ state machine (and any
    /// retry/timer counts) lives in the AGWPE host or the kernel, not in
    /// this app, for every backend we support.
    bytes_sent: Cell<u64>,
    bytes_received: Cell<u64>,
    stats_label: gtk::Label,
    /// `Some` when this tab's replies are being driven by the personal
    /// mailbox's auto-responder instead of the user typing — set on an
    /// unsolicited incoming connection while the mailbox is enabled.
    pub mailbox_state: RefCell<Option<MailboxState>>,
    /// Separate from `pending_line` (which drives history persistence):
    /// buffers incoming bytes into complete lines for the mailbox command
    /// parser specifically.
    mailbox_input: RefCell<String>,
    buffer: gtk::TextBuffer,
    text_view: gtk::TextView,
    state: Rc<AppState>,
    tags: TagCache,
}

impl SessionTab {
    pub fn new(available_ports: Vec<PortEntry>, address_book: Vec<AddressBookEntry>, state: Rc<AppState>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        root.set_margin_top(4);
        root.set_margin_bottom(4);
        root.set_margin_start(4);
        root.set_margin_end(4);

        // --- controls row ---
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);

        let port_names: Vec<&str> = available_ports.iter().map(|p| p.name.as_str()).collect();
        let port_model = gtk::StringList::new(&port_names);
        let port_dropdown = gtk::DropDown::builder().model(&port_model).build();
        controls.append(&port_dropdown);

        let node_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let node_entry = gtk::Entry::builder().placeholder_text("Node (callsign)").width_chars(12).build();
        let via_entry = gtk::Entry::builder().placeholder_text("Via (optional)").width_chars(14).build();

        let mut available_address_book: Vec<String> = address_book.iter().map(|e| e.callsign.clone()).collect();
        available_address_book.sort();
        let mut address_book_names: Vec<String> = vec!["From Address Book\u{2026}".to_string()];
        for callsign in &available_address_book {
            let entry = address_book.iter().find(|e| &e.callsign == callsign);
            let extra = entry.and_then(|e| e.name.as_deref().or(e.alias.as_deref()));
            match extra {
                Some(extra) if !extra.is_empty() => address_book_names.push(format!("{callsign} \u{2014} {extra}")),
                _ => address_book_names.push(callsign.clone()),
            }
        }
        let address_book_refs: Vec<&str> = address_book_names.iter().map(String::as_str).collect();
        let address_book_model = gtk::StringList::new(&address_book_refs);
        let address_book_dropdown = gtk::DropDown::builder().model(&address_book_model).build();
        address_book_dropdown.set_tooltip_text(Some("From Address Book\u{2026}"));
        // Blank factory for the closed button (leaves just the dropdown's own
        // arrow visible) but a real text factory for the popup list, so the
        // picker reads as a small arrow next to the node entry instead of a
        // wide "From Address Book..." button.
        let button_factory = gtk::SignalListItemFactory::new();
        button_factory.connect_setup(|_, _list_item| {});
        address_book_dropdown.set_factory(Some(&button_factory));
        let list_factory = gtk::SignalListItemFactory::new();
        list_factory.connect_setup(|_, list_item| {
            let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else { return };
            let label = gtk::Label::new(None);
            label.set_halign(gtk::Align::Start);
            list_item.set_child(Some(&label));
        });
        list_factory.connect_bind(|_, list_item| {
            let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else { return };
            let label = list_item.child().and_then(|c| c.downcast::<gtk::Label>().ok());
            let text = list_item.item().and_then(|o| o.downcast::<gtk::StringObject>().ok());
            if let (Some(label), Some(text)) = (label, text) {
                label.set_text(&text.string());
            }
        });
        address_book_dropdown.set_list_factory(Some(&list_factory));

        let unproto_toggle = gtk::CheckButton::with_label("Unproto");
        node_row.append(&node_entry);
        node_row.append(&address_book_dropdown);
        node_row.append(&via_entry);
        node_row.append(&unproto_toggle);

        let connect_button = gtk::Button::with_label("Connect");
        connect_button.add_css_class("suggested-action");
        let disconnect_button = gtk::Button::with_label("Disconnect");
        disconnect_button.set_sensitive(false);
        controls.append(&connect_button);
        controls.append(&disconnect_button);

        // Pushes Save/Clear History/stats to the row's right edge, away from
        // the port/Connect/Disconnect controls on the left.
        let controls_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        controls_spacer.set_hexpand(true);
        controls.append(&controls_spacer);

        let save_button = gtk::Button::with_label("Save\u{2026}");
        controls.append(&save_button);

        let clear_history_button = gtk::Button::with_label("Clear History\u{2026}");
        controls.append(&clear_history_button);

        let stats_label = gtk::Label::new(Some("\u{2191}0 B \u{2193}0 B"));
        stats_label.add_css_class("dim-label");
        controls.append(&stats_label);

        root.append(&controls);
        root.append(&node_row);

        // --- scrollback + input ---
        let text_view = gtk::TextView::builder()
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
        let buffer = text_view.buffer();
        let scrolled = gtk::ScrolledWindow::builder().child(&text_view).vexpand(true).hexpand(true).build();
        root.append(&scrolled);

        let input_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let input_entry = gtk::Entry::builder().hexpand(true).placeholder_text("Type and press Enter\u{2026}").build();
        input_entry.set_sensitive(false);
        input_row.append(&input_entry);
        let send_input_button = gtk::Button::with_label("Send");
        send_input_button.add_css_class("suggested-action");
        send_input_button.set_sensitive(false);
        input_row.append(&send_input_button);
        root.append(&input_row);

        // --- notebook tab label: title + Pin + Close ---
        let tab_label = gtk::Label::new(Some("New Tab"));
        // Recolored via CSS (`.pin-toggle:checked`, set up once in
        // `window::apply_base_css`) instead of swapping icons, so the pin
        // itself stays a plain pushpin glyph and only its color signals
        // whether this tab is currently pinned.
        let pin_toggle = gtk::ToggleButton::builder().icon_name("pin-symbolic").tooltip_text("Pin").build();
        pin_toggle.add_css_class("flat");
        pin_toggle.add_css_class("pin-toggle");

        SessionTab {
            root,
            tab_label,
            pin_toggle,
            port_dropdown,
            available_ports,
            node_row,
            node_entry,
            via_entry,
            address_book_dropdown,
            available_address_book,
            unproto_toggle,
            connect_button,
            disconnect_button,
            save_button,
            clear_history_button,
            input_entry,
            send_input_button,
            conn_id: Cell::new(None),
            pending_line: RefCell::new(String::new()),
            pinned_identity: RefCell::new(None),
            bytes_sent: Cell::new(0),
            bytes_received: Cell::new(0),
            stats_label,
            mailbox_state: RefCell::new(None),
            mailbox_input: RefCell::new(String::new()),
            buffer,
            text_view,
            state,
            tags: TagCache::new(),
        }
    }

    /// The port currently selected in the dropdown, if any.
    pub fn selected_port(&self) -> Option<&PortEntry> {
        self.available_ports.get(self.port_dropdown.selected() as usize)
    }

    /// The callsign for the currently selected address-book dropdown entry,
    /// if the selection isn't the "From Address Book..." placeholder (index 0).
    pub fn selected_address_book_call(&self) -> Option<&str> {
        let idx = self.address_book_dropdown.selected();
        if idx == 0 {
            return None;
        }
        self.available_address_book.get(idx as usize - 1).map(String::as_str)
    }

    pub fn set_connected(&self, connected: bool) {
        self.connect_button.set_sensitive(!connected);
        self.disconnect_button.set_sensitive(connected);
        self.input_entry.set_sensitive(connected);
        self.send_input_button.set_sensitive(connected);
        self.port_dropdown.set_sensitive(!connected);
        self.node_entry.set_sensitive(!connected);
        self.via_entry.set_sensitive(!connected);
    }

    /// Append a chunk of received bytes to the mailbox's own line buffer
    /// (separate from history's `pending_line`) and return every newly
    /// completed line, for the mailbox command parser to process one at a
    /// time.
    pub fn take_mailbox_lines(&self, chunk: &str) -> Vec<String> {
        let mut buf = self.mailbox_input.borrow_mut();
        buf.push_str(chunk);
        let mut lines = Vec::new();
        while let Some(pos) = buf.find('\n') {
            let line: String = buf.drain(..=pos).collect();
            lines.push(line.trim_end_matches(['\r', '\n']).to_string());
        }
        lines
    }

    /// The via digipeater path as an ordered, uppercased, non-empty list —
    /// split on commas/whitespace.
    pub fn via(&self) -> Vec<String> {
        self.via_entry
            .text()
            .split([',', ' '])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_uppercase())
            .collect()
    }

    /// Insert raw text (NUL-sanitized) at the end of the buffer, scroll it
    /// into view, and return the char offset it was inserted at plus the
    /// sanitized text actually inserted — callers use the offset to
    /// highlight exactly the span they just added.
    fn insert(&self, text: &str) -> (i32, String) {
        // GTK's string marshaling panics on embedded NUL bytes; a peer could
        // send arbitrary/binary data over a connection.
        let sanitized = if text.contains('\0') { text.replace('\0', "") } else { text.to_string() };
        let mut end = self.buffer.end_iter();
        let start_offset = end.offset();
        self.buffer.insert(&mut end, &sanitized);
        let end_mark = self.buffer.create_mark(None, &self.buffer.end_iter(), false);
        self.text_view.scroll_mark_onscreen(&end_mark);
        (start_offset, sanitized)
    }

    /// Highlight every *complete* line within the just-inserted span
    /// starting at `start_offset` (a trailing partial line with no `\n` yet
    /// is left for a later call, once it's completed by more text — matches
    /// essentially never span a chunk boundary at packet-radio line rates,
    /// so this is not worth the complexity of re-scanning on every chunk).
    fn highlight_new_lines(&self, start_offset: i32, text: &str) {
        let highlighter = Highlighter::build(&self.state.config.borrow());
        let mut line_start = start_offset;
        let mut line_start_byte = 0;
        for (byte_idx, ch) in text.char_indices() {
            if ch == '\n' {
                let line_text = &text[line_start_byte..byte_idx];
                highlight_line(&highlighter, &self.buffer, &self.tags, line_start, line_text);
                line_start += (line_text.chars().count() + 1) as i32; // +1 for the '\n' itself
                line_start_byte = byte_idx + 1;
            }
        }
    }

    /// `unproto` is part of the key so unproto traffic and a connected-mode
    /// session to the same (port, remote) get separate history buckets.
    pub fn history_key(&self) -> Option<(String, String, bool)> {
        let port = self.selected_port().filter(|p| port_needs_node(&p.config))?;
        Some((port.id.clone(), self.node_entry.text().to_string(), self.unproto_toggle.is_active()))
    }

    /// Append a chunk of received bytes (already UTF8-decoded), highlighting
    /// completed lines and splitting them off to persist to node history —
    /// the backend gives us whatever it read off the wire, not necessarily
    /// aligned to line boundaries.
    pub fn receive_data(&self, text: &str) {
        self.bytes_received.set(self.bytes_received.get() + text.len() as u64);
        self.update_stats_label();

        let (start_offset, sanitized) = self.insert(text);
        self.highlight_new_lines(start_offset, &sanitized);

        if let Some((port_id, remote, unproto)) = self.history_key() {
            let mut pending = self.pending_line.borrow_mut();
            pending.push_str(&sanitized);
            while let Some(pos) = pending.find('\n') {
                let line: String = pending.drain(..=pos).collect();
                self.state.append_history_line(&port_id, &remote, unproto, line.trim_end_matches(['\r', '\n']));
            }
        }
    }

    /// Flush any trailing partial line to history (it'll never see a
    /// trailing `\n` otherwise) — call on disconnect.
    pub fn flush_pending(&self) {
        let Some((port_id, remote, unproto)) = self.history_key() else { return };
        let mut pending = self.pending_line.borrow_mut();
        if !pending.is_empty() {
            self.state.append_history_line(&port_id, &remote, unproto, pending.trim_end_matches(['\r', '\n']));
            pending.clear();
        }
    }

    /// Echo a locally-sent line (the operator's own typed text) into the
    /// scrollback and history. Connected-mode AX.25/AGWPE backends don't
    /// echo our own transmissions back to us, so without this the buffer
    /// would only ever show what the *other* station sent.
    pub fn append_sent_line(&self, text: &str) {
        self.bytes_sent.set(self.bytes_sent.get() + text.len() as u64);
        self.update_stats_label();

        let formatted = format!("\u{00BB} {text}\n");
        let (start_offset, sanitized) = self.insert(&formatted);
        self.highlight_new_lines(start_offset, &sanitized);

        if let Some((port_id, remote, unproto)) = self.history_key() {
            self.state.append_history_line(&port_id, &remote, unproto, &format!("\u{00BB} {text}"));
        }
    }

    /// Replace the whole scrollback with the given historical lines (used
    /// when previewing a previous node's history before connecting).
    pub fn load_history(&self, lines: &[String]) {
        self.buffer.set_text("");
        if lines.is_empty() {
            return;
        }
        let text = format!("{}\n", lines.join("\n"));
        let (start_offset, sanitized) = self.insert(&text);
        self.highlight_new_lines(start_offset, &sanitized);
    }

    pub fn clear_text(&self) {
        self.buffer.set_text("");
    }

    /// The full scrollback text, e.g. for exporting to a file.
    pub fn full_text(&self) -> String {
        self.buffer.text(&self.buffer.start_iter(), &self.buffer.end_iter(), true).to_string()
    }

    fn update_stats_label(&self) {
        self.stats_label.set_text(&format!(
            "\u{2191}{} \u{2193}{}",
            format_bytes(self.bytes_sent.get()),
            format_bytes(self.bytes_received.get())
        ));
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
