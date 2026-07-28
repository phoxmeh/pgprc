use std::cell::{Cell, RefCell};

use gtk::prelude::*;

use pr_core::{ConnectionId, PortConfig, PortEntry};

pub type TabId = u64;

/// True for port kinds that support opening a connected-mode session to a
/// specific remote callsign (AGWPE, AX.25 raw socket). Telnet/SSH/KISS have
/// no node concept — connecting the port *is* the whole session.
pub fn port_needs_node(config: &PortConfig) -> bool {
    matches!(config, PortConfig::Agwpe { .. } | PortConfig::Ax25RawSocket { .. })
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
    pub address_book_button: gtk::Button,
    pub connect_button: gtk::Button,
    pub disconnect_button: gtk::Button,
    pub input_entry: gtk::Entry,
    pub conn_id: Cell<Option<ConnectionId>>,
    /// Text received since the last completed line, for splitting arbitrary
    /// byte chunks into history lines.
    pub pending_line: RefCell<String>,
    /// The (port_id, remote) currently persisted as pinned for this tab, if
    /// any — lets us unpin the *old* identity when the user edits a pinned
    /// tab's port/node instead of leaking an orphaned pin entry.
    pub pinned_identity: RefCell<Option<(String, String)>>,
    buffer: gtk::TextBuffer,
    text_view: gtk::TextView,
}

impl SessionTab {
    pub fn new(available_ports: Vec<PortEntry>) -> Self {
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
        let address_book_button = gtk::Button::with_label("From Address Book\u{2026}");
        node_row.append(&node_entry);
        node_row.append(&address_book_button);
        controls.append(&node_row);

        let connect_button = gtk::Button::with_label("Connect");
        connect_button.add_css_class("suggested-action");
        let disconnect_button = gtk::Button::with_label("Disconnect");
        disconnect_button.set_sensitive(false);
        controls.append(&connect_button);
        controls.append(&disconnect_button);

        root.append(&controls);

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

        let input_entry = gtk::Entry::builder().hexpand(true).placeholder_text("Type and press Enter\u{2026}").build();
        input_entry.set_sensitive(false);
        root.append(&input_entry);

        // --- notebook tab label: title + Pin + Close ---
        let tab_label = gtk::Label::new(Some("New Tab"));
        let pin_toggle = gtk::ToggleButton::builder().label("Pin").build();
        pin_toggle.add_css_class("flat");

        SessionTab {
            root,
            tab_label,
            pin_toggle,
            port_dropdown,
            available_ports,
            node_row,
            node_entry,
            address_book_button,
            connect_button,
            disconnect_button,
            input_entry,
            conn_id: Cell::new(None),
            pending_line: RefCell::new(String::new()),
            pinned_identity: RefCell::new(None),
            buffer,
            text_view,
        }
    }

    /// The port currently selected in the dropdown, if any.
    pub fn selected_port(&self) -> Option<&PortEntry> {
        self.available_ports.get(self.port_dropdown.selected() as usize)
    }

    pub fn set_connected(&self, connected: bool) {
        self.connect_button.set_sensitive(!connected);
        self.disconnect_button.set_sensitive(connected);
        self.input_entry.set_sensitive(connected);
        self.port_dropdown.set_sensitive(!connected);
        self.node_entry.set_sensitive(!connected);
    }

    pub fn append_text(&self, text: &str) {
        // GTK's string marshaling panics on embedded NUL bytes; a peer could
        // send arbitrary/binary data over a connection.
        let sanitized = if text.contains('\0') { text.replace('\0', "") } else { text.to_string() };
        let mut end = self.buffer.end_iter();
        self.buffer.insert(&mut end, &sanitized);
        let end_mark = self.buffer.create_mark(None, &self.buffer.end_iter(), false);
        self.text_view.scroll_mark_onscreen(&end_mark);
    }

    /// Replace the whole scrollback with the given historical lines (used
    /// when previewing a previous node's history before connecting).
    pub fn load_history(&self, lines: &[String]) {
        self.buffer.set_text(&lines.join("\n"));
        if !lines.is_empty() {
            self.buffer.insert(&mut self.buffer.end_iter(), "\n");
        }
        let end_mark = self.buffer.create_mark(None, &self.buffer.end_iter(), false);
        self.text_view.scroll_mark_onscreen(&end_mark);
    }

    pub fn clear_text(&self) {
        self.buffer.set_text("");
    }
}
