use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use pr_core::AddressBookEntry;

use crate::ports_dialog::{dialog_window, labeled};
use crate::window::Ui;

/// Open the Address Book: stations heard automatically on any port (via
/// `PortEvent::StationHeard`) plus manually-entered ones, with add/edit/
/// remove. See `AppState::record_heard` for how entries get auto-created.
pub fn show(ui: &Rc<Ui>) {
    let (win, root) = dialog_window(&ui.window, "Address Book", 560);
    win.set_default_height(440);

    let list_box = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    list_box.add_css_class("boxed-list");
    let scrolled = gtk::ScrolledWindow::builder()
        .child(&list_box)
        .vexpand(true)
        .min_content_height(260)
        .build();
    root.append(&scrolled);

    rebuild_list(ui, &list_box);

    let add_button = gtk::Button::with_label("Add Entry\u{2026}");
    {
        let ui = ui.clone();
        let win = win.clone();
        let list_box = list_box.clone();
        add_button.connect_clicked(move |_| {
            edit_entry_dialog(&ui, &win, None, &list_box);
        });
    }
    root.append(&add_button);

    win.present();
}

fn rebuild_list(ui: &Rc<Ui>, list_box: &gtk::ListBox) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    let mut entries: Vec<AddressBookEntry> = ui.state.config.borrow().address_book.clone();
    entries.sort_by(|a, b| b.last_heard.cmp(&a.last_heard).then_with(|| a.callsign.cmp(&b.callsign)));
    if entries.is_empty() {
        let label = gtk::Label::new(Some(
            "No known stations yet. Entries appear automatically as stations are heard, or add one manually.",
        ));
        label.set_wrap(true);
        label.set_margin_top(12);
        label.set_margin_bottom(12);
        list_box.append(&label);
        return;
    }
    for entry in entries {
        list_box.append(&build_entry_row(ui, entry, list_box));
    }
}

fn build_entry_row(ui: &Rc<Ui>, entry: AddressBookEntry, list_box: &gtk::ListBox) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_margin_top(6);
    row.set_margin_bottom(6);
    row.set_margin_start(6);
    row.set_margin_end(6);

    let mut summary = entry.callsign.clone();
    if let Some(alias) = &entry.alias {
        if !alias.is_empty() {
            summary.push_str(&format!(" \"{alias}\""));
        }
    }
    if let Some(name) = &entry.name {
        if !name.is_empty() {
            summary.push_str(&format!("  \u{2014}  {name}"));
        }
    }
    let mut detail = match &entry.last_heard {
        Some(when) => format!("heard {} time(s), last: {when}", entry.heard_count),
        None => "never heard \u{2014} manual entry".to_string(),
    };
    if let Some(loc) = &entry.location {
        if !loc.is_empty() {
            detail.push_str(&format!("  \u{b7}  {loc}"));
        }
    }

    let text_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let summary_label = gtk::Label::new(Some(&summary));
    summary_label.set_halign(gtk::Align::Start);
    let detail_label = gtk::Label::new(Some(&detail));
    detail_label.set_halign(gtk::Align::Start);
    detail_label.add_css_class("dim-label");
    text_box.append(&summary_label);
    text_box.append(&detail_label);
    text_box.set_hexpand(true);
    row.append(&text_box);

    let edit_button = gtk::Button::with_label("Edit\u{2026}");
    {
        let ui = ui.clone();
        let entry = entry.clone();
        let list_box = list_box.clone();
        edit_button.connect_clicked(move |btn| {
            let win = btn
                .root()
                .and_then(|r| r.downcast::<adw::Window>().ok())
                .expect("row is inside a Window");
            edit_entry_dialog(&ui, &win, Some(entry.clone()), &list_box);
        });
    }
    row.append(&edit_button);

    let remove_button = gtk::Button::with_label("Remove");
    {
        let ui = ui.clone();
        let callsign = entry.callsign.clone();
        let list_box = list_box.clone();
        remove_button.connect_clicked(move |_| {
            ui.state.config.borrow_mut().address_book.retain(|e| e.callsign != callsign);
            ui.state.save_config();
            rebuild_list(&ui, &list_box);
        });
    }
    row.append(&remove_button);

    row.upcast()
}

/// A label above a small multi-line, scrollable text view, for the Notes
/// field. Returns the container to append and the buffer to read/write.
fn labeled_notes(text: &str, initial: &str) -> (gtk::Box, gtk::TextBuffer) {
    let container = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let label = gtk::Label::new(Some(text));
    label.set_halign(gtk::Align::Start);
    container.append(&label);

    let text_view = gtk::TextView::builder().wrap_mode(gtk::WrapMode::WordChar).build();
    let buffer = text_view.buffer();
    buffer.set_text(initial);
    let scrolled = gtk::ScrolledWindow::builder()
        .child(&text_view)
        .min_content_height(100)
        .has_frame(true)
        .build();
    container.append(&scrolled);

    (container, buffer)
}

