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
//! Scope: unconnected (UI/beacon) traffic, connected-mode AX.25 (modulus-8
//! ARQ, see `crate::arq`), and XID/SABME handling (declined, see
//! `crate::xid`/`crate::wire` -- this app only supports modulus-8).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use ax25::frame::{Address, Ax25Frame, FrameContent, ProtocolIdentifier, RouteEntry};

use pr_core::{ConnState, ConnectionId, KissArqParams, KissParams, NodesBroadcastEntry, PortCommand, PortEvent, PortRunner};

use crate::arq::{arq_config_from, Action, ArqConfig, ArqSession};
use crate::kiss::{encode_data_frame, encode_param_frame, KissDecoder, CMD_FULL_DUPLEX, CMD_PERSISTENCE, CMD_SLOT_TIME, CMD_TX_DELAY};
use crate::netrom;
use crate::wire;
use crate::xid::{self, XidDecision};

const READ_TIMEOUT: Duration = Duration::from_millis(300);
/// Every KISS frame we send/receive uses this port; Direwolf's/most TNCs'
/// raw KISS servers are single-channel, so there's nothing to configure here.
const KISS_PORT: u8 = 0;
/// How often `command_loop` wakes up with no real command pending, to drive
/// ARQ timer checks. Keeps all ARQ state and the stream writer on one
/// thread (no locks) at the cost of up to this much latency reacting to an
/// inbound frame -- negligible against multi-second T1 timers.
const TICK_INTERVAL: Duration = Duration::from_millis(200);

pub enum KissTransport {
    Tcp { host: String, port: u16 },
    Serial { device: String, baud: u32 },
    /// `address` is the BLE device address as BlueZ knows it (e.g.
    /// `AA:BB:CC:DD:EE:FF` on Linux), entered by the user after pairing the
    /// device via the OS's own Bluetooth settings -- see `run_ble`.
    Ble { address: String },
}

pub struct KissRunner {
    pub transport: KissTransport,
    pub my_call: String,
    pub params: KissParams,
    pub arq: KissArqParams,
}

impl PortRunner for KissRunner {
    fn run(self: Box<Self>, cmd_rx: mpsc::Receiver<PortCommand>, event_tx: async_channel::Sender<PortEvent>) {
        let KissRunner { transport, my_call, params, arq } = *self;
        match transport {
            KissTransport::Tcp { host, port } => run_tcp(&host, port, &my_call, &params, &arq, cmd_rx, event_tx),
            KissTransport::Serial { device, baud } => run_serial(&device, baud, &my_call, &params, &arq, cmd_rx, event_tx),
            KissTransport::Ble { address } => run_ble(&address, &my_call, &params, &arq, cmd_rx, event_tx),
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
    arq: &KissArqParams,
    cmd_rx: mpsc::Receiver<PortCommand>,
    event_tx: async_channel::Sender<PortEvent>,
) {
    let local = match parse_address(my_call) {
        Ok(a) => a,
        Err(e) => {
            let _ = event_tx.send_blocking(PortEvent::PortError { message: e });
            return;
        }
    };
    let arq_cfg = arq_config_from(arq);

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
    let (internal_tx, internal_rx) = mpsc::channel();
    let reader_local = local.clone();
    let reader_handle = thread::spawn(move || {
        kiss_read_loop(reader_stream, &reader_events, &reader_stop, &internal_tx, &reader_local);
    });

    let mut writer = stream;
    command_loop(&mut writer, &local, &arq_cfg, &cmd_rx, &internal_rx, &event_tx);

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
    arq: &KissArqParams,
    cmd_rx: mpsc::Receiver<PortCommand>,
    event_tx: async_channel::Sender<PortEvent>,
) {
    let local = match parse_address(my_call) {
        Ok(a) => a,
        Err(e) => {
            let _ = event_tx.send_blocking(PortEvent::PortError { message: e });
            return;
        }
    };
    let arq_cfg = arq_config_from(arq);

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
    let (internal_tx, internal_rx) = mpsc::channel();
    let reader_local = local.clone();
    let reader_handle = thread::spawn(move || {
        kiss_read_loop(reader_port, &reader_events, &reader_stop, &internal_tx, &reader_local);
    });

    let mut writer = port;
    command_loop(&mut writer, &local, &arq_cfg, &cmd_rx, &internal_rx, &event_tx);

    stop.store(true, Ordering::Relaxed);
    let _ = event_tx.send_blocking(PortEvent::PortDisconnected { reason: None });
    let _ = reader_handle.join();
}

/// Nordic UART Service UUIDs -- see `docs/kiss_protocol.md` in the
/// lora-kiss-tnc repo. Standard NUS UUIDs, not vendor-specific.
const BLE_NUS_SERVICE_UUID: &str = "6E400001-B5A3-F393-E0A9-E50E24DCCA9E";
const BLE_NUS_RX_CHAR_UUID: &str = "6E400002-B5A3-F393-E0A9-E50E24DCCA9E";
const BLE_NUS_TX_CHAR_UUID: &str = "6E400003-B5A3-F393-E0A9-E50E24DCCA9E";
/// Matches the firmware's own `BleNusTransport::sendFrame` chunk size
/// (`ble_nimble_impl.cpp`). No explicit MTU negotiation is needed on
/// Linux -- BlueZ's D-Bus `WriteValue`/notify transparently fragments and
/// reassembles payloads regardless of the negotiated ATT MTU, and this
/// exact chunk size is already verified end-to-end against real hardware
/// by the sibling repo's `tools/ble_kiss_client.py`.
const BLE_CHUNK_SIZE: usize = 180;

/// `Read` half of the BLE transport: drains bytes forwarded from the TX
/// (notify) characteristic by `notify_forward_task`. A closed channel
/// (sender dropped, e.g. the whole tokio runtime torn down on disconnect)
/// reads as `Ok(0)` -- end of stream -- matching TCP/serial's `Ok(0)`
/// convention so `kiss_read_loop` needs no BLE-specific handling.
struct BleReader {
    rx: mpsc::Receiver<Vec<u8>>,
    leftover: std::collections::VecDeque<u8>,
}

impl Read for BleReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.leftover.is_empty() {
            match self.rx.recv_timeout(READ_TIMEOUT) {
                Ok(chunk) => self.leftover.extend(chunk),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "no BLE notification"));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(0),
            }
        }
        let n = self.leftover.len().min(buf.len());
        for (i, b) in self.leftover.drain(..n).enumerate() {
            buf[i] = b;
        }
        Ok(n)
    }
}

