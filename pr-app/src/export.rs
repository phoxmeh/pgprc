//! Shared "save this text to a file" helper, used for session/Monitor
//! transcript export and ADIF export.

use gtk::prelude::*;

/// Opens a native Save dialog and, if the user picks a location, writes
/// `text` there. Errors are logged, not surfaced to the user — this mirrors
/// how `AppState::save_config` already handles its own I/O failures.
/// `initial_folder`, when given, defaults the dialog into that directory
/// (created first if needed) — used so manual saves start out in the same
/// `history/` tree the app organizes its own archives under.
pub fn save_text(
    parent: &impl IsA<gtk::Window>,
    suggested_name: &str,
    text: String,
    initial_folder: Option<&std::path::Path>,
) {
    let mut builder = gtk::FileDialog::builder().title("Save").initial_name(suggested_name);
    if let Some(dir) = initial_folder {
        let _ = std::fs::create_dir_all(dir);
        builder = builder.initial_folder(&gtk::gio::File::for_path(dir));
    }
    let dialog = builder.build();
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
