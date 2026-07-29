//! Dial dialog: pick a port and destination to open a new connected-session
//! tab. "Open Connected" issues the dial immediately; "Open Disconnected"
//! just creates the tab shell showing history, for manual reconnect or
//! offline review later — useful when you just want to check what was said
//! last without actually keying up.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib::object::IsA;

use pr_core::{AddressBookEntry, PortConfig, PortEntry};

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

    // A one-shot picker: selecting an entry copies its callsign (and, if
    // set, its via path) into node_entry/via_entry. Blank factory for the
    // closed button (leaves just the dropdown's own arrow visible) but a
    // real text factory for the popup list, matching the address-book
    // picker's original look before it moved here from the (now much
    // simpler) session tab.
    let mut address_book: Vec<AddressBookEntry> = ui.state.config.borrow().address_book.clone();
    address_book.sort_by(|a, b| a.callsign.cmp(&b.callsign));
    let address_book_names: Vec<String> = address_book
        .iter()
        .map(|e| {
            let extra = e.name.as_deref().or(e.alias.as_deref());
            match extra {
                Some(extra) if !extra.is_empty() => format!("{} \u{2014} {extra}", e.callsign),
                _ => e.callsign.clone(),
            }
        })
        .collect();
    let address_book_refs: Vec<&str> = address_book_names.iter().map(String::as_str).collect();
    let address_book_dropdown = gtk::DropDown::builder().model(&gtk::StringList::new(&address_book_refs)).build();
    address_book_dropdown.set_tooltip_text(Some("From Address Book\u{2026}"));
    let blank_factory = gtk::SignalListItemFactory::new();
    blank_factory.connect_setup(|_, _list_item| {});
    address_book_dropdown.set_factory(Some(&blank_factory));
    let list_factory = gtk::SignalListItemFactory::new();
    list_factory.connect_setup(|_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else { return };
        let label = gtk::Label::new(None);
        label.set_halign(gtk::Align::Start);
        list_item.set_child(Some(&label));
    });
    list_factory.connect_bind(|_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else { return };
        let label = list_item.child().and_then(|c| c.downcast::<gtk::Label>().ok());
        let text = list_item.item().and_then(|o| o.downcast::<gtk::StringObject>().ok());
        if let (Some(label), Some(text)) = (label, text) {
            label.set_text(&text.string());
        }
    });
    address_book_dropdown.set_list_factory(Some(&list_factory));
    node_row.append(&address_book_dropdown);

    let via_entry = gtk::Entry::builder().build();
    force_uppercase(&via_entry);
    let (via_row, via_label) = labeled_dynamic("Via", &via_entry);
    root.append(&via_row);

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

    {
        let address_book = address_book.clone();
        let node_entry = node_entry.clone();
        let via_entry = via_entry.clone();
        address_book_dropdown.connect_selected_notify(move |dropdown| {
            if let Some(entry) = address_book.get(dropdown.selected() as usize) {
                node_entry.set_text(&entry.callsign);
                if !entry.via.trim().is_empty() {
                    via_entry.set_text(&entry.via);
                }
            }
        });
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
    let open_connected_button = gtk::Button::with_label("Open Connected");
    open_connected_button.add_css_class("suggested-action");
    button_row.append(&cancel_button);
    button_row.append(&open_disconnected_button);
    button_row.append(&open_connected_button);
    root.append(&button_row);

    // Enter in any field triggers "Open Connected" -- the common case.
    win.set_default_widget(Some(&open_connected_button));

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
            let needs_node = port_supports_connect(&port.config);
            let node = node_entry.text().trim().to_uppercase();
            if needs_node && node.is_empty() {
                error_label.set_text("Enter a node/callsign.");
                return;
            }
            let via_raw = via_entry.text().trim().to_uppercase();
            ui.add_connection_tab(port, node, via_raw, connect);
            win.close();
        }
    };
    // Pressing Enter in either text field submits "Open Connected" directly
    // -- `set_default_widget` alone doesn't reliably fire on every GTK4
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
    open_connected_button.connect_clicked(move |_| resolve(true));

    win.present();
}
