//! Tracks received frames that matched a "beacon monitor" rule — separate
//! from the general Custom Rules highlight/notify list in Preferences,
//! since beacon destinations are matched by a real regex rather than a
//! literal token list, and get their own log here rather than being mixed
//! into the address book's "heard" tracking or the Notified Packets list.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;

use pr_core::{BeaconMonitorRule, IncomingBeacon};

use crate::app_state::find_entry;
use crate::ports_dialog::dialog_window;
use crate::window::Ui;

fn next_rule_id(rules: &[BeaconMonitorRule]) -> String {
    let mut n = rules.len();
    loop {
        let candidate = format!("beacon-rule-{n}");
        if !rules.iter().any(|r| r.id == candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Opens the Incoming Beacons dialog and clears the header button's lit
/// state — the simplest "mark as seen" trigger available.
pub fn show(ui: &Rc<Ui>) {
    ui.clear_beacon_lit();

    let (win, root) = dialog_window(&ui.window, "Incoming Beacons", 560);
    win.set_default_height(480);

    // Log: every received frame that matched a rule, newest first. Acts
    // immediately (delete/clear-all), same as Notified Packets/Address Book.
    let list_box = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    list_box.add_css_class("boxed-list");
    let list_scrolled = gtk::ScrolledWindow::builder().child(&list_box).vexpand(true).min_content_height(260).build();
    root.append(&list_scrolled);
    rebuild_list(ui, &list_box);

    let button_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);

    let clear_all_button = gtk::Button::with_label("Clear All");
    {
        let ui = ui.clone();
        let win = win.clone();
        let list_box = list_box.clone();
        clear_all_button.connect_clicked(move |_| {
            confirm_clear_all(&ui, &win, &list_box);
        });
    }
    button_row.append(&clear_all_button);

    // Kept as its own dialog (rather than inline here) so this window stays
    // a plain, immediate-action list -- same reasoning as any other
    // deferred-edit rule list in this app getting its own window.
    let rules_button = gtk::Button::with_label("Beacon Monitor Rules\u{2026}");
    {
        let ui = ui.clone();
        let win = win.clone();
        rules_button.connect_clicked(move |_| {
            show_rules_dialog(&ui, &win);
        });
    }
    button_row.append(&rules_button);

    root.append(&button_row);

    win.present();
}

/// Destination regex patterns to watch for -- a match logs in the Incoming
/// Beacons list and, if "Beacon Notifications" is on in Preferences, raises
/// a desktop notification. Deferred Save, same pattern as Preferences'
/// Custom Rules list.
fn show_rules_dialog(ui: &Rc<Ui>, parent: &adw::Window) {
    let (win, root) = dialog_window(parent, "Beacon Monitor Rules", 480);
    win.set_default_height(420);

    let desc = gtk::Label::new(Some(
        "Destination regex patterns to watch for in received traffic.",
    ));
    desc.set_wrap(true);
    desc.set_halign(gtk::Align::Start);
    desc.add_css_class("dim-label");
    desc.add_css_class("caption");
    root.append(&desc);

    let rules_list_box = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    rules_list_box.add_css_class("boxed-list");
    let rules_scrolled = gtk::ScrolledWindow::builder().child(&rules_list_box).vexpand(true).min_content_height(240).build();
    root.append(&rules_scrolled);

    let rules: Rc<RefCell<Vec<BeaconMonitorRule>>> = Rc::new(RefCell::new(ui.state.config.borrow().beacon_rules.clone()));
    rebuild_rules_list(&rules_list_box, &rules);

    let add_rule_button = gtk::Button::with_label("Add Rule\u{2026}");
    add_rule_button.set_halign(gtk::Align::Start);
    {
        let rules = rules.clone();
        let rules_list_box = rules_list_box.clone();
        add_rule_button.connect_clicked(move |_| {
            let id = next_rule_id(&rules.borrow());
            rules.borrow_mut().push(BeaconMonitorRule { id, label: "New Rule".to_string(), pattern: String::new(), enabled: true });
            rebuild_rules_list(&rules_list_box, &rules);
        });
    }
    root.append(&add_rule_button);

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
        let rules = rules.clone();
        save_button.connect_clicked(move |_| {
            for rule in rules.borrow().iter() {
                if regex::Regex::new(&rule.pattern).is_err() {
                    error_label.set_text(&format!("Rule \u{201c}{}\u{201d} has an invalid regex.", rule.label));
                    return;
                }
            }
            ui.state.config.borrow_mut().beacon_rules = rules.borrow().clone();
            ui.state.save_config();
            win.close();
        });
    }
    button_row.append(&cancel_button);
    button_row.append(&save_button);
    root.append(&button_row);

    win.present();
}

fn rebuild_list(ui: &Rc<Ui>, list_box: &gtk::ListBox) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    let mut beacons = ui.state.config.borrow().incoming_beacons.clone();
    beacons.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    if beacons.is_empty() {
        let label = gtk::Label::new(Some("No incoming beacons detected yet."));
        label.set_margin_top(12);
        label.set_margin_bottom(12);
        list_box.append(&label);
        return;
    }

    for beacon in beacons {
        list_box.append(&build_row(ui, list_box, beacon));
    }
}

