use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;

use pr_core::{ConnectionId, PortConfig, PortEntry};

use crate::app_state::AppState;
use crate::highlight::{highlight_line, Highlighter, TagCache};
use crate::mailbox::MailboxState;

pub type TabId = u64;

/// True for port kinds that support opening a connected-mode session to a
/// specific remote callsign (AGWPE, AX.25 raw socket). Telnet/SSH have no
/// node concept — connecting the port *is* the whole session, and KISS
/// ports have no connected mode at all.
pub fn port_supports_connect(config: &PortConfig) -> bool {
    matches!(config, PortConfig::Agwpe { .. } | PortConfig::Ax25RawSocket { .. })
}

/// True for port kinds the dial dialog can offer at all — every connect-
/// capable port plus Telnet/SSH (whose "connection" is the whole port).
/// KISS ports are unproto-only and never appear here.
pub fn port_dialable(config: &PortConfig) -> bool {
    port_supports_connect(config) || matches!(config, PortConfig::Telnet { .. } | PortConfig::Ssh { .. })
}

/// True for port kinds that can send one-shot unconnected (UI) traffic —
/// used by the shared bottom bar's default/no-tab-selected compose mode.
pub fn port_supports_unproto(config: &PortConfig) -> bool {
    matches!(
        config,
        PortConfig::Agwpe { .. } | PortConfig::KissTcp { .. } | PortConfig::KissSerial { .. } | PortConfig::Ax25RawSocket { .. }
    )
}

/// A connected-session tab: created by the dial dialog with a fixed
/// (port, node, via/address) identity for its whole lifetime — redialing a
/// different destination means opening a new tab, not editing this one.
/// Persists across disconnects (only its Close button removes it) so its
/// history can be reviewed and it can be reconnected.
pub struct SessionTab {
    pub root: gtk::Box,
    pub pin_toggle: gtk::ToggleButton,
    /// Fixed at creation time by the dial dialog.
    pub port: PortEntry,
    pub node: String,
    /// Raw text as entered in the dial dialog: a digipeater path for
    /// Agwpe/Ax25RawSocket (parsed by `via()`), or a greeting line sent
    /// verbatim right after connecting for Telnet/SSH.
    pub via_raw: String,
    pub save_button: gtk::Button,
    pub clear_history_button: gtk::Button,
    pub conn_id: Cell<Option<ConnectionId>>,
    /// When the current connected-mode session started, for the window
    /// status bar's elapsed-time display -- `None` while disconnected.
    connected_since: Cell<Option<std::time::Instant>>,
    /// Text received since the last completed line, for splitting arbitrary
    /// byte chunks into history lines.
    pending_line: RefCell<String>,
    /// The (port_id, remote) currently persisted as pinned for this tab, if
    /// any — lets us unpin the *old* identity when the user edits a pinned
    /// tab's identity instead of leaking an orphaned pin entry.
    pub pinned_identity: RefCell<Option<(String, String)>>,
    /// Bytes/packets actually sent/received over the wire — the only
    /// traffic stats honestly available here, since the AX.25 ARQ state
    /// machine (and any retry/timer counts) lives in the AGWPE host or the
    /// kernel, not in this app, for every backend we support. Displayed in
    /// the window's status bar (`Ui::refresh_status_bar`) for whichever tab
    /// is currently selected, not in the tab itself.
    bytes_sent: Cell<u64>,
    bytes_received: Cell<u64>,
    packets_sent: Cell<u64>,
    packets_received: Cell<u64>,
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
    pub fn new(port: PortEntry, node: String, via_raw: String, state: Rc<AppState>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        root.set_margin_top(4);
        root.set_margin_bottom(4);
        root.set_margin_start(4);
        root.set_margin_end(4);

        // Per-tab actions that don't belong on the shared bottom bar (which
        // is about composing/dialing, not per-tab file actions).
        let tab_toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let save_button = gtk::Button::with_label("Save\u{2026}");
        tab_toolbar.append(&save_button);
        let clear_history_button = gtk::Button::with_label("Clear History\u{2026}");
        tab_toolbar.append(&clear_history_button);
        root.append(&tab_toolbar);

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

        // Recolored via CSS (`.pin-toggle.pin-pinned`, set up once in
        // `window::apply_base_css`) instead of swapping icons, so the pin
        // itself stays a plain pushpin glyph and only its color signals
        // whether this tab is currently pinned. Placed left of the title in
        // the tab strip's chip -- see `Ui::add_tab_chip`.
        let pin_toggle = gtk::ToggleButton::builder().icon_name("pin-symbolic").tooltip_text("Pin").build();
        pin_toggle.add_css_class("flat");
        pin_toggle.add_css_class("pin-toggle");

        SessionTab {
            root,
            pin_toggle,
            port,
            node,
            via_raw,
            save_button,
            clear_history_button,
            conn_id: Cell::new(None),
            connected_since: Cell::new(None),
            pending_line: RefCell::new(String::new()),
            pinned_identity: RefCell::new(None),
            bytes_sent: Cell::new(0),
            bytes_received: Cell::new(0),
            packets_sent: Cell::new(0),
            packets_received: Cell::new(0),
            mailbox_state: RefCell::new(None),
            mailbox_input: RefCell::new(String::new()),
            buffer,
            text_view,
            state,
            tags: TagCache::new(),
        }
    }

    /// The via digipeater path as an ordered, uppercased, non-empty list —
    /// split on commas/whitespace. Only meaningful for Agwpe/Ax25RawSocket;
    /// for Telnet/SSH `via_raw` is a greeting line instead, sent verbatim
    /// (see `Ui::connect_tab`).
    pub fn via(&self) -> Vec<String> {
        self.via_raw.split([',', ' ']).map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_uppercase()).collect()
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

