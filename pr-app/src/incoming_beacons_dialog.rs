//! The Notifications dialog: a unified log of every event that lit up the
//! header's bell button — Custom Notification Rule matches and directed
//! notifications (incoming connection or a frame addressed to your callsign).
//! Each kind is shown in its own section, newest first.
//!
//! Also hosts the "Custom Notification Rules" sub-dialog for editing rules
//! (destination regex + optional sender filter), and a sound-settings popup
//! for choosing an optional audio cue. Opening this dialog clears the header
//! button's "lit" state (the simplest "mark as seen" trigger available).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;

use pr_core::{BeaconMonitorRule, IncomingBeacon, NotifiedPacket};

use crate::app_state::find_entry;
use crate::highlight::{highlight_to_markup, Highlighter};
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

fn refresh_directed_toggle(btn: &gtk::ToggleButton, enabled: bool) {
    btn.set_active(enabled);
    btn.set_tooltip_text(Some(if enabled {
        "Directed notifications on \u{2014} click to disable"
    } else {
        "Directed notifications off \u{2014} click to enable"
    }));
}

/// Opens the Notifications dialog and clears the header button's lit state.
pub fn show(ui: &Rc<Ui>) {
    use crate::ports_dialog::dialog_window_with_header;
    ui.clear_notification_received();

    let (win, root, header) = dialog_window_with_header(&ui.window, "Notifications", 580);
    win.set_default_height(540);

    // Enable/disable toggle for directed notifications in the header.
    let directed_toggle = gtk::ToggleButton::new();
    directed_toggle.set_icon_name("go-next-symbolic");
    directed_toggle.add_css_class("flat");
    refresh_directed_toggle(&directed_toggle, ui.state.config.borrow().notify.directed_enabled);
    {
        let ui = ui.clone();
        let btn = directed_toggle.clone();
        directed_toggle.connect_toggled(move |_| {
            let enabled = {
                let mut cfg = ui.state.config.borrow_mut();
                cfg.notify.directed_enabled = !cfg.notify.directed_enabled;
                cfg.notify.directed_enabled
            };
            ui.state.save_config();
            refresh_directed_toggle(&btn, enabled);
        });
    }
    header.pack_end(&directed_toggle);

    // --- Directed Notifications section ---
    let directed_heading = gtk::Label::new(Some("Directed Notifications"));
    directed_heading.add_css_class("heading");
    directed_heading.set_halign(gtk::Align::Start);
    directed_heading.set_margin_top(4);
    root.append(&directed_heading);

    let directed_sub = gtk::Label::new(Some("Incoming connections and frames addressed to your callsign"));
    directed_sub.add_css_class("dim-label");
    directed_sub.add_css_class("caption");
    directed_sub.set_halign(gtk::Align::Start);
    root.append(&directed_sub);

    let directed_list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    directed_list.add_css_class("boxed-list");
    let directed_scrolled =
        gtk::ScrolledWindow::builder().child(&directed_list).min_content_height(140).max_content_height(220).build();
    root.append(&directed_scrolled);
    rebuild_directed_list(ui, &directed_list);

    let directed_clear = gtk::Button::with_label("Clear All");
    directed_clear.set_halign(gtk::Align::Start);
    {
        let ui = ui.clone();
        let win = win.clone();
        let directed_list = directed_list.clone();
        directed_clear.connect_clicked(move |_| {
            confirm_clear_directed(&ui, &win, &directed_list);
        });
    }
    root.append(&directed_clear);

    root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    // --- Custom Notification Matches section ---
    let monitor_heading = gtk::Label::new(Some("Custom Notification Matches"));
    monitor_heading.add_css_class("heading");
    monitor_heading.set_halign(gtk::Align::Start);
    monitor_heading.set_margin_top(4);
    root.append(&monitor_heading);

    let monitor_sub = gtk::Label::new(Some("Frames matching a Custom Notification Rule"));
    monitor_sub.add_css_class("dim-label");
    monitor_sub.add_css_class("caption");
    monitor_sub.set_halign(gtk::Align::Start);
    root.append(&monitor_sub);

    let monitor_list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    monitor_list.add_css_class("boxed-list");
    let monitor_scrolled =
        gtk::ScrolledWindow::builder().child(&monitor_list).min_content_height(140).max_content_height(220).build();
    root.append(&monitor_scrolled);
    rebuild_monitor_list(ui, &monitor_list);

    let bottom_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);

    let monitor_clear = gtk::Button::with_label("Clear All");
    {
        let ui = ui.clone();
        let win = win.clone();
        let monitor_list = monitor_list.clone();
        monitor_clear.connect_clicked(move |_| {
            confirm_clear_monitor(&ui, &win, &monitor_list);
        });
    }
    bottom_row.append(&monitor_clear);

    let rules_button = gtk::Button::with_label("Custom Notification Rules\u{2026}");
    {
        let ui = ui.clone();
        let win = win.clone();
        rules_button.connect_clicked(move |_| {
            show_rules_dialog(&ui, &win);
        });
    }
    bottom_row.append(&rules_button);

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    bottom_row.append(&spacer);

    let sound_button = gtk::Button::with_label("Sound\u{2026}");
    sound_button.set_tooltip_text(Some("Configure a notification sound"));
    {
        let ui = ui.clone();
        let win = win.clone();
        sound_button.connect_clicked(move |_| {
            show_sound_dialog(&ui, &win);
        });
    }
    bottom_row.append(&sound_button);

    root.append(&bottom_row);

    win.present();
}