/// `Write` half of the BLE transport: hands whole KISS-encoded frames off
/// to `write_drain_task`, which does the actual chunked GATT write on the
/// tokio runtime. Message-queue semantics rather than a byte stream --
/// `write()` always accepts the full buffer at once (`command_loop` never
/// calls it with anything but one already-encoded frame per call) -- so no
/// partial-write bookkeeping is needed.
struct BleWriter {
    tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
}

impl Write for BleWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.tx
            .send(buf.to_vec())
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "BLE write channel closed"))?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Connects to `address` (must already be paired/bonded via the OS's own
/// Bluetooth settings -- see `docs/kiss_protocol.md`'s "BLE pairing"
/// section in the lora-kiss-tnc repo), discovers the NUS service, and
/// spawns the background tasks that bridge btleplug's async API into the
/// synchronous `BleReader`/`BleWriter` above. Returns the connected
/// peripheral (kept alive so `run_ble` can call `disconnect()` on
/// teardown) once everything is wired up.
async fn ble_connect(
    address: &str,
    notify_tx: mpsc::Sender<Vec<u8>>,
    mut write_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    disconnect_tx: mpsc::Sender<InternalEvent>,
) -> Result<btleplug::platform::Peripheral, String> {
    use btleplug::api::{BDAddr, Central, Characteristic, Manager as _, Peripheral as _, WriteType};
    use futures::StreamExt;

    let target = BDAddr::from_str_delim(address).or_else(|_| BDAddr::from_str_no_delim(address)).map_err(|e| format!("invalid BLE address '{address}': {e}"))?;

    let manager = btleplug::platform::Manager::new().await.map_err(|e| format!("BLE manager init failed: {e}"))?;
    let adapters = manager.adapters().await.map_err(|e| format!("list Bluetooth adapters: {e}"))?;
    let central = adapters.into_iter().next().ok_or_else(|| "no Bluetooth adapter found".to_string())?;

    let peripherals = central.peripherals().await.map_err(|e| format!("list BLE devices: {e}"))?;
    let peripheral = peripherals
        .into_iter()
        .find(|p| p.address() == target)
        .ok_or_else(|| format!("BLE device {address} not known to the system -- pair it via your OS Bluetooth settings first"))?;

    peripheral.connect().await.map_err(|e| format!("BLE connect {address}: {e}"))?;
    peripheral.discover_services().await.map_err(|e| format!("BLE discover services: {e}"))?;

    let service_uuid = uuid::Uuid::parse_str(BLE_NUS_SERVICE_UUID).expect("valid UUID constant");
    let characteristics = peripheral.characteristics();
    let find_char = |uuid_str: &str| -> Option<Characteristic> {
        let uuid = uuid::Uuid::parse_str(uuid_str).ok()?;
        characteristics.iter().find(|c| c.uuid == uuid && c.service_uuid == service_uuid).cloned()
    };
    let rx_char = find_char(BLE_NUS_RX_CHAR_UUID).ok_or_else(|| "NUS RX characteristic not found -- is this a lora-kiss-tnc device?".to_string())?;
    let tx_char = find_char(BLE_NUS_TX_CHAR_UUID).ok_or_else(|| "NUS TX characteristic not found -- is this a lora-kiss-tnc device?".to_string())?;

    peripheral.subscribe(&tx_char).await.map_err(|e| format!("BLE subscribe: {e}"))?;

    // Forward TX notifications into `BleReader`'s channel.
    let mut notifications = peripheral.notifications().await.map_err(|e| format!("BLE notifications stream: {e}"))?;
    let tx_uuid = tx_char.uuid;
    tokio::spawn(async move {
        while let Some(notification) = notifications.next().await {
            if notification.uuid == tx_uuid && notify_tx.send(notification.value).is_err() {
                break;
            }
        }
    });

    // Drain queued writes from `BleWriter`, chunked to match the
    // firmware's own convention (see `BLE_CHUNK_SIZE`).
    let write_peripheral = peripheral.clone();
    tokio::spawn(async move {
        while let Some(bytes) = write_rx.recv().await {
            for chunk in bytes.chunks(BLE_CHUNK_SIZE) {
                if write_peripheral.write(&rx_char, chunk, WriteType::WithoutResponse).await.is_err() {
                    return;
                }
            }
        }
    });

    // Watch for a peripheral-initiated disconnect (TNC reboot, out of
    // range, etc.) -- a dropped link isn't otherwise caught promptly since
    // `BleWriter::write` only fails once the write-drain task above
    // notices, which won't happen at all if nothing is being sent.
    let my_id = peripheral.id();
    let mut events = central.events().await.map_err(|e| format!("BLE central events stream: {e}"))?;
    tokio::spawn(async move {
        while let Some(event) = events.next().await {
            if matches!(event, btleplug::api::CentralEvent::DeviceDisconnected(id) if id == my_id) {
                let _ = disconnect_tx.send(InternalEvent::Disconnected);
                break;
            }
        }
    });

    Ok(peripheral)
}

fn run_ble(address: &str, my_call: &str, params: &KissParams, arq: &KissArqParams, cmd_rx: mpsc::Receiver<PortCommand>, event_tx: async_channel::Sender<PortEvent>) {
    use btleplug::api::Peripheral as _;

    let local = match parse_address(my_call) {
        Ok(a) => a,
        Err(e) => {
            let _ = event_tx.send_blocking(PortEvent::PortError { message: e });
            return;
        }
    };
    let arq_cfg = arq_config_from(arq);

    let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            let _ = event_tx.send_blocking(PortEvent::PortError { message: format!("start BLE runtime: {e}") });
            return;
        }
    };

    let (notify_tx, notify_rx) = mpsc::channel::<Vec<u8>>();
    let (write_tx, write_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let (internal_tx, internal_rx) = mpsc::channel::<InternalEvent>();
    let disconnect_tx = internal_tx.clone();

    let peripheral = match rt.block_on(ble_connect(address, notify_tx, write_rx, disconnect_tx)) {
        Ok(p) => p,
        Err(e) => {
            let _ = event_tx.send_blocking(PortEvent::PortError { message: e });
            rt.shutdown_timeout(Duration::from_secs(2));
            return;
        }
    };
    let _ = event_tx.send_blocking(PortEvent::PortConnected);

    let mut writer = BleWriter { tx: write_tx };
    if let Err(e) = send_kiss_params(&mut writer, params) {
        let _ = event_tx.send_blocking(PortEvent::PortError { message: format!("send TNC params: {e}") });
    }

    let stop = Arc::new(AtomicBool::new(false));
    let reader_stop = stop.clone();
    let reader_events = event_tx.clone();
    let reader_local = local.clone();
    let reader_handle = thread::spawn(move || {
        let reader = BleReader { rx: notify_rx, leftover: std::collections::VecDeque::new() };
        kiss_read_loop(reader, &reader_events, &reader_stop, &internal_tx, &reader_local);
    });

    command_loop(&mut writer, &local, &arq_cfg, &cmd_rx, &internal_rx, &event_tx);

    stop.store(true, Ordering::Relaxed);
    rt.block_on(async {
        let _ = peripheral.disconnect().await;
    });
    // Drops the notify-forward/write-drain/disconnect-watcher tasks along
    // with the runtime, closing `BleReader`'s channel -- its next
    // `recv_timeout` reads that as `Ok(0)` and `kiss_read_loop` exits.
    rt.shutdown_timeout(Duration::from_secs(2));
    let _ = event_tx.send_blocking(PortEvent::PortDisconnected { reason: None });
    let _ = reader_handle.join();
}

