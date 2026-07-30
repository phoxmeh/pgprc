use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;

use crate::ports_dialog::{collapse_listen_ports, dialog_window, force_uppercase, port_listen_checklist};
use crate::window::Ui;

/// Keyboard-to-keyboard mode settings: welcome message, availability beacon,
/// and which ports to listen on. There's no enable switch here -- that's the
/// header button's own job (left-click toggles it directly); this dialog is
/// only reachable by right-clicking that button.
pub fn show_settings(ui: &Rc<Ui>) {
    let (win, root) = dialog_window(&ui.window, "Keyboard-to-Keyboard Settings", 480);

    let current = ui.state.config.borrow().keyboard_mode.clone();
    let ports = ui.state.config.borrow().ports.clone();

    let settings_group = adw::PreferencesGroup::builder()
        .description(
            "The callsign this mode answers as \u{2014} independent of your Profile callsign and the mailbox's own \
             Respond From Callsign, so the two auto-responders can't intercept each other's connects. Leave blank to \
             fall back to your Profile callsign. Note: the beacon and any connected-mode replies always go out under \
             whichever port sends them (each port has its own fixed callsign) \u{2014} for them to actually appear as \
             this callsign on the air, point \u{201c}Listen On\u{201d} below at a port configured with this same call.",
        )
        .build();

    let node_call_row = adw::EntryRow::builder().title("Node Callsign").build();
    node_call_row.set_text(&current.node_call);
    force_uppercase(&node_call_row);
    settings_group.add(&node_call_row);

    let welcome_hint = if current.welcome_message.trim().is_empty() {
        "Using the default greeting".to_string()
    } else {
        current.welcome_message.lines().next().unwrap_or("").to_string()
    };
    let welcome_row = adw::ActionRow::builder().title("Welcome Message").subtitle(&welcome_hint).build();
    let welcome_button = gtk::Button::with_label("Set Welcome Message\u{2026}");
    welcome_button.set_valign(gtk::Align::Center);
    welcome_row.add_suffix(&welcome_button);
    settings_group.add(&welcome_row);

    let welcome_message: Rc<RefCell<String>> = Rc::new(RefCell::new(current.welcome_message.clone()));
    {
        let win = win.clone();
        let welcome_row = welcome_row.clone();
        let welcome_message = welcome_message.clone();
        welcome_button.connect_clicked(move |_| {
            edit_welcome_message(&win, &welcome_row, &welcome_message);
        });
    }

    let beacon_text_row = adw::EntryRow::builder()
        .title("Availability Beacon Text")
        .tooltip_text("$$NODE/$$NAME/$$LOC/$$BBSHOME available; $$NODE is this Node Callsign")
        .build();
    beacon_text_row.set_text(&current.beacon_text);
    settings_group.add(&beacon_text_row);

    let beacon_interval_row = adw::EntryRow::builder().title("Beacon Interval (seconds)").build();
    beacon_interval_row.set_text(&current.beacon_interval_secs.to_string());
    settings_group.add(&beacon_interval_row);

    root.append(&settings_group);

    let listen_group = adw::PreferencesGroup::builder()
        .title("Listen On")
        .description("Ports to answer keyboard-to-keyboard connects (and send the availability beacon) on")
        .build();
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
            let beacon_interval_secs = match beacon_interval_row.text().trim().parse::<u32>() {
                Ok(n) if n > 0 => n,
                _ => {
                    error_label.set_text("Beacon Interval must be a positive number.");
                    return;
                }
            };
            {
                let mut cfg = ui.state.config.borrow_mut();
                cfg.keyboard_mode.node_call = node_call_row.text().trim().to_uppercase();
                cfg.keyboard_mode.welcome_message = welcome_message.borrow().clone();
                cfg.keyboard_mode.beacon_text = beacon_text_row.text().to_string();
                cfg.keyboard_mode.beacon_interval_secs = beacon_interval_secs;
                cfg.keyboard_mode.listen_ports = collapse_listen_ports(&listen_switches);
            }
            ui.state.save_config();
            ui.reschedule_keyboard_mode_beacon();
            win.close();
        });
    }
    button_row.append(&cancel_button);
    button_row.append(&save_button);
    root.append(&button_row);

    win.present();
}

/// A small Save/Cancel modal for the welcome message, mirroring the
/// mailbox's intro-message editor -- free text gets its own focused editor
/// rather than living inline in the settings form. Writes back into the
/// settings dialog's in-memory `welcome_message`, not config directly; the
/// outer Save/Cancel still governs whether it actually persists.
fn edit_welcome_message(parent: &adw::Window, welcome_row: &adw::ActionRow, welcome_message: &Rc<RefCell<String>>) {
    let (win, root) = dialog_window(parent, "Welcome Message", 480);

    let label = gtk::Label::new(Some(
        "Sent when a station connects for keyboard-to-keyboard. Leave blank to use the default greeting. \
         $$NODE/$$NAME/$$LOC/$$BBSHOME are available; $$NODE is this Node Callsign.",
    ));
    label.set_halign(gtk::Align::Start);
    label.set_wrap(true);
    root.append(&label);

    let text_view = gtk::TextView::builder().wrap_mode(gtk::WrapMode::WordChar).build();
    let buffer = text_view.buffer();
    buffer.set_text(&welcome_message.borrow());
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
        let welcome_row = welcome_row.clone();
        let welcome_message = welcome_message.clone();
        save_button.connect_clicked(move |_| {
            let (start, end) = buffer.bounds();
            let text = buffer.text(&start, &end, false).to_string();
            *welcome_message.borrow_mut() = text.clone();
            welcome_row.set_subtitle(if text.trim().is_empty() {
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
