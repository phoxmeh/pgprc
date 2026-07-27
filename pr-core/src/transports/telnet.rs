use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc;
use std::thread;

use crate::port::{ConnState, PortCommand, PortEvent, PortRunner};

use super::CONN_ID;

pub struct TelnetRunner {
    pub host: String,
    pub port: u16,
}

impl PortRunner for TelnetRunner {
    fn run(self: Box<Self>, cmd_rx: mpsc::Receiver<PortCommand>, event_tx: async_channel::Sender<PortEvent>) {
        let addr = format!("{}:{}", self.host, self.port);
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
        let _ = event_tx.send_blocking(PortEvent::ConnectionOpened {
            id: CONN_ID,
            label: addr.clone(),
        });
        let _ = event_tx.send_blocking(PortEvent::ConnState {
            id: CONN_ID,
            state: ConnState::Connected,
        });

        let reader_stream = match stream.try_clone() {
            Ok(s) => s,
            Err(e) => {
                let _ = event_tx.send_blocking(PortEvent::PortError {
                    message: format!("clone stream: {e}"),
                });
                return;
            }
        };
        let reader_events = event_tx.clone();
        let reader_handle = thread::spawn(move || read_loop(reader_stream, reader_events));

        let mut writer = stream;
        loop {
            match cmd_rx.recv() {
                Ok(PortCommand::Send { id, bytes }) if id == CONN_ID => {
                    if writer.write_all(&bytes).is_err() {
                        break;
                    }
                }
                Ok(PortCommand::Disconnect) | Ok(PortCommand::CloseConnection { .. }) => {
                    let _ = writer.shutdown(std::net::Shutdown::Both);
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }

        let _ = event_tx.send_blocking(PortEvent::ConnState {
            id: CONN_ID,
            state: ConnState::Disconnected,
        });
        let _ = event_tx.send_blocking(PortEvent::ConnectionClosed { id: CONN_ID });
        let _ = event_tx.send_blocking(PortEvent::PortDisconnected { reason: None });
        let _ = reader_handle.join();
    }
}

fn read_loop(mut stream: TcpStream, events: async_channel::Sender<PortEvent>) {
    let mut filter = TelnetFilter::new();
    let mut buf = [0u8; 4096];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let mut data = Vec::new();
                let mut replies = Vec::new();
                filter.process(&buf[..n], &mut data, &mut replies);
                if !replies.is_empty() && stream.write_all(&replies).is_err() {
                    break;
                }
                if !data.is_empty()
                    && events
                        .send_blocking(PortEvent::Data { id: CONN_ID, bytes: data })
                        .is_err()
                {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

const IAC: u8 = 255;
const WILL: u8 = 251;
const WONT: u8 = 252;
const DO: u8 = 253;
const DONT: u8 = 254;
const SE: u8 = 240;

enum TelnetState {
    Data,
    Iac,
    Neg(u8),
    Sub,
    SubIac,
}

/// Minimal telnet IAC filter: strips negotiation/subnegotiation sequences
/// from the byte stream and refuses every option (WONT/DONT to everything),
/// which is a safe, simple default for a plain text terminal session.
struct TelnetFilter {
    state: TelnetState,
}

impl TelnetFilter {
    fn new() -> Self {
        Self { state: TelnetState::Data }
    }

    fn process(&mut self, input: &[u8], out_data: &mut Vec<u8>, replies: &mut Vec<u8>) {
        for &b in input {
            match self.state {
                TelnetState::Data => {
                    if b == IAC {
                        self.state = TelnetState::Iac;
                    } else {
                        out_data.push(b);
                    }
                }
                TelnetState::Iac => match b {
                    IAC => {
                        out_data.push(IAC);
                        self.state = TelnetState::Data;
                    }
                    WILL | WONT | DO | DONT => self.state = TelnetState::Neg(b),
                    250 => self.state = TelnetState::Sub,
                    _ => self.state = TelnetState::Data,
                },
                TelnetState::Neg(cmd) => {
                    let opt = b;
                    let reply_cmd = match cmd {
                        WILL => DONT,
                        DO => WONT,
                        WONT => DONT,
                        DONT => WONT,
                        _ => WONT,
                    };
                    replies.push(IAC);
                    replies.push(reply_cmd);
                    replies.push(opt);
                    self.state = TelnetState::Data;
                }
                TelnetState::Sub => {
                    if b == IAC {
                        self.state = TelnetState::SubIac;
                    }
                }
                TelnetState::SubIac => {
                    self.state = if b == SE { TelnetState::Data } else { TelnetState::Sub };
                }
            }
        }
    }
}