/// One connected-mode session per remote callsign, multiplexed over the
/// one shared KISS stream -- mirrors AGWPE's `ConnMap` precedent (AX.25
/// addressing has no numeric connection id of its own either), except a
/// single monotonic id counter is enough here since only `command_loop`'s
/// thread ever touches this map (no incoming/outgoing id-range race to
/// avoid, unlike AGWPE's reader+writer-thread-shared `Mutex<ConnMap>`).
struct ConnMap {
    sessions: HashMap<ConnectionId, ArqSession>,
    call_to_id: HashMap<String, ConnectionId>,
    next_id: AtomicU64,
}

impl ConnMap {
    fn new() -> Self {
        Self { sessions: HashMap::new(), call_to_id: HashMap::new(), next_id: AtomicU64::new(1) }
    }
    fn alloc_id(&self) -> ConnectionId {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }
    fn id_for_call(&self, call: &str) -> Option<ConnectionId> {
        self.call_to_id.get(call).copied()
    }
    fn insert(&mut self, id: ConnectionId, call: String, session: ArqSession) {
        self.call_to_id.insert(call, id);
        self.sessions.insert(id, session);
    }
    fn remove(&mut self, id: ConnectionId) {
        if self.sessions.remove(&id).is_some() {
            self.call_to_id.retain(|_, v| *v != id);
        }
    }
}

