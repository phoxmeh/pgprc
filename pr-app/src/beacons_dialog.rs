use std::rc::Rc;

use adw::prelude::*;

use pr_core::{AppConfig, Beacon};

use crate::ports_dialog::{dialog_window, force_uppercase, labeled};
use crate::window::Ui;

fn next_id(config: &AppConfig) -> String {
    let mut n = config.beacons.len();
    loop {
        let candidate = format!("beacon-{n}");
        if !config.beacons.iter().any(|b| b.id == candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Manage scheduled beacons: list, add/edit/remove, each firing
/// automatically on its own interval while its port is connected.
pub fn show(ui: &Rc<Ui>) {
    let (win, root) = dialog_window(&ui.window, "Beacons", 560);
    win.set_default_height(420);

    // Global kill switch for every scheduled beacon at once, independent of
    // each row's own toggle -- acts immediately, same as the per-row
    // switches below (this dialog has no Save button; every control here
    // writes straight through).
    let global_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let global_label = gtk::Label::new(Some("Outgoing Beacons Enabled"));
    global_label.set_hexpand(true);
    global_label.set_halign(gtk::Align::Start);
    let global_switch = gtk::Switch::new();
    global_switch.set_active(ui.state.config.borrow().beacon_prefs.enabled);
    global_switch.set_valign(gtk::Align::Center);
    {
        let ui = ui.clone();
        global_switch.connect_active_notify(move |sw| {
            ui.state.config.borrow_mut().beacon_prefs.enabled = sw.is_active();
            ui.state.save_config();
            ui.reschedule_beacons();
        });
    }
    global_row.append(&global_label);
    global_row.append(&global_switch);
    root.append(&global_row);
    root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let list_box = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    list_box.add_css_class("boxed-list");
    let scrolled = gtk::ScrolledWindow::builder().child(&list_box).vexpand(true).min_content_height(240).build();
    root.append(&scrolled);

    rebuild_list(ui, &list_box);

    let add_button = gtk::Button::with_label("Add Beacon\u{2026}");
    {
        let ui = ui.clone();
        let win = win.clone();
        let list_box = list_box.clone();
        add_button.connect_clicked(move |_| {
            edit_beacon_dialog(&ui, &win, None, &list_box);
        });
    }
    root.append(&add_button);

    win.present();
}

fn rebuild_list(ui: &Rc<Ui>, list_box: &gtk::ListBox) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    // Pinned beacons first, then the rest in their stored order.
    let all: Vec<Beacon> = ui.state.config.borrow().beacons.clone();
    let pinned: Vec<_> = all.iter().filter(|b| b.pinned).cloned().collect();
    let normal: Vec<_> = all.iter().filter(|b| !b.pinned).cloned().collect();
    for entry in pinned.into_iter().chain(normal) {
        list_box.append(&build_beacon_row(ui, entry, list_box));
    }
}

fn build_beacon_row(ui: &Rc<Ui>, entry: Beacon, list_box: &gtk::ListBox) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_margin_top(6);
    row.set_margin_bottom(6);
    row.set_margin_start(6);
    row.set_margin_end(6);

    // Per-beacon on/off, quick to flip without opening Edit -- acts
    // immediately, same as the dialog's global switch above it.
    let enabled_switch = gtk::Switch::new();
    enabled_switch.set_active(entry.enabled);
    enabled_switch.set_valign(gtk::Align::Center);
    {
        let ui = ui.clone();
        let id = entry.id.clone();
        enabled_switch.connect_active_notify(move |sw| {
            if let Some(b) = ui.state.config.borrow_mut().beacons.iter_mut().find(|b| b.id == id) {
                b.enabled = sw.is_active();
            }
            ui.state.save_config();
            ui.reschedule_beacons();
        });
    }
    row.append(&enabled_switch);

    let port_name =
        ui.state.config.borrow().ports.iter().find(|p| p.id == entry.port_id).map(|p| p.name.clone()).unwrap_or_else(|| "(unknown port)".to_string());
    let label = gtk::Label::new(Some(&format!("{port_name} \u{2192} {} every {}s", entry.dest, entry.interval_secs)));
    label.set_hexpand(true);
    label.set_halign(gtk::Align::Start);
    row.append(&label);

    let edit_button = gtk::Button::with_label("Edit\u{2026}");
    {
        let ui = ui.clone();
        let entry = entry.clone();
        let list_box = list_box.clone();
        edit_button.connect_clicked(move |btn| {
            let win = btn.root().and_then(|r| r.downcast::<adw::Window>().ok()).expect("row is inside a Window");
            edit_beacon_dialog(&ui, &win, Some(entry.clone()), &list_box);
        });
    }
    row.append(&edit_button);

    if !entry.pinned {
        let remove_button = gtk::Button::with_label("Remove");
        {
            let ui = ui.clone();
            let id = entry.id.clone();
            let list_box = list_box.clone();
            remove_button.connect_clicked(move |_| {
                ui.state.config.borrow_mut().beacons.retain(|b| b.id != id);
                ui.state.save_config();
                ui.reschedule_beacons();
                rebuild_list(&ui, &list_box);
            });
        }
        row.append(&remove_button);
    }

    row.upcast()
}

fn edit_beacon_dialog(ui: &Rc<Ui>, parent: &adw::Window, existing: Option<Beacon>, list_box: &gtk::ListBox) {
    let is_pinned = existing.as_ref().is_some_and(|e| e.pinned);
    let title = if is_pinned {
        existing.as_ref().map(|e| format!("Edit {} Beacon", e.dest)).unwrap_or_else(|| "Edit Beacon".to_string())
    } else if existing.is_some() {
        "Edit Beacon".to_string()
    } else {
        "Add Beacon".to_string()
    };
    let (win, root) = dialog_window(parent, &title, 420);

    let ports = ui.state.config.borrow().ports.clone();
    let port_names: Vec<&str> = ports.iter().map(|p| p.name.as_str()).collect();
    let port_model = gtk::StringList::new(&port_names);
    let port_dropdown = gtk::DropDown::builder().model(&port_model).build();
    if let Some(e) = &existing {
        if let Some(idx) = ports.iter().position(|p| p.id == e.port_id) {
            port_dropdown.set_selected(idx as u32);
        }
    }
    root.append(&labeled("Port", &port_dropdown));

    // Pinned beacons have a fixed destination — show it as a read-only label
    // so it's visible but can't be accidentally changed.
    let dest_text = existing.as_ref().map(|e| e.dest.as_str()).unwrap_or("BEACON");
    if is_pinned {
        let dest_label = gtk::Label::new(Some(dest_text));
        dest_label.set_halign(gtk::Align::Start);
        dest_label.add_css_class("dim-label");
        root.append(&labeled("Destination", &dest_label));
    }
    let dest_entry = if is_pinned {
        // Entry still needed for the save handler; just hidden.
        gtk::Entry::builder().text(dest_text).build()
    } else {
        let e = gtk::Entry::builder().placeholder_text("BEACON").text(dest_text).build();
        root.append(&labeled("Destination", &e));
        e
    };

    let via_entry =
        gtk::Entry::builder().placeholder_text("WIDE1-1,WIDE2-1 (optional)").text(existing.as_ref().map(|e| e.via.as_str()).unwrap_or("")).build();
    force_uppercase(&via_entry);
    root.append(&labeled("Via", &via_entry));

    let message_entry =
        gtk::Entry::builder().placeholder_text("Message text").text(existing.as_ref().map(|e| e.message.as_str()).unwrap_or("")).build();
    message_entry.set_tooltip_text(Some("$$NODE/$$NAME/$$LOC/$$BBSHOME available; $$NODE is your Profile Callsign"));
    root.append(&labeled("Message", &message_entry));

    let interval_entry =
        gtk::Entry::builder().text(existing.as_ref().map(|e| e.interval_secs.to_string()).unwrap_or_else(|| "600".to_string())).build();
    root.append(&labeled("Interval (s)", &interval_entry));

    let enabled_check = gtk::CheckButton::with_label("Enabled");
    enabled_check.set_active(existing.as_ref().is_none_or(|e| e.enabled));
    root.append(&enabled_check);

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
    button_row.append(&cancel_button);
    button_row.append(&save_button);
    root.append(&button_row);

    {
        let ui = ui.clone();
        let win = win.clone();
        let list_box = list_box.clone();
        let existing_id = existing.map(|e| e.id);
        save_button.connect_clicked(move |_| {
            let dest = dest_entry.text().to_string();
            if dest.trim().is_empty() {
                error_label.set_text("Destination is required.");
                return;
            }
            let message = message_entry.text().to_string();
            let interval_secs = match interval_entry.text().trim().parse::<u32>() {
                Ok(n) if n > 0 => n,
                _ => {
                    error_label.set_text("Interval must be a positive number of seconds.");
                    return;
                }
            };
            let Some(port) = ports.get(port_dropdown.selected() as usize) else {
                error_label.set_text("Select a port.");
                return;
            };

            let mut cfg = ui.state.config.borrow_mut();
            if let Some(id) = &existing_id {
                if let Some(slot) = cfg.beacons.iter_mut().find(|b| &b.id == id) {
                    slot.port_id = port.id.clone();
                    slot.dest = dest.to_uppercase();
                    slot.via = via_entry.text().trim().to_uppercase();
                    slot.message = message;
                    slot.interval_secs = interval_secs;
                    slot.enabled = enabled_check.is_active();
                    // `pinned` is never changed through the UI.
                }
            } else {
                let id = next_id(&cfg);
                cfg.beacons.push(Beacon {
                    id,
                    port_id: port.id.clone(),
                    dest: dest.to_uppercase(),
                    via: via_entry.text().trim().to_uppercase(),
                    message,
                    interval_secs,
                    enabled: enabled_check.is_active(),
                    pinned: false,
                });
            }
            drop(cfg);
            ui.state.save_config();
            ui.reschedule_beacons();
            rebuild_list(&ui, &list_box);
            win.close();
        });
    }

    win.present();
}
