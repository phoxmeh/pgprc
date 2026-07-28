use std::rc::Rc;

use adw::prelude::*;
use gtk::glib::object::IsA;

use pr_core::{AgwpeLogin, AppConfig, KissParams, PortConfig, PortEntry};

use crate::window::Ui;

/// Build a modal dialog window with a native header bar (so it always has a
/// title and a close button) and return it along with its content box.
pub(crate) fn dialog_window(parent: &impl IsA<gtk::Window>, title: &str, width: i32) -> (adw::Window, gtk::Box) {
    let win = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title(title)
        .default_width(width)
        .build();

    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_top(12);
    root.set_margin_bottom(12);
    root.set_margin_start(12);
    root.set_margin_end(12);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&root));
    win.set_content(Some(&toolbar));

    (win, root)
}

fn next_id(config: &AppConfig) -> String {
    let mut n = config.ports.len();
    loop {
        let candidate = format!("port-{n}");
        if !config.ports.iter().any(|p| p.id == candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Open the Port Manager: list configured ports, add/edit/remove them, and
/// connect/disconnect each one.
pub fn show(ui: &Rc<Ui>) {
    let (win, root) = dialog_window(&ui.window, "Ports", 560);
    win.set_default_height(420);

    let list_box = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    list_box.add_css_class("boxed-list");
    let scrolled = gtk::ScrolledWindow::builder()
        .child(&list_box)
        .vexpand(true)
        .min_content_height(240)
        .build();
    root.append(&scrolled);

    rebuild_list(ui, &list_box);

    let add_button = gtk::Button::with_label("Add Port\u{2026}");
    {
        let ui = ui.clone();
        let win = win.clone();
        let list_box = list_box.clone();
        add_button.connect_clicked(move |_| {
            edit_port_dialog(&ui, &win, None, &list_box);
        });
    }
    root.append(&add_button);

    win.present();
}

fn rebuild_list(ui: &Rc<Ui>, list_box: &gtk::ListBox) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    let entries: Vec<PortEntry> = ui.state.config.borrow().ports.clone();
    for entry in entries {
        list_box.append(&build_port_row(ui, entry, list_box));
    }
}

fn build_port_row(ui: &Rc<Ui>, entry: PortEntry, list_box: &gtk::ListBox) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_margin_top(6);
    row.set_margin_bottom(6);
    row.set_margin_start(6);
    row.set_margin_end(6);

    let label = gtk::Label::new(Some(&format!("{}  \u{2014}  {}", entry.name, entry.config.kind_label())));
    label.set_hexpand(true);
    label.set_halign(gtk::Align::Start);
    row.append(&label);

    let is_active = ui.state.is_active(&entry.id);
    let conn_button = gtk::Button::with_label(if is_active { "Disconnect" } else { "Connect" });
    {
        let ui = ui.clone();
        let id = entry.id.clone();
        conn_button.connect_clicked(move |btn| {
            if btn.label().as_deref() == Some("Connect") {
                ui.connect_port(&id);
                btn.set_label("Disconnect");
            } else {
                ui.disconnect_port(&id);
                btn.set_label("Connect");
            }
        });
    }
    row.append(&conn_button);

    let edit_button = gtk::Button::with_label("Edit\u{2026}");
    {
        let ui = ui.clone();
        let entry = entry.clone();
        let list_box = list_box.clone();
        edit_button.connect_clicked(move |btn| {
            let win = btn
                .root()
                .and_then(|r| r.downcast::<adw::Window>().ok())
                .expect("row is inside a Window");
            edit_port_dialog(&ui, &win, Some(entry.clone()), &list_box);
        });
    }
    row.append(&edit_button);

    let remove_button = gtk::Button::with_label("Remove");
    {
        let ui = ui.clone();
        let id = entry.id.clone();
        let list_box = list_box.clone();
        remove_button.connect_clicked(move |_| {
            ui.disconnect_port(&id);
            ui.state.config.borrow_mut().ports.retain(|p| p.id != id);
            ui.state.save_config();
            rebuild_list(&ui, &list_box);
        });
    }
    row.append(&remove_button);

    row.upcast()
}

const KIND_NAMES: [&str; 6] =
    ["Telnet", "SSH", "AGWPE", "AX.25 raw socket", "KISS (TCP)", "KISS (Serial)"];

fn edit_port_dialog(ui: &Rc<Ui>, parent: &adw::Window, existing: Option<PortEntry>, list_box: &gtk::ListBox) {
    let (win, root) = dialog_window(parent, if existing.is_some() { "Edit Port" } else { "Add Port" }, 420);

    let name_entry = gtk::Entry::builder().placeholder_text("Port name").build();
    if let Some(e) = &existing {
        name_entry.set_text(&e.name);
    }
    root.append(&labeled("Name", &name_entry));

    let kind_model = gtk::StringList::new(&KIND_NAMES);
    let kind_dropdown = gtk::DropDown::builder().model(&kind_model).build();
    root.append(&labeled("Kind", &kind_dropdown));

    let stack = gtk::Stack::new();

    // Telnet
    let telnet_host = gtk::Entry::builder().placeholder_text("host").build();
    let telnet_port = gtk::Entry::builder().placeholder_text("port").text("23").build();
    let telnet_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    telnet_box.append(&labeled("Host", &telnet_host));
    telnet_box.append(&labeled("Port", &telnet_port));
    stack.add_named(&telnet_box, Some("Telnet"));

    // SSH
    let ssh_host = gtk::Entry::builder().placeholder_text("host").build();
    let ssh_port = gtk::Entry::builder().placeholder_text("port").text("22").build();
    let ssh_user = gtk::Entry::builder().placeholder_text("username").build();
    let ssh_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    ssh_box.append(&labeled("Host", &ssh_host));
    ssh_box.append(&labeled("Port", &ssh_port));
    ssh_box.append(&labeled("User", &ssh_user));
    stack.add_named(&ssh_box, Some("SSH"));

    // AGWPE
    let agw_host = gtk::Entry::builder().placeholder_text("host").text("127.0.0.1").build();
    let agw_port = gtk::Entry::builder().placeholder_text("port").text("8000").build();
    let agw_radio_port = gtk::Entry::builder().placeholder_text("radio port").text("0").build();
    let agw_my_call = gtk::Entry::builder().placeholder_text("MYCALL-1").build();
    let agw_user = gtk::Entry::builder().placeholder_text("username (optional)").build();
    let agw_pass = gtk::PasswordEntry::builder().show_peek_icon(true).build();
    let agw_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    agw_box.append(&labeled("Host", &agw_host));
    agw_box.append(&labeled("TCP Port", &agw_port));
    agw_box.append(&labeled("Radio Port", &agw_radio_port));
    agw_box.append(&labeled("My Callsign", &agw_my_call));
    agw_box.append(&labeled("Login User", &agw_user));
    agw_box.append(&labeled_widget("Login Password", agw_pass.clone().upcast()));
    stack.add_named(&agw_box, Some("AGWPE"));

    // AX.25 raw socket
    let ax25_device = gtk::Entry::builder().placeholder_text("axports device name, e.g. wl2k").build();
    let ax25_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    ax25_box.append(&labeled("Device", &ax25_device));
    stack.add_named(&ax25_box, Some("AX.25 raw socket"));

    // KISS (TCP)
    let kt_host = gtk::Entry::builder().placeholder_text("host").text("127.0.0.1").build();
    let kt_port = gtk::Entry::builder().placeholder_text("port").text("8001").build();
    let kt_my_call = gtk::Entry::builder().placeholder_text("MYCALL-1").build();
    let kt_tx_delay = gtk::Entry::builder().placeholder_text("TNC default").build();
    let kt_persistence = gtk::Entry::builder().placeholder_text("TNC default").build();
    let kt_slot_time = gtk::Entry::builder().placeholder_text("TNC default").build();
    let kt_full_duplex = gtk::CheckButton::with_label("Force full duplex");
    let kt_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    kt_box.append(&labeled("Host", &kt_host));
    kt_box.append(&labeled("Port", &kt_port));
    kt_box.append(&labeled("My Callsign", &kt_my_call));
    kt_box.append(&labeled("TXDELAY (x10ms)", &kt_tx_delay));
    kt_box.append(&labeled("Persistence", &kt_persistence));
    kt_box.append(&labeled("Slot Time (x10ms)", &kt_slot_time));
    kt_box.append(&kt_full_duplex);
    stack.add_named(&kt_box, Some("KISS (TCP)"));

    // KISS (Serial)
    let ks_device = gtk::Entry::builder().placeholder_text("/dev/ttyUSB0").build();
    let ks_baud = gtk::Entry::builder().placeholder_text("baud").text("9600").build();
    let ks_my_call = gtk::Entry::builder().placeholder_text("MYCALL-1").build();
    let ks_tx_delay = gtk::Entry::builder().placeholder_text("TNC default").build();
    let ks_persistence = gtk::Entry::builder().placeholder_text("TNC default").build();
    let ks_slot_time = gtk::Entry::builder().placeholder_text("TNC default").build();
    let ks_full_duplex = gtk::CheckButton::with_label("Force full duplex");
    let ks_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    ks_box.append(&labeled("Device", &ks_device));
    ks_box.append(&labeled("Baud", &ks_baud));
    ks_box.append(&labeled("My Callsign", &ks_my_call));
    ks_box.append(&labeled("TXDELAY (x10ms)", &ks_tx_delay));
    ks_box.append(&labeled("Persistence", &ks_persistence));
    ks_box.append(&labeled("Slot Time (x10ms)", &ks_slot_time));
    ks_box.append(&ks_full_duplex);
    stack.add_named(&ks_box, Some("KISS (Serial)"));

    if existing.is_none() {
        if let Some(default_call) = &ui.state.config.borrow().ui.default_call {
            agw_my_call.set_text(default_call);
            kt_my_call.set_text(default_call);
            ks_my_call.set_text(default_call);
        }
    }

    let initial_kind = match &existing {
        Some(e) => e.config.kind_label(),
        None => "Telnet",
    };
    stack.set_visible_child_name(initial_kind);
    if let Some(pos) = KIND_NAMES.iter().position(|k| *k == initial_kind) {
        kind_dropdown.set_selected(pos as u32);
    }
    {
        let stack = stack.clone();
        kind_dropdown.connect_selected_notify(move |dd| {
            if let Some(name) = KIND_NAMES.get(dd.selected() as usize) {
                stack.set_visible_child_name(name);
            }
        });
    }

    if let Some(e) = &existing {
        match &e.config {
            PortConfig::Telnet { host, port } => {
                telnet_host.set_text(host);
                telnet_port.set_text(&port.to_string());
            }
            PortConfig::Ssh { host, port, user } => {
                ssh_host.set_text(host);
                ssh_port.set_text(&port.to_string());
                ssh_user.set_text(user);
            }
            PortConfig::Agwpe { host, port, radio_port, my_call, login } => {
                agw_host.set_text(host);
                agw_port.set_text(&port.to_string());
                agw_radio_port.set_text(&radio_port.to_string());
                agw_my_call.set_text(my_call);
                if let Some(login) = login {
                    agw_user.set_text(&login.username);
                    agw_pass.set_text(&login.password);
                }
            }
            PortConfig::Ax25RawSocket { device } => {
                ax25_device.set_text(device);
            }
            PortConfig::KissTcp { host, port, my_call, kiss_params } => {
                kt_host.set_text(host);
                kt_port.set_text(&port.to_string());
                kt_my_call.set_text(my_call);
                load_kiss_params(kiss_params, &kt_tx_delay, &kt_persistence, &kt_slot_time, &kt_full_duplex);
            }
            PortConfig::KissSerial { device, baud, my_call, kiss_params } => {
                ks_device.set_text(device);
                ks_baud.set_text(&baud.to_string());
                ks_my_call.set_text(my_call);
                load_kiss_params(kiss_params, &ks_tx_delay, &ks_persistence, &ks_slot_time, &ks_full_duplex);
            }
        }
    }

    root.append(&stack);

    let autoconnect_check = gtk::CheckButton::with_label("Connect automatically at startup");
    autoconnect_check.set_active(existing.as_ref().is_some_and(|e| e.autoconnect));
    root.append(&autoconnect_check);

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
    button_row.append(&cancel_button);
    button_row.append(&save_button);
    root.append(&button_row);

    {
        let ui = ui.clone();
        let win = win.clone();
        let list_box = list_box.clone();
        let existing_id = existing.map(|e| e.id);
        save_button.connect_clicked(move |_| {
            let name = name_entry.text().to_string();
            if name.trim().is_empty() {
                error_label.set_text("Name is required.");
                return;
            }
            let selected_kind = KIND_NAMES[kind_dropdown.selected() as usize];
            let config = match selected_kind {
                "Telnet" => {
                    let port = match telnet_port.text().parse::<u16>() {
                        Ok(p) => p,
                        Err(_) => {
                            error_label.set_text("Telnet port must be a number 0-65535.");
                            return;
                        }
                    };
                    PortConfig::Telnet { host: telnet_host.text().to_string(), port }
                }
                "SSH" => {
                    let port = match ssh_port.text().parse::<u16>() {
                        Ok(p) => p,
                        Err(_) => {
                            error_label.set_text("SSH port must be a number 0-65535.");
                            return;
                        }
                    };
                    PortConfig::Ssh {
                        host: ssh_host.text().to_string(),
                        port,
                        user: ssh_user.text().to_string(),
                    }
                }
                "AGWPE" => {
                    let port = match agw_port.text().parse::<u16>() {
                        Ok(p) => p,
                        Err(_) => {
                            error_label.set_text("AGWPE TCP port must be a number 0-65535.");
                            return;
                        }
                    };
                    let radio_port = match agw_radio_port.text().parse::<u8>() {
                        Ok(p) => p,
                        Err(_) => {
                            error_label.set_text("Radio port must be a number 0-255.");
                            return;
                        }
                    };
                    let username = agw_user.text().to_string();
                    let password = agw_pass.text().to_string();
                    let login = if username.is_empty() && password.is_empty() {
                        None
                    } else {
                        Some(AgwpeLogin { username, password })
                    };
                    PortConfig::Agwpe {
                        host: agw_host.text().to_string(),
                        port,
                        radio_port,
                        my_call: agw_my_call.text().to_string(),
                        login,
                    }
                }
                "AX.25 raw socket" => PortConfig::Ax25RawSocket { device: ax25_device.text().to_string() },
                "KISS (TCP)" => {
                    let port = match kt_port.text().parse::<u16>() {
                        Ok(p) => p,
                        Err(_) => {
                            error_label.set_text("KISS TCP port must be a number 0-65535.");
                            return;
                        }
                    };
                    let kiss_params = match parse_kiss_params(&kt_tx_delay, &kt_persistence, &kt_slot_time, &kt_full_duplex) {
                        Ok(p) => p,
                        Err(msg) => {
                            error_label.set_text(&msg);
                            return;
                        }
                    };
                    PortConfig::KissTcp {
                        host: kt_host.text().to_string(),
                        port,
                        my_call: kt_my_call.text().to_string(),
                        kiss_params,
                    }
                }
                _ => {
                    let baud = match ks_baud.text().parse::<u32>() {
                        Ok(b) => b,
                        Err(_) => {
                            error_label.set_text("Baud rate must be a number.");
                            return;
                        }
                    };
                    let kiss_params = match parse_kiss_params(&ks_tx_delay, &ks_persistence, &ks_slot_time, &ks_full_duplex) {
                        Ok(p) => p,
                        Err(msg) => {
                            error_label.set_text(&msg);
                            return;
                        }
                    };
                    PortConfig::KissSerial {
                        device: ks_device.text().to_string(),
                        baud,
                        my_call: ks_my_call.text().to_string(),
                        kiss_params,
                    }
                }
            };

            let autoconnect = autoconnect_check.is_active();
            let mut cfg = ui.state.config.borrow_mut();
            if let Some(id) = &existing_id {
                if let Some(slot) = cfg.ports.iter_mut().find(|p| &p.id == id) {
                    slot.name = name;
                    slot.config = config;
                    slot.autoconnect = autoconnect;
                }
            } else {
                let id = next_id(&cfg);
                cfg.ports.push(PortEntry { id, name, config, autoconnect });
            }
            drop(cfg);
            ui.state.save_config();
            rebuild_list(&ui, &list_box);
            win.close();
        });
    }

    win.present();
}

/// Prompt for a destination address and message, and send a one-shot
/// unconnected (UI/beacon) frame over an already-connected AGWPE port.
pub fn show_send_unproto(ui: &Rc<Ui>) {
    let candidates: Vec<PortEntry> = ui
        .state
        .config
        .borrow()
        .ports
        .iter()
        .filter(|p| {
            ui.state.is_active(&p.id)
                && matches!(
                    p.config,
                    PortConfig::Agwpe { .. } | PortConfig::KissTcp { .. } | PortConfig::KissSerial { .. }
                )
        })
        .cloned()
        .collect();

    let (win, root) = dialog_window(&ui.window, "Send Beacon", 400);

    if candidates.is_empty() {
        root.append(&gtk::Label::new(Some(
            "No connected AGWPE or KISS ports. Connect one first via Ports\u{2026}",
        )));
        win.present();
        return;
    }

    let names: Vec<&str> = candidates.iter().map(|p| p.name.as_str()).collect();
    let port_model = gtk::StringList::new(&names);
    let port_dropdown = gtk::DropDown::builder().model(&port_model).build();
    root.append(&labeled("Port", &port_dropdown));

    let dest_entry = gtk::Entry::builder().placeholder_text("BEACON").text("BEACON").build();
    root.append(&labeled("Destination", &dest_entry));

    let via_entry = gtk::Entry::builder().placeholder_text("WIDE1-1,WIDE2-1 (optional)").build();
    root.append(&labeled("Via", &via_entry));

    let message_entry = gtk::Entry::builder().placeholder_text("Message text").build();
    root.append(&labeled("Message", &message_entry));

    let button_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    button_row.set_halign(gtk::Align::End);
    let cancel_button = gtk::Button::with_label("Cancel");
    {
        let win = win.clone();
        cancel_button.connect_clicked(move |_| win.close());
    }
    let send_button = gtk::Button::with_label("Send");
    send_button.add_css_class("suggested-action");
    {
        let ui = ui.clone();
        let win = win.clone();
        send_button.connect_clicked(move |_| {
            let dest = dest_entry.text().to_string();
            if dest.trim().is_empty() {
                return;
            }
            let message = message_entry.text().to_string();
            let via: Vec<String> = via_entry
                .text()
                .split([',', ' '])
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_uppercase())
                .collect();
            let idx = port_dropdown.selected() as usize;
            if let Some(entry) = candidates.get(idx) {
                ui.send_unproto(&entry.id, dest.to_uppercase(), via, message.into_bytes());
            }
            win.close();
        });
    }
    button_row.append(&cancel_button);
    button_row.append(&send_button);
    root.append(&button_row);

    win.present();
}

