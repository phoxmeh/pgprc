use std::rc::Rc;

use adw::prelude::*;

use pr_core::MailboxMessage;

use crate::ports_dialog::dialog_window;
use crate::window::Ui;

/// Lists every stored mailbox message (read or not) with a Delete button.
/// Enabling/disabling the mailbox itself lives in Preferences.
pub fn show(ui: &Rc<Ui>) {
    let (win, root) = dialog_window(&ui.window, "Mailbox", 560);
    win.set_default_height(420);

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
    let mut entries: Vec<MailboxMessage> = ui.state.config.borrow().mailbox.messages.clone();
    entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    if entries.is_empty() {
        let label = gtk::Label::new(Some("No mailbox messages yet."));
        label.set_margin_top(12);
        label.set_margin_bottom(12);
        list_box.append(&label);
        return;
    }
    for entry in entries {
        list_box.append(&build_message_row(ui, entry, list_box));
    }
}

fn build_message_row(ui: &Rc<Ui>, entry: MailboxMessage, list_box: &gtk::ListBox) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_margin_top(6);
    row.set_margin_bottom(6);
    row.set_margin_start(6);
    row.set_margin_end(6);

    let status = if entry.read { "" } else { " (unread)" };
    let text_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let summary = gtk::Label::new(Some(&format!("To {}  from {}  \u{2014}  {}{status}", entry.to, entry.from, entry.subject)));
    summary.set_halign(gtk::Align::Start);
    let detail = gtk::Label::new(Some(&format!("{}  \u{b7}  {}", entry.timestamp, entry.body.lines().next().unwrap_or(""))));
    detail.set_halign(gtk::Align::Start);
    detail.add_css_class("dim-label");
    text_box.append(&summary);
    text_box.append(&detail);
    text_box.set_hexpand(true);
    row.append(&text_box);

    let remove_button = gtk::Button::with_label("Delete");
    {
        let ui = ui.clone();
        let id = entry.id;
        let list_box = list_box.clone();
        remove_button.connect_clicked(move |_| {
            ui.state.config.borrow_mut().mailbox.messages.retain(|m| m.id != id);
            ui.state.save_config();
            rebuild_list(&ui, &list_box);
        });
    }
    row.append(&remove_button);

    row.upcast()
}
