//! KISS TNC transport (TCP or serial).
//!
//! Hand-rolled rather than built on the `ax25_tnc` crate: that crate's TCP
//! KISS backend doesn't detect a locally shut-down socket returning `Ok(0)`
//! from `read()` as end-of-stream, so its background reader thread spins at
//! ~60% CPU forever instead of exiting on disconnect. Rolling our own here
//! mirrors the same read-loop pattern already used successfully for Telnet/
//! SSH/AGWPE (`Ok(0)` / socket shutdown ends the read loop cleanly), plus a
//! read-timeout + stop-flag for the serial case, which has no socket-level
//! shutdown to rely on.
//!
//! Scope: unconnected (UI/beacon) traffic and monitor decode only — see
//! `raw_socket.rs`/`runner.rs` doc comments for why connected mode is out of
//! scope for bare KISS.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use ax25::frame::{Address, Ax25Frame, FrameContent, ProtocolIdentifier, RouteEntry};

use pr_core::{KissParams, PortCommand, PortEvent, PortRunner};

use crate::kiss::{encode_data_frame, encode_param_frame, KissDecoder, CMD_FULL_DUPLEX, CMD_PERSISTENCE, CMD_SLOT_TIME, CMD_TX_DELAY};

const READ_TIMEOUT: Duration = Duration::from_millis(300);
/// Every KISS frame we send/receive uses this port; Direwolf's/most TNCs'
/// raw KISS servers are single-channel, so there's nothing to configure here.
const KISS_PORT: u8 = 0;

pub enum KissTransport {
    Tcp { host: String, port: u16 },
    Serial { device: String, baud: u32 },
}

pub struct KissRunner {
    pub transport: KissTransport,
    pub my_call: String,
    pub params: KissParams,
}

impl PortRunner for KissRunner {
    fn run(self: Box<Self>, cmd_rx: mpsc::Receiver<PortCommand>, event_tx: async_channel::Sender<PortEvent>) {
        let KissRunner { transport, my_call, params } = *self;
        match transport {
            KissTransport::Tcp { host, port } => run_tcp(&host, port, &my_call, &params, cmd_rx, event_tx),
            KissTransport::Serial { device, baud } => run_serial(&device, baud, &my_call, &params, cmd_rx, event_tx),
        }
    }
}

/// Send any configured TNC transmit parameters as KISS command frames
/// (TXDELAY/persistence/slot time/full duplex — command types 1/2/3/5).
/// `None` fields are left alone, so a config with no `KissParams` set sends
/// nothing and behaves exactly as before this feature existed.
fn send_kiss_params(writer: &mut impl Write, params: &KissParams) -> std::io::Result<()> {
    if let Some(v) = params.tx_delay {
        writer.write_all(&encode_param_frame(KISS_PORT, CMD_TX_DELAY, v))?;
    }
    if let Some(v) = params.persistence {
        writer.write_all(&encode_param_frame(KISS_PORT, CMD_PERSISTENCE, v))?;
    }
    if let Some(v) = params.slot_time {
        writer.write_all(&encode_param_frame(KISS_PORT, CMD_SLOT_TIME, v))?;
    }
    if let Some(v) = params.full_duplex {
        writer.write_all(&encode_param_frame(KISS_PORT, CMD_FULL_DUPLEX, v as u8))?;
    }
    Ok(())
}

fn run_tcp(
    host: &str,
    port: u16,
    my_call: &str,
    params: &KissParams,
    cmd_rx: mpsc::Receiver<PortCommand>,
    event_tx: async_channel::Sender<PortEvent>,
) {
    let addr = format!("{host}:{port}");
    let mut stream = match TcpStream::connect(&addr) {
        Ok(s) => s,
        Err(e) => {
            let _ = event_tx.send_blocking(PortEvent::PortError {
                message: format!("connect {addr} failed: {e}"),
            });
            return;
        }
    };
    let _ = event_tx.send_blocking(PortEvent::PortConnected);
    if let Err(e) = send_kiss_params(&mut stream, params) {
        let _ = event_tx.send_blocking(PortEvent::PortError { message: format!("send TNC params: {e}") });
    }

    let reader_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            let _ = event_tx.send_blocking(PortEvent::PortError { message: format!("clone stream: {e}") });
            return;
        }
    };
    let _ = reader_stream.set_read_timeout(Some(READ_TIMEOUT));

    let stop = Arc::new(AtomicBool::new(false));
    let reader_stop = stop.clone();
    let reader_events = event_tx.clone();
    let reader_handle = thread::spawn(move || {
        kiss_read_loop(reader_stream, &reader_events, &reader_stop);
    });

    let mut writer = stream;
    command_loop(&mut writer, my_call, &cmd_rx, &event_tx);

    stop.store(true, Ordering::Relaxed);
    let _ = writer.shutdown(std::net::Shutdown::Both);
    let _ = event_tx.send_blocking(PortEvent::PortDisconnected { reason: None });
    let _ = reader_handle.join();
}

