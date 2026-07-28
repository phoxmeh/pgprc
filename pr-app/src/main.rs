mod address_book_dialog;
mod adif;
mod app_state;
mod beacons_dialog;
mod export;
mod highlight;
mod mailbox;
mod mailbox_dialog;
mod monitor_view;
mod notify;
mod ports_dialog;
mod preferences_dialog;
mod qrz;
mod rules_dialog;
mod session_tab;
mod window;

use adw::prelude::*;
use gtk::glib;

const APP_ID: &str = "net.packetradio.PacketRadio";

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt::init();
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(window::build_ui);
    app.run()
}
