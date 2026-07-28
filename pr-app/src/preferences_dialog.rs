use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;

use pr_core::HighlightRule;

use crate::ports_dialog::{dialog_window, labeled, labeled_widget};
use crate::window::{apply_font, Ui};

pub fn show(ui: &Rc<Ui>) {
    let (win, root) = dialog_window(&ui.window, "Preferences", 480);
    win.set_default_height(600);

    let scrolled = gtk::ScrolledWindow::builder().vexpand(true).build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    scrolled.set_child(Some(&content));

    let current = ui.state.config.borrow().ui.clone();
    let current_hl = ui.state.config.borrow().highlighting.clone();

    let font_entry = gtk::Entry::builder()
        .placeholder_text("Monospace 11")
        .text(current.font.as_deref().unwrap_or("Monospace 11"))
        .build();
    content.append(&labeled("Font", &font_entry));

    let timestamps_check = gtk::CheckButton::with_label("Show timestamps in Monitor");
    timestamps_check.set_active(current.show_timestamps);
    content.append(&timestamps_check);

    let default_call_entry = gtk::Entry::builder()
        .placeholder_text("MYCALL-1 (optional)")
        .text(current.default_call.as_deref().unwrap_or(""))
        .build();
    content.append(&labeled("Default Callsign", &default_call_entry));

    let qrz_user_entry = gtk::Entry::builder()
        .placeholder_text("QRZ username (optional)")
        .text(current.qrz_username.as_deref().unwrap_or(""))
        .build();
    content.append(&labeled("QRZ Username", &qrz_user_entry));

    let qrz_pass_entry = gtk::PasswordEntry::builder().show_peek_icon(true).build();
    if let Some(pass) = &current.qrz_password {
        qrz_pass_entry.set_text(pass);
    }
    content.append(&labeled_widget("QRZ Password", qrz_pass_entry.clone().upcast()));

    let history_lines_entry = gtk::Entry::builder().text(current.history_lines.to_string()).build();
    content.append(&labeled("History Lines", &history_lines_entry));

    content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let hl_heading = gtk::Label::new(Some("Highlighting"));
    hl_heading.add_css_class("heading");
    hl_heading.set_halign(gtk::Align::Start);
    content.append(&hl_heading);

    let hl_enabled_check = gtk::CheckButton::with_label("Enable scrollback highlighting");
    hl_enabled_check.set_active(current_hl.enabled);
    content.append(&hl_enabled_check);

    let callsign_color_btn = color_button(&current_hl.callsign_color);
    content.append(&labeled_widget("Callsigns", callsign_color_btn.clone().upcast()));

    let known_color_btn = color_button(&current_hl.known_callsign_color);
    content.append(&labeled_widget("Known Callsigns", known_color_btn.clone().upcast()));

    let ax25_color_btn = color_button(&current_hl.ax25_command_color);
    content.append(&labeled_widget("AX.25 Command Tags", ax25_color_btn.clone().upcast()));

    let rules_heading = gtk::Label::new(Some("Keyword / Bulletin Rules (e.g. CQ, BEACON, or your own)"));
    rules_heading.set_halign(gtk::Align::Start);
    content.append(&rules_heading);

    let rules_list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    rules_list.add_css_class("boxed-list");
    let rules_scrolled =
        gtk::ScrolledWindow::builder().child(&rules_list).min_content_height(160).vexpand(false).build();
    content.append(&rules_scrolled);

    let rules: Rc<RefCell<Vec<HighlightRule>>> = Rc::new(RefCell::new(current_hl.rules.clone()));
    rebuild_rules_list(&rules_list, &rules);

    let add_rule_button = gtk::Button::with_label("Add Rule\u{2026}");
    {
        let rules = rules.clone();
        let rules_list = rules_list.clone();
        add_rule_button.connect_clicked(move |_| {
            rules.borrow_mut().push(HighlightRule {
                label: "New Rule".to_string(),
                pattern: String::new(),
                regex: false,
                color: "#FFD700".to_string(),
                enabled: true,
            });
            rebuild_rules_list(&rules_list, &rules);
        });
    }
    content.append(&add_rule_button);

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
        let rules = rules.clone();
        save_button.connect_clicked(move |_| {
            let font = font_entry.text().to_string();
            let show_timestamps = timestamps_check.is_active();
            let default_call = default_call_entry.text().to_string();
            let qrz_username = qrz_user_entry.text().to_string();
            let qrz_password = qrz_pass_entry.text().to_string();
            let history_lines = match history_lines_entry.text().trim().parse::<u32>() {
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

                cfg.highlighting.enabled = hl_enabled_check.is_active();
                cfg.highlighting.callsign_color = rgba_to_hex(&callsign_color_btn.rgba());
                cfg.highlighting.known_callsign_color = rgba_to_hex(&known_color_btn.rgba());
                cfg.highlighting.ax25_command_color = rgba_to_hex(&ax25_color_btn.rgba());
                cfg.highlighting.rules = rules.borrow().clone();
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

fn rebuild_rules_list(list_box: &gtk::ListBox, rules: &Rc<RefCell<Vec<HighlightRule>>>) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    let len = rules.borrow().len();
    for idx in 0..len {
        list_box.append(&build_rule_row(list_box, rules, idx));
    }
}

fn build_rule_row(list_box: &gtk::ListBox, rules: &Rc<RefCell<Vec<HighlightRule>>>, idx: usize) -> gtk::Widget {
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
        gtk::Entry::builder().text(&rule.pattern).hexpand(true).placeholder_text("CQ, BEACON, ...").build();
    {
        let rules = rules.clone();
        pattern_entry.connect_changed(move |e| {
            if let Some(r) = rules.borrow_mut().get_mut(idx) {
                r.pattern = e.text().to_string();
            }
        });
    }
    row.append(&pattern_entry);

    let regex_check = gtk::CheckButton::with_label("Regex");
    regex_check.set_active(rule.regex);
    {
        let rules = rules.clone();
        regex_check.connect_toggled(move |btn| {
            if let Some(r) = rules.borrow_mut().get_mut(idx) {
                r.regex = btn.is_active();
            }
        });
    }
    row.append(&regex_check);

    let color_btn = color_button(&rule.color);
    {
        let rules = rules.clone();
        color_btn.connect_rgba_notify(move |btn| {
            if let Some(r) = rules.borrow_mut().get_mut(idx) {
                r.color = rgba_to_hex(&btn.rgba());
            }
        });
    }
    row.append(&color_btn);

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

fn color_button(hex: &str) -> gtk::ColorDialogButton {
    let button = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::new()));
    if let Ok(rgba) = gtk::gdk::RGBA::parse(hex) {
        button.set_rgba(&rgba);
    }
    button
}

fn rgba_to_hex(rgba: &gtk::gdk::RGBA) -> String {
    format!(
        "#{:02X}{:02X}{:02X}",
        (rgba.red() * 255.0).round() as u8,
        (rgba.green() * 255.0).round() as u8,
        (rgba.blue() * 255.0).round() as u8,
    )
}
