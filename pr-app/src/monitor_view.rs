use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::File;
use std::io::Write;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use regex::{Regex, RegexBuilder};

use crate::app_state::AppState;
use crate::highlight::{highlight_line, Highlighter, TagCache};

/// One previously-appended line, kept around so the filter can re-render the
/// buffer without losing the original timestamp (re-running `timestamp()`
/// later would show "now", not when the line actually arrived).
struct StoredLine {
    /// Id of the port this line came from, for the port-filter popover.
    port_id: String,
    /// Exactly what was inserted into the buffer (timestamp prefix + content,
    /// or just content if timestamps are off).
    prefixed: String,
    /// The sanitized content alone, for highlighting offset math.
    content: String,
}

/// The current text filter: the raw (lowercased) text, plus a compiled regex
/// when it parses as one. An unparseable regex (e.g. an unbalanced paren
/// typed mid-edit) falls back to a plain substring match rather than just
/// going blank while the user is still typing.
#[derive(Default)]
struct FilterState {
    raw: String,
    regex: Option<Regex>,
}

/// The live-traffic Monitor panel: a read-only, auto-scrolling log of actual
/// packet traffic only (connect/disconnect/error noise lives in `LogView`
/// instead), filterable by port and by a regex-or-substring text filter.
pub struct MonitorView {
    /// The scrollback panel — show/hide this, not `widget`, to follow the
    /// "Monitor" visibility toggle. `filter_entry`/`port_filter_button` live
    /// in the header instead (next to "Send Beacon..."), so they stay
    /// visible either way.
    pub container: gtk::Box,
    pub filter_entry: gtk::Entry,
    pub port_filter_button: gtk::MenuButton,
    port_filter_popover: gtk::Popover,
    port_checks: RefCell<HashMap<String, gtk::CheckButton>>,
    select_all_check: gtk::CheckButton,
    /// Ports the user has unchecked in the popover; empty (the default)
    /// means every port is shown.
    disabled_ports: RefCell<HashSet<String>>,
    buffer: gtk::TextBuffer,
    text_view: gtk::TextView,
    show_timestamps: Cell<bool>,
    state: Rc<AppState>,
    tags: TagCache,
    lines: RefCell<Vec<StoredLine>>,
    filter: RefCell<FilterState>,
    /// Session transcript file, opened once at startup in `build_ui` --
    /// every appended line (regardless of the live filter) is mirrored into
    /// it. Replaces the old manual "Save Monitor Log..." button entirely.
    session_log: RefCell<Option<File>>,
}

impl MonitorView {
    pub fn new(state: Rc<AppState>) -> Self {
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
        let widget = gtk::ScrolledWindow::builder()
            .child(&text_view)
            .vexpand(true)
            .hexpand(true)
            .build();

        // Deliberately small rather than stretched wide — this is a
        // rarely-needed filter, not a primary control, and lives in the
        // header (see `build_ui`) rather than this panel.
        let filter_entry = gtk::Entry::builder().placeholder_text("Filter (regex)\u{2026}").width_chars(16).build();
        filter_entry.set_icon_from_icon_name(gtk::EntryIconPosition::Secondary, Some("edit-clear-symbolic"));
        filter_entry.set_icon_activatable(gtk::EntryIconPosition::Secondary, true);
        filter_entry.set_icon_tooltip_text(gtk::EntryIconPosition::Secondary, Some("Clear filter"));
        filter_entry.connect_icon_release(|entry, pos| {
            if pos == gtk::EntryIconPosition::Secondary {
                entry.set_text("");
            }
        });

        let port_filter_button = gtk::MenuButton::builder().label("Ports").tooltip_text("Filter by port\u{2026}").build();
        let port_filter_popover = gtk::Popover::new();
        port_filter_button.set_popover(Some(&port_filter_popover));
        let select_all_check = gtk::CheckButton::with_label("Select All");
        select_all_check.set_active(true);

        let container = gtk::Box::new(gtk::Orientation::Vertical, 4);
        container.append(&widget);

        MonitorView {
            container,
            filter_entry,
            port_filter_button,
            port_filter_popover,
            port_checks: RefCell::new(HashMap::new()),
            select_all_check,
            disabled_ports: RefCell::new(HashSet::new()),
            buffer,
            text_view,
            show_timestamps: Cell::new(true),
            state,
            tags: TagCache::new(),
            lines: RefCell::new(Vec::new()),
            filter: RefCell::new(FilterState::default()),
            session_log: RefCell::new(None),
        }
    }

    pub fn set_show_timestamps(&self, show: bool) {
        self.show_timestamps.set(show);
    }

    /// Point this Monitor at a session-transcript file -- opened once at
    /// startup in `build_ui`, named after that run's start time.
    pub fn set_session_log(&self, file: File) {
        *self.session_log.borrow_mut() = Some(file);
    }

    /// Restrict the visible buffer to lines whose content matches `filter`,
    /// tried first as a case-insensitive regex and, if that fails to
    /// compile (e.g. an unbalanced paren typed mid-edit), as a plain
    /// case-insensitive substring instead. An empty filter shows everything.
    pub fn set_filter(&self, filter: &str) {
        let regex = if filter.is_empty() { None } else { RegexBuilder::new(filter).case_insensitive(true).build().ok() };
        *self.filter.borrow_mut() = FilterState { raw: filter.to_lowercase(), regex };
        self.rerender();
    }