fn build_row(ui: &Rc<Ui>, list_box: &gtk::ListBox, beacon: IncomingBeacon) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_margin_top(6);
    row.set_margin_bottom(6);
    row.set_margin_start(8);
    row.set_margin_end(8);

    let text_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text_box.set_hexpand(true);

    let port_name = find_entry(&ui.state.config.borrow(), &beacon.port_id).map(|e| e.name).unwrap_or_else(|| beacon.port_id.clone());
    let caption = gtk::Label::new(Some(&format!("{} \u{b7} {}", beacon.timestamp, port_name)));
    caption.set_halign(gtk::Align::Start);
    caption.add_css_class("dim-label");
    caption.add_css_class("caption");
    text_box.append(&caption);

    let line = gtk::Label::new(Some(&format!("{} \u{2192} {}: {}", beacon.from, beacon.to, beacon.message)));
    line.set_halign(gtk::Align::Start);
    line.set_wrap(true);
    line.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    line.set_xalign(0.0);
    text_box.append(&line);

    row.append(&text_box);

    // Two-tap delete: first tap arms it (red, checkmark icon to confirm);
    // second tap actually removes the entry.
    let delete_button = gtk::Button::from_icon_name("user-trash-symbolic");
    delete_button.add_css_class("flat");
    delete_button.set_valign(gtk::Align::Center);
    delete_button.set_tooltip_text(Some("Delete"));
    let armed = Rc::new(Cell::new(false));
    {
        let ui = ui.clone();
        let list_box = list_box.clone();
        let id = beacon.id;
        let armed = armed.clone();
        delete_button.connect_clicked(move |btn| {
            if armed.get() {
                ui.state.remove_incoming_beacon(id);
                rebuild_list(&ui, &list_box);
            } else {
                armed.set(true);
                btn.set_icon_name("object-select-symbolic");
                btn.set_tooltip_text(Some("Click again to confirm"));
                btn.add_css_class("destructive-action");
            }
        });
    }
    row.append(&delete_button);

    row.upcast()
}

/// Confirm-then-clear the whole log, mirroring `Ui::confirm_clear_history`'s
/// `adw::AlertDialog` pattern in `window.rs`.
fn confirm_clear_all(ui: &Rc<Ui>, win: &adw::Window, list_box: &gtk::ListBox) {
    let dialog = adw::AlertDialog::builder()
        .heading("Clear All Incoming Beacons?")
        .body("This permanently deletes every logged incoming beacon. This can't be undone.")
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("clear", "Clear");
    dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let ui = ui.clone();
    let list_box = list_box.clone();
    dialog.choose(win, gtk::gio::Cancellable::NONE, move |response| {
        if response == "clear" {
            ui.state.clear_incoming_beacons();
            rebuild_list(&ui, &list_box);
        }
    });
}

fn rebuild_rules_list(list_box: &gtk::ListBox, rules: &Rc<RefCell<Vec<BeaconMonitorRule>>>) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    let len = rules.borrow().len();
    for idx in 0..len {
        list_box.append(&build_rule_row(list_box, rules, idx));
    }
}

fn build_rule_row(list_box: &gtk::ListBox, rules: &Rc<RefCell<Vec<BeaconMonitorRule>>>, idx: usize) -> gtk::Widget {
    let rule = rules.borrow()[idx].clone();

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.set_margin_top(4);
    row.set_margin_bottom(4);
    row.set_margin_start(6);
    row.set_margin_end(6);

    let enabled_check = gtk::CheckButton::new();
    enabled_check.set_active(rule.enabled);
    {
        let rules = rules.clone();
        enabled_check.connect_toggled(move |btn| {
            if let Some(r) = rules.borrow_mut().get_mut(idx) {
                r.enabled = btn.is_active();
            }
        });
    }
    row.append(&enabled_check);

    let label_entry = gtk::Entry::builder().text(&rule.label).width_chars(10).build();
    {
        let rules = rules.clone();
        label_entry.connect_changed(move |e| {
            if let Some(r) = rules.borrow_mut().get_mut(idx) {
                r.label = e.text().to_string();
            }
        });
    }
    row.append(&label_entry);

    let pattern_entry =
        gtk::Entry::builder().text(&rule.pattern).hexpand(true).placeholder_text("^(CQ|BEACON.*)$").build();
    {
        let rules = rules.clone();
        pattern_entry.connect_changed(move |e| {
            if let Some(r) = rules.borrow_mut().get_mut(idx) {
                r.pattern = e.text().to_string();
            }
        });
    }
    row.append(&pattern_entry);

    let remove_button = gtk::Button::with_label("\u{2715}");
    remove_button.add_css_class("flat");
    {
        let rules = rules.clone();
        let list_box = list_box.clone();
        remove_button.connect_clicked(move |_| {
            rules.borrow_mut().remove(idx);
            rebuild_rules_list(&list_box, &rules);
        });
    }
    row.append(&remove_button);

    row.upcast()
}
