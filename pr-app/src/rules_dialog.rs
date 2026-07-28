//! Combined editor for the app's two "list of custom match rules"
//! preferences — scrollback highlighting keywords and notification
//! destination rules. Given their own dialog (opened from Preferences)
//! instead of two small lists cramped inside the main Preferences window,
//! so there's real room to add/edit/reorder rules.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;

use pr_core::{HighlightRule, NotifyRule};

use crate::ports_dialog::dialog_window;
use crate::preferences_dialog::{color_button, rgba_to_hex};
use crate::window::Ui;

pub fn show(ui: &Rc<Ui>) {
    let (win, root) = dialog_window(&ui.window, "Custom Rules", 560);
    win.set_default_height(720);

    let scrolled = gtk::ScrolledWindow::builder().vexpand(true).build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
    scrolled.set_child(Some(&content));

    let current_hl_rules = ui.state.config.borrow().highlighting.rules.clone();
    let current_notify_rules = ui.state.config.borrow().notify.rules.clone();

    // --- Keyword / Bulletin Highlighting ---
    let hl_group = adw::PreferencesGroup::builder()
        .title("Keyword / Bulletin Highlighting")
        .description("e.g. CQ, BEACON, or your own nets/bulletins")
        .build();

    let hl_list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    hl_list.add_css_class("boxed-list");
    let hl_scrolled = gtk::ScrolledWindow::builder().child(&hl_list).min_content_height(260).vexpand(false).build();
    hl_group.add(&hl_scrolled);

    let hl_rules: Rc<RefCell<Vec<HighlightRule>>> = Rc::new(RefCell::new(current_hl_rules));
    rebuild_hl_rules_list(&hl_list, &hl_rules);

    let add_hl_rule_button = gtk::Button::with_label("Add Rule\u{2026}");
    add_hl_rule_button.set_margin_top(8);
    add_hl_rule_button.set_halign(gtk::Align::Start);
    {
        let hl_rules = hl_rules.clone();
        let hl_list = hl_list.clone();
        add_hl_rule_button.connect_clicked(move |_| {
            hl_rules.borrow_mut().push(HighlightRule {
                label: "New Rule".to_string(),
                pattern: String::new(),
                regex: false,
                color: "#FFD700".to_string(),
                enabled: true,
            });
            rebuild_hl_rules_list(&hl_list, &hl_rules);
        });
    }
    hl_group.add(&add_hl_rule_button);
    content.append(&hl_group);

    // --- Notification Destination Rules ---
    let notify_group = adw::PreferencesGroup::builder()
        .title("Notification Destination Rules")
        .description("Notify on other destinations too, e.g. a bulletin address or a callsign you watch for")
        .build();

    let notify_list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    notify_list.add_css_class("boxed-list");
    let notify_scrolled =
        gtk::ScrolledWindow::builder().child(&notify_list).min_content_height(220).vexpand(false).build();
    notify_group.add(&notify_scrolled);

    let notify_rules: Rc<RefCell<Vec<NotifyRule>>> = Rc::new(RefCell::new(current_notify_rules));
    rebuild_notify_rules_list(&notify_list, &notify_rules);

    let add_notify_rule_button = gtk::Button::with_label("Add Rule\u{2026}");
    add_notify_rule_button.set_margin_top(8);
    add_notify_rule_button.set_halign(gtk::Align::Start);
    {
        let notify_rules = notify_rules.clone();
        let notify_list = notify_list.clone();
        add_notify_rule_button.connect_clicked(move |_| {
            notify_rules.borrow_mut().push(NotifyRule {
                label: "New Rule".to_string(),
                pattern: String::new(),
                regex: false,
                enabled: true,
            });
            rebuild_notify_rules_list(&notify_list, &notify_rules);
        });
    }
    notify_group.add(&add_notify_rule_button);
    content.append(&notify_group);

    root.append(&scrolled);

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
            let mut cfg = ui.state.config.borrow_mut();
            cfg.highlighting.rules = hl_rules.borrow().clone();
            cfg.notify.rules = notify_rules.borrow().clone();
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

fn rebuild_hl_rules_list(list_box: &gtk::ListBox, rules: &Rc<RefCell<Vec<HighlightRule>>>) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    let len = rules.borrow().len();
    for idx in 0..len {
        list_box.append(&build_hl_rule_row(list_box, rules, idx));
    }
}

fn build_hl_rule_row(list_box: &gtk::ListBox, rules: &Rc<RefCell<Vec<HighlightRule>>>, idx: usize) -> gtk::Widget {
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
            rebuild_hl_rules_list(&list_box, &rules);
        });
    }
    row.append(&remove_button);

    row.upcast()
}

fn rebuild_notify_rules_list(list_box: &gtk::ListBox, rules: &Rc<RefCell<Vec<NotifyRule>>>) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    let len = rules.borrow().len();
    for idx in 0..len {
        list_box.append(&build_notify_rule_row(list_box, rules, idx));
    }
}

fn build_notify_rule_row(list_box: &gtk::ListBox, rules: &Rc<RefCell<Vec<NotifyRule>>>, idx: usize) -> gtk::Widget {
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
        gtk::Entry::builder().text(&rule.pattern).hexpand(true).placeholder_text("WIDE1-1, CQ, ...").build();
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

    let remove_button = gtk::Button::with_label("\u{2715}");
    remove_button.add_css_class("flat");
    {
        let rules = rules.clone();
        let list_box = list_box.clone();
        remove_button.connect_clicked(move |_| {
            rules.borrow_mut().remove(idx);
            rebuild_notify_rules_list(&list_box, &rules);
        });
    }
    row.append(&remove_button);

    row.upcast()
}
