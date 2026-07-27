use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use pr_core::{ConnState, ConnectionId, PortCommand, PortEvent, PortRunner};

use crate::codec::{AgwFrame, FrameDecoder};

pub struct AgwpeRunner {
    pub host: String,
    pub tcp_port: u16,
    pub radio_port: u8,
    pub my_call: String,
    pub login: Option<(String, String)>,
}

/// Maps between the UI-visible `ConnectionId` and the AX.25 callsign of the
/// remote station on the other end of that link.
#[derive(Default)]
struct ConnMap {
    id_to_call: HashMap<ConnectionId, String>,
    call_to_id: HashMap<String, ConnectionId>,
    next_incoming_id: u64,
}

impl ConnMap {
    fn insert(&mut self, id: ConnectionId, call: String) {
        self.id_to_call.insert(id, call.clone());
        self.call_to_id.insert(call, id);
    }

    fn id_for_call(&self, call: &str) -> Option<ConnectionId> {
        self.call_to_id.get(call).copied()
    }

    fn call_for_id(&self, id: ConnectionId) -> Option<String> {
        self.id_to_call.get(&id).cloned()
    }

    fn remove_call(&mut self, call: &str) {
        if let Some(id) = self.call_to_id.remove(call) {
            self.id_to_call.remove(&id);
        }
    }
}

// Incoming (peer-initiated) connections get ids from a high range so they
// never collide with ids the UI mints for outgoing connections.
const INCOMING_ID_BASE: u64 = 1 << 32;

impl PortRunner for AgwpeRunner {
    fn run(self: Box<Self>, cmd_rx: mpsc::Receiver<PortCommand>, event_tx: async_channel::Sender<PortEvent>) {
        let addr = format!("{}:{}", self.host, self.tcp_port);
        let stream = match TcpStream::connect(&addr) {
            Ok(s) => s,
            Err(e) => {
                let _ = event_tx.send_blocking(PortEvent::PortError {
                    message: format!("connect {addr} failed: {e}"),
                });
                return;
            }
        };
        let _ = event_tx.send_blocking(PortEvent::PortConnected);

        let mut writer = match stream.try_clone() {
            Ok(s) => s,
            Err(e) => {
                let _ = event_tx.send_blocking(PortEvent::PortError {
                    message: format!("clone stream: {e}"),
                });
                return;
            }
        };
        let mut reader_stream = match stream.try_clone() {
            Ok(s) => s,
            Err(e) => {
                let _ = event_tx.send_blocking(PortEvent::PortError {
                    message: format!("clone stream: {e}"),
                });
                return;
            }
        };

        if let Some((user, pass)) = &self.login {
            let _ = writer.write_all(&AgwFrame::login(user, pass).encode());
        }
        let _ = writer.write_all(&AgwFrame::new(self.radio_port, 'G', "", "", vec![]).encode());
        let _ = writer.write_all(&AgwFrame::new(self.radio_port, 'm', "", "", vec![]).encode());

        let conns = Arc::new(Mutex::new(ConnMap {
            next_incoming_id: INCOMING_ID_BASE,
            ..ConnMap::default()
        }));

        let reader_events = event_tx.clone();
        let reader_conns = conns.clone();
        let reader_handle = thread::spawn(move || {
            read_loop(&mut reader_stream, &reader_events, &reader_conns);
        });

        let radio_port = self.radio_port;
        let my_call = self.my_call.clone();
        loop {
            match cmd_rx.recv() {
                Ok(PortCommand::OpenConnection { remote }) => {
                    let id = {
                        let mut c = conns.lock().unwrap();
                        if let Some(existing) = c.id_for_call(&remote) {
                            existing
                        } else {
                            let id = c.next_incoming_id;
                            c.next_incoming_id += 1;
                            c.insert(id, remote.clone());
                            id
                        }
                    };
                    let _ = event_tx.send_blocking(PortEvent::ConnectionOpened {
                        id,
                        label: remote.clone(),
                    });
                    let _ = event_tx.send_blocking(PortEvent::ConnState {
                        id,
                        state: ConnState::Connecting,
                    });
                    let frame = AgwFrame::new(radio_port, 'C', &my_call, &remote, vec![]);
                    if writer.write_all(&frame.encode()).is_err() {
                        break;
                    }
                }
                Ok(PortCommand::Send { id, bytes }) => {
                    let remote = conns.lock().unwrap().call_for_id(id);
                    if let Some(remote) = remote {
                        let frame = AgwFrame::new(radio_port, 'D', &my_call, &remote, bytes);
                        if writer.write_all(&frame.encode()).is_err() {
                            break;
                        }
                    }
                }
                Ok(PortCommand::CloseConnection { id }) => {
                    let remote = conns.lock().unwrap().call_for_id(id);
                    if let Some(remote) = remote {
                        let frame = AgwFrame::new(radio_port, 'd', &my_call, &remote, vec![]);
                        let _ = writer.write_all(&frame.encode());
                    }
                }
                Ok(PortCommand::Disconnect) => break,
                Ok(PortCommand::Connect) => {}
                Err(_) => break,
            }
        }

        let _ = stream.shutdown(std::net::Shutdown::Both);
        let _ = event_tx.send_blocking(PortEvent::PortDisconnected { reason: None });
        let _ = reader_handle.join();
    }
}