fn remove_if_disconnected(id: ConnectionId, conns: &mut ConnMap) {
    if conns.sessions.get(&id).map(ArqSession::state) == Some(ConnState::Disconnected) {
        conns.remove(id);
    }
}

/// A decoded-off-the-wire event handed from the reader thread to
/// `command_loop`, which owns all ARQ state and the stream writer.
enum InternalEvent {
    /// An ARQ-relevant frame the `ax25` crate could decode, addressed to us.
    Frame(Ax25Frame),
    /// SABME (extended-mode connect) -- the crate can't decode this at all;
    /// `wire::parse_header` recovered just the addressing.
    RawSabme { remote: Address, route: Vec<RouteEntry> },
    /// XID (parameter negotiation) -- same situation as SABME.
    RawXid { remote: Address, route: Vec<RouteEntry>, payload: Vec<u8> },
    /// The transport itself dropped (currently BLE-only: the peripheral
    /// disconnected). Unlike TCP/serial, a dead BLE link isn't reliably
    /// caught by the next `write_all` failing (the write hands off to an
    /// async task -- see `BleWriter`), so the disconnect watcher reports it
    /// explicitly through the same internal-event path used for decoded
    /// frames, tearing `command_loop` down the same way a write failure
    /// would.
    Disconnected,
}

/// Shared by both transports: owns the ARQ state for every live connection
/// on this port, drains decoded frames from the reader thread, handles UI
/// commands, and drives periodic T1 timer checks -- all on this one
/// thread, so no locking is needed anywhere in `crate::arq`.
fn command_loop(
    writer: &mut impl Write,
    local: &Address,
    arq_cfg: &ArqConfig,
    cmd_rx: &mpsc::Receiver<PortCommand>,
    internal_rx: &mpsc::Receiver<InternalEvent>,
    event_tx: &async_channel::Sender<PortEvent>,
) {
    let mut conns = ConnMap::new();
    'outer: loop {
        while let Ok(ev) = internal_rx.try_recv() {
            if !handle_internal_event(ev, local, arq_cfg, &mut conns, writer, event_tx) {
                break 'outer;
            }
        }

        let alive = match cmd_rx.recv_timeout(TICK_INTERVAL) {
            Ok(PortCommand::SendUnproto { dest, via, bytes }) => send_unproto(writer, local, &dest, &via, bytes, event_tx),
            Ok(PortCommand::OpenConnection { remote, via }) => open_connection(local, remote, via, arq_cfg, &mut conns, writer, event_tx),
            Ok(PortCommand::Send { id, bytes }) => {
                if let Some(session) = conns.sessions.get_mut(&id) {
                    let actions = session.send(bytes, Instant::now());
                    let ok = apply_actions(id, actions, writer, event_tx);
                    remove_if_disconnected(id, &mut conns);
                    ok
                } else {
                    true
                }
            }
            Ok(PortCommand::CloseConnection { id }) => {
                if let Some(session) = conns.sessions.get_mut(&id) {
                    let actions = session.close(Instant::now());
                    let ok = apply_actions(id, actions, writer, event_tx);
                    remove_if_disconnected(id, &mut conns);
                    ok
                } else {
                    true
                }
            }
            Ok(PortCommand::Disconnect) => break 'outer,
            Ok(PortCommand::Connect) => true,
            Err(mpsc::RecvTimeoutError::Timeout) => true,
            Err(mpsc::RecvTimeoutError::Disconnected) => break 'outer,
        };
        if !alive {
            break 'outer;
        }

        let ids: Vec<ConnectionId> = conns.sessions.keys().copied().collect();
        for id in ids {
            if let Some(session) = conns.sessions.get_mut(&id) {
                let actions = session.tick(Instant::now());
                if !apply_actions(id, actions, writer, event_tx) {
                    break 'outer;
                }
            }
            remove_if_disconnected(id, &mut conns);
        }
    }

    // Give every still-live peer a real DISC instead of just vanishing.
    // Best-effort: ignore further write failures, we're exiting either way.
    let ids: Vec<ConnectionId> = conns.sessions.keys().copied().collect();
    for id in ids {
        if let Some(session) = conns.sessions.get_mut(&id) {
            let actions = session.close(Instant::now());
            apply_actions(id, actions, writer, event_tx);
        }
    }
}

