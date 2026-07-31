use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use gtk::glib::object::IsA;

use pr_core::AddressBookEntry;

use crate::ports_dialog::{dialog_window, force_uppercase, labeled};
use crate::window::Ui;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortMode {
    Callsign,
    LastHeard,
}

impl SortMode {
    fn label(self) -> &'static str {
        match self {
            SortMode::Callsign => "Sort: Callsign (click for Last Heard)",
            SortMode::LastHeard => "Sort: Last Heard (click for Callsign)",
        }
    }

    fn toggled(self) -> SortMode {
        match self {
            SortMode::Callsign => SortMode::LastHeard,
            SortMode::LastHeard => SortMode::Callsign,
        }
    }
}

/// Case-insensitive substring match against callsign/alias/name, then
/// sorted per `sort` -- shared by the main list and the picker so filtering/
/// sorting behavior can't drift between the two views.
fn filtered_sorted_entries(ui: &Rc<Ui>, filter: &str, sort: SortMode) -> Vec<AddressBookEntry> {
    let filter = filter.trim().to_lowercase();
    let mut entries: Vec<AddressBookEntry> = ui
        .state
        .config
        .borrow()
        .address_book
        .iter()
        .filter(|e| {
            filter.is_empty()
                || e.callsign.to_lowercase().contains(&filter)
                || e.alias.as_deref().unwrap_or("").to_lowercase().contains(&filter)
                || e.name.as_deref().unwrap_or("").to_lowercase().contains(&filter)
        })
        .cloned()
        .collect();
    match sort {
        SortMode::Callsign => entries.sort_by(|a, b| a.callsign.cmp(&b.callsign)),
        SortMode::LastHeard => entries.sort_by(|a, b| b.last_heard.cmp(&a.last_heard).then_with(|| a.callsign.cmp(&b.callsign))),
    }
    entries
}

/// A filter entry + sort-toggle button row, shared by the main dialog and
/// the picker. Returns the widgets so the caller can wire up its own
/// rebuild-on-change handlers (the two views rebuild differently, so the
/// wiring itself isn't shared, only the widget construction).
fn filter_sort_row(sort_state: &Rc<Cell<SortMode>>) -> (gtk::Box, gtk::Entry, gtk::Button) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let filter_entry = gtk::Entry::builder().placeholder_text("Filter\u{2026}").hexpand(true).build();
    filter_entry.set_icon_from_icon_name(gtk::EntryIconPosition::Secondary, Some("edit-clear-symbolic"));
    filter_entry.set_icon_activatable(gtk::EntryIconPosition::Secondary, true);
    filter_entry.set_icon_tooltip_text(gtk::EntryIconPosition::Secondary, Some("Clear filter"));
    filter_entry.connect_icon_release(|entry, pos| {
        if pos == gtk::EntryIconPosition::Secondary {
            entry.set_text("");
        }
    });
    row.append(&filter_entry);

    let sort_button = gtk::Button::from_icon_name("view-sort-descending-symbolic");
    sort_button.set_tooltip_text(Some(sort_state.get().label()));
    row.append(&sort_button);

    (row, filter_entry, sort_button)
}

/// The dot + callsign/alias + last-heard subtitle content, shared by the
/// main list and the picker rows.
fn entry_row_content(entry: &AddressBookEntry) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_margin_top(6);
    row.set_margin_bottom(6);
    row.set_margin_start(6);
    row.set_margin_end(6);

    // A pale-orange dot marks a station we only know about via a NET/ROM
    // NODES broadcast, never heard directly ourselves (`.heard-indirect-dot`,
    // defined in `window::apply_base_css`). A plain space of the same width
    // keeps callsigns aligned into a column either way.
    let dot = gtk::Label::new(Some(if entry.heard_direct { " " } else { "\u{25CF}" }));
    if !entry.heard_direct {
        dot.add_css_class("heard-indirect-dot");
    }
    dot.set_width_chars(1);
    row.append(&dot);

    let mut summary = entry.callsign.clone();
    if let Some(alias) = entry.alias.as_deref().filter(|s| !s.is_empty()) {
        summary.push_str(&format!("  \u{2014}  {alias}"));
    }
    let detail = match &entry.last_heard {
        Some(when) => format!("Last heard {when}"),
        None => "Manual entry".to_string(),
    };

    let text_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let summary_label = gtk::Label::new(Some(&summary));
    summary_label.set_halign(gtk::Align::Start);
    summary_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let detail_label = gtk::Label::new(Some(&detail));
    detail_label.set_halign(gtk::Align::Start);
    detail_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    detail_label.add_css_class("dim-label");
    detail_label.add_css_class("caption");
    text_box.append(&summary_label);
    text_box.append(&detail_label);
    text_box.set_hexpand(true);
    text_box.set_valign(gtk::Align::Center);
    row.append(&text_box);

    row.upcast()
}

