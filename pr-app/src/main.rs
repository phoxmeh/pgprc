mod about_dialog;
mod address_book_dialog;
mod adif;
mod app_state;
mod beacons_dialog;
mod dial_dialog;
mod direwolf;
mod direwolf_dialog;
mod export;
mod help_dialog;
mod highlight;
mod incoming_beacons_dialog;
mod keyboard_mode;
mod keyboard_mode_dialog;
mod log_view;
mod mailbox;
mod mailbox_dialog;
mod monitor_view;
mod notify;
mod ports_dialog;
mod preferences_dialog;
mod qrz;
mod session_tab;
mod template_vars;
mod window;

use adw::prelude::*;
use gtk::glib;

const APP_ID: &str = "net.packetradio.PGPRC";

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt::init();
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(window::build_ui);
    app.run()
}