/// Returns `false` if a write failed (dead socket) -- the caller then tears
/// the whole port down, matching the pre-existing single-purpose behavior
/// this generalizes (the old `SendUnproto`-only `command_loop` broke out of
/// its loop on a failed `write_all`).
fn handle_internal_event(
    ev: InternalEvent,
    local: &Address,
    arq_cfg: &ArqConfig,
    conns: &mut ConnMap,
    writer: &mut impl Write,
    event_tx: &async_channel::Sender<PortEvent>,
) -> bool {
    match ev {
        InternalEvent::Disconnected => false,
        InternalEvent::RawSabme { remote, route } => {
            let dm = wire::build_dm(local, &remote, wire::reverse_route(&route), true);
            let ok = write_frame(writer, &dm);
            let _ = event_tx.send_blocking(PortEvent::Monitor {
                line: format!("{remote} > {local} [SABME] declined (extended mode not supported)"),
                from: None,
                to: None,
                message: None,
            });
            ok
        }
        InternalEvent::RawXid { remote, route, payload } => match xid::handle_peer_xid(local, &remote, &route, &payload, arq_cfg) {
            XidDecision::Reply(bytes) => write_raw(writer, &bytes),
            XidDecision::Decline(dm) => write_frame(writer, &dm),
        },
        InternalEvent::Frame(frame) => {
            let remote_call = frame.source.to_string();
            if let Some(id) = conns.id_for_call(&remote_call) {
                let ok = if let Some(session) = conns.sessions.get_mut(&id) {
                    let actions = session.on_frame(&frame, Instant::now());
                    apply_actions(id, actions, writer, event_tx)
                } else {
                    true
                };
                remove_if_disconnected(id, conns);
                ok
            } else if let FrameContent::SetAsynchronousBalancedMode(s) = &frame.content {
                // A bare incoming SABM with no existing session -- mint one
                // and accept it, mirroring the raw-socket backend's accept
                // path (StationHeard -> ConnectionOpened -> Connected
                // directly, skipping Connecting).
                let id = conns.alloc_id();
                let _ = event_tx.send_blocking(PortEvent::StationHeard { callsign: remote_call.clone() });
                let _ = event_tx.send_blocking(PortEvent::ConnectionOpened { id, label: remote_call.clone(), to: Some(local.to_string()) });
                let route = wire::reverse_route(&frame.route);
                let mut session = ArqSession::new(local.clone(), frame.source.clone(), route, arq_cfg.clone());
                let actions = session.accept_incoming(s.poll, Instant::now());
                conns.insert(id, remote_call, session);
                apply_actions(id, actions, writer, event_tx)
            } else if matches!(frame.content, FrameContent::Disconnect(_)) {
                // A DISC for a link we have no record of -- decline politely
                // rather than staying silent.
                let dm = wire::build_dm(local, &frame.source, wire::reverse_route(&frame.route), true);
                write_frame(writer, &dm)
            } else {
                // I/RR/RNR/REJ/UA/DM/FRMR with no matching session: stale or
                // unroutable traffic -- already Monitor-logged by the reader
                // thread, nothing more to do.
                true
            }
        }
    }
}

fn send_unproto(writer: &mut impl Write, local: &Address, dest: &str, via: &[String], bytes: Vec<u8>, event_tx: &async_channel::Sender<PortEvent>) -> bool {
    // A raw KISS TNC (unlike AGWPE) won't echo our own transmission back to
    // us, so log it locally.
    let my_call = local.to_string();
    let text = String::from_utf8_lossy(&bytes).replace('\0', "");
    match build_ui_frame(&my_call, dest, via, bytes) {
        Ok(frame) => {
            let ok = write_frame(writer, &frame);
            let via_suffix = if via.is_empty() { String::new() } else { format!(" via {}", via.join(",")) };
            // `to: None` — our own transmission, never "directed at me"
            // regardless of `dest`.
            let _ = event_tx.send_blocking(PortEvent::Monitor {
                line: format!("{my_call} > {dest}{via_suffix} [unproto TX]: {text}"),
                from: None,
                to: None,
                message: None,
            });
            ok
        }
        Err(e) => {
            let _ = event_tx.send_blocking(PortEvent::PortError { message: e });
            true // a bad address isn't a dead socket -- don't tear the port down over it
        }
    }
}