/// Open the Address Book: stations heard automatically on any port (via
/// `PortEvent::StationHeard`, or indirectly via a NET/ROM `PortEvent::NodesBroadcast`
/// -- see `AppState::record_heard`/`record_nodes_broadcast`) plus manually-
/// entered ones, with add/remove and a per-station detail view.
pub fn show(ui: &Rc<Ui>) {
    let (win, root) = dialog_window(&ui.window, "Address Book", 560);
    win.set_default_height(440);

    let sort_state = Rc::new(Cell::new(SortMode::LastHeard));
    let (filter_row, filter_entry, sort_button) = filter_sort_row(&sort_state);
    root.append(&filter_row);

    let list_box = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    list_box.add_css_class("boxed-list");
    let scrolled = gtk::ScrolledWindow::builder()
        .child(&list_box)
        .vexpand(true)
        .min_content_height(260)
        .build();
    root.append(&scrolled);

    rebuild_list(ui, &list_box, &filter_entry, &sort_state);

    {
        let ui = ui.clone();
        let list_box = list_box.clone();
        let sort_state = sort_state.clone();
        filter_entry.connect_changed(move |entry| rebuild_list(&ui, &list_box, entry, &sort_state));
    }
    {
        let ui = ui.clone();
        let list_box = list_box.clone();
        let filter_entry = filter_entry.clone();
        let sort_state = sort_state.clone();
        let sort_button_self = sort_button.clone();
        sort_button.connect_clicked(move |_| {
            sort_state.set(sort_state.get().toggled());
            sort_button_self.set_tooltip_text(Some(sort_state.get().label()));
            rebuild_list(&ui, &list_box, &filter_entry, &sort_state);
        });
    }

    let button_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);

    let add_button = gtk::Button::with_label("Add Entry\u{2026}");
    {
        let ui = ui.clone();
        let win = win.clone();
        let list_box = list_box.clone();
        let filter_entry = filter_entry.clone();
        let sort_state = sort_state.clone();
        add_button.connect_clicked(move |_| {
            edit_entry_dialog(&ui, &win, None, make_refresh(&ui, &list_box, &filter_entry, &sort_state));
        });
    }
    button_row.append(&add_button);

    // "Export ADIF..." exports real connected-mode QSOs (`AppConfig.qso_log`)
    // — distinct from this list, which includes any monitored traffic.
    let export_button = gtk::Button::with_label("Export ADIF\u{2026}");
    {
        let ui = ui.clone();
        export_button.connect_clicked(move |_| {
            let adif = crate::adif::format_adif(&ui.state.config.borrow().qso_log);
            crate::export::save_text(&ui.window, "log.adi", adif, None);
        });
    }
    button_row.append(&export_button);

    root.append(&button_row);

    win.present();
}

/// Builds an `Rc<dyn Fn()>` that re-runs `rebuild_list` with the given
/// list/filter/sort state, so deeply-nested dialogs (detail view, edit
/// dialog) can trigger a refresh without needing to know about filter/sort
/// internals themselves.
fn make_refresh(ui: &Rc<Ui>, list_box: &gtk::ListBox, filter_entry: &gtk::Entry, sort_state: &Rc<Cell<SortMode>>) -> Rc<dyn Fn()> {
    let ui = ui.clone();
    let list_box = list_box.clone();
    let filter_entry = filter_entry.clone();
    let sort_state = sort_state.clone();
    Rc::new(move || rebuild_list(&ui, &list_box, &filter_entry, &sort_state))
}

