//! Shared path/filename conventions for the per-node history archive, used
//! both by `AppConfig`'s one-time legacy migration and by `pr-app`'s runtime
//! read/write of the same files — kept in one place so the two never drift
//! apart on naming.

use std::path::{Path, PathBuf};

/// Replace anything that isn't safe as a path component (slashes, colons,
/// whitespace, ...) with `_`, so port names and callsigns — which can
/// contain spaces or, in principle, stranger characters — always produce a
/// valid file/directory name.
pub fn sanitize_component(s: &str) -> String {
    let cleaned: String =
        s.trim().chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).collect();
    if cleaned.is_empty() {
        "_".to_string()
    } else {
        cleaned
    }
}

/// The directory holding every history file for one port, e.g.
/// `~/.config/packet-radio/history/Direwolf/`.
pub fn history_dir(config_dir: &Path, port_name: &str) -> PathBuf {
    config_dir.join("history").join(sanitize_component(port_name))
}

/// The auto-managed, ever-appended scrollback file for one (port, node) —
/// the program's own record, read back (tail-capped) as the "previous
/// conversation" preview. Every tab is a two-way connection now, so there's
/// no separate unproto-vs-connected bucket to key on any more.
pub fn history_file_path(config_dir: &Path, port_name: &str, remote: &str) -> PathBuf {
    let node = sanitize_component(remote);
    history_dir(config_dir, port_name).join(format!("{node}.txt"))
}