    /// Insert raw text (NUL-sanitized) at the end of the buffer, scroll it
    /// into view, and return the char offset it was inserted at plus the
    /// sanitized text actually inserted — callers use the offset to
    /// highlight exactly the span they just added. Trims the buffer's start
    /// if it's grown past `UiPrefs.tab_buffer_max_lines` (a live-session
    /// display cap only — the on-disk history file this feeds is never
    /// trimmed).
    fn insert(&self, text: &str) -> (i32, String) {
        // GTK's string marshaling panics on embedded NUL bytes; a peer could
        // send arbitrary/binary data over a connection.
        let sanitized = if text.contains('\0') { text.replace('\0', "") } else { text.to_string() };
        self.trim_buffer_if_needed(sanitized.matches('\n').count());
        let mut end = self.buffer.end_iter();
        let start_offset = end.offset();
        self.buffer.insert(&mut end, &sanitized);
        let end_mark = self.buffer.create_mark(None, &self.buffer.end_iter(), false);
        self.text_view.scroll_mark_onscreen(&end_mark);

        (start_offset, sanitized)
    }

    /// Drop the oldest lines from the live scrollback once it would exceed
    /// `tab_buffer_max_lines` after adding `incoming_lines` more — keeps a
    /// long-running connected session bounded in memory without ever
    /// touching the complete on-disk history file.
    fn trim_buffer_if_needed(&self, incoming_lines: usize) {
        let max_lines = self.state.config.borrow().ui.tab_buffer_max_lines as usize;
        let current_lines = self.buffer.line_count().max(0) as usize;
        let projected = current_lines + incoming_lines;
        if projected <= max_lines {
            return;
        }
        let excess = projected - max_lines;
        let start = self.buffer.start_iter();
        let mut cut = self.buffer.start_iter();
        cut.forward_lines(excess as i32);
        self.buffer.delete(&mut start.clone(), &mut cut);
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

    /// `(port_id, node)` key this tab's history is stored/loaded under.
    pub fn history_key(&self) -> (String, String) {
        (self.port.id.clone(), self.node.clone())
    }

    /// Append a chunk of received bytes (already UTF8-decoded), highlighting
    /// completed lines and splitting them off to persist to node history —
    /// the backend gives us whatever it read off the wire, not necessarily
    /// aligned to line boundaries.
    pub fn receive_data(&self, text: &str) {
        self.bytes_received.set(self.bytes_received.get() + text.len() as u64);
        self.packets_received.set(self.packets_received.get() + 1);

        let (start_offset, sanitized) = self.insert(text);
        self.highlight_new_lines(start_offset, &sanitized);

        let (port_id, remote) = self.history_key();
        let mut pending = self.pending_line.borrow_mut();
        pending.push_str(&sanitized);
        while let Some(pos) = pending.find('\n') {
            let line: String = pending.drain(..=pos).collect();
            self.state.append_history_line(&port_id, &remote, line.trim_end_matches(['\r', '\n']));
        }
    }

    /// Flush any trailing partial line to history (it'll never see a
    /// trailing `\n` otherwise) — call on disconnect.
    pub fn flush_pending(&self) {
        let (port_id, remote) = self.history_key();
        let mut pending = self.pending_line.borrow_mut();
        if !pending.is_empty() {
            self.state.append_history_line(&port_id, &remote, pending.trim_end_matches(['\r', '\n']));
            pending.clear();
        }
    }

    /// Append a connection-lifecycle status message to the scrollback —
    /// visually distinct from data/sent lines and not persisted to history
    /// (connecting/connected/disconnected are ephemeral session events, not
    /// part of the QSO transcript).
    pub fn append_status_line(&self, text: &str) {
        let formatted = format!("\u{2014} {text} \u{2014}\n");
        self.insert(&formatted);
    }

    /// Echo a locally-sent line (the operator's own typed text) into the
    /// scrollback and history. Connected-mode AX.25/AGWPE backends don't
    /// echo our own transmissions back to us, so without this the buffer
    /// would only ever show what the *other* station sent.
    pub fn append_sent_line(&self, text: &str) {
        self.bytes_sent.set(self.bytes_sent.get() + text.len() as u64);
        self.packets_sent.set(self.packets_sent.get() + 1);

        let formatted = format!("\u{00BB} {text}\n");
        let (start_offset, sanitized) = self.insert(&formatted);
        self.highlight_new_lines(start_offset, &sanitized);

        let (port_id, remote) = self.history_key();
        self.state.append_history_line(&port_id, &remote, &format!("\u{00BB} {text}"));
    }

    /// Replace the whole scrollback with the given historical lines (used
    /// when previewing history right after opening the tab, before/without
    /// connecting).
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

    /// Mark a connected-mode session as having just started, for the status
    /// bar's elapsed-time display.
    pub fn mark_connected(&self) {
        self.connected_since.set(Some(std::time::Instant::now()));
    }

    /// Clear the elapsed-time tracking (call on disconnect).
    pub fn mark_disconnected(&self) {
        self.connected_since.set(None);
    }

    /// Time since `mark_connected` as `H:MM:SS`, or `None` if not currently
    /// tracking a connected-mode session.
    pub fn elapsed_text(&self) -> Option<String> {
        self.connected_since.get().map(|since| {
            let secs = since.elapsed().as_secs();
            format!("{}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
        })
    }

    /// Formatted for the window's status bar: packet count and total bytes,
    /// sent and received.
    pub fn stats_text(&self) -> String {
        format!(
            "\u{2191}{} pkt / {}   \u{2193}{} pkt / {}",
            self.packets_sent.get(),
            format_bytes(self.bytes_sent.get()),
            self.packets_received.get(),
            format_bytes(self.bytes_received.get())
        )
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