// ---------------------------------------------------------------------------
// Directed notifications list
// ---------------------------------------------------------------------------

fn rebuild_directed_list(ui: &Rc<Ui>, list_box: &gtk::ListBox) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    let cfg = ui.state.config.borrow();
    let mut packets = cfg.notified_packets.clone();
    let highlighter = Highlighter::build(&cfg);
    drop(cfg);
    packets.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    if packets.is_empty() {
        let label = gtk::Label::new(Some("No directed notifications yet."));
        label.set_margin_top(10);
        label.set_margin_bottom(10);
        list_box.append(&label);
        return;
    }
    for packet in packets {
        list_box.append(&build_directed_row(ui, list_box, &highlighter, packet));
    }
}

fn build_directed_row(
    ui: &Rc<Ui>,
    list_box: &gtk::ListBox,
    highlighter: &Highlighter,
    packet: NotifiedPacket,
) -> gtk::Widget {
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
                rebuild_directed_list(&ui, &list_box);
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

fn confirm_clear_directed(ui: &Rc<Ui>, win: &adw::Window, list_box: &gtk::ListBox) {
    let dialog = adw::AlertDialog::builder()
        .heading("Clear All Directed Notifications?")
        .body("This permanently deletes every logged directed notification. This can't be undone.")
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
            ui.state.config.borrow_mut().notified_packets.clear();
            ui.state.save_config();
            rebuild_directed_list(&ui, &list_box);
        }
    });
}

// ---------------------------------------------------------------------------
// Destination monitor matches list
// ---------------------------------------------------------------------------

fn rebuild_monitor_list(ui: &Rc<Ui>, list_box: &gtk::ListBox) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    let mut beacons = ui.state.config.borrow().incoming_beacons.clone();
    beacons.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    if beacons.is_empty() {
        let label = gtk::Label::new(Some("No destination monitor matches yet."));
        label.set_margin_top(10);
        label.set_margin_bottom(10);
        list_box.append(&label);
        return;
    }
    for beacon in beacons {
        list_box.append(&build_monitor_row(ui, list_box, beacon));
    }
}

fn build_monitor_row(ui: &Rc<Ui>, list_box: &gtk::ListBox, beacon: IncomingBeacon) -> gtk::Widget {
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
                rebuild_monitor_list(&ui, &list_box);
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

fn confirm_clear_monitor(ui: &Rc<Ui>, win: &adw::Window, list_box: &gtk::ListBox) {
    let dialog = adw::AlertDialog::builder()
        .heading("Clear All Custom Notification Matches?")
        .body("This permanently deletes every logged custom notification match. This can't be undone.")
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
            rebuild_monitor_list(&ui, &list_box);
        }
    });
}

// ---------------------------------------------------------------------------
// Custom Notification Rules sub-dialog
// ---------------------------------------------------------------------------

