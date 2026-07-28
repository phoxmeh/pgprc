//! UI for the Direwolf process feature: a console window (opened by
//! right-clicking the header's Direwolf button) with a read-only live log
//! plus Start/Stop buttons, and a settings dialog for the auto-start toggle
//! and the raw `direwolf.conf` text.

use std::rc::Rc;

use adw::prelude::*;

use crate::ports_dialog::dialog_window;
use crate::window::Ui;

/// Right-click on the Direwolf header button: Direwolf's captured
/// stdout/stderr (read-only, live — the same `gtk::TextBuffer` Direwolf's
/// own process keeps appending to), plus Start/Stop and buttons to save the
/// log or open Settings.
pub fn show_console(ui: &Rc<Ui>) {
    let (win, root) = dialog_window(&ui.window, "Direwolf Console", 640);
    win.set_default_height(420);

    let text_view = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .buffer(ui.direwolf.log_buffer())
        .top_margin(4)
        .bottom_margin(4)
        .left_margin(6)
        .right_margin(6)
        .build();
    text_view.add_css_class("pr-mono");
    let scrolled = gtk::ScrolledWindow::builder().child(&text_view).vexpand(true).hexpand(true).build();
    root.append(&scrolled);

    // Jump to the current end once on open — this buffer keeps growing
    // live while the window stays open, not a point-in-time snapshot.
    let buffer = ui.direwolf.log_buffer();
    let end_mark = buffer.create_mark(None, &buffer.end_iter(), false);
    text_view.scroll_mark_onscreen(&end_mark);

    let button_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);

    let start_button = gtk::Button::with_label("Start");
    start_button.set_sensitive(!ui.direwolf.is_running());
    {
        let ui = ui.clone();
        start_button.connect_clicked(move |_| {
            if let Some(dir) = pr_core::AppConfig::config_dir() {
                let config_text = ui.state.config.borrow().direwolf.config_text.clone();
                ui.direwolf.start(&dir.join("direwolf.conf"), &config_text);
            }
        });
    }
    button_row.append(&start_button);

    let stop_button = gtk::Button::with_label("Stop");
    stop_button.set_sensitive(ui.direwolf.is_running());
    {
        let ui = ui.clone();
        stop_button.connect_clicked(move |_| {
            ui.direwolf.stop();
        });
    }
    button_row.append(&stop_button);

    // Keep Start/Stop's sensitivity live while this window stays open —
    // registered via `glib::WeakRef` (not a strong clone) since
    // `add_on_change` callbacks are never unregistered; a strong ref here
    // would leak this window's buttons every time it's reopened.
    {
        let start_weak = start_button.downgrade();
        let stop_weak = stop_button.downgrade();
        let direwolf = ui.direwolf.clone();
        ui.direwolf.add_on_change(move || {
            let (Some(start), Some(stop)) = (start_weak.upgrade(), stop_weak.upgrade()) else { return };
            start.set_sensitive(!direwolf.is_running());
            stop.set_sensitive(direwolf.is_running());
        });
    }

    let save_button = gtk::Button::with_label("Save Log\u{2026}");
    {
        let ui = ui.clone();
        let win = win.clone();
        save_button.connect_clicked(move |_| {
            crate::export::save_text(&win, "direwolf.log", ui.direwolf.full_log_text(), None);
        });
    }
    button_row.append(&save_button);

    let settings_button = gtk::Button::with_label("Settings\u{2026}");
    {
        let ui = ui.clone();
        settings_button.connect_clicked(move |_| {
            show_settings(&ui);
        });
    }
    button_row.append(&settings_button);
    root.append(&button_row);

    win.present();
}

/// Auto-start checkbox + a plain-text editor for `direwolf.conf`'s full
/// contents — kept as one text blob rather than a structured form, since
/// Direwolf's config format is large/varied and not worth parsing here.
pub fn show_settings(ui: &Rc<Ui>) {
    let (win, root) = dialog_window(&ui.window, "Direwolf Settings", 560);
    win.set_default_height(520);

    let auto_start_check = gtk::CheckButton::with_label("Start Direwolf automatically when this app starts");
    auto_start_check.set_active(ui.state.config.borrow().direwolf.auto_start);
    root.append(&auto_start_check);

    let config_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let config_label = gtk::Label::new(Some("direwolf.conf"));
    config_label.set_hexpand(true);
    config_label.set_halign(gtk::Align::Start);
    config_header.append(&config_label);
    let load_button = gtk::Button::with_label("Load from File\u{2026}");
    config_header.append(&load_button);
    root.append(&config_header);

    let text_view = gtk::TextView::builder().monospace(true).wrap_mode(gtk::WrapMode::None).build();
    let buffer = text_view.buffer();
    buffer.set_text(&ui.state.config.borrow().direwolf.config_text);
    let scrolled =
        gtk::ScrolledWindow::builder().child(&text_view).vexpand(true).min_content_height(320).has_frame(true).build();
    root.append(&scrolled);

    {
        let win = win.clone();
        let buffer = buffer.clone();
        load_button.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::builder().title("Load direwolf.conf").build();
            let buffer = buffer.clone();
            dialog.open(Some(&win), gtk::gio::Cancellable::NONE, move |result| {
                let Ok(file) = result else { return };
                let Some(path) = file.path() else { return };
                match std::fs::read_to_string(&path) {
                    Ok(text) => buffer.set_text(&text),
                    Err(e) => tracing::warn!("failed to read {}: {e}", path.display()),
                }
            });
        });
    }

    let hint = gtk::Label::new(Some(
        "Saved to ~/.config/packet-radio/direwolf.conf and used as-is (\"direwolf -c ...\") when started from the header button.",
    ));
    hint.set_wrap(true);
    hint.set_halign(gtk::Align::Start);
    hint.add_css_class("dim-label");
    hint.add_css_class("caption");
    root.append(&hint);

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
            let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true).to_string();
            let mut cfg = ui.state.config.borrow_mut();
            cfg.direwolf.auto_start = auto_start_check.is_active();
            cfg.direwolf.config_text = text;
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