fn rebuild_list(ui: &Rc<Ui>, list_box: &gtk::ListBox, filter_entry: &gtk::Entry, sort_state: &Rc<Cell<SortMode>>) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    let entries = filtered_sorted_entries(ui, &filter_entry.text(), sort_state.get());
    if entries.is_empty() {
        let text = if ui.state.config.borrow().address_book.is_empty() {
            "No known stations yet. Entries appear automatically as stations are heard, or add one manually."
        } else {
            "No entries match the filter."
        };
        let label = gtk::Label::new(Some(text));
        label.set_wrap(true);
        label.set_margin_top(12);
        label.set_margin_bottom(12);
        list_box.append(&label);
        return;
    }
    let refresh = make_refresh(ui, list_box, filter_entry, sort_state);
    for entry in entries {
        list_box.append(&build_entry_row(ui, entry, refresh.clone()));
    }
}

/// A whole-row-clickable button (opens the detail view) plus a separate
/// "Remove" button. Clicking anywhere on the row content -- not just a
/// dedicated "Edit" button -- opens `show_detail`; "Remove" stays its own
/// explicit button so it can't be triggered by an accidental row click.
fn build_entry_row(ui: &Rc<Ui>, entry: AddressBookEntry, refresh: Rc<dyn Fn()>) -> gtk::Widget {
    let outer = gtk::Box::new(gtk::Orientation::Horizontal, 4);

    let content_button = gtk::Button::new();
    content_button.add_css_class("flat");
    content_button.set_child(Some(&entry_row_content(&entry)));
    content_button.set_hexpand(true);
    {
        let ui = ui.clone();
        let entry = entry.clone();
        let refresh = refresh.clone();
        content_button.connect_clicked(move |btn| {
            let win = btn
                .root()
                .and_then(|r| r.downcast::<adw::Window>().ok())
                .expect("row is inside a Window");
            show_detail(&ui, entry.clone(), &win, refresh.clone());
        });
    }
    outer.append(&content_button);

    let remove_button = gtk::Button::with_label("Remove");
    {
        let ui = ui.clone();
        let callsign = entry.callsign.clone();
        remove_button.connect_clicked(move |_| {
            ui.state.config.borrow_mut().address_book.retain(|e| e.callsign != callsign);
            ui.state.save_config();
            refresh();
        });
    }
    outer.append(&remove_button);

    outer.upcast()
}

/// Per-station detail view: read-only heard-telemetry (direct/indirect
/// status, heard count/last heard, last 5 unique BEACON packets) plus one
/// editable field (Alias). An "Edit Details..." button reaches the existing
/// `edit_entry_dialog` for name/location/notes/via/home_bbs/QRZ lookup.
fn show_detail(ui: &Rc<Ui>, entry: AddressBookEntry, parent: &adw::Window, refresh: Rc<dyn Fn()>) {
    let (win, root) = dialog_window(parent, &entry.callsign, 420);

    let status_text = if entry.heard_direct {
        "Heard directly"
    } else {
        "Known only via a NET/ROM NODES broadcast \u{2014} never heard directly"
    };
    let status_label = gtk::Label::new(Some(status_text));
    status_label.set_halign(gtk::Align::Start);
    status_label.add_css_class("dim-label");
    root.append(&status_label);

    let heard_text = match &entry.last_heard {
        Some(when) => format!("Heard {} time(s) \u{2014} last: {when}", entry.heard_count),
        None => "Never heard".to_string(),
    };
    let heard_label = gtk::Label::new(Some(&heard_text));
    heard_label.set_halign(gtk::Align::Start);
    root.append(&heard_label);

    let alias_entry = gtk::Entry::builder().placeholder_text("Node/BBS alias (optional)").build();
    alias_entry.set_text(entry.alias.as_deref().unwrap_or(""));
    root.append(&labeled("Alias", &alias_entry));

    let beacons_heading = gtk::Label::new(Some("Last BEACON packets"));
    beacons_heading.set_halign(gtk::Align::Start);
    beacons_heading.set_margin_top(6);
    root.append(&beacons_heading);
    if entry.recent_beacons.is_empty() {
        let none_label = gtk::Label::new(Some("No packets to \u{201c}BEACON\u{201d} seen yet."));
        none_label.set_halign(gtk::Align::Start);
        none_label.add_css_class("dim-label");
        root.append(&none_label);
    } else {
        for beacon in &entry.recent_beacons {
            let line = gtk::Label::new(Some(&format!("{}  \u{2014}  {}", beacon.when, beacon.text)));
            line.set_halign(gtk::Align::Start);
            line.set_wrap(true);
            line.set_ellipsize(gtk::pango::EllipsizeMode::End);
            root.append(&line);
        }
    }

    let button_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    button_row.set_halign(gtk::Align::End);
    button_row.set_margin_top(6);

    let edit_details_button = gtk::Button::with_label("Edit Details\u{2026}");
    {
        let ui = ui.clone();
        let win = win.clone();
        let entry = entry.clone();
        let refresh = refresh.clone();
        edit_details_button.connect_clicked(move |_| {
            edit_entry_dialog(&ui, &win, Some(entry.clone()), refresh.clone());
        });
    }
    button_row.append(&edit_details_button);

    let save_button = gtk::Button::with_label("Save Alias");
    save_button.add_css_class("suggested-action");
    {
        let ui = ui.clone();
        let win = win.clone();
        let callsign = entry.callsign.clone();
        save_button.connect_clicked(move |_| {
            let alias = alias_entry.text().trim().to_string();
            let mut cfg = ui.state.config.borrow_mut();
            if let Some(slot) = cfg.address_book.iter_mut().find(|e| e.callsign == callsign) {
                slot.alias = if alias.is_empty() { None } else { Some(alias) };
            }
            drop(cfg);
            ui.state.save_config();
            refresh();
            win.close();
        });
    }
    button_row.append(&save_button);

    root.append(&button_row);

    win.present();
}

