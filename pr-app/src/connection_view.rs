use gtk::prelude::*;

/// One Connection tab: terminal-style scrollback plus a single-line input.
pub struct ConnectionTab {
    pub root: gtk::Box,
    pub entry: gtk::Entry,
    buffer: gtk::TextBuffer,
    text_view: gtk::TextView,
}

impl ConnectionTab {
    pub fn new() -> Self {
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
        let scrolled = gtk::ScrolledWindow::builder()
            .child(&text_view)
            .vexpand(true)
            .hexpand(true)
            .build();

        let entry = gtk::Entry::builder()
            .hexpand(true)
            .placeholder_text("Type and press Enter\u{2026}")
            .margin_start(4)
            .margin_end(4)
            .margin_bottom(4)
            .build();

        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        root.append(&scrolled);
        root.append(&entry);

        ConnectionTab { root, entry, buffer, text_view }
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
}

impl Default for ConnectionTab {
    fn default() -> Self {
        Self::new()
    }
}
