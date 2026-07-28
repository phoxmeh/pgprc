//! The "Help" window (menu → Help): a quick-reference summary of basic
//! usage and keyboard shortcuts, for when the README isn't handy. Kept
//! deliberately short — the README has the full walkthrough.

use std::rc::Rc;

use adw::prelude::*;

use crate::ports_dialog::dialog_window;
use crate::window::Ui;

const USAGE_STEPS: &str = "\
1. Open the menu (hamburger icon) \u{2192} Ports\u{2026} and add a port.
2. Click + on the tab bar to open a new session tab, pick the port, and \
(for node-capable ports) enter a destination callsign \u{2014} or pick one \
from the Address Book via the small arrow next to the node field.
3. Press Connect. Type in the input box and press Enter or click Send.
4. Check Unproto in a tab to send one-shot unconnected frames instead of \
opening a session.
5. Use the menu for Address Book, Mailbox, Beacons, and Preferences.
6. Right-click the handset icon in the header to start/stop a managed \
Direwolf process and view its console.";

const SHORTCUTS: [(&str, &str); 6] = [
    ("Escape", "Close the frontmost dialog"),
    ("Ctrl+N", "New session tab"),
    ("Ctrl+W", "Close the current session tab"),
    ("Ctrl+,", "Open Preferences"),
    ("Ctrl+F", "Focus the Monitor filter"),
    ("Ctrl+Q", "Quit"),
];

pub fn show(ui: &Rc<Ui>) {
    let (win, root) = dialog_window(&ui.window, "Help", 520);
    win.set_default_height(520);

    let scrolled = gtk::ScrolledWindow::builder().vexpand(true).build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 16);

    let usage_group = adw::PreferencesGroup::builder().title("Basic Usage").description(USAGE_STEPS).build();
    content.append(&usage_group);

    let shortcuts_group = adw::PreferencesGroup::builder().title("Keyboard Shortcuts").build();
    for (keys, description) in SHORTCUTS {
        let row = adw::ActionRow::builder().title(description).build();
        let keys_label = gtk::Label::new(Some(keys));
        keys_label.add_css_class("dim-label");
        keys_label.add_css_class("caption");
        row.add_suffix(&keys_label);
        shortcuts_group.add(&row);
    }
    content.append(&shortcuts_group);

    scrolled.set_child(Some(&content));
    root.append(&scrolled);

    win.present();
}
