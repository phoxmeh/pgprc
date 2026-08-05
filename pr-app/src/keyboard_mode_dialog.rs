use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;

use crate::ports_dialog::{
    base_call, collapse_listen_ports, dialog_window, get_ssid_from_dropdown, make_ssid_dropdown,
    port_listen_checklist, split_call_ssid, ssid_hint_button,
};
use crate::window::Ui;

/// Keyboard-to-keyboard mode settings: welcome message, availability beacon,
/// and which ports to listen on. There's no enable switch here -- that's the
/// header button's own job (left-click toggles it directly); this dialog is
/// only reachable by right-clicking that button.
pub fn show_settings(ui: &Rc<Ui>) {
    let (win, root) = dialog_window(&ui.window, "Keyboard-to-Keyboard Settings", 480);

    let current = ui.state.config.borrow().keyboard_mode.clone();
    let ports = ui.state.config.borrow().ports.clone();

    // Derive the node call sign: base from Profile, SSID editable here.
    let profile_base = base_call(ui.state.config.borrow().ui.default_call.as_deref().unwrap_or(""));
    let current_ssid = split_call_ssid(&current.node_call).1;

    let settings_group = adw::PreferencesGroup::builder()
        .description(
            "The call sign this mode answers as \u{2014} base comes from your Profile, choose an SSID to \
             distinguish it from your other stations. Note: the beacon and connected-mode replies go out \
             under whichever port sends them; for them to appear as this call sign on the air, point \
             \u{201c}Listen On\u{201d} below at a port configured with this same call.",
        )
        .build();

    let node_call_row = adw::ActionRow::builder()
        .title("Node Call Sign")
        .subtitle(if profile_base.is_empty() { "Set your Profile call sign first" } else { "" })
        .build();
    let call_label = gtk::Label::new(Some(&format!("{profile_base}-")));
    call_label.set_valign(gtk::Align::Center);
    call_label.add_css_class("monospace");
    let ssid_dd = make_ssid_dropdown(current_ssid);
    ssid_dd.set_valign(gtk::Align::Center);
    let hint_btn = ssid_hint_button(win.clone());
    let call_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    call_box.set_valign(gtk::Align::Center);
    call_box.append(&call_label);
    call_box.append(&ssid_dd);
    call_box.append(&hint_btn);
    node_call_row.add_suffix(&call_box);
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
        .tooltip_text("$$NODE/$$NAME/$$LOC/$$BBSHOME available; $$NODE is this Node Call Sign")
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
    let (listen_widget, all_ports_switch, listen_switches) =
        port_listen_checklist(&ports, &current.listen_ports);
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
                let pb = base_call(ui.state.config.borrow().ui.default_call.as_deref().unwrap_or(""));
                let node_call = if pb.is_empty() {
                    String::new()
                } else {
                    format!("{}-{}", pb, get_ssid_from_dropdown(&ssid_dd))
                };
                let mut cfg = ui.state.config.borrow_mut();
                cfg.keyboard_mode.node_call = node_call;
                cfg.keyboard_mode.welcome_message = welcome_message.borrow().clone();
                cfg.keyboard_mode.beacon_text = beacon_text_row.text().to_string();
                cfg.keyboard_mode.beacon_interval_secs = beacon_interval_secs;
                cfg.keyboard_mode.listen_ports =
                    collapse_listen_ports(&all_ports_switch, &listen_switches);
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
         $$NODE/$$NAME/$$LOC/$$BBSHOME are available; $$NODE is this Node Call Sign.",
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
