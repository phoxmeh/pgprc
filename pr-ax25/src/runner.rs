use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use pr_core::{ConnState, ConnectionId, PortCommand, PortEvent, PortRunner};

use crate::raw_socket::{read_axports, RawAx25Socket};

/// A port backed by the Linux kernel `AF_AX25` stack. `device` names an
/// entry in `/etc/ax25/axports`, which supplies the local callsign to bind
/// to (the kernel then routes traffic to whatever interface that axports
/// entry was attached to via `kissattach`/`ax25rtd`).
///
/// Each outgoing connection opens its own `SOCK_SEQPACKET` socket bound to
/// that local callsign. A separate listening socket (best-effort — see
/// `run`) accepts incoming connections too, e.g. for the personal mailbox.
pub struct Ax25RawSocketRunner {
    pub device: String,
}

impl PortRunner for Ax25RawSocketRunner {
    fn run(self: Box<Self>, cmd_rx: mpsc::Receiver<PortCommand>, event_tx: async_channel::Sender<PortEvent>) {
        let local_call = match resolve_local_call(&self.device) {
            Ok(call) => call,
            Err(e) => {
                let _ = event_tx.send_blocking(PortEvent::PortError { message: e });
                return;
            }
        };
        let _ = event_tx.send_blocking(PortEvent::PortConnected);

        let sockets: Arc<Mutex<HashMap<ConnectionId, RawAx25Socket>>> = Arc::new(Mutex::new(HashMap::new()));
        let next_id = Arc::new(AtomicU64::new(1));
        let mut readers = Vec::new();

        // Best-effort: not every axports/kernel setup necessarily allows a
        // separate listening bind alongside outgoing sockets on the same
        // callsign, so a failure here just means incoming connections
        // aren't available on this port, not a fatal port error.
        let accept_handle = RawAx25Socket::bind(&local_call).ok().and_then(|listener| {
            listener.listen(4).ok()?;
            let listener_shutdown = listener.try_clone().ok()?;
            let accept_events = event_tx.clone();
            let accept_sockets = sockets.clone();
            let accept_next_id = next_id.clone();
            let handle = thread::spawn(move || {
                accept_loop(listener, &accept_events, &accept_sockets, &accept_next_id);
            });
            Some((handle, listener_shutdown))
        });

        loop {
            match cmd_rx.recv() {
                Ok(PortCommand::OpenConnection { remote, via }) => {
                    let id = next_id.fetch_add(1, Ordering::SeqCst);
                    let _ = event_tx.send_blocking(PortEvent::StationHeard { callsign: remote.clone() });
                    let _ = event_tx.send_blocking(PortEvent::ConnectionOpened {
                        id,
                        label: remote.clone(),
                    });
                    let _ = event_tx.send_blocking(PortEvent::ConnState {
                        id,
                        state: ConnState::Connecting,
                    });
                    match open_connection(&local_call, &remote, &via) {
                        Ok(socket) => {
                            let reader_socket = match socket.try_clone() {
                                Ok(s) => s,
                                Err(e) => {
                                    let _ = event_tx.send_blocking(PortEvent::PortError {
                                        message: format!("clone ax25 socket: {e}"),
                                    });
                                    continue;
                                }
                            };
                            sockets.lock().unwrap().insert(id, socket);
                            let _ = event_tx.send_blocking(PortEvent::ConnState {
                                id,
                                state: ConnState::Connected,
                            });
                            let reader_events = event_tx.clone();
                            let reader_sockets = sockets.clone();
                            readers.push(thread::spawn(move || {
                                connection_read_loop(id, reader_socket, reader_events, reader_sockets);
                            }));
                        }
                        Err(e) => {
                            let _ = event_tx.send_blocking(PortEvent::Monitor {
                                line: format!("connect to {remote} failed: {e}"),
                                from: None,
                                to: None,
                                message: None,
                            });
                            let _ = event_tx.send_blocking(PortEvent::ConnState {
                                id,
                                state: ConnState::Disconnected,
                            });
                            let _ = event_tx.send_blocking(PortEvent::ConnectionClosed { id });
                        }
                    }
                }
                Ok(PortCommand::Send { id, bytes }) => {
                    if let Some(socket) = sockets.lock().unwrap().get(&id) {
                        let _ = socket.write(&bytes);
                    }
                }
                Ok(PortCommand::CloseConnection { id }) => {
                    if let Some(socket) = sockets.lock().unwrap().remove(&id) {
                        socket.shutdown();
                    }
                }
                Ok(PortCommand::Disconnect) => break,
                Ok(PortCommand::Connect) => {}
                // Sending unconnected UI frames over a raw AF_AX25 socket
                // would need a separate SOCK_DGRAM socket; not implemented yet.
                Ok(PortCommand::SendUnproto { .. }) => {}
                Err(_) => break,
            }
        }

        for socket in sockets.lock().unwrap().values() {
            socket.shutdown();
        }
        if let Some((handle, listener_shutdown)) = accept_handle {
            listener_shutdown.shutdown();
            let _ = handle.join();
        }
        let _ = event_tx.send_blocking(PortEvent::PortDisconnected { reason: None });
        for reader in readers {
            let _ = reader.join();
        }
    }
}