fn run_serial(
    device: &str,
    baud: u32,
    my_call: &str,
    params: &KissParams,
    cmd_rx: mpsc::Receiver<PortCommand>,
    event_tx: async_channel::Sender<PortEvent>,
) {
    let mut port = match serialport::new(device, baud).timeout(READ_TIMEOUT).open() {
        Ok(p) => p,
        Err(e) => {
            let _ = event_tx.send_blocking(PortEvent::PortError { message: format!("open {device}: {e}") });
            return;
        }
    };
    let _ = event_tx.send_blocking(PortEvent::PortConnected);
    if let Err(e) = send_kiss_params(&mut port, params) {
        let _ = event_tx.send_blocking(PortEvent::PortError { message: format!("send TNC params: {e}") });
    }

    let reader_port = match port.try_clone() {
        Ok(p) => p,
        Err(e) => {
            let _ = event_tx.send_blocking(PortEvent::PortError {
                message: format!("clone serial port: {e}"),
            });
            return;
        }
    };

    let stop = Arc::new(AtomicBool::new(false));
    let reader_stop = stop.clone();
    let reader_events = event_tx.clone();
    let reader_handle = thread::spawn(move || {
        kiss_read_loop(reader_port, &reader_events, &reader_stop);
    });

    let mut writer = port;
    command_loop(&mut writer, my_call, &cmd_rx, &event_tx);

    stop.store(true, Ordering::Relaxed);
    let _ = event_tx.send_blocking(PortEvent::PortDisconnected { reason: None });
    let _ = reader_handle.join();
}

/// Shared by both transports: blocks on commands and writes KISS-framed
/// AX.25 frames until told to disconnect.
fn command_loop(writer: &mut impl Write, my_call: &str, cmd_rx: &mpsc::Receiver<PortCommand>, event_tx: &async_channel::Sender<PortEvent>) {
    loop {
        match cmd_rx.recv() {
            Ok(PortCommand::SendUnproto { dest, via, bytes }) => {
                // A raw KISS TNC (unlike AGWPE) won't echo our own
                // transmission back to us, so log it locally.
                let text = String::from_utf8_lossy(&bytes).replace('\0', "");
                match build_ui_frame(my_call, &dest, &via, bytes) {
                    Ok(frame) => {
                        let kiss_bytes = encode_data_frame(KISS_PORT, &frame.to_bytes());
                        if writer.write_all(&kiss_bytes).is_err() {
                            break;
                        }
                        let via_suffix =
                            if via.is_empty() { String::new() } else { format!(" via {}", via.join(",")) };
                        // `to: None` — our own transmission, never "directed
                        // at me" regardless of `dest`.
                        let _ = event_tx.send_blocking(PortEvent::Monitor {
                            line: format!("{my_call} > {dest}{via_suffix} [unproto TX]: {text}"),
                            to: None,
                        });
                    }
                    Err(e) => {
                        let _ = event_tx.send_blocking(PortEvent::PortError { message: e });
                    }
                }
            }
            Ok(PortCommand::Disconnect) => break,
            // Connected-mode over bare KISS isn't implemented; ignore.
            Ok(_) => {}
            Err(_) => break,
        }
    }
}

/// Read loop shared by TCP and serial: both have a read timeout set so this
/// wakes up periodically to check `stop`, since neither transport gives us a
/// single portable "you're cancelled now" signal otherwise.
fn kiss_read_loop(mut reader: impl Read, events: &async_channel::Sender<PortEvent>, stop: &AtomicBool) {
    let mut decoder = KissDecoder::new();
    let mut buf = [0u8; 1024];
    while !stop.load(Ordering::Relaxed) {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                for (cmd, payload) in decoder.feed(&buf[..n]) {
                    if cmd & 0x0F != 0 {
                        continue; // only interested in data frames
                    }
                    if let Ok(frame) = Ax25Frame::from_bytes(&payload) {
                        let _ = events.send_blocking(PortEvent::StationHeard { callsign: frame.source.to_string() });
                        // Only UI frames are notification-eligible — I/S/etc
                        // frames belong to someone else's connected-mode
                        // session we're passively monitoring, not a
                        // standalone directed message, and would otherwise
                        // fire once per frame of an ongoing conversation.
                        let to = matches!(frame.content, FrameContent::UnnumberedInformation(_))
                            .then(|| frame.destination.to_string());
                        let _ = events.send_blocking(PortEvent::Monitor { line: describe_frame(&frame), to });
                    }
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted
                ) =>
            {
                continue
            }
            Err(_) => break,
        }
    }
}

