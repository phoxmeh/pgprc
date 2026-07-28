//! Manages an optional Direwolf child process this app starts/stops
//! itself — entirely separate from any AGWPE/KISS *port* pointed at it
//! (Ports still has to be configured and connected the normal way; this
//! only owns the OS process). Captures stdout/stderr into a small log
//! buffer shown in a read-only window on right-click, and tracks state so
//! the header button can reflect it (green = running, yellow = failed to
//! start).

use std::cell::{Cell, RefCell};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirewolfState {
    Stopped,
    Running,
    FailedToStart,
}

pub struct DirewolfProcess {
    pid: Cell<Option<u32>>,
    pub state: Cell<DirewolfState>,
    buffer: gtk::TextBuffer,
    /// Every registered callback fires on each state/log change — e.g. the
    /// header button's color/tooltip, plus (while open) the console
    /// window's Start/Stop sensitivity. Callbacks are never removed once
    /// added, so callers that close/reopen a window repeatedly (like the
    /// console) must capture widgets via `glib::WeakRef`, not a strong
    /// clone, or every open would leak that window's whole widget tree.
    on_change: RefCell<Vec<Rc<dyn Fn()>>>,
}

impl DirewolfProcess {
    pub fn new() -> Rc<Self> {
        Rc::new(DirewolfProcess {
            pid: Cell::new(None),
            state: Cell::new(DirewolfState::Stopped),
            buffer: gtk::TextBuffer::new(None::<&gtk::TextTagTable>),
            on_change: RefCell::new(Vec::new()),
        })
    }

    /// Register a callback that fires after every state/log change, for as
    /// long as the app runs (see the field doc on why this never
    /// unregisters — use `glib::WeakRef` for any widget it touches).
    pub fn add_on_change(&self, f: impl Fn() + 'static) {
        self.on_change.borrow_mut().push(Rc::new(f));
    }

    fn notify(&self) {
        for f in self.on_change.borrow().clone() {
            f();
        }
    }

    pub fn is_running(&self) -> bool {
        self.pid.get().is_some()
    }

    pub fn log_buffer(&self) -> &gtk::TextBuffer {
        &self.buffer
    }

    pub fn full_log_text(&self) -> String {
        self.buffer.text(&self.buffer.start_iter(), &self.buffer.end_iter(), true).to_string()
    }

    fn append_log(&self, line: &str) {
        // GTK's string marshaling panics on embedded NUL bytes; Direwolf's
        // own output shouldn't contain any, but backend data elsewhere in
        // this app has surprised us before (see AGWPE null-padded fields).
        let sanitized = if line.contains('\0') { line.replace('\0', "") } else { line.to_string() };
        let mut end = self.buffer.end_iter();
        self.buffer.insert(&mut end, &format!("{sanitized}\n"));
    }

    /// Start Direwolf (no-op if already running): writes `config_text` to
    /// `config_path` — so it's edited as one plain file, matching
    /// Direwolf's own on-disk config format exactly — and launches
    /// `direwolf -c <path>` with its working directory set to the config's
    /// own directory, since Direwolf resolves relative paths (audio device
    /// files, log paths) against cwd.
    pub fn start(self: &Rc<Self>, config_path: &Path, config_text: &str) {
        if self.is_running() {
            return;
        }
        if let Some(parent) = config_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                self.append_log(&format!("Failed to create {}: {e}", parent.display()));
                self.state.set(DirewolfState::FailedToStart);
                self.notify();
                return;
            }
        }
        if let Err(e) = std::fs::write(config_path, config_text) {
            self.append_log(&format!("Failed to write {}: {e}", config_path.display()));
            self.state.set(DirewolfState::FailedToStart);
            self.notify();
            return;
        }

        let mut command = Command::new("direwolf");
        command.arg("-c").arg(config_path);
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        if let Some(parent) = config_path.parent() {
            command.current_dir(parent);
        }

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) => {
                self.append_log(&format!("Failed to start direwolf: {e}"));
                self.state.set(DirewolfState::FailedToStart);
                self.notify();
                return;
            }
        };

        self.pid.set(Some(child.id()));
        self.state.set(DirewolfState::Running);
        self.append_log(&format!("--- direwolf started (pid {}) ---", child.id()));
        self.notify();

        if let Some(stdout) = child.stdout.take() {
            self.spawn_log_reader(stdout);
        }
        if let Some(stderr) = child.stderr.take() {
            self.spawn_log_reader(stderr);
        }

        // Hand the `Child` itself to a dedicated blocking-wait thread —
        // `stop()` only needs the pid (already saved above) to send a
        // signal, not the `Child` handle, so this is the only place that
        // needs it.
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let status = child.wait();
            let _ = tx.send_blocking(status);
        });
        let this = self.clone();
        glib::spawn_future_local(async move {
            if let Ok(status) = rx.recv().await {
                this.pid.set(None);
                this.state.set(DirewolfState::Stopped);
                let desc = status.map(|s| s.to_string()).unwrap_or_else(|e| format!("wait error: {e}"));
                this.append_log(&format!("--- direwolf exited: {desc} ---"));
                this.notify();
            }
        });
    }

    fn spawn_log_reader(self: &Rc<Self>, stream: impl std::io::Read + Send + 'static) {
        let (tx, rx) = async_channel::unbounded::<String>();
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines() {
                match line {
                    Ok(line) => {
                        if tx.send_blocking(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        let this = self.clone();
        glib::spawn_future_local(async move {
            while let Ok(line) = rx.recv().await {
                this.append_log(&line);
                this.notify();
            }
        });
    }

    /// Ask the process to shut down (SIGTERM — the same clean-shutdown
    /// request Ctrl+C in a terminal sends); the exit-watcher spawned in
    /// `start` picks up the actual exit asynchronously and updates state.
    pub fn stop(&self) {
        if let Some(pid) = self.pid.get() {
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
            }
        }
    }
}
