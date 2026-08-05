use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;

use pr_core::HighlightRule;

use crate::ports_dialog::{
    dialog_window, force_alphanumeric_uppercase, force_uppercase, get_ssid_from_dropdown,
    make_ssid_dropdown, split_call_ssid, ssid_hint_button,
};
use crate::window::{apply_font, Ui};

pub fn show(ui: &Rc<Ui>) {
    let (win, root) = dialog_window(&ui.window, "Preferences", 480);
    win.set_default_height(640);

    let scrolled = gtk::ScrolledWindow::builder().vexpand(true).build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
    scrolled.set_child(Some(&content));

    let current = ui.state.config.borrow().ui.clone();
    let current_hl = ui.state.config.borrow().highlighting.clone();
    let current_notify = ui.state.config.borrow().notify.clone();

    // --- Profile: who you are, on every port you connect. Callsign lives
    // here (not General) since it's identity, not a display preference. ---
    let profile_group = adw::PreferencesGroup::builder()
        .title("Profile")
        .description(
            "Available as $$NAME, $$LOC, $$BBSHOME in mailbox/keyboard-to-keyboard/beacon message text \
             ($$NODE resolves separately for each of those)",
        )
        .build();

    let name_row = adw::EntryRow::builder().title("Name").build();
    name_row.set_text(current.name.as_deref().unwrap_or(""));
    profile_group.add(&name_row);

    let (call_base, call_ssid) = split_call_ssid(current.default_call.as_deref().unwrap_or(""));
    let call_sign_row = adw::EntryRow::builder().title("Call Sign").build();
    call_sign_row.set_text(&call_base);
    force_alphanumeric_uppercase(&call_sign_row);
    let call_ssid_dd = make_ssid_dropdown(call_ssid);
    call_ssid_dd.set_valign(gtk::Align::Center);
    let ssid_hint_btn = ssid_hint_button(win.clone());
    let call_suffix_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    call_suffix_box.set_valign(gtk::Align::Center);
    call_suffix_box.append(&gtk::Label::new(Some("-")));
    call_suffix_box.append(&call_ssid_dd);
    call_suffix_box.append(&ssid_hint_btn);
    call_sign_row.add_suffix(&call_suffix_box);
    profile_group.add(&call_sign_row);

    let location_row = adw::EntryRow::builder().title("Location").build();
    location_row.set_text(current.location.as_deref().unwrap_or(""));
    profile_group.add(&location_row);

    let home_bbs_row = adw::EntryRow::builder().title("Home BBS Address").build();
    home_bbs_row.set_text(current.home_bbs.as_deref().unwrap_or(""));
    force_uppercase(&home_bbs_row);
    profile_group.add(&home_bbs_row);

    content.append(&profile_group);

    // --- General ---
    let general_group = adw::PreferencesGroup::builder().title("General").build();

    let font_row = adw::EntryRow::builder().title("Font").build();
    font_row.set_text(current.font.as_deref().unwrap_or("Monospace 11"));
    general_group.add(&font_row);

    let timestamps_row = adw::SwitchRow::builder().title("Show Timestamps in Monitor").active(current.show_timestamps).build();
    general_group.add(&timestamps_row);

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

    // --- Personal Mailbox: enable toggle, respond-from callsign, and intro
    // message all live in the Mailbox window itself now (Header menu ->
    // Mailbox), alongside the message list, instead of here. ---

    // --- Notifications: directed-at-me toggle only. Sound and silence are
    // configured in the Notifications dialog (header bell button). ---
    let notify_group = adw::PreferencesGroup::builder().title("Notifications").build();
    let notify_directed_row = adw::SwitchRow::builder()
        .title("Directed Notifications")
        .subtitle("An incoming connection, or a frame directed to your call sign")
        .active(current_notify.directed_enabled)
        .build();
    notify_group.add(&notify_directed_row);
    content.append(&notify_group);

    // --- Highlighting ---
    let hl_group = adw::PreferencesGroup::builder().title("Highlighting").build();

    let hl_enabled_row = adw::SwitchRow::builder().title("Enable Scrollback Highlighting").active(current_hl.enabled).build();
    hl_group.add(&hl_enabled_row);

    let callsign_color_btn = color_button(&current_hl.callsign_color);
    let callsign_color_row = adw::ActionRow::builder().title("Call Signs").build();
    callsign_color_row.add_suffix(&callsign_color_btn);
    hl_group.add(&callsign_color_row);

    let known_color_btn = color_button(&current_hl.known_callsign_color);
    let known_color_row = adw::ActionRow::builder().title("Known Call Signs").build();
    known_color_row.add_suffix(&known_color_btn);
    hl_group.add(&known_color_row);

    let my_call_color_btn = color_button(&current_hl.my_call_color);
    let my_call_color_row = adw::ActionRow::builder()
        .title("My Call Sign")
        .subtitle("Matches the Profile Call Sign above, wherever it's mentioned")
        .build();
    my_call_color_row.add_suffix(&my_call_color_btn);
    hl_group.add(&my_call_color_row);

    let ax25_color_btn = color_button(&current_hl.ax25_command_color);
    let ax25_color_row = adw::ActionRow::builder().title("AX.25 Command Tags").build();
    ax25_color_row.add_suffix(&ax25_color_btn);
    hl_group.add(&ax25_color_row);

    content.append(&hl_group);

    // --- Custom Rules: destination-address rules that highlight matching
    // traffic. Managed here, used by the Monitor and session scrollbacks. ---
    let rules_group = adw::PreferencesGroup::builder()
        .title("Custom Rules")
        .description("Destination addresses to highlight, e.g. CQ or a digipeater alias")
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
            let name = name_row.text().to_string();
            let call_base = call_sign_row.text().trim().to_uppercase();
            let default_call = if call_base.is_empty() {
                String::new()
            } else {
                format!("{}-{}", call_base, get_ssid_from_dropdown(&call_ssid_dd))
            };
            let location = location_row.text().to_string();
            let home_bbs = home_bbs_row.text().to_string();
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
                cfg.ui.name = if name.trim().is_empty() { None } else { Some(name) };
                cfg.ui.default_call = if default_call.is_empty() { None } else { Some(default_call) };
                cfg.ui.location = if location.trim().is_empty() { None } else { Some(location) };
                cfg.ui.home_bbs = if home_bbs.trim().is_empty() { None } else { Some(home_bbs.to_uppercase()) };
                cfg.ui.qrz_username = if qrz_username.trim().is_empty() { None } else { Some(qrz_username) };
                cfg.ui.qrz_password = if qrz_password.is_empty() { None } else { Some(qrz_password) };
                cfg.ui.history_lines = history_lines;

                cfg.highlighting.enabled = hl_enabled_row.is_active();
                cfg.highlighting.callsign_color = rgba_to_hex(&callsign_color_btn.rgba());
                cfg.highlighting.known_callsign_color = rgba_to_hex(&known_color_btn.rgba());
                cfg.highlighting.my_call_color = rgba_to_hex(&my_call_color_btn.rgba());
                cfg.highlighting.ax25_command_color = rgba_to_hex(&ax25_color_btn.rgba());
                cfg.highlighting.rules = rules.borrow().clone();

                cfg.notify.directed_enabled = notify_directed_row.is_active();
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
