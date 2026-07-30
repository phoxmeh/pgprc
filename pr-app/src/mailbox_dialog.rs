use std::rc::Rc;

use adw::prelude::*;
use gtk::glib::object::IsA;

use pr_core::MailboxMessage;

use crate::ports_dialog::{collapse_listen_ports, dialog_window, dialog_window_with_header, force_uppercase, port_listen_checklist};
use crate::window::Ui;

/// The mailbox's message list (read or not), with a Delete button per row.
/// The Enable button lives in this window's own titlebar (left-click
/// toggles, right-click opens Settings) rather than the content area, same
/// idea as the app's own header buttons.
pub fn show(ui: &Rc<Ui>) {
    let (win, root, header) = dialog_window_with_header(&ui.window, "Mailbox", 560);
    win.set_default_height(480);

    let enable_button = gtk::Button::with_label("Enable");
    recolor_enable_button(&enable_button, current_state(ui));
    header.pack_start(&enable_button);
    {
        let ui = ui.clone();
        let win = win.clone();
        let button_for_recolor = enable_button.clone();
        enable_button.connect_clicked(move |_| {
            let has_callsign = !ui.state.config.borrow().mailbox.respond_call.trim().is_empty();
            let was_enabled = ui.state.config.borrow().mailbox.enabled;
            if !was_enabled && !has_callsign {
                // Can't turn on without a callsign of its own -- send the
                // user straight to Settings instead of silently refusing.
                show_settings(&ui, &win);
                return;
            }
            ui.state.config.borrow_mut().mailbox.enabled = !was_enabled;
            ui.state.save_config();
            ui.refresh_mailbox_button();
            ui.reschedule_mailbox_beacon();
            recolor_enable_button(&button_for_recolor, current_state(&ui));
        });
    }
    {
        let ui = ui.clone();
        let win = win.clone();
        let gesture = gtk::GestureClick::new();
        gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
        gesture.connect_pressed(move |_, _, _, _| {
            show_settings(&ui, &win);
        });
        enable_button.add_controller(gesture);
    }

    let list_box = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    list_box.add_css_class("boxed-list");
    let scrolled = gtk::ScrolledWindow::builder().child(&list_box).vexpand(true).min_content_height(300).build();
    root.append(&scrolled);

    rebuild_list(ui, &list_box);

    win.present();
}

fn current_state(ui: &Rc<Ui>) -> (bool, bool) {
    let cfg = ui.state.config.borrow();
    (cfg.mailbox.enabled, cfg.mailbox.messages.iter().any(|m| !m.read))
}

fn recolor_enable_button(button: &gtk::Button, (enabled, has_unread): (bool, bool)) {
    button.remove_css_class("state-success");
    button.remove_css_class("state-warning");
    if let Some(class) = crate::mailbox::status_class(enabled, has_unread) {
        button.add_css_class(class);
    }
    let tooltip = if enabled {
        "Mailbox: on \u{2014} click to turn off, right-click for settings"
    } else {
        "Mailbox: off \u{2014} click to turn on, right-click for settings"
    };
    button.set_tooltip_text(Some(tooltip));
}