fn open_connection(
    local: &Address,
    remote: String,
    via: Vec<String>,
    arq_cfg: &ArqConfig,
    conns: &mut ConnMap,
    writer: &mut impl Write,
    event_tx: &async_channel::Sender<PortEvent>,
) -> bool {
    let remote_addr = match parse_address(&remote) {
        Ok(a) => a,
        Err(e) => {
            let _ = event_tx.send_blocking(PortEvent::PortError { message: e });
            return true;
        }
    };
    let mut route = Vec::with_capacity(via.len());
    for digi in &via {
        match parse_address(digi) {
            Ok(repeater) => route.push(RouteEntry { repeater, has_repeated: false }),
            Err(e) => {
                let _ = event_tx.send_blocking(PortEvent::PortError { message: e });
                return true;
            }
        }
    }

    let id = conns.alloc_id();
    let _ = event_tx.send_blocking(PortEvent::StationHeard { callsign: remote.clone() });
    let _ = event_tx.send_blocking(PortEvent::ConnectionOpened { id, label: remote.clone(), to: Some(local.to_string()) });
    let mut session = ArqSession::new(local.clone(), remote_addr, route, arq_cfg.clone());
    let actions = session.open(Instant::now());
    conns.insert(id, remote, session);
    apply_actions(id, actions, writer, event_tx)
}

fn apply_actions(id: ConnectionId, actions: Vec<Action>, writer: &mut impl Write, event_tx: &async_channel::Sender<PortEvent>) -> bool {
    let mut ok = true;
    for action in actions {
        match action {
            Action::Transmit(frame) => ok &= write_frame(writer, &frame),
            Action::TransmitRaw(bytes) => ok &= write_raw(writer, &bytes),
            Action::Data(bytes) => {
                let _ = event_tx.send_blocking(PortEvent::Data { id, bytes });
            }
            Action::StateChanged(state) => {
                let _ = event_tx.send_blocking(PortEvent::ConnState { id, state });
            }
            Action::Monitor(line) => {
                let _ = event_tx.send_blocking(PortEvent::Monitor { line, from: None, to: None, message: None });
            }
            Action::Closed => {
                let _ = event_tx.send_blocking(PortEvent::ConnectionClosed { id });
            }
        }
    }
    ok
}

fn write_frame(writer: &mut impl Write, frame: &Ax25Frame) -> bool {
    writer.write_all(&encode_data_frame(KISS_PORT, &frame.to_bytes())).is_ok()
}

fn write_raw(writer: &mut impl Write, bytes: &[u8]) -> bool {
    writer.write_all(&encode_data_frame(KISS_PORT, bytes)).is_ok()
}