fn edit_entry_dialog(ui: &Rc<Ui>, parent: &adw::Window, existing: Option<AddressBookEntry>, list_box: &gtk::ListBox) {
    let (win, root) = dialog_window(parent, if existing.is_some() { "Edit Station" } else { "Add Station" }, 420);

    let callsign_entry = gtk::Entry::builder().placeholder_text("N0CALL-1").build();
    let alias_entry = gtk::Entry::builder().placeholder_text("Node/BBS alias (optional)").build();
    let name_entry = gtk::Entry::builder().placeholder_text("Operator name (optional)").build();
    let location_entry = gtk::Entry::builder().placeholder_text("City, state, grid square\u{2026} (optional)").build();
    if let Some(e) = &existing {
        callsign_entry.set_text(&e.callsign);
        alias_entry.set_text(e.alias.as_deref().unwrap_or(""));
        name_entry.set_text(e.name.as_deref().unwrap_or(""));
        location_entry.set_text(e.location.as_deref().unwrap_or(""));
    }
    root.append(&labeled("Callsign", &callsign_entry));
    root.append(&labeled("Alias", &alias_entry));
    root.append(&labeled("Name", &name_entry));
    root.append(&labeled("Location", &location_entry));

    let (notes_container, notes_buffer) =
        labeled_notes("Notes", existing.as_ref().and_then(|e| e.notes.as_deref()).unwrap_or(""));
    root.append(&notes_container);

    if let Some(e) = &existing {
        let detail = match &e.last_heard {
            Some(when) => format!("Heard {} time(s), last: {when}", e.heard_count),
            None => "Never heard \u{2014} manual entry".to_string(),
        };
        let detail_label = gtk::Label::new(Some(&detail));
        detail_label.set_halign(gtk::Align::Start);
        detail_label.add_css_class("dim-label");
        root.append(&detail_label);
    }

    let lookup_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let lookup_button = gtk::Button::with_label("Lookup QRZ\u{2026}");
    let lookup_status = gtk::Label::new(None);
    lookup_status.set_halign(gtk::Align::Start);
    lookup_status.add_css_class("dim-label");
    lookup_row.append(&lookup_button);
    lookup_row.append(&lookup_status);
    root.append(&lookup_row);
    {
        let ui = ui.clone();
        let callsign_entry = callsign_entry.clone();
        let name_entry = name_entry.clone();
        let location_entry = location_entry.clone();
        let lookup_status = lookup_status.clone();
        lookup_button.connect_clicked(move |_| {
            let callsign = callsign_entry.text().trim().to_uppercase();
            if callsign.is_empty() {
                lookup_status.set_text("Enter a callsign first.");
                return;
            }
            let (username, password) = {
                let cfg = ui.state.config.borrow();
                (cfg.ui.qrz_username.clone(), cfg.ui.qrz_password.clone())
            };
            let (Some(username), Some(password)) = (username, password) else {
                lookup_status.set_text("Configure QRZ credentials in Preferences first.");
                return;
            };
            let session = ui.state.qrz_session.borrow().clone();
            lookup_status.set_text("Looking up\u{2026}");

            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let mut session = session;
                let result = crate::qrz::lookup(&username, &password, &mut session, &callsign);
                let _ = tx.send_blocking((result, session));
            });

            let ui = ui.clone();
            let name_entry = name_entry.clone();
            let location_entry = location_entry.clone();
            let lookup_status = lookup_status.clone();
            glib::spawn_future_local(async move {
                let Ok((result, new_session)) = rx.recv().await else { return };
                *ui.state.qrz_session.borrow_mut() = new_session;
                match result {
                    Ok(info) => {
                        if let Some(name) = info.name {
                            name_entry.set_text(&name);
                        }
                        let loc = match (info.location, info.grid) {
                            (Some(l), Some(g)) => Some(format!("{l} ({g})")),
                            (Some(l), None) => Some(l),
                            (None, Some(g)) => Some(g),
                            (None, None) => None,
                        };
                        if let Some(loc) = loc {
                            location_entry.set_text(&loc);
                        }
                        lookup_status.set_text("Lookup succeeded.");
                    }
                    Err(e) => lookup_status.set_text(&format!("Lookup failed: {e}")),
                }
            });
        });
    }

    let error_label = gtk::Label::new(None);
    error_label.add_css_class("error");
    root.append(&error_label);

    let button_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    button_row.set_halign(gtk::Align::End);
    let cancel_button = gtk::Button::with_label("Cancel");
    {
        let win = win.clone();
        cancel_button.connect_clicked(move |_| win.close());
    }
    let save_button = gtk::Button::with_label("Save");
    save_button.add_css_class("suggested-action");
    {
        let ui = ui.clone();
        let win = win.clone();
        let list_box = list_box.clone();
        let original_callsign = existing.as_ref().map(|e| e.callsign.clone());
        save_button.connect_clicked(move |_| {
            let callsign = callsign_entry.text().trim().to_uppercase();
            if callsign.is_empty() {
                error_label.set_text("Callsign is required.");
                return;
            }
            let alias = alias_entry.text().to_string();
            let name = name_entry.text().to_string();
            let location = location_entry.text().to_string();
            let notes = notes_buffer.text(&notes_buffer.start_iter(), &notes_buffer.end_iter(), true).to_string();

            let mut cfg = ui.state.config.borrow_mut();
            let key = original_callsign.as_deref().unwrap_or(&callsign);
            if let Some(slot) = cfg.address_book.iter_mut().find(|e| e.callsign == key) {
                slot.callsign = callsign;
                slot.alias = if alias.is_empty() { None } else { Some(alias) };
                slot.name = if name.is_empty() { None } else { Some(name) };
                slot.location = if location.is_empty() { None } else { Some(location) };
                slot.notes = if notes.is_empty() { None } else { Some(notes) };
            } else {
                cfg.address_book.push(AddressBookEntry {
                    callsign,
                    alias: if alias.is_empty() { None } else { Some(alias) },
                    name: if name.is_empty() { None } else { Some(name) },
                    location: if location.is_empty() { None } else { Some(location) },
                    notes: if notes.is_empty() { None } else { Some(notes) },
                    last_heard: None,
                    heard_count: 0,
                });
            }
            drop(cfg);
            ui.state.save_config();
            rebuild_list(&ui, &list_box);
            win.close();
        });
    }
    button_row.append(&cancel_button);
    button_row.append(&save_button);
    root.append(&button_row);

    win.present();
}