/// A selection-only picker over the address book: same filter/sort as the
/// main dialog, but no Add/Edit/Remove controls -- clicking a row calls
/// `on_pick` and closes the picker immediately (matching how the dial
/// dialog's old inline dropdown behaved).
pub fn show_picker(ui: &Rc<Ui>, parent: &impl IsA<gtk::Window>, on_pick: impl Fn(&AddressBookEntry) + 'static) {
    let (win, root) = dialog_window(parent, "Choose from Address Book", 460);
    win.set_default_height(420);

    let sort_state = Rc::new(Cell::new(SortMode::Callsign));
    let (filter_row, filter_entry, sort_button) = filter_sort_row(&sort_state);
    root.append(&filter_row);

    let list_box = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    list_box.add_css_class("boxed-list");
    let scrolled = gtk::ScrolledWindow::builder()
        .child(&list_box)
        .vexpand(true)
        .min_content_height(280)
        .build();
    root.append(&scrolled);

    let on_pick: Rc<dyn Fn(&AddressBookEntry)> = Rc::new(on_pick);

    rebuild_picker_list(ui, &list_box, &filter_entry, &sort_state, &win, &on_pick);
    {
        let ui = ui.clone();
        let list_box = list_box.clone();
        let sort_state = sort_state.clone();
        let win = win.clone();
        let on_pick = on_pick.clone();
        filter_entry.connect_changed(move |entry| rebuild_picker_list(&ui, &list_box, entry, &sort_state, &win, &on_pick));
    }
    {
        let ui = ui.clone();
        let list_box = list_box.clone();
        let filter_entry = filter_entry.clone();
        let sort_state = sort_state.clone();
        let win = win.clone();
        let on_pick = on_pick.clone();
        let sort_button_self = sort_button.clone();
        sort_button.connect_clicked(move |_| {
            sort_state.set(sort_state.get().toggled());
            sort_button_self.set_tooltip_text(Some(sort_state.get().label()));
            rebuild_picker_list(&ui, &list_box, &filter_entry, &sort_state, &win, &on_pick);
        });
    }

    let button_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    button_row.set_halign(gtk::Align::End);
    let cancel_button = gtk::Button::with_label("Cancel");
    {
        let win = win.clone();
        cancel_button.connect_clicked(move |_| win.close());
    }
    button_row.append(&cancel_button);
    root.append(&button_row);

    win.present();
}

fn rebuild_picker_list(
    ui: &Rc<Ui>,
    list_box: &gtk::ListBox,
    filter_entry: &gtk::Entry,
    sort_state: &Rc<Cell<SortMode>>,
    win: &adw::Window,
    on_pick: &Rc<dyn Fn(&AddressBookEntry)>,
) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    let entries = filtered_sorted_entries(ui, &filter_entry.text(), sort_state.get());
    if entries.is_empty() {
        let text = if ui.state.config.borrow().address_book.is_empty() {
            "No known stations yet."
        } else {
            "No entries match the filter."
        };
        let label = gtk::Label::new(Some(text));
        label.set_wrap(true);
        label.set_margin_top(12);
        label.set_margin_bottom(12);
        list_box.append(&label);
        return;
    }
    for entry in entries {
        let button = gtk::Button::new();
        button.add_css_class("flat");
        button.set_child(Some(&entry_row_content(&entry)));
        {
            let win = win.clone();
            let on_pick = on_pick.clone();
            let entry = entry.clone();
            button.connect_clicked(move |_| {
                on_pick(&entry);
                win.close();
            });
        }
        list_box.append(&button);
    }
}

