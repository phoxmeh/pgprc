use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use gtk::glib::object::IsA;

use pr_core::{AgwpeLogin, AppConfig, KissArqParams, KissParams, PortConfig, PortEntry};

use crate::window::Ui;

/// Build a modal dialog window with a native header bar (so it always has a
/// title and a close button) and return it along with its content box.
///
/// Escape closes it, same as clicking Cancel/the close button — every one of
/// these dialogs requires an explicit "Save"/"Send" click to actually
/// persist anything, so closing (by any means) never discards a save that
/// already happened; it's always safe to bail out with Escape.
pub(crate) fn dialog_window(parent: &impl IsA<gtk::Window>, title: &str, width: i32) -> (adw::Window, gtk::Box) {
    let (win, root, _header) = dialog_window_with_header(parent, title, width);
    (win, root)
}

/// Same as `dialog_window`, but also returns the native header bar so a
/// caller can pack a widget into its titlebar space (e.g. the mailbox
/// window's Enable button) instead of the content area below it.
pub(crate) fn dialog_window_with_header(parent: &impl IsA<gtk::Window>, title: &str, width: i32) -> (adw::Window, gtk::Box, adw::HeaderBar) {
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

    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&root));
    win.set_content(Some(&toolbar));

    let escape_controller = gtk::EventControllerKey::new();
    {
        let win = win.clone();
        escape_controller.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gtk::gdk::Key::Escape {
                win.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }
    win.add_controller(escape_controller);

    (win, root, header)
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
    let count = entries.len();
    for (idx, entry) in entries.into_iter().enumerate() {
        list_box.append(&build_port_row(ui, entry, idx, count, list_box));
    }
}

/// Swap the port at `id` with its neighbor in the given direction (-1 = up,
/// +1 = down), if one exists. The port dropdown everywhere else in the app
/// just iterates `AppConfig.ports` in order, so reordering here is all that's
/// needed to reorder those dropdowns too.
fn move_port(ui: &Rc<Ui>, id: &str, direction: isize) {
    let mut cfg = ui.state.config.borrow_mut();
    if let Some(idx) = cfg.ports.iter().position(|p| p.id == id) {
        let new_idx = idx as isize + direction;
        if new_idx >= 0 && (new_idx as usize) < cfg.ports.len() {
            cfg.ports.swap(idx, new_idx as usize);
        }
    }
    drop(cfg);
    ui.state.save_config();
}