/// Callsign / intro-message / listen-ports, in their own Save/Cancel dialog
/// -- separated out of the main Mailbox window (which otherwise gets
/// cluttered mixing one-off settings with the scrolling message list).
/// There's no enable toggle here -- that's the Mailbox window's own Enable
/// button's job, matching keyboard-to-keyboard's settings dialog.
pub(crate) fn show_settings(ui: &Rc<Ui>, parent: &impl IsA<gtk::Window>) {
    let (win, root) = dialog_window(parent, "Mailbox Settings", 480);

    let current = ui.state.config.borrow().mailbox.clone();

    let settings_group = adw::PreferencesGroup::builder()
        .description("Required to enable the mailbox; never falls back to your Profile callsign.")
        .build();

    let respond_call_row = adw::EntryRow::builder().title("Mailbox Callsign").build();
    respond_call_row.set_text(&current.respond_call);
    force_uppercase(&respond_call_row);
    settings_group.add(&respond_call_row);

    let intro_hint = if current.intro_message.trim().is_empty() {
        "Using the default greeting".to_string()
    } else {
        current.intro_message.lines().next().unwrap_or("").to_string()
    };
    let intro_row = adw::ActionRow::builder().title("Intro Message").subtitle(&intro_hint).build();
    let intro_button = gtk::Button::with_label("Set Intro Message\u{2026}");
    intro_button.set_valign(gtk::Align::Center);
    intro_row.add_suffix(&intro_button);
    settings_group.add(&intro_row);

    // The Intro Message sub-dialog edits this in-memory value, not config
    // directly -- it only actually persists when this dialog's own Save is
    // clicked, same as the callsign field, so Cancel here discards an
    // intro-message edit too.
    let intro_message: Rc<std::cell::RefCell<String>> = Rc::new(std::cell::RefCell::new(current.intro_message.clone()));
    {
        let win = win.clone();
        let intro_row = intro_row.clone();
        let intro_message = intro_message.clone();
        intro_button.connect_clicked(move |_| {
            edit_intro_message(&win, &intro_row, &intro_message);
        });
    }

    root.append(&settings_group);

    let beacon_group = adw::PreferencesGroup::builder()
        .title("Availability Beacon")
        .description("Sent periodically on every listen port that supports unproto, while enabled")
        .build();
    let beacon_text_row = adw::EntryRow::builder().title("Beacon Text").build();
    beacon_text_row.set_text(&current.beacon_text);
    beacon_group.add(&beacon_text_row);
    let beacon_interval_row = adw::EntryRow::builder().title("Beacon Interval (seconds)").build();
    beacon_interval_row.set_text(&current.beacon_interval_secs.to_string());
    beacon_group.add(&beacon_interval_row);
    root.append(&beacon_group);

    let listen_group = adw::PreferencesGroup::builder()
        .title("Listen On")
        .description("Ports to auto-answer unsolicited connections (as the mailbox) on")
        .build();
    let ports = ui.state.config.borrow().ports.clone();
    let (listen_widget, listen_switches) = port_listen_checklist(&ports, &current.listen_ports);
    listen_group.add(&listen_widget);
    root.append(&listen_group);

    let error_label = gtk::Label::new(None);
    error_label.add_css_class("error");
    error_label.set_halign(gtk::Align::Start);
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
        save_button.connect_clicked(move |_| {
            let respond_call = respond_call_row.text().trim().to_uppercase();
            if respond_call.is_empty() && ui.state.config.borrow().mailbox.enabled {
                error_label.set_text("A Mailbox Callsign is required while the mailbox is enabled.");
                return;
            }
            let beacon_interval_secs = match beacon_interval_row.text().trim().parse::<u32>() {
                Ok(n) if n > 0 => n,
                _ => {
                    error_label.set_text("Beacon Interval must be a positive number.");
                    return;
                }
            };
            {
                let mut cfg = ui.state.config.borrow_mut();
                cfg.mailbox.respond_call = respond_call;
                cfg.mailbox.intro_message = intro_message.borrow().clone();
                cfg.mailbox.beacon_text = beacon_text_row.text().to_string();
                cfg.mailbox.beacon_interval_secs = beacon_interval_secs;
                cfg.mailbox.listen_ports = collapse_listen_ports(&listen_switches);
            }
            ui.state.save_config();
            ui.reschedule_mailbox_beacon();
            win.close();
        });
    }
    button_row.append(&cancel_button);
    button_row.append(&save_button);
    root.append(&button_row);

    win.present();
}

/// A small Save/Cancel modal for the mailbox's custom connect greeting,
/// mirroring the address book's per-entry edit dialogs -- it's free text,
/// not a short field, so it gets its own focused editor rather than living
/// inline in the settings form. Writes back into the settings dialog's
/// in-memory `intro_message`, not config directly -- the outer Save/Cancel
/// still governs whether it actually persists.
fn edit_intro_message(parent: &adw::Window, intro_row: &adw::ActionRow, intro_message: &Rc<std::cell::RefCell<String>>) {
    let (win, root) = dialog_window(parent, "Intro Message", 480);

    let label = gtk::Label::new(Some("Sent when a station connects to the mailbox. Leave blank to use the default greeting."));
    label.set_halign(gtk::Align::Start);
    label.set_wrap(true);
    root.append(&label);

    let text_view = gtk::TextView::builder().wrap_mode(gtk::WrapMode::WordChar).build();
    let buffer = text_view.buffer();
    buffer.set_text(&intro_message.borrow());
    let text_scrolled = gtk::ScrolledWindow::builder().child(&text_view).min_content_height(140).has_frame(true).vexpand(true).build();
    root.append(&text_scrolled);

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
        let win = win.clone();
        let buffer = buffer.clone();
        let intro_row = intro_row.clone();
        let intro_message = intro_message.clone();
        save_button.connect_clicked(move |_| {
            let (start, end) = buffer.bounds();
            let text = buffer.text(&start, &end, false).to_string();
            *intro_message.borrow_mut() = text.clone();
            intro_row.set_subtitle(if text.trim().is_empty() {
                "Using the default greeting"
            } else {
                text.lines().next().unwrap_or("")
            });
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
            ui.refresh_mailbox_button();
            rebuild_list(&ui, &list_box);
        });
    }
    row.append(&remove_button);

    row.upcast()
}
