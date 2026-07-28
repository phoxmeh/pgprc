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

/// The auto-managed, ever-appended scrollback file for one (port, node,
/// mode) — the program's own record, read back (tail-capped) as the
/// "previous conversation" preview.
pub fn history_file_path(config_dir: &Path, port_name: &str, remote: &str, unproto: bool) -> PathBuf {
    let node = sanitize_component(remote);
    let filename = if unproto { format!("{node}_unproto.txt") } else { format!("{node}.txt") };
    history_dir(config_dir, port_name).join(filename)
}

/// A one-off archive/capture filename: `<node>_<YYYY-MM-DD>_<HHMMSS>.txt`,
/// used by both the manual "Save..." export and the live-capture checkbox
/// so a single conversation can accumulate several distinct dated captures
/// over time without overwriting each other.
pub fn capture_file_path(config_dir: &Path, port_name: &str, node: &str, date: &str, time: &str) -> PathBuf {
    let node = sanitize_component(node);
    history_dir(config_dir, port_name).join(format!("{node}_{date}_{time}.txt"))
}
