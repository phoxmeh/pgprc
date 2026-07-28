use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::app_state::AppState;
use crate::highlight::{highlight_line, Highlighter, TagCache};

/// One previously-appended line, kept around so the filter can re-render the
/// buffer without losing the original timestamp (re-running `timestamp()`
/// later would show "now", not when the line actually arrived).
struct StoredLine {
    /// Exactly what was inserted into the buffer (timestamp prefix + content,
    /// or just content if timestamps are off).
    prefixed: String,
    /// The sanitized content alone, for highlighting offset math.
    content: String,
}

/// The live-traffic Monitor panel: a read-only, auto-scrolling log with an
/// optional substring filter.
pub struct MonitorView {
    /// The whole panel (filter entry + scrollback) — show/hide this, not
    /// `widget`, so the filter entry follows the "Monitor" visibility toggle.
    pub container: gtk::Box,
    pub filter_entry: gtk::Entry,
    buffer: gtk::TextBuffer,
    text_view: gtk::TextView,
    show_timestamps: Cell<bool>,
    state: Rc<AppState>,
    tags: TagCache,
    lines: RefCell<Vec<StoredLine>>,
    /// Lowercased filter text; empty means "show everything".
    filter: RefCell<String>,
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

        let filter_entry = gtk::Entry::builder().placeholder_text("Filter (callsign or keyword)\u{2026}").build();
        let container = gtk::Box::new(gtk::Orientation::Vertical, 4);
        container.append(&filter_entry);
        container.append(&widget);

        MonitorView {
            container,
            filter_entry,
            buffer,
            text_view,
            show_timestamps: Cell::new(true),
            state,
            tags: TagCache::new(),
            lines: RefCell::new(Vec::new()),
            filter: RefCell::new(String::new()),
        }
    }

    pub fn set_show_timestamps(&self, show: bool) {
        self.show_timestamps.set(show);
    }

    /// Get the visible buffer's full text, e.g. for exporting to a file.
    pub fn full_text(&self) -> String {
        self.buffer.text(&self.buffer.start_iter(), &self.buffer.end_iter(), true).to_string()
    }

    /// Restrict the visible buffer to lines containing `filter`
    /// (case-insensitive substring match against the whole line, including
    /// its timestamp). An empty filter shows everything again.
    pub fn set_filter(&self, filter: &str) {
        *self.filter.borrow_mut() = filter.to_lowercase();
        self.rerender();
    }

    pub fn append_line(&self, line: &str) {
        // GTK's string marshaling panics on embedded NUL bytes; backend
        // data (e.g. AGWPE's null-padded text fields) can contain them.
        let sanitized = if line.contains('\0') { line.replace('\0', "") } else { line.to_string() };
        let prefixed = if self.show_timestamps.get() {
            format!("{} {sanitized}", timestamp())
        } else {
            sanitized.clone()
        };

        {
            let mut lines = self.lines.borrow_mut();
            lines.push(StoredLine { prefixed: prefixed.clone(), content: sanitized.clone() });
            let max_lines = self.state.config.borrow().ui.monitor_buffer_lines as usize;
            if lines.len() > max_lines {
                let excess = lines.len() - max_lines;
                lines.drain(0..excess);
            }
        }

        if self.matches_filter(&prefixed) {
            let highlighter = Highlighter::build(&self.state.config.borrow());
            self.render_line(&prefixed, &sanitized, &highlighter);
        }
    }

    fn matches_filter(&self, prefixed: &str) -> bool {
        let filter = self.filter.borrow();
        filter.is_empty() || prefixed.to_lowercase().contains(filter.as_str())
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

    /// Rebuild the visible buffer from `lines`, applying the current filter.
    fn rerender(&self) {
        self.buffer.set_text("");
        let highlighter = Highlighter::build(&self.state.config.borrow());
        let lines = self.lines.borrow();
        for line in lines.iter() {
            if self.matches_filter(&line.prefixed) {
                self.render_line(&line.prefixed, &line.content, &highlighter);
            }
        }
    }
}

fn timestamp() -> String {
    glib::DateTime::now_local()
        .and_then(|t| t.format("%H:%M:%S"))
        .map(|s| s.to_string())
        .unwrap_or_default()
}
