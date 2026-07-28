//! Lists every packet that raised a desktop notification, most recent
//! first — the OS notification itself is transient, but these can be
//! bulletins worth revisiting later. Each row's text is highlighted the
//! same way it was in the Monitor/session scrollback.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;

use crate::app_state::find_entry;
use crate::highlight::{highlight_to_markup, Highlighter};
use crate::ports_dialog::dialog_window;
use crate::window::Ui;

pub fn show(ui: &Rc<Ui>) {
    let (win, root) = dialog_window(&ui.window, "Notified Packets", 560);
    win.set_default_height(480);

    let list_box = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    list_box.add_css_class("boxed-list");
    let scrolled = gtk::ScrolledWindow::builder().child(&list_box).vexpand(true).min_content_height(260).build();
    root.append(&scrolled);

    rebuild_list(ui, &list_box);

    win.present();
}

fn rebuild_list(ui: &Rc<Ui>, list_box: &gtk::ListBox) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    let cfg = ui.state.config.borrow();
    let mut packets = cfg.notified_packets.clone();
    let highlighter = Highlighter::build(&cfg);
    drop(cfg);
    packets.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    if packets.is_empty() {
        let label = gtk::Label::new(Some("No notified packets yet."));
        label.set_margin_top(12);
        label.set_margin_bottom(12);
        list_box.append(&label);
        return;
    }

    for packet in packets {
        list_box.append(&build_row(ui, list_box, &highlighter, packet));
    }
}

fn build_row(ui: &Rc<Ui>, list_box: &gtk::ListBox, highlighter: &Highlighter, packet: pr_core::NotifiedPacket) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_margin_top(6);
    row.set_margin_bottom(6);
    row.set_margin_start(8);
    row.set_margin_end(8);

    let text_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text_box.set_hexpand(true);

    let port_name = find_entry(&ui.state.config.borrow(), &packet.port_id).map(|e| e.name).unwrap_or(packet.port_id.clone());
    let caption = gtk::Label::new(Some(&format!("{} \u{b7} {}", packet.timestamp, port_name)));
    caption.set_halign(gtk::Align::Start);
    caption.add_css_class("dim-label");
    text_box.append(&caption);

    let line_label = gtk::Label::new(None);
    line_label.set_markup(&highlight_to_markup(highlighter, &packet.line));
    line_label.set_halign(gtk::Align::Start);
    line_label.set_wrap(true);
    line_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    line_label.set_xalign(0.0);
    text_box.append(&line_label);

    row.append(&text_box);

    // Two-click delete: first click arms it (red, question-mark icon);
    // second click actually removes the entry.
    let delete_button = gtk::Button::from_icon_name("user-trash-symbolic");
    delete_button.add_css_class("flat");
    delete_button.set_valign(gtk::Align::Center);
    delete_button.set_tooltip_text(Some("Delete"));
    let armed = Rc::new(Cell::new(false));
    {
        let ui = ui.clone();
        let list_box = list_box.clone();
        let id = packet.id;
        let armed = armed.clone();
        delete_button.connect_clicked(move |btn| {
            if armed.get() {
                ui.state.remove_notified_packet(id);
                rebuild_list(&ui, &list_box);
            } else {
                armed.set(true);
                btn.set_icon_name("question-symbolic");
                btn.set_tooltip_text(Some("Click again to confirm"));
                btn.add_css_class("destructive-action");
            }
        });
    }
    row.append(&delete_button);

    row.upcast()
}
