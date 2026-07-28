use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;

use pr_core::HighlightRule;

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

    // --- Custom Rules: one list of destination-address rules that both
    // highlight matching traffic and (via the bell toggle) can also raise a
    // desktop notification ---
    let rules_group = adw::PreferencesGroup::builder()
        .title("Custom Rules")
        .description("Destination addresses to highlight, e.g. CQ or a digipeater alias \u{2014} tap the bell to also notify on a match")
        .build();

    let rules_list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    rules_list.add_css_class("boxed-list");
    let rules_scrolled = gtk::ScrolledWindow::builder().child(&rules_list).min_content_height(200).vexpand(false).build();
    rules_group.add(&rules_scrolled);

    let rules: Rc<RefCell<Vec<HighlightRule>>> = Rc::new(RefCell::new(current_hl.rules.clone()));
    rebuild_rules_list(&rules_list, &rules);

    let add_rule_button = gtk::Button::with_label("Add Rule\u{2026}");
    add_rule_button.set_margin_top(8);
    add_rule_button.set_halign(gtk::Align::Start);
    {
        let rules = rules.clone();
        let rules_list = rules_list.clone();
        let default_color = current_hl.callsign_color.clone();
        add_rule_button.connect_clicked(move |_| {
            rules.borrow_mut().push(HighlightRule {
                label: "New Rule".to_string(),
                pattern: String::new(),
                color: default_color.clone(),
                notify: false,
                enabled: true,
            });
            rebuild_rules_list(&rules_list, &rules);
        });
    }
    rules_group.add(&add_rule_button);
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
        let rules = rules.clone();
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
                cfg.highlighting.rules = rules.borrow().clone();

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
        gtk::Entry::builder().text(&rule.pattern).hexpand(true).placeholder_text("CQ, WIDE1-1, ...").build();
    {
        let rules = rules.clone();
        pattern_entry.connect_changed(move |e| {
            if let Some(r) = rules.borrow_mut().get_mut(idx) {
                r.pattern = e.text().to_string();
            }
        });
    }
    row.append(&pattern_entry);

    // Bell toggle: also raise a desktop notification on a match, lighting up
    // (accent color, via the same `.notify-rule-active` class pattern as the
    // tab pin toggle) when active.
    let notify_toggle =
        gtk::ToggleButton::builder().icon_name("notifications-symbolic").tooltip_text("Notify on match").build();
    notify_toggle.add_css_class("flat");
    notify_toggle.add_css_class("notify-rule-toggle");
    notify_toggle.set_active(rule.notify);
    if rule.notify {
        notify_toggle.add_css_class("notify-rule-active");
    }
    {
        let rules = rules.clone();
        notify_toggle.connect_toggled(move |btn| {
            if btn.is_active() {
                btn.add_css_class("notify-rule-active");
            } else {
                btn.remove_css_class("notify-rule-active");
            }
            if let Some(r) = rules.borrow_mut().get_mut(idx) {
                r.notify = btn.is_active();
            }
        });
    }
    row.append(&notify_toggle);

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
    button.set_valign(gtk::Align::Center);
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
