use std::rc::Rc;

use adw::prelude::*;

use crate::ports_dialog::{dialog_window, labeled, labeled_widget};
use crate::window::{apply_font, Ui};

pub fn show(ui: &Rc<Ui>) {
    let (win, root) = dialog_window(&ui.window, "Preferences", 420);

    let current = ui.state.config.borrow().ui.clone();

    let font_entry = gtk::Entry::builder()
        .placeholder_text("Monospace 11")
        .text(current.font.as_deref().unwrap_or("Monospace 11"))
        .build();
    root.append(&labeled("Font", &font_entry));

    let timestamps_check = gtk::CheckButton::with_label("Show timestamps in Monitor");
    timestamps_check.set_active(current.show_timestamps);
    root.append(&timestamps_check);

    let default_call_entry = gtk::Entry::builder()
        .placeholder_text("MYCALL-1 (optional)")
        .text(current.default_call.as_deref().unwrap_or(""))
        .build();
    root.append(&labeled("Default Callsign", &default_call_entry));

    let qrz_user_entry = gtk::Entry::builder()
        .placeholder_text("QRZ username (optional)")
        .text(current.qrz_username.as_deref().unwrap_or(""))
        .build();
    root.append(&labeled("QRZ Username", &qrz_user_entry));

    let qrz_pass_entry = gtk::PasswordEntry::builder().show_peek_icon(true).build();
    if let Some(pass) = &current.qrz_password {
        qrz_pass_entry.set_text(pass);
    }
    root.append(&labeled_widget("QRZ Password", qrz_pass_entry.clone().upcast()));

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
            let font = font_entry.text().to_string();
            let show_timestamps = timestamps_check.is_active();
            let default_call = default_call_entry.text().to_string();
            let qrz_username = qrz_user_entry.text().to_string();
            let qrz_password = qrz_pass_entry.text().to_string();

            {
                let mut cfg = ui.state.config.borrow_mut();
                cfg.ui.font = if font.trim().is_empty() { None } else { Some(font.clone()) };
                cfg.ui.show_timestamps = show_timestamps;
                cfg.ui.default_call =
                    if default_call.trim().is_empty() { None } else { Some(default_call.to_uppercase()) };
                cfg.ui.qrz_username = if qrz_username.trim().is_empty() { None } else { Some(qrz_username) };
                cfg.ui.qrz_password = if qrz_password.is_empty() { None } else { Some(qrz_password) };
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
