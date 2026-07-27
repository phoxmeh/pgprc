use std::io::{Read, Write};
use std::sync::mpsc;
use std::thread;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

use crate::port::{ConnState, PortCommand, PortEvent, PortRunner};

use super::CONN_ID;

/// A terminal session reached via the system `ssh` binary running inside a
/// PTY. This deliberately shells out rather than reimplementing the SSH
/// protocol, so the user's existing keys, agent and `~/.ssh/config` all work
/// unmodified.
pub struct SshRunner {
    pub host: String,
    pub port: u16,
    pub user: String,
}

impl PortRunner for SshRunner {
    fn run(self: Box<Self>, cmd_rx: mpsc::Receiver<PortCommand>, event_tx: async_channel::Sender<PortEvent>) {
        let pty_system = native_pty_system();
        let pair = match pty_system.openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        }) {
            Ok(p) => p,
            Err(e) => {
                let _ = event_tx.send_blocking(PortEvent::PortError {
                    message: format!("openpty: {e}"),
                });
                return;
            }
        };

        let mut cmd = CommandBuilder::new("ssh");
        cmd.arg("-p");
        cmd.arg(self.port.to_string());
        cmd.arg(format!("{}@{}", self.user, self.host));

        let mut child = match pair.slave.spawn_command(cmd) {
            Ok(c) => c,
            Err(e) => {
                let _ = event_tx.send_blocking(PortEvent::PortError {
                    message: format!("spawn ssh: {e}"),
                });
                return;
            }
        };
        drop(pair.slave);

        let label = format!("{}@{}:{}", self.user, self.host, self.port);
        let _ = event_tx.send_blocking(PortEvent::PortConnected);
        let _ = event_tx.send_blocking(PortEvent::ConnectionOpened { id: CONN_ID, label });
        let _ = event_tx.send_blocking(PortEvent::ConnState {
            id: CONN_ID,
            state: ConnState::Connected,
        });

        let mut reader = match pair.master.try_clone_reader() {
            Ok(r) => r,
            Err(e) => {
                let _ = event_tx.send_blocking(PortEvent::PortError {
                    message: format!("clone pty reader: {e}"),
                });
                return;
            }
        };
        let mut writer = match pair.master.take_writer() {
            Ok(w) => w,
            Err(e) => {
                let _ = event_tx.send_blocking(PortEvent::PortError {
                    message: format!("take pty writer: {e}"),
                });
                return;
            }
        };

        let reader_events = event_tx.clone();
        let reader_handle = thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if reader_events
                            .send_blocking(PortEvent::Data {
                                id: CONN_ID,
                                bytes: buf[..n].to_vec(),
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        loop {
            match cmd_rx.recv() {
                Ok(PortCommand::Send { id, bytes }) if id == CONN_ID => {
                    if writer.write_all(&bytes).is_err() {
                        break;
                    }
                }
                Ok(PortCommand::Disconnect) | Ok(PortCommand::CloseConnection { .. }) => {
                    let _ = child.kill();
                    break;
                }
                Ok(_) => {}
                Err(_) => {
                    let _ = child.kill();
                    break;
                }
            }
        }

        let _ = child.wait();
        let _ = event_tx.send_blocking(PortEvent::ConnState {
            id: CONN_ID,
            state: ConnState::Disconnected,
        });
        let _ = event_tx.send_blocking(PortEvent::ConnectionClosed { id: CONN_ID });
        let _ = event_tx.send_blocking(PortEvent::PortDisconnected { reason: None });
        let _ = reader_handle.join();
    }
}
