//! Shared "save this text to a file" helper, used for session/Monitor
//! transcript export and ADIF export.

use gtk::prelude::*;

/// Opens a native Save dialog and, if the user picks a location, writes
/// `text` there. Errors are logged, not surfaced to the user — this mirrors
/// how `AppState::save_config` already handles its own I/O failures.
pub fn save_text(parent: &impl IsA<gtk::Window>, suggested_name: &str, text: String) {
    let dialog = gtk::FileDialog::builder().title("Save").initial_name(suggested_name).build();
    dialog.save(Some(parent), gtk::gio::Cancellable::NONE, move |result| {
        let file = match result {
            Ok(file) => file,
            Err(_) => return, // cancelled or failed to pick a location
        };
        let Some(path) = file.path() else { return };
        if let Err(e) = std::fs::write(&path, &text) {
            tracing::warn!("failed to save {}: {e}", path.display());
        }
    });
}
