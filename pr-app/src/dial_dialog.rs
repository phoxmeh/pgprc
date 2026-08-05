//! Dial dialog: pick a port and destination to open a new connected-session
//! tab. "Open Connected" issues the dial immediately; "Open Disconnected"
//! just creates the tab shell showing history, for manual reconnect or
//! offline review later — useful when you just want to check what was said
//! last without actually keying up.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib::object::IsA;

use pr_core::{PortConfig, PortEntry};

use crate::address_book_dialog;
use crate::ports_dialog::{dialog_window, force_uppercase};
use crate::session_tab::{port_dialable, port_supports_connect};
use crate::window::Ui;

/// Same shape as `ports_dialog::labeled_widget`, but also returns the label
/// so the caller can relabel it later (the Via/Address row's text depends
/// on the selected port kind).
fn labeled_dynamic(text: &str, widget: &impl IsA<gtk::Widget>) -> (gtk::Box, gtk::Label) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let label = gtk::Label::new(Some(text));
    label.set_width_chars(12);
    label.set_halign(gtk::Align::Start);
    row.append(&label);
    widget.set_hexpand(true);
    row.append(widget);
    (row, label)
}

pub fn show(ui: &Rc<Ui>) {
    let (win, root) = dialog_window(&ui.window, "Dial", 440);

    let ports: Vec<PortEntry> = ui.state.config.borrow().ports.iter().filter(|p| port_dialable(&p.config)).cloned().collect();

    if ports.is_empty() {
        root.append(&gtk::Label::new(Some(
            "No dialable ports configured. Add an AGWPE, AX.25 raw socket, Telnet, or SSH port first via Ports\u{2026}",
        )));
        win.present();
        return;
    }

    let names: Vec<&str> = ports.iter().map(|p| p.name.as_str()).collect();
    let port_dropdown = gtk::DropDown::builder().model(&gtk::StringList::new(&names)).build();
    let (port_row, _) = labeled_dynamic("Port", &port_dropdown);
    root.append(&port_row);

    let node_entry = gtk::Entry::builder().placeholder_text("N0CALL-1").build();
    force_uppercase(&node_entry);
    let (node_row, _) = labeled_dynamic("Node", &node_entry);
    root.append(&node_row);

    let via_entry = gtk::Entry::builder().build();
    force_uppercase(&via_entry);
    let (via_row, via_label) = labeled_dynamic("Via", &via_entry);
    root.append(&via_row);

    // Opens a dedicated picker dialog (filterable/sortable, selection-only)
    // rather than an inline dropdown -- selecting an entry copies its
    // callsign (and, if set, its via path) into node_entry/via_entry.
    let address_book_button = gtk::Button::from_icon_name("address-book-new-symbolic");
    address_book_button.set_tooltip_text(Some("Choose from Address Book\u{2026}"));
    {
        let ui = ui.clone();
        let win = win.clone();
        let node_entry = node_entry.clone();
        let via_entry = via_entry.clone();
        address_book_button.connect_clicked(move |_| {
            let node_entry = node_entry.clone();
            let via_entry = via_entry.clone();
            address_book_dialog::show_picker(&ui, &win, move |entry| {
                node_entry.set_text(&entry.callsign);
                if !entry.via.trim().is_empty() {
                    via_entry.set_text(&entry.via);
                }
            });
        });
    }
    node_row.append(&address_book_button);

    // Relabel/placeholder-swap Via <-> Address, and show/hide Node,
    // depending on the selected port's kind.
    let update_for_port = {
        let ports = ports.clone();
        let node_row = node_row.clone();
        let via_label = via_label.clone();
        let via_entry = via_entry.clone();
        move |dropdown: &gtk::DropDown| {
            let Some(port) = ports.get(dropdown.selected() as usize) else { return };
            node_row.set_visible(port_supports_connect(&port.config));
            if matches!(port.config, PortConfig::Telnet { .. } | PortConfig::Ssh { .. }) {
                via_label.set_text("Address");
                via_entry.set_placeholder_text(Some("Sent verbatim as the first line after connecting (optional)"));
            } else {
                via_label.set_text("Via");
                via_entry.set_placeholder_text(Some("WIDE1-1,WIDE2-1 (optional)"));
            }
        }
    };
    update_for_port(&port_dropdown);
    {
        let update_for_port = update_for_port.clone();
        port_dropdown.connect_selected_notify(move |dd| update_for_port(dd));
    }

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
    let open_disconnected_button = gtk::Button::with_label("Open Disconnected");
    let dial_button = gtk::Button::with_label("Dial");
    dial_button.add_css_class("suggested-action");
    button_row.append(&cancel_button);
    button_row.append(&open_disconnected_button);
    button_row.append(&dial_button);
    root.append(&button_row);

    // Enter in any field triggers "Dial" when the port is active.
    win.set_default_widget(Some(&dial_button));

    // Refresh Dial sensitivity whenever the port selection changes.
    let refresh_dial = {
        let ports = ports.clone();
        let port_dropdown = port_dropdown.clone();
        let ui = ui.clone();
        let dial_button = dial_button.clone();
        move || {
            let active = ports
                .get(port_dropdown.selected() as usize)
                .is_some_and(|p| ui.state.is_active(&p.id));
            dial_button.set_sensitive(active);
        }
    };
    refresh_dial();

    let resolve = {
        let ui = ui.clone();
        let win = win.clone();
        let ports = ports.clone();
        let port_dropdown = port_dropdown.clone();
        let node_entry = node_entry.clone();
        let via_entry = via_entry.clone();
        let error_label = error_label.clone();
        move |connect: bool| {
            let Some(port) = ports.get(port_dropdown.selected() as usize).cloned() else {
                error_label.set_text("Select a port.");
                return;
            };
            if connect && !ui.state.is_active(&port.id) {
                error_label.set_text("Port not connected \u{2014} start the port first.");
                return;
            }
            let needs_node = port_supports_connect(&port.config);
            let node = node_entry.text().trim().to_uppercase();
            if needs_node && node.is_empty() {
                error_label.set_text("Enter a node/call sign.");
                return;
            }
            let via_raw = via_entry.text().trim().to_uppercase();
            ui.add_connection_tab(port, node, via_raw, connect);
            win.close();
        }
    };
    // Pressing Enter in either text field submits Dial directly --
    // `set_default_widget` alone doesn't reliably fire on every GTK4
    // version/theme combination, so this is the belt-and-suspenders path.
    {
        let resolve = resolve.clone();
        node_entry.connect_activate(move |_| resolve(true));
    }
    {
        let resolve = resolve.clone();
        via_entry.connect_activate(move |_| resolve(true));
    }
    {
        let resolve = resolve.clone();
        open_disconnected_button.connect_clicked(move |_| resolve(false));
    }
    {
        let refresh_dial = refresh_dial.clone();
        port_dropdown.connect_selected_notify(move |_| refresh_dial());
    }
    dial_button.connect_clicked(move |_| resolve(true));

    win.present();
}