fn build_port_row(ui: &Rc<Ui>, entry: PortEntry, idx: usize, count: usize, list_box: &gtk::ListBox) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    row.set_margin_top(3);
    row.set_margin_bottom(3);
    row.set_margin_start(4);
    row.set_margin_end(4);

    let reorder_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let up_button = gtk::Button::from_icon_name("go-up-symbolic");
    up_button.add_css_class("flat");
    up_button.add_css_class("compact");
    up_button.set_sensitive(idx > 0);
    {
        let ui = ui.clone();
        let id = entry.id.clone();
        let list_box = list_box.clone();
        up_button.connect_clicked(move |_| {
            move_port(&ui, &id, -1);
            rebuild_list(&ui, &list_box);
        });
    }
    reorder_box.append(&up_button);
    let down_button = gtk::Button::from_icon_name("go-down-symbolic");
    down_button.add_css_class("flat");
    down_button.add_css_class("compact");
    down_button.set_sensitive(idx + 1 < count);
    {
        let ui = ui.clone();
        let id = entry.id.clone();
        let list_box = list_box.clone();
        down_button.connect_clicked(move |_| {
            move_port(&ui, &id, 1);
            rebuild_list(&ui, &list_box);
        });
    }
    reorder_box.append(&down_button);
    row.append(&reorder_box);

    let label = gtk::Label::new(Some(&format!("{}  \u{2014}  {}", entry.name, entry.config.kind_label())));
    label.set_hexpand(true);
    label.set_halign(gtk::Align::Start);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
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
            ui.rebuild_favorites_bar();
            ui.rebuild_bottom_ports();
            ui.monitor.rebuild_port_filter();
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
    let kt_window = gtk::Entry::builder().placeholder_text("app default (4)").build();
    let kt_t1_ms = gtk::Entry::builder().placeholder_text("app default (4000)").build();
    let kt_n2 = gtk::Entry::builder().placeholder_text("app default (10)").build();
    let kt_paclen = gtk::Entry::builder().placeholder_text("app default (256)").build();
    let kt_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    kt_box.append(&labeled("Host", &kt_host));
    kt_box.append(&labeled("Port", &kt_port));
    kt_box.append(&labeled("My Callsign", &kt_my_call));
    kt_box.append(&labeled("TXDELAY (x10ms)", &kt_tx_delay));
    kt_box.append(&labeled("Persistence", &kt_persistence));
    kt_box.append(&labeled("Slot Time (x10ms)", &kt_slot_time));
    kt_box.append(&kt_full_duplex);
    kt_box.append(&labeled("Window Size (k)", &kt_window));
    kt_box.append(&labeled("Ack Timer T1 (ms)", &kt_t1_ms));
    kt_box.append(&labeled("Max Retries (N2)", &kt_n2));
    kt_box.append(&labeled("Max I-Frame Size (N1, bytes)", &kt_paclen));
    stack.add_named(&kt_box, Some("KISS (TCP)"));

    // KISS (Serial)
    let ks_device = gtk::Entry::builder().placeholder_text("/dev/ttyUSB0").build();
    let ks_baud = gtk::Entry::builder().placeholder_text("baud").text("9600").build();
    let ks_my_call = gtk::Entry::builder().placeholder_text("MYCALL-1").build();
    let ks_tx_delay = gtk::Entry::builder().placeholder_text("TNC default").build();
    let ks_persistence = gtk::Entry::builder().placeholder_text("TNC default").build();
    let ks_slot_time = gtk::Entry::builder().placeholder_text("TNC default").build();
    let ks_full_duplex = gtk::CheckButton::with_label("Force full duplex");
    let ks_window = gtk::Entry::builder().placeholder_text("app default (4)").build();
    let ks_t1_ms = gtk::Entry::builder().placeholder_text("app default (4000)").build();
    let ks_n2 = gtk::Entry::builder().placeholder_text("app default (10)").build();
    let ks_paclen = gtk::Entry::builder().placeholder_text("app default (256)").build();
    let ks_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    ks_box.append(&labeled("Device", &ks_device));
    ks_box.append(&labeled("Baud", &ks_baud));
    ks_box.append(&labeled("My Callsign", &ks_my_call));
    ks_box.append(&labeled("TXDELAY (x10ms)", &ks_tx_delay));
    ks_box.append(&labeled("Persistence", &ks_persistence));
    ks_box.append(&labeled("Slot Time (x10ms)", &ks_slot_time));
    ks_box.append(&ks_full_duplex);
    ks_box.append(&labeled("Window Size (k)", &ks_window));
    ks_box.append(&labeled("Ack Timer T1 (ms)", &ks_t1_ms));
    ks_box.append(&labeled("Max Retries (N2)", &ks_n2));
    ks_box.append(&labeled("Max I-Frame Size (N1, bytes)", &ks_paclen));
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
            PortConfig::KissTcp { host, port, my_call, kiss_params, kiss_arq } => {
                kt_host.set_text(host);
                kt_port.set_text(&port.to_string());
                kt_my_call.set_text(my_call);
                load_kiss_params(kiss_params, &kt_tx_delay, &kt_persistence, &kt_slot_time, &kt_full_duplex);
                load_kiss_arq_params(kiss_arq, &kt_window, &kt_t1_ms, &kt_n2, &kt_paclen);
            }
            PortConfig::KissSerial { device, baud, my_call, kiss_params, kiss_arq } => {
                ks_device.set_text(device);
                ks_baud.set_text(&baud.to_string());
                ks_my_call.set_text(my_call);
                load_kiss_params(kiss_params, &ks_tx_delay, &ks_persistence, &ks_slot_time, &ks_full_duplex);
                load_kiss_arq_params(kiss_arq, &ks_window, &ks_t1_ms, &ks_n2, &ks_paclen);
            }
        }
    }

    root.append(&stack);

    let autoconnect_check = gtk::CheckButton::with_label("Connect automatically at startup");
    autoconnect_check.set_active(existing.as_ref().is_some_and(|e| e.autoconnect));
    root.append(&autoconnect_check);

    let favorite_check = gtk::CheckButton::with_label("Favorite (quick-connect button in main window)");
    favorite_check.set_active(existing.as_ref().is_some_and(|e| e.favorite));
    root.append(&favorite_check);

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
                    let kiss_arq = match parse_kiss_arq_params(&kt_window, &kt_t1_ms, &kt_n2, &kt_paclen) {
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
                        kiss_arq,
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
                    let kiss_arq = match parse_kiss_arq_params(&ks_window, &ks_t1_ms, &ks_n2, &ks_paclen) {
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
                        kiss_arq,
                    }
                }
            };

            let autoconnect = autoconnect_check.is_active();
            let favorite = favorite_check.is_active();
            let mut cfg = ui.state.config.borrow_mut();
            if let Some(id) = &existing_id {
                if let Some(slot) = cfg.ports.iter_mut().find(|p| &p.id == id) {
                    slot.name = name;
                    slot.config = config;
                    slot.autoconnect = autoconnect;
                    slot.favorite = favorite;
                }
            } else {
                let id = next_id(&cfg);
                cfg.ports.push(PortEntry { id, name, config, autoconnect, favorite });
            }
            drop(cfg);
            ui.state.save_config();
            rebuild_list(&ui, &list_box);
            ui.rebuild_favorites_bar();
            ui.rebuild_bottom_ports();
            ui.monitor.rebuild_port_filter();
            win.close();
        });
    }

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