fn read_loop(stream: &mut TcpStream, events: &async_channel::Sender<PortEvent>, conns: &Arc<Mutex<ConnMap>>) {
    let mut decoder = FrameDecoder::default();
    let mut buf = [0u8; 4096];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                decoder.feed(&buf[..n]);
                while let Some(frame) = decoder.next_frame() {
                    if handle_frame(frame, events, conns).is_err() {
                        return;
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

/// `Err(())` signals the UI side has gone away and the reader loop should stop.
fn handle_frame(frame: AgwFrame, events: &async_channel::Sender<PortEvent>, conns: &Arc<Mutex<ConnMap>>) -> Result<(), ()> {
    let kind = frame.kind();
    match kind {
        'G' | 'R' | 'H' | 'g' => {
            let text = String::from_utf8_lossy(&frame.data).trim().to_string();
            events
                .send_blocking(PortEvent::Monitor { line: format!("[{kind}] {text}") })
                .map_err(|_| ())?;
        }
        'C' => {
            // An unsolicited 'C' with a call_to matching our own callsign,
            // and no existing mapping, means a remote station connected to
            // us rather than the other way around.
            let remote = frame.call_from.clone();
            let id = {
                let mut c = conns.lock().unwrap();
                match c.id_for_call(&remote) {
                    Some(id) => id,
                    None => {
                        let id = c.next_incoming_id;
                        c.next_incoming_id += 1;
                        c.insert(id, remote.clone());
                        id
                    }
                }
            };
            let text = String::from_utf8_lossy(&frame.data).trim().to_string();
            events
                .send_blocking(PortEvent::ConnectionOpened { id, label: remote })
                .map_err(|_| ())?;
            events
                .send_blocking(PortEvent::ConnState { id, state: ConnState::Connected })
                .map_err(|_| ())?;
            if !text.is_empty() {
                events
                    .send_blocking(PortEvent::Monitor { line: text })
                    .map_err(|_| ())?;
            }
        }
        'd' => {
            let remote = frame.call_from.clone();
            let id = conns.lock().unwrap().id_for_call(&remote);
            if let Some(id) = id {
                conns.lock().unwrap().remove_call(&remote);
                events
                    .send_blocking(PortEvent::ConnState { id, state: ConnState::Disconnected })
                    .map_err(|_| ())?;
                events
                    .send_blocking(PortEvent::ConnectionClosed { id })
                    .map_err(|_| ())?;
            }
        }
        'D' => {
            let remote = frame.call_from.clone();
            let id = conns.lock().unwrap().id_for_call(&remote);
            if let Some(id) = id {
                events
                    .send_blocking(PortEvent::Data { id, bytes: frame.data })
                    .map_err(|_| ())?;
            }
        }
        'U' | 'S' | 'I' | 'T' => {
            let text = String::from_utf8_lossy(&frame.data).trim().to_string();
            events
                .send_blocking(PortEvent::Monitor {
                    line: format!("{} > {} [{kind}]: {text}", frame.call_from, frame.call_to),
                })
                .map_err(|_| ())?;
        }
        _ => {}
    }
    Ok(())
}