/// Read loop shared by TCP and serial: both have a read timeout set so this
/// wakes up periodically to check `stop`, since neither transport gives us
/// a single portable "you're cancelled now" signal otherwise.
fn kiss_read_loop(mut reader: impl Read, events: &async_channel::Sender<PortEvent>, stop: &AtomicBool, internal_tx: &mpsc::Sender<InternalEvent>, local: &Address) {
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
                    handle_kiss_payload(&payload, events, internal_tx, local);
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

/// Classifies one decoded KISS payload and both logs it (Monitor/
/// StationHeard, exactly as before this feature existed) and, for traffic
/// addressed to us that's relevant to the ARQ engine, forwards it to
/// `command_loop` via `internal_tx`.
fn handle_kiss_payload(payload: &[u8], events: &async_channel::Sender<PortEvent>, internal_tx: &mpsc::Sender<InternalEvent>, local: &Address) {
    let Some(header) = wire::parse_header(payload) else { return };
    let Some(control) = wire::control_byte(payload, &header) else { return };

    if wire::is_sabme(control) {
        let _ = events.send_blocking(PortEvent::StationHeard { callsign: header.source.to_string() });
        let _ = events.send_blocking(PortEvent::Monitor {
            line: format!("{} > {} [SABME]", header.source, header.destination),
            from: None,
            to: None,
            message: None,
        });
        if header.destination == *local {
            let _ = internal_tx.send(InternalEvent::RawSabme { remote: header.source, route: header.route });
        }
        return;
    }
    if wire::is_xid(control) {
        let _ = events.send_blocking(PortEvent::StationHeard { callsign: header.source.to_string() });
        let _ = events.send_blocking(PortEvent::Monitor {
            line: format!("{} > {} [XID]", header.source, header.destination),
            from: None,
            to: None,
            message: None,
        });
        if header.destination == *local {
            let xid_payload = payload[header.control_offset + 1..].to_vec();
            let _ = internal_tx.send(InternalEvent::RawXid { remote: header.source, route: header.route, payload: xid_payload });
        }
        return;
    }

    let Ok(frame) = Ax25Frame::from_bytes(payload) else { return };
    let _ = events.send_blocking(PortEvent::StationHeard { callsign: frame.source.to_string() });
    // Only UI frames are notification-eligible — I/S/etc frames belong to
    // someone else's connected-mode session we're passively monitoring, not
    // a standalone directed message, and would otherwise fire once per
    // frame of an ongoing conversation.
    let (to, from, message) = match &frame.content {
        FrameContent::UnnumberedInformation(ui) => (
            Some(frame.destination.to_string()),
            Some(frame.source.to_string()),
            Some(String::from_utf8_lossy(&ui.info).replace('\0', "")),
        ),
        _ => (None, None, None),
    };
    if let FrameContent::UnnumberedInformation(ui) = &frame.content {
        if ui.pid == ProtocolIdentifier::NetRom && frame.destination.callsign().eq_ignore_ascii_case("NODES") {
            if let Some(broadcast) = netrom::parse_nodes_broadcast(&ui.info) {
                let entries = broadcast
                    .entries
                    .into_iter()
                    .map(|e| NodesBroadcastEntry { callsign: e.callsign, alias: e.alias })
                    .collect();
                let _ = events.send_blocking(PortEvent::NodesBroadcast {
                    from: frame.source.to_string(),
                    sender_alias: broadcast.sender_alias,
                    entries,
                });
            }
        }
    }
    let _ = events.send_blocking(PortEvent::Monitor { line: describe_frame(&frame), from, to, message });

    let is_arq_relevant = matches!(
        frame.content,
        FrameContent::Information(_)
            | FrameContent::ReceiveReady(_)
            | FrameContent::ReceiveNotReady(_)
            | FrameContent::Reject(_)
            | FrameContent::SetAsynchronousBalancedMode(_)
            | FrameContent::Disconnect(_)
            | FrameContent::DisconnectedMode(_)
            | FrameContent::UnnumberedAcknowledge(_)
            | FrameContent::FrameReject(_)
    );
    if is_arq_relevant && frame.destination == *local {
        let _ = internal_tx.send(InternalEvent::Frame(frame));
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

#[cfg(test)]
mod tests {
    use super::*;
    use ax25::frame::{CommandResponse, SetAsynchronousBalancedMode};

    fn addr(s: &str) -> Address {
        s.parse().unwrap()
    }

    fn drain_events(rx: &async_channel::Receiver<PortEvent>) -> Vec<PortEvent> {
        let mut out = Vec::new();
        while let Ok(e) = rx.try_recv() {
            out.push(e);
        }
        out
    }

    #[test]
    fn conn_map_alloc_insert_remove() {
        let mut conns = ConnMap::new();
        let id1 = conns.alloc_id();
        let id2 = conns.alloc_id();
        assert_ne!(id1, id2);
        conns.insert(id1, "N0CALL-1".to_string(), ArqSession::new(addr("KD3BFP-9"), addr("N0CALL-1"), vec![], ArqConfig::default()));
        assert_eq!(conns.id_for_call("N0CALL-1"), Some(id1));
        conns.remove(id1);
        assert_eq!(conns.id_for_call("N0CALL-1"), None);
    }

    #[test]
    fn handle_kiss_payload_forwards_sabme_addressed_to_us_and_tags_monitor() {
        let local = addr("N0CALL-1");
        let (events_tx, events_rx) = async_channel::unbounded();
        let (internal_tx, internal_rx) = mpsc::channel();

        let scaffold = Ax25Frame {
            source: addr("KD3BFP-9"),
            destination: local.clone(),
            route: vec![],
            command_or_response: Some(CommandResponse::Command),
            content: FrameContent::SetAsynchronousBalancedMode(SetAsynchronousBalancedMode { poll: true }),
        };
        let mut bytes = scaffold.to_bytes();
        let header = wire::parse_header(&bytes).unwrap();
        bytes[header.control_offset] = wire::CTRL_SABME;

        handle_kiss_payload(&bytes, &events_tx, &internal_tx, &local);

        let events = drain_events(&events_rx);
        assert!(events.iter().any(|e| matches!(e, PortEvent::StationHeard { callsign } if callsign == "KD3BFP-9")));
        assert!(events.iter().any(|e| matches!(e, PortEvent::Monitor { line, .. } if line.contains("[SABME]"))));
        match internal_rx.try_recv() {
            Ok(InternalEvent::RawSabme { remote, .. }) => assert_eq!(remote, addr("KD3BFP-9")),
            Ok(_) => panic!("expected RawSabme"),
            Err(_) => panic!("internal channel was empty"),
        }
    }

    #[test]
    fn handle_kiss_payload_does_not_forward_sabme_addressed_to_someone_else() {
        let local = addr("N0CALL-1");
        let (events_tx, events_rx) = async_channel::unbounded();
        let (internal_tx, internal_rx) = mpsc::channel();

        let scaffold = Ax25Frame {
            source: addr("KD3BFP-9"),
            destination: addr("OTHER-2"),
            route: vec![],
            command_or_response: Some(CommandResponse::Command),
            content: FrameContent::SetAsynchronousBalancedMode(SetAsynchronousBalancedMode { poll: true }),
        };
        let mut bytes = scaffold.to_bytes();
        let header = wire::parse_header(&bytes).unwrap();
        bytes[header.control_offset] = wire::CTRL_SABME;

        handle_kiss_payload(&bytes, &events_tx, &internal_tx, &local);

        // Still logged for passive monitoring...
        let events = drain_events(&events_rx);
        assert!(events.iter().any(|e| matches!(e, PortEvent::Monitor { line, .. } if line.contains("[SABME]"))));
        // ...but never handed to the ARQ engine, since it's not for us.
        assert!(internal_rx.try_recv().is_err());
    }

    #[test]
    fn handle_kiss_payload_forwards_xid_addressed_to_us() {
        let local = addr("N0CALL-1");
        let (events_tx, events_rx) = async_channel::unbounded();
        let (internal_tx, internal_rx) = mpsc::channel();

        let scaffold = Ax25Frame {
            source: addr("KD3BFP-9"),
            destination: local.clone(),
            route: vec![],
            command_or_response: Some(CommandResponse::Command),
            content: FrameContent::SetAsynchronousBalancedMode(SetAsynchronousBalancedMode { poll: true }),
        };
        let mut bytes = scaffold.to_bytes();
        let header = wire::parse_header(&bytes).unwrap();
        bytes.truncate(header.control_offset);
        bytes.push(wire::CTRL_XID);
        bytes.extend_from_slice(&crate::xid::encode(&crate::xid::our_params(&ArqConfig::default())));

        handle_kiss_payload(&bytes, &events_tx, &internal_tx, &local);

        let events = drain_events(&events_rx);
        assert!(events.iter().any(|e| matches!(e, PortEvent::Monitor { line, .. } if line.contains("[XID]"))));
        match internal_rx.try_recv() {
            Ok(InternalEvent::RawXid { remote, payload, .. }) => {
                assert_eq!(remote, addr("KD3BFP-9"));
                assert!(crate::xid::decode(&payload).is_ok());
            }
            Ok(_) => panic!("expected RawXid"),
            Err(_) => panic!("internal channel was empty"),
        }
    }

    #[test]
    fn handle_kiss_payload_forwards_arq_relevant_frame_addressed_to_us() {
        let local = addr("N0CALL-1");
        let (events_tx, _events_rx) = async_channel::unbounded();
        let (internal_tx, internal_rx) = mpsc::channel();

        let frame = Ax25Frame {
            source: addr("KD3BFP-9"),
            destination: local.clone(),
            route: vec![],
            command_or_response: Some(CommandResponse::Command),
            content: FrameContent::SetAsynchronousBalancedMode(SetAsynchronousBalancedMode { poll: true }),
        };
        let bytes = frame.to_bytes();

        handle_kiss_payload(&bytes, &events_tx, &internal_tx, &local);

        match internal_rx.try_recv() {
            Ok(InternalEvent::Frame(f)) => assert_eq!(f.source, addr("KD3BFP-9")),
            Ok(_) => panic!("expected Frame"),
            Err(_) => panic!("internal channel was empty"),
        }
    }

    #[test]
    fn handle_kiss_payload_does_not_forward_ui_frames_to_the_arq_engine() {
        let local = addr("N0CALL-1");
        let (events_tx, _events_rx) = async_channel::unbounded();
        let (internal_tx, internal_rx) = mpsc::channel();

        let frame = Ax25Frame::new_simple_ui_frame(addr("KD3BFP-9"), local.clone(), b"CQ CQ".to_vec());
        let bytes = frame.to_bytes();

        handle_kiss_payload(&bytes, &events_tx, &internal_tx, &local);

        assert!(internal_rx.try_recv().is_err());
    }
}