fn resolve_local_call(device: &str) -> Result<String, String> {
    let ports = read_axports().map_err(|e| format!("reading /etc/ax25/axports: {e}"))?;
    ports
        .into_iter()
        .find(|p| p.device == device)
        .map(|p| p.callsign)
        .ok_or_else(|| format!("no '{device}' entry in /etc/ax25/axports"))
}

fn open_connection(local_call: &str, remote_call: &str, via: &[String]) -> Result<RawAx25Socket, crate::raw_socket::Ax25Error> {
    let socket = RawAx25Socket::bind(local_call)?;
    socket.connect(remote_call, via)?;
    Ok(socket)
}

/// Accepts incoming connections until the listening socket is shut down
/// (from the main loop, on port disconnect) or errors out.
fn accept_loop(
    listener: RawAx25Socket,
    events: &async_channel::Sender<PortEvent>,
    sockets: &Arc<Mutex<HashMap<ConnectionId, RawAx25Socket>>>,
    next_id: &Arc<AtomicU64>,
) {
    loop {
        let (socket, remote) = match listener.accept() {
            Ok(pair) => pair,
            Err(_) => break,
        };
        let id = next_id.fetch_add(1, Ordering::SeqCst);
        let _ = events.send_blocking(PortEvent::StationHeard { callsign: remote.clone() });
        let _ = events.send_blocking(PortEvent::ConnectionOpened { id, label: remote });
        let _ = events.send_blocking(PortEvent::ConnState { id, state: ConnState::Connected });
        let reader_socket = match socket.try_clone() {
            Ok(s) => s,
            Err(_) => continue,
        };
        sockets.lock().unwrap().insert(id, socket);
        let reader_events = events.clone();
        let reader_sockets = sockets.clone();
        thread::spawn(move || {
            connection_read_loop(id, reader_socket, reader_events, reader_sockets);
        });
    }
}

fn connection_read_loop(
    id: ConnectionId,
    socket: RawAx25Socket,
    events: async_channel::Sender<PortEvent>,
    sockets: Arc<Mutex<HashMap<ConnectionId, RawAx25Socket>>>,
) {
    let mut buf = [0u8; 512];
    loop {
        match socket.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if events
                    .send_blocking(PortEvent::Data { id, bytes: buf[..n].to_vec() })
                    .is_err()
                {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    sockets.lock().unwrap().remove(&id);
    let _ = events.send_blocking(PortEvent::ConnState { id, state: ConnState::Disconnected });
    let _ = events.send_blocking(PortEvent::ConnectionClosed { id });
}