/// Describes a decoded frame with its standard AX.25 command/response
/// mnemonic in brackets (matching the bracketed style AGWPE's monitor
/// already uses), so both backends give the highlighter one consistent
/// "frame tag" shape to color.
fn describe_frame(frame: &Ax25Frame) -> String {
    let from = frame.source.to_string();
    let to = frame.destination.to_string();
    match &frame.content {
        FrameContent::Information(i) => {
            let text = String::from_utf8_lossy(&i.info).replace('\0', "");
            let pid_suffix = pid_suffix(&i.pid);
            format!("{from} > {to} [I N(S)={} N(R)={}]{pid_suffix}: {text}", i.send_sequence, i.receive_sequence)
        }
        FrameContent::UnnumberedInformation(ui) => {
            let text = String::from_utf8_lossy(&ui.info).replace('\0', "");
            let pid_suffix = pid_suffix(&ui.pid);
            format!("{from} > {to} [UI]{pid_suffix}: {text}")
        }
        FrameContent::ReceiveReady(rr) => format!("{from} > {to} [RR N(R)={}]", rr.receive_sequence),
        FrameContent::ReceiveNotReady(rnr) => format!("{from} > {to} [RNR N(R)={}]", rnr.receive_sequence),
        FrameContent::Reject(rej) => format!("{from} > {to} [REJ N(R)={}]", rej.receive_sequence),
        FrameContent::SetAsynchronousBalancedMode(_) => format!("{from} > {to} [SABM]"),
        FrameContent::Disconnect(_) => format!("{from} > {to} [DISC]"),
        FrameContent::DisconnectedMode(_) => format!("{from} > {to} [DM]"),
        FrameContent::UnnumberedAcknowledge(_) => format!("{from} > {to} [UA]"),
        FrameContent::FrameReject(_) => format!("{from} > {to} [FRMR]"),
        FrameContent::UnknownContent(_) => format!("{from} > {to} [?]"),
    }
}

/// `" [PID: ...]"` when the frame carries something other than plain text
/// (0xF0, "no layer 3"), else empty — pure decode/labeling, matching
/// `pr_ax25::pid_label`'s byte-based labels (the `ax25` crate's own
/// `ProtocolIdentifier -> byte` conversion is private, so this matches the
/// enum variants directly instead of round-tripping through a byte).
fn pid_suffix(pid: &ProtocolIdentifier) -> String {
    let label = match pid {
        ProtocolIdentifier::None => return String::new(),
        ProtocolIdentifier::X25Plp => "X.25 PLP",
        ProtocolIdentifier::CompressedTcpIp => "Compressed TCP/IP",
        ProtocolIdentifier::UncompressedTcpIp => "Uncompressed TCP/IP",
        ProtocolIdentifier::SegmentationFragment => "Segmentation Fragment",
        ProtocolIdentifier::TexnetDatagram => "TEXNET",
        ProtocolIdentifier::LinkQuality => "Link Quality",
        ProtocolIdentifier::Appletalk => "Appletalk",
        ProtocolIdentifier::AppletalkArp => "Appletalk ARP",
        ProtocolIdentifier::ArpaIp => "ARPA IP",
        ProtocolIdentifier::ArpaAddress => "ARPA ARP",
        ProtocolIdentifier::Flexnet => "FlexNet",
        ProtocolIdentifier::NetRom => "NET/ROM",
        ProtocolIdentifier::Layer3Impl => "Layer 3",
        ProtocolIdentifier::Escape => "Escape",
        ProtocolIdentifier::Unknown(_) => return String::new(),
    };
    format!(" [PID: {label}]")
}

fn build_ui_frame(my_call: &str, dest: &str, via: &[String], info: Vec<u8>) -> Result<Ax25Frame, String> {
    let source = parse_address(my_call)?;
    let destination = parse_address(dest)?;
    let mut frame = Ax25Frame::new_simple_ui_frame(source, destination, info);
    for digi in via {
        frame.route.push(RouteEntry { repeater: parse_address(digi)?, has_repeated: false });
    }
    Ok(frame)
}

fn parse_address(input: &str) -> Result<Address, String> {
    let (call, ssid) = match input.split_once('-') {
        Some((call, ssid_str)) => {
            let ssid: u8 = ssid_str.parse().map_err(|_| format!("invalid SSID in '{input}'"))?;
            (call, ssid)
        }
        None => (input, 0),
    };
    Address::from_parts(call.to_string(), ssid).map_err(|e| format!("invalid callsign '{input}': {e}"))
}