/// A label above a small multi-line, scrollable text view, for the Notes
/// field. Returns the container to append and the buffer to read/write.
fn labeled_notes(text: &str, initial: &str) -> (gtk::Box, gtk::TextBuffer) {
    let container = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let label = gtk::Label::new(Some(text));
    label.set_halign(gtk::Align::Start);
    container.append(&label);

    let text_view = gtk::TextView::builder().wrap_mode(gtk::WrapMode::WordChar).build();
    let buffer = text_view.buffer();
    buffer.set_text(initial);
    let scrolled = gtk::ScrolledWindow::builder()
        .child(&text_view)
        .min_content_height(100)
        .has_frame(true)
        .build();
    container.append(&scrolled);

    (container, buffer)
}

/// Full editable fields (name/location/notes/via/home_bbs + QRZ lookup) for
/// a manually-added or auto-created entry. Reached either directly ("Add
/// Entry...") or via the detail view's "Edit Details..." button -- the
/// auto-tracked heard-telemetry (`heard_direct`/`heard_count`/`last_heard`/
/// `recent_beacons`) lives in `show_detail` instead and is never editable
/// here.
fn edit_entry_dialog(ui: &Rc<Ui>, parent: &adw::Window, existing: Option<AddressBookEntry>, refresh: Rc<dyn Fn()>) {
    let (win, root) = dialog_window(parent, if existing.is_some() { "Edit Station" } else { "Add Station" }, 420);

    let name_entry = gtk::Entry::builder().placeholder_text("Operator name (optional)").build();
    let callsign_entry = gtk::Entry::builder().placeholder_text("N0CALL-1").build();
    let via_entry = gtk::Entry::builder().placeholder_text("Digipeater path, e.g. WIDE1-1,WIDE2-1 (optional)").build();
    force_uppercase(&via_entry);
    let home_bbs_entry = gtk::Entry::builder().placeholder_text("Home BBS address (optional)").build();
    force_uppercase(&home_bbs_entry);
    let alias_entry = gtk::Entry::builder().placeholder_text("Node/BBS alias (optional)").build();
    let location_entry = gtk::Entry::builder().placeholder_text("City, state, grid square\u{2026} (optional)").build();
    if let Some(e) = &existing {
        name_entry.set_text(e.name.as_deref().unwrap_or(""));
        callsign_entry.set_text(&e.callsign);
        via_entry.set_text(&e.via);
        home_bbs_entry.set_text(&e.home_bbs);
        alias_entry.set_text(e.alias.as_deref().unwrap_or(""));
        location_entry.set_text(e.location.as_deref().unwrap_or(""));
    }
    root.append(&labeled("Name", &name_entry));
    root.append(&labeled("Callsign", &callsign_entry));
    root.append(&labeled("Via", &via_entry));
    root.append(&labeled("Home BBS", &home_bbs_entry));
    root.append(&labeled("Alias", &alias_entry));
    root.append(&labeled("Location", &location_entry));

    let (notes_container, notes_buffer) =
        labeled_notes("Notes", existing.as_ref().and_then(|e| e.notes.as_deref()).unwrap_or(""));
    root.append(&notes_container);

    if let Some(e) = &existing {
        let detail = match &e.last_heard {
            Some(when) => format!("Heard {} time(s), last: {when}", e.heard_count),
            None => "Never heard \u{2014} manual entry".to_string(),
        };
        let detail_label = gtk::Label::new(Some(&detail));
        detail_label.set_halign(gtk::Align::Start);
        detail_label.add_css_class("dim-label");
        root.append(&detail_label);
    }

    let lookup_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let lookup_button = gtk::Button::with_label("Lookup QRZ\u{2026}");
    let lookup_status = gtk::Label::new(None);
    lookup_status.set_halign(gtk::Align::Start);
    lookup_status.add_css_class("dim-label");
    lookup_row.append(&lookup_button);
    lookup_row.append(&lookup_status);
    root.append(&lookup_row);
    {
        let ui = ui.clone();
        let callsign_entry = callsign_entry.clone();
        let name_entry = name_entry.clone();
        let location_entry = location_entry.clone();
        let lookup_status = lookup_status.clone();
        lookup_button.connect_clicked(move |_| {
            let callsign = callsign_entry.text().trim().to_uppercase();
            if callsign.is_empty() {
                lookup_status.set_text("Enter a callsign first.");
                return;
            }
            let (username, password) = {
                let cfg = ui.state.config.borrow();
                (cfg.ui.qrz_username.clone(), cfg.ui.qrz_password.clone())
            };
            let (Some(username), Some(password)) = (username, password) else {
                lookup_status.set_text("Configure QRZ credentials in Preferences first.");
                return;
            };
            let session = ui.state.qrz_session.borrow().clone();
            lookup_status.set_text("Looking up\u{2026}");

            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let mut session = session;
                let result = crate::qrz::lookup(&username, &password, &mut session, &callsign);
                let _ = tx.send_blocking((result, session));
            });

            let ui = ui.clone();
            let name_entry = name_entry.clone();
            let location_entry = location_entry.clone();
            let lookup_status = lookup_status.clone();
            glib::spawn_future_local(async move {
                let Ok((result, new_session)) = rx.recv().await else { return };
                *ui.state.qrz_session.borrow_mut() = new_session;
                match result {
                    Ok(info) => {
                        if let Some(name) = info.name {
                            name_entry.set_text(&name);
                        }
                        let loc = match (info.location, info.grid) {
                            (Some(l), Some(g)) => Some(format!("{l} ({g})")),
                            (Some(l), None) => Some(l),
                            (None, Some(g)) => Some(g),
                            (None, None) => None,
                        };
                        if let Some(loc) = loc {
                            location_entry.set_text(&loc);
                        }
                        lookup_status.set_text("Lookup succeeded.");
                    }
                    Err(e) => lookup_status.set_text(&format!("Lookup failed: {e}")),
                }
            });
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
    let save_button = gtk::Button::with_label("Save");
    save_button.add_css_class("suggested-action");
    {
        let ui = ui.clone();
        let win = win.clone();
        let original_callsign = existing.as_ref().map(|e| e.callsign.clone());
        save_button.connect_clicked(move |_| {
            let callsign = callsign_entry.text().trim().to_uppercase();
            if callsign.is_empty() {
                error_label.set_text("Callsign is required.");
                return;
            }
            let alias = alias_entry.text().to_string();
            let name = name_entry.text().to_string();
            let location = location_entry.text().to_string();
            let via = via_entry.text().trim().to_uppercase();
            let home_bbs = home_bbs_entry.text().trim().to_uppercase();
            let notes = notes_buffer.text(&notes_buffer.start_iter(), &notes_buffer.end_iter(), true).to_string();

            let mut cfg = ui.state.config.borrow_mut();
            let key = original_callsign.as_deref().unwrap_or(&callsign);
            if let Some(slot) = cfg.address_book.iter_mut().find(|e| e.callsign == key) {
                slot.callsign = callsign;
                slot.alias = if alias.is_empty() { None } else { Some(alias) };
                slot.name = if name.is_empty() { None } else { Some(name) };
                slot.location = if location.is_empty() { None } else { Some(location) };
                slot.notes = if notes.is_empty() { None } else { Some(notes) };
                slot.via = via;
                slot.home_bbs = home_bbs;
            } else {
                cfg.address_book.push(AddressBookEntry {
                    callsign,
                    alias: if alias.is_empty() { None } else { Some(alias) },
                    name: if name.is_empty() { None } else { Some(name) },
                    location: if location.is_empty() { None } else { Some(location) },
                    notes: if notes.is_empty() { None } else { Some(notes) },
                    last_heard: None,
                    heard_count: 0,
                    via,
                    home_bbs,
                    heard_direct: true,
                    recent_beacons: Vec::new(),
                });
            }
            drop(cfg);
            ui.state.save_config();
            refresh();
            win.close();
        });
    }
    button_row.append(&cancel_button);
    button_row.append(&save_button);
    root.append(&button_row);

    win.present();
}