/// Populate a KISS parameter form (three optional-number entries + a
/// force-full-duplex checkbox) from a loaded `KissParams` — `None` fields
/// are left as their "TNC default" placeholder, i.e. blank.
fn load_kiss_params(params: &KissParams, tx_delay: &gtk::Entry, persistence: &gtk::Entry, slot_time: &gtk::Entry, full_duplex: &gtk::CheckButton) {
    if let Some(v) = params.tx_delay {
        tx_delay.set_text(&v.to_string());
    }
    if let Some(v) = params.persistence {
        persistence.set_text(&v.to_string());
    }
    if let Some(v) = params.slot_time {
        slot_time.set_text(&v.to_string());
    }
    // Only "on" is representable via a single checkbox; explicitly forcing
    // half-duplex is rare enough not to need its own control here.
    full_duplex.set_active(params.full_duplex == Some(true));
}

/// Parse a KISS parameter form back into a `KissParams`, blank = `None`.
fn parse_kiss_params(
    tx_delay: &gtk::Entry,
    persistence: &gtk::Entry,
    slot_time: &gtk::Entry,
    full_duplex: &gtk::CheckButton,
) -> Result<KissParams, String> {
    let parse_optional_u8 = |entry: &gtk::Entry, field: &str| -> Result<Option<u8>, String> {
        let text = entry.text();
        let text = text.trim();
        if text.is_empty() {
            return Ok(None);
        }
        text.parse::<u8>().map(Some).map_err(|_| format!("{field} must be a number 0-255."))
    };
    Ok(KissParams {
        tx_delay: parse_optional_u8(tx_delay, "TXDELAY")?,
        persistence: parse_optional_u8(persistence, "Persistence")?,
        slot_time: parse_optional_u8(slot_time, "Slot Time")?,
        full_duplex: if full_duplex.is_active() { Some(true) } else { None },
    })
}

pub(crate) fn labeled(text: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    labeled_widget(text, widget.clone().upcast())
}

pub(crate) fn labeled_widget(text: &str, widget: gtk::Widget) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let label = gtk::Label::new(Some(text));
    label.set_width_chars(12);
    label.set_halign(gtk::Align::Start);
    row.append(&label);
    widget.set_hexpand(true);
    row.append(&widget);
    row
}
