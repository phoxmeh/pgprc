use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;

use crate::app_state::AppState;
use crate::monitor_view::timestamp;

/// Diagnostic/status log: port connecting/connected/disconnected/error noise
/// and AX.25 connection-state transitions -- everything that isn't actual
/// packet traffic (that's `MonitorView`'s job). No filter, no port tagging,
/// no session-log persistence: purely a live, ephemeral view, capped at the
/// same `monitor_buffer_lines` limit Monitor uses.
pub struct LogView {
    pub container: gtk::Box,
    buffer: gtk::TextBuffer,
    text_view: gtk::TextView,
    state: Rc<AppState>,
    line_count: RefCell<usize>,
}

impl LogView {
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
        text_view.add_css_class("dim-label");
        let buffer = text_view.buffer();
        let widget = gtk::ScrolledWindow::builder().child(&text_view).vexpand(true).hexpand(true).build();
        let container = gtk::Box::new(gtk::Orientation::Vertical, 4);
        container.append(&widget);

        LogView { container, buffer, text_view, state, line_count: RefCell::new(0) }
    }

    pub fn append_line(&self, line: &str) {
        // GTK's string marshaling panics on embedded NUL bytes; backend
        // data (e.g. AGWPE's null-padded text fields) can contain them.
        let sanitized = if line.contains('\0') { line.replace('\0', "") } else { line.to_string() };
        let prefixed = format!("{} {sanitized}", timestamp());

        let max_lines = self.state.config.borrow().ui.monitor_buffer_lines as usize;
        let mut count = self.line_count.borrow_mut();
        if *count >= max_lines {
            let mut start = self.buffer.start_iter();
            let mut next_line = self.buffer.start_iter();
            next_line.forward_line();
            self.buffer.delete(&mut start, &mut next_line);
        } else {
            *count += 1;
        }
        drop(count);

        let mut end = self.buffer.end_iter();
        self.buffer.insert(&mut end, &format!("{prefixed}\n"));
        let end_mark = self.buffer.create_mark(None, &self.buffer.end_iter(), false);
        self.text_view.scroll_mark_onscreen(&end_mark);
    }
}