/// Populate a KISS connected-mode ARQ tuning form (four optional-number
/// entries) from a loaded `KissArqParams` — `None` fields are left blank,
/// showing this app's own built-in default via the entry's placeholder.
fn load_kiss_arq_params(params: &KissArqParams, window: &gtk::Entry, t1_ms: &gtk::Entry, n2: &gtk::Entry, paclen: &gtk::Entry) {
    if let Some(v) = params.window {
        window.set_text(&v.to_string());
    }
    if let Some(v) = params.t1_ms {
        t1_ms.set_text(&v.to_string());
    }
    if let Some(v) = params.n2 {
        n2.set_text(&v.to_string());
    }
    if let Some(v) = params.n1_bytes {
        paclen.set_text(&v.to_string());
    }
}

/// Parse a KISS ARQ tuning form back into a `KissArqParams`, blank = `None`
/// (this app's own default).
fn parse_kiss_arq_params(window: &gtk::Entry, t1_ms: &gtk::Entry, n2: &gtk::Entry, paclen: &gtk::Entry) -> Result<KissArqParams, String> {
    let text = window.text();
    let window_text = text.trim();
    let window = if window_text.is_empty() {
        None
    } else {
        let value: u8 = window_text.parse().map_err(|_| "Window size must be a number.".to_string())?;
        if !(1..=7).contains(&value) {
            return Err("Window size must be 1-7.".to_string());
        }
        Some(value)
    };
    let parse_optional_u32 = |entry: &gtk::Entry, field: &str| -> Result<Option<u32>, String> {
        let text = entry.text();
        let text = text.trim();
        if text.is_empty() {
            return Ok(None);
        }
        text.parse::<u32>().map(Some).map_err(|_| format!("{field} must be a non-negative number."))
    };
    Ok(KissArqParams {
        window,
        t1_ms: parse_optional_u32(t1_ms, "Ack Timer T1")?,
        n2: parse_optional_u32(n2, "Max Retries (N2)")?,
        n1_bytes: parse_optional_u32(paclen, "Max I-Frame Size (N1)")?.map(|v| v as usize),
    })
}

/// Force an editable field's text to uppercase live as the user types (used
/// for every Node/Via/Home BBS Address field in the app — both plain
/// `gtk::Entry` and `adw::EntryRow`, which both implement `gtk::Editable`).
/// `set_text` alone jumps the cursor to the end, so the cursor position is
/// captured and restored explicitly — safe here since an uppercase
/// transform never changes string length. Comparing before writing avoids
/// infinite `connect_changed` recursion (the second, recursive call sees
/// text already matching and no-ops).
pub(crate) fn force_uppercase(entry: &impl IsA<gtk::Editable>) {
    entry.connect_changed(|entry| {
        let text = entry.text();
        let upper = text.to_uppercase();
        if text.as_str() != upper {
            let pos = entry.position();
            entry.set_text(&upper);
            entry.set_position(pos);
        }
    });
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

/// A boxed-list of switches, one per connect-capable port, shared by any
/// feature (mailbox, keyboard-to-keyboard mode) that listens for unsolicited
/// connections on a configurable subset of ports. `selected` empty means
/// "every port" (each such feature's own pre-existing behavior before this
/// per-port filtering existed), shown as every switch defaulting on.
/// Returns the widget to append plus each port's `(id, Switch)`, so the
/// caller can read back which are active on Save -- see
/// `collapse_listen_ports` for turning that back into the "empty means all"
/// storage convention.
pub(crate) fn port_listen_checklist(ports: &[PortEntry], selected: &[String]) -> (gtk::Widget, Vec<(String, gtk::Switch)>) {
    let list_box = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    list_box.add_css_class("boxed-list");

    let connect_ports: Vec<&PortEntry> = ports.iter().filter(|p| crate::session_tab::port_supports_connect(&p.config)).collect();
    if connect_ports.is_empty() {
        let label = gtk::Label::new(Some("No connect-capable ports configured yet."));
        label.set_margin_top(8);
        label.set_margin_bottom(8);
        list_box.append(&label);
        return (list_box.upcast(), Vec::new());
    }

    let mut switches = Vec::new();
    for port in connect_ports {
        let row = adw::ActionRow::builder().title(&port.name).build();
        let switch = gtk::Switch::builder()
            .active(selected.is_empty() || selected.iter().any(|p| p == &port.id))
            .valign(gtk::Align::Center)
            .build();
        row.add_suffix(&switch);
        row.set_activatable_widget(Some(&switch));
        list_box.append(&row);
        switches.push((port.id.clone(), switch));
    }
    (list_box.upcast(), switches)
}

/// Reads back a `port_listen_checklist`'s switches into the "empty means
/// every port" storage convention: only persists an explicit subset when at
/// least one port was actually turned off, so leaving every switch at its
/// (all-on) default keeps listening on ports added later too.
pub(crate) fn collapse_listen_ports(switches: &[(String, gtk::Switch)]) -> Vec<String> {
    let chosen: Vec<String> = switches.iter().filter(|(_, sw)| sw.is_active()).map(|(id, _)| id.clone()).collect();
    if chosen.len() == switches.len() {
        Vec::new()
    } else {
        chosen
    }
}
