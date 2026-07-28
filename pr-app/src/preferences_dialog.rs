use std::rc::Rc;

use adw::prelude::*;

use crate::ports_dialog::dialog_window;
use crate::window::{apply_font, Ui};

pub fn show(ui: &Rc<Ui>) {
    let (win, root) = dialog_window(&ui.window, "Preferences", 480);
    win.set_default_height(640);

    let scrolled = gtk::ScrolledWindow::builder().vexpand(true).build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
    scrolled.set_child(Some(&content));

    let current = ui.state.config.borrow().ui.clone();
    let current_hl = ui.state.config.borrow().highlighting.clone();
    let current_mailbox_enabled = ui.state.config.borrow().mailbox.enabled;
    let current_notify_enabled = ui.state.config.borrow().notify.enabled;

    // --- General ---
    let general_group = adw::PreferencesGroup::builder().title("General").build();

    let font_row = adw::EntryRow::builder().title("Font").build();
    font_row.set_text(current.font.as_deref().unwrap_or("Monospace 11"));
    general_group.add(&font_row);

    let timestamps_row = adw::SwitchRow::builder().title("Show Timestamps in Monitor").active(current.show_timestamps).build();
    general_group.add(&timestamps_row);

    let default_call_row = adw::EntryRow::builder().title("Default Callsign").build();
    default_call_row.set_text(current.default_call.as_deref().unwrap_or(""));
    general_group.add(&default_call_row);

    let history_lines_row = adw::EntryRow::builder().title("History Lines").build();
    history_lines_row.set_text(&current.history_lines.to_string());
    general_group.add(&history_lines_row);

    content.append(&general_group);

    // --- QRZ Lookup ---
    let qrz_group = adw::PreferencesGroup::builder()
        .title("QRZ Lookup")
        .description("Used by \u{201c}Lookup QRZ\u{2026}\u{201d} in the Address Book")
        .build();

    let qrz_user_row = adw::EntryRow::builder().title("Username").build();
    qrz_user_row.set_text(current.qrz_username.as_deref().unwrap_or(""));
    qrz_group.add(&qrz_user_row);

    let qrz_pass_row = adw::PasswordEntryRow::builder().title("Password").build();
    if let Some(pass) = &current.qrz_password {
        qrz_pass_row.set_text(pass);
    }
    qrz_group.add(&qrz_pass_row);

    content.append(&qrz_group);

    // --- Personal Mailbox ---
    let mailbox_group = adw::PreferencesGroup::builder().title("Personal Mailbox").build();
    let mailbox_enabled_row = adw::SwitchRow::builder()
        .title("Enable Mailbox")
        .subtitle("Answer unsolicited connections with a BBS-style prompt (local only)")
        .active(current_mailbox_enabled)
        .build();
    mailbox_group.add(&mailbox_enabled_row);
    content.append(&mailbox_group);

    // --- Notifications ---
    let notify_group = adw::PreferencesGroup::builder().title("Notifications").build();
    let notify_enabled_row = adw::SwitchRow::builder()
        .title("Enable Notifications")
        .subtitle("Desktop notification for an incoming connection, a frame directed to your callsign, or a Destination Rule")
        .active(current_notify_enabled)
        .build();
    notify_group.add(&notify_enabled_row);
    content.append(&notify_group);

    // --- Highlighting ---
    let hl_group = adw::PreferencesGroup::builder().title("Highlighting").build();

    let hl_enabled_row = adw::SwitchRow::builder().title("Enable Scrollback Highlighting").active(current_hl.enabled).build();
    hl_group.add(&hl_enabled_row);

    let callsign_color_btn = color_button(&current_hl.callsign_color);
    let callsign_color_row = adw::ActionRow::builder().title("Callsigns").build();
    callsign_color_row.add_suffix(&callsign_color_btn);
    hl_group.add(&callsign_color_row);

    let known_color_btn = color_button(&current_hl.known_callsign_color);
    let known_color_row = adw::ActionRow::builder().title("Known Callsigns").build();
    known_color_row.add_suffix(&known_color_btn);
    hl_group.add(&known_color_row);

    let my_call_color_btn = color_button(&current_hl.my_call_color);
    let my_call_color_row = adw::ActionRow::builder()
        .title("My Callsign")
        .subtitle("Matches the Default Callsign above, wherever it's mentioned")
        .build();
    my_call_color_row.add_suffix(&my_call_color_btn);
    hl_group.add(&my_call_color_row);

    let ax25_color_btn = color_button(&current_hl.ax25_command_color);
    let ax25_color_row = adw::ActionRow::builder().title("AX.25 Command Tags").build();
    ax25_color_row.add_suffix(&ax25_color_btn);
    hl_group.add(&ax25_color_row);

    content.append(&hl_group);

    // --- Custom Rules (its own dialog, since editing a list of keyword/
    // destination rules needs a lot more room than fits comfortably here) ---
    let rules_group = adw::PreferencesGroup::builder()
        .title("Custom Rules")
        .description("Keyword highlighting and notification destination rules share one editor")
        .build();
    let rules_row = adw::ActionRow::builder()
        .title("Highlighting \u{0026} Notification Rules")
        .subtitle("Add, edit, or remove keyword and destination rules")
        .build();
    let manage_rules_button = gtk::Button::with_label("Manage\u{2026}");
    manage_rules_button.set_valign(gtk::Align::Center);
    {
        let ui = ui.clone();
        manage_rules_button.connect_clicked(move |_| {
            crate::rules_dialog::show(&ui);
        });
    }
    rules_row.add_suffix(&manage_rules_button);
    rules_group.add(&rules_row);
    content.append(&rules_group);

    root.append(&scrolled);

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
        save_button.connect_clicked(move |_| {
            let font = font_row.text().to_string();
            let show_timestamps = timestamps_row.is_active();
            let default_call = default_call_row.text().to_string();
            let qrz_username = qrz_user_row.text().to_string();
            let qrz_password = qrz_pass_row.text().to_string();
            let history_lines = match history_lines_row.text().trim().parse::<u32>() {
                Ok(n) if n > 0 => n,
                _ => {
                    error_label.set_text("History Lines must be a positive number.");
                    return;
                }
            };

            {
                let mut cfg = ui.state.config.borrow_mut();
                cfg.ui.font = if font.trim().is_empty() { None } else { Some(font.clone()) };
                cfg.ui.show_timestamps = show_timestamps;
                cfg.ui.default_call =
                    if default_call.trim().is_empty() { None } else { Some(default_call.to_uppercase()) };
                cfg.ui.qrz_username = if qrz_username.trim().is_empty() { None } else { Some(qrz_username) };
                cfg.ui.qrz_password = if qrz_password.is_empty() { None } else { Some(qrz_password) };
                cfg.ui.history_lines = history_lines;

                cfg.highlighting.enabled = hl_enabled_row.is_active();
                cfg.highlighting.callsign_color = rgba_to_hex(&callsign_color_btn.rgba());
                cfg.highlighting.known_callsign_color = rgba_to_hex(&known_color_btn.rgba());
                cfg.highlighting.my_call_color = rgba_to_hex(&my_call_color_btn.rgba());
                cfg.highlighting.ax25_command_color = rgba_to_hex(&ax25_color_btn.rgba());

                cfg.mailbox.enabled = mailbox_enabled_row.is_active();

                cfg.notify.enabled = notify_enabled_row.is_active();
            }
            // Credentials may have changed; force a fresh login next lookup.
            *ui.state.qrz_session.borrow_mut() = None;
            ui.state.save_config();

            apply_font(&font);
            ui.monitor.set_show_timestamps(show_timestamps);

            win.close();
        });
    }
    button_row.append(&cancel_button);
    button_row.append(&save_button);
    root.append(&button_row);

    win.present();
}

pub(crate) fn color_button(hex: &str) -> gtk::ColorDialogButton {
    let button = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::new()));
    button.set_valign(gtk::Align::Center);
    if let Ok(rgba) = gtk::gdk::RGBA::parse(hex) {
        button.set_rgba(&rgba);
    }
    button
}

pub(crate) fn rgba_to_hex(rgba: &gtk::gdk::RGBA) -> String {
    format!(
        "#{:02X}{:02X}{:02X}",
        (rgba.red() * 255.0).round() as u8,
        (rgba.green() * 255.0).round() as u8,
        (rgba.blue() * 255.0).round() as u8,
    )
}
