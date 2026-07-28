//! The "About" dialog (menu → About), a standard `adw::AboutDialog`.

use std::rc::Rc;

use adw::prelude::*;

use crate::window::Ui;

pub fn show(ui: &Rc<Ui>) {
    let about = adw::AboutDialog::builder()
        .application_name("Pretty Good Packet Radio Client")
        .application_icon("net.packetradio.PGPRC")
        .version(env!("CARGO_PKG_VERSION"))
        .developer_name("KD3BFP")
        .copyright("\u{00A9} 2026 D'Vano Canto KD3BFP")
        .comments(
            "A Linux-native packet radio client supporting AGWPE, AX.25 \
             (raw kernel sockets), and bare KISS TNCs.",
        )
        .license_type(gtk::License::MitX11)
        .build();
    about.present(Some(&ui.window));
}