    /// Rebuild the port-filter popover's checkbox list from the current
    /// config's ports. Call after anything that adds/removes/renames a port
    /// (the Ports dialog) as well as once at startup.
    pub fn rebuild_port_filter(self: &Rc<Self>) {
        self.port_checks.borrow_mut().clear();

        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let ports = self.state.config.borrow().ports.clone();
        for port in &ports {
            let check = gtk::CheckButton::with_label(&port.name);
            check.set_active(!self.disabled_ports.borrow().contains(&port.id));
            {
                let view = self.clone();
                let port_id = port.id.clone();
                check.connect_toggled(move |check| {
                    if check.is_active() {
                        view.disabled_ports.borrow_mut().remove(&port_id);
                    } else {
                        view.disabled_ports.borrow_mut().insert(port_id.clone());
                    }
                    view.refresh_select_all();
                    view.rerender();
                });
            }
            list.append(&check);
            self.port_checks.borrow_mut().insert(port.id.clone(), check);
        }
        list.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        list.append(&self.select_all_check);
        list.set_margin_top(6);
        list.set_margin_bottom(6);
        list.set_margin_start(8);
        list.set_margin_end(8);
        self.port_filter_popover.set_child(Some(&list));
        self.refresh_select_all();

        // `connect_toggled` fires only on the first `rebuild_port_filter`
        // call, matching the once-per-view lifetime of `select_all_check`
        // itself (it's a field, not rebuilt each time).
        if self.select_all_check.tooltip_text().is_none() {
            self.select_all_check.set_tooltip_text(Some("Toggle every port"));
            let view = self.clone();
            self.select_all_check.connect_toggled(move |check| {
                let all_on = check.is_active();
                for (port_id, port_check) in view.port_checks.borrow().iter() {
                    port_check.set_active(all_on);
                    if all_on {
                        view.disabled_ports.borrow_mut().remove(port_id);
                    } else {
                        view.disabled_ports.borrow_mut().insert(port_id.clone());
                    }
                }
                view.rerender();
            });
        }
    }

    /// Reflect whether every port is currently enabled onto `select_all_check`
    /// without re-triggering its own `connect_toggled` (each port checkbox's
    /// handler calls this after updating `disabled_ports`, so this only ever
    /// *reads* that state, never round-trips through the button's signal).
    fn refresh_select_all(&self) {
        let all_enabled = self.disabled_ports.borrow().is_empty();
        self.select_all_check.set_active(all_enabled);
    }

    pub fn append_line(&self, port_id: &str, line: &str) {
        // GTK's string marshaling panics on embedded NUL bytes; backend
        // data (e.g. AGWPE's null-padded text fields) can contain them.
        let sanitized = if line.contains('\0') { line.replace('\0', "") } else { line.to_string() };
        let prefixed = if self.show_timestamps.get() {
            format!("{} {sanitized}", timestamp())
        } else {
            sanitized.clone()
        };

        if let Some(file) = self.session_log.borrow_mut().as_mut() {
            let _ = writeln!(file, "{prefixed}");
        }

        {
            let mut lines = self.lines.borrow_mut();
            lines.push(StoredLine { port_id: port_id.to_string(), prefixed: prefixed.clone(), content: sanitized.clone() });
            let max_lines = self.state.config.borrow().ui.monitor_buffer_lines as usize;
            if lines.len() > max_lines {
                let excess = lines.len() - max_lines;
                lines.drain(0..excess);
            }
        }

        if self.matches_filter(port_id, &prefixed) {
            let highlighter = Highlighter::build(&self.state.config.borrow());
            self.render_line(&prefixed, &sanitized, &highlighter);
        }
    }

    fn matches_filter(&self, port_id: &str, prefixed: &str) -> bool {
        if self.disabled_ports.borrow().contains(port_id) {
            return false;
        }
        let filter = self.filter.borrow();
        if filter.raw.is_empty() {
            return true;
        }
        match &filter.regex {
            Some(re) => re.is_match(prefixed),
            None => prefixed.to_lowercase().contains(filter.raw.as_str()),
        }
    }

    /// Insert one already-filtered line at the end of the buffer, highlight
    /// its content span, and scroll it into view.
    fn render_line(&self, prefixed: &str, content: &str, highlighter: &Highlighter) {
        let mut end = self.buffer.end_iter();
        let insert_offset = end.offset();
        self.buffer.insert(&mut end, prefixed);
        self.buffer.insert(&mut self.buffer.end_iter(), "\n");
        let end_mark = self.buffer.create_mark(None, &self.buffer.end_iter(), false);
        self.text_view.scroll_mark_onscreen(&end_mark);

        // Highlight only the actual content, not the timestamp prefix.
        let content_start = insert_offset + (prefixed.chars().count() - content.chars().count()) as i32;
        highlight_line(highlighter, &self.buffer, &self.tags, content_start, content);
    }

    /// Rebuild the visible buffer from `lines`, applying the current filters.
    fn rerender(&self) {
        self.buffer.set_text("");
        let highlighter = Highlighter::build(&self.state.config.borrow());
        let lines = self.lines.borrow();
        for line in lines.iter() {
            if self.matches_filter(&line.port_id, &line.prefixed) {
                self.render_line(&line.prefixed, &line.content, &highlighter);
            }
        }
    }
}

pub(crate) fn timestamp() -> String {
    glib::DateTime::now_local()
        .and_then(|t| t.format("%H:%M:%S"))
        .map(|s| s.to_string())
        .unwrap_or_default()
}
