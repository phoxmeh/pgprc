mod app_state;
mod connection_view;
mod monitor_view;
mod ports_dialog;
mod preferences_dialog;
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
