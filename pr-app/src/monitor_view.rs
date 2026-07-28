use std::cell::Cell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::app_state::AppState;
use crate::highlight::{highlight_line, Highlighter, TagCache};

/// The live-traffic Monitor panel: a read-only, auto-scrolling log.
pub struct MonitorView {
    pub widget: gtk::ScrolledWindow,
    buffer: gtk::TextBuffer,
    text_view: gtk::TextView,
    show_timestamps: Cell<bool>,
    state: Rc<AppState>,
    tags: TagCache,
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
        MonitorView { widget, buffer, text_view, show_timestamps: Cell::new(true), state, tags: TagCache::new() }
    }

    pub fn set_show_timestamps(&self, show: bool) {
        self.show_timestamps.set(show);
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
        let mut end = self.buffer.end_iter();
        let insert_offset = end.offset();
        self.buffer.insert(&mut end, &prefixed);
        self.buffer.insert(&mut self.buffer.end_iter(), "\n");
        let end_mark = self.buffer.create_mark(None, &self.buffer.end_iter(), false);
        self.text_view.scroll_mark_onscreen(&end_mark);

        // Highlight only the actual content, not the timestamp prefix.
        let content_start = insert_offset + (prefixed.chars().count() - sanitized.chars().count()) as i32;
        let highlighter = Highlighter::build(&self.state.config.borrow());
        highlight_line(&highlighter, &self.buffer, &self.tags, content_start, &sanitized);
    }
}

fn timestamp() -> String {
    glib::DateTime::now_local()
        .and_then(|t| t.format("%H:%M:%S"))
        .map(|s| s.to_string())
        .unwrap_or_default()
}