fn show_rules_dialog(ui: &Rc<Ui>, parent: &adw::Window) {
    let (win, root) = dialog_window(parent, "Custom Notification Rules", 520);
    win.set_default_height(440);

    let desc = gtk::Label::new(Some(
        "Watch for frames with a matching destination (regex) and optional sender callsign. \
         Leave Sender blank (shown as @ALL) to match any sender. \
         Address-book entries at the top are managed from the Address Book.",
    ));
    desc.set_wrap(true);
    desc.set_halign(gtk::Align::Start);
    desc.add_css_class("dim-label");
    desc.add_css_class("caption");
    root.append(&desc);

    // Column header row
    let col_header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    col_header.set_margin_start(28);
    col_header.set_margin_end(8);
    for (text, expand) in [("Sender", false), ("Destination", true)] {
        let lbl = gtk::Label::new(Some(text));
        lbl.add_css_class("dim-label");
        lbl.add_css_class("caption");
        lbl.set_halign(gtk::Align::Start);
        if expand { lbl.set_hexpand(true); }
        col_header.append(&lbl);
    }
    root.append(&col_header);

    let rules_list_box = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    rules_list_box.add_css_class("boxed-list");
    let rules_scrolled =
        gtk::ScrolledWindow::builder().child(&rules_list_box).vexpand(true).min_content_height(240).build();
    root.append(&rules_scrolled);

    // Sort: address-book rules first (alphabetically by label), then normal
    // rules (alphabetically by label).
    let sorted_rules: Vec<BeaconMonitorRule> = {
        let raw = ui.state.config.borrow().beacon_rules.clone();
        let mut ab: Vec<_> = raw.iter().filter(|r| r.from_address_book).cloned().collect();
        let mut normal: Vec<_> = raw.iter().filter(|r| !r.from_address_book).cloned().collect();
        ab.sort_by(|a, b| a.label.cmp(&b.label));
        normal.sort_by(|a, b| a.label.cmp(&b.label));
        ab.into_iter().chain(normal).collect()
    };
    let rules: Rc<RefCell<Vec<BeaconMonitorRule>>> = Rc::new(RefCell::new(sorted_rules));
    rebuild_rules_list(&rules_list_box, &rules);

    let add_rule_button = gtk::Button::with_label("Add Rule\u{2026}");
    add_rule_button.set_halign(gtk::Align::Start);
    {
        let rules = rules.clone();
        let rules_list_box = rules_list_box.clone();
        add_rule_button.connect_clicked(move |_| {
            let id = next_rule_id(&rules.borrow());
            rules.borrow_mut().push(BeaconMonitorRule {
                id,
                label: String::new(),
                pattern: String::new(),
                sender: String::new(),
                enabled: true,
                from_address_book: false,
            });
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
            for rule in rules.borrow().iter().filter(|r| !r.from_address_book) {
                if regex::Regex::new(&rule.pattern).is_err() {
                    error_label.set_text(&format!("Destination pattern \u{201c}{}\u{201d} is not a valid regex.", rule.pattern));
                    return;
                }
            }
            // Auto-derive a display label from sender + destination so
            // notification toasts still have a meaningful name.
            for rule in rules.borrow_mut().iter_mut().filter(|r| !r.from_address_book) {
                rule.label = if rule.sender.trim().is_empty() {
                    rule.pattern.clone()
                } else {
                    format!("{}\u{2192}{}", rule.sender.trim(), rule.pattern)
                };
            }
            // Keep address-book rules exactly as-is (managed from Address Book).
            // Overwrite normal rules with the edited list.
            let new_normal: Vec<_> = rules.borrow().iter().filter(|r| !r.from_address_book).cloned().collect();
            let mut cfg = ui.state.config.borrow_mut();
            cfg.beacon_rules.retain(|r| r.from_address_book);
            cfg.beacon_rules.extend(new_normal);
            drop(cfg);
            ui.state.save_config();
            win.close();
        });
    }
    button_row.append(&cancel_button);
    button_row.append(&save_button);
    root.append(&button_row);

    win.present();
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

    if rule.from_address_book {
        // Address-book-added rules: read-only with book icon + delete only.
        let book_icon = gtk::Image::from_icon_name("x-office-address-book-symbolic");
        book_icon.set_tooltip_text(Some("Added from Address Book"));
        row.append(&book_icon);

        let sender_lbl = gtk::Label::new(Some(&rule.sender));
        sender_lbl.set_width_chars(10);
        sender_lbl.set_halign(gtk::Align::Start);
        sender_lbl.add_css_class("dim-label");
        row.append(&sender_lbl);

        let dest_lbl = gtk::Label::new(Some(&rule.pattern));
        dest_lbl.set_hexpand(true);
        dest_lbl.set_halign(gtk::Align::Start);
        dest_lbl.add_css_class("dim-label");
        row.append(&dest_lbl);
    } else {
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

        let sender_entry = gtk::Entry::builder().text(&rule.sender).width_chars(10).placeholder_text("@ALL").build();
        {
            let rules = rules.clone();
            sender_entry.connect_changed(move |e| {
                if let Some(r) = rules.borrow_mut().get_mut(idx) {
                    r.sender = e.text().to_string();
                }
            });
        }
        row.append(&sender_entry);

        let pattern_entry = gtk::Entry::builder().text(&rule.pattern).hexpand(true).placeholder_text("^(CQ|BEACON.*)$").build();
        {
            let rules = rules.clone();
            pattern_entry.connect_changed(move |e| {
                if let Some(r) = rules.borrow_mut().get_mut(idx) {
                    r.pattern = e.text().to_string();
                }
            });
        }
        row.append(&pattern_entry);
    }

    let remove_button = gtk::Button::from_icon_name("user-trash-symbolic");
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

// ---------------------------------------------------------------------------
// Sound settings popup
// ---------------------------------------------------------------------------

fn show_sound_dialog(ui: &Rc<Ui>, parent: &adw::Window) {
    let (win, root) = dialog_window(parent, "Notification Sound", 400);

    let desc = gtk::Label::new(Some(
        "Choose an audio file (WAV/OGG/FLAC) to play when a notification fires. \
         Leave empty for no sound. Requires paplay (PulseAudio/PipeWire) or aplay (ALSA).",
    ));
    desc.set_wrap(true);
    desc.set_halign(gtk::Align::Start);
    desc.add_css_class("dim-label");
    desc.add_css_class("caption");
    root.append(&desc);

    let sound_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let current_path = ui.state.config.borrow().notify.notification_sound.clone().unwrap_or_default();
    let path_entry = gtk::Entry::builder().hexpand(true).placeholder_text("Path to audio file\u{2026}").text(&current_path).build();
    sound_row.append(&path_entry);

    let browse_button = gtk::Button::with_label("Browse\u{2026}");
    {
        let win = win.clone();
        let path_entry = path_entry.clone();
        browse_button.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::new();
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("Audio files"));
            filter.add_mime_type("audio/*");
            let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            dialog.set_filters(Some(&filters));
            let path_entry = path_entry.clone();
            if let Some(w) = win.upcast_ref::<gtk::Widget>().root().and_then(|r| r.downcast::<gtk::Window>().ok()) {
                dialog.open(Some(&w), gtk::gio::Cancellable::NONE, move |result| {
                    if let Ok(file) = result {
                        if let Some(path) = file.path() {
                            path_entry.set_text(&path.to_string_lossy());
                        }
                    }
                });
            }
        });
    }
    sound_row.append(&browse_button);

    let test_button = gtk::Button::with_label("Test");
    {
        let path_entry = path_entry.clone();
        let ui = ui.clone();
        test_button.connect_clicked(move |_| {
            let path = path_entry.text().to_string();
            if !path.is_empty() {
                // Temporarily override the config sound to test.
                let mut cfg = ui.state.config.borrow_mut();
                let old = cfg.notify.notification_sound.clone();
                cfg.notify.notification_sound = Some(path);
                crate::notify::play_sound(&cfg);
                cfg.notify.notification_sound = old;
            }
        });
    }
    sound_row.append(&test_button);

    root.append(&sound_row);

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
        save_button.connect_clicked(move |_| {
            let path = path_entry.text().to_string();
            ui.state.config.borrow_mut().notify.notification_sound =
                if path.trim().is_empty() { None } else { Some(path) };
            ui.state.save_config();
            win.close();
        });
    }
    button_row.append(&cancel_button);
    button_row.append(&save_button);
    root.append(&button_row);

    win.present();
}
