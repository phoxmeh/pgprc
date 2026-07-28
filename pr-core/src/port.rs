use std::sync::mpsc;
use std::thread::JoinHandle;

/// Identifies one logical connection (AX.25 connected-mode session, or a
/// terminal session) multiplexed over a single port.
pub type ConnectionId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Connecting,
    Connected,
    Disconnecting,
    Disconnected,
}

/// Events flowing from a port's background thread to the UI.
#[derive(Debug, Clone)]
pub enum PortEvent {
    PortConnected,
    PortDisconnected { reason: Option<String> },
    PortError { message: String },
    /// A line to show in the live traffic Monitor view (already formatted).
    Monitor { line: String },
    ConnectionOpened { id: ConnectionId, label: String },
    ConnectionClosed { id: ConnectionId },
    ConnState { id: ConnectionId, state: ConnState },
    Data { id: ConnectionId, bytes: Vec<u8> },
    /// A definite station callsign was observed (a monitored frame's source,
    /// a connect notification, ...), for address-book "last heard" tracking.
    /// Separate from `Monitor` (which is just display text) so the UI
    /// doesn't need to re-parse callsigns back out of formatted strings.
    StationHeard { callsign: String },
}

/// Commands flowing from the UI to a port's background thread.
///
/// `OpenConnection` does not carry an id: the port assigns one (it must also
/// be able to mint ids for connections initiated by the *other* end, e.g. an
/// AGWPE peer connecting to us) and reports it back via
/// `PortEvent::ConnectionOpened`.
#[derive(Debug, Clone)]
pub enum PortCommand {
    Connect,
    Disconnect,
    /// `via` is an optional digipeater path (e.g. `["WIDE1-1", "WIDE2-1"]`),
    /// empty for a direct connection. Backends with no digipeating support
    /// ignore a non-empty list.
    OpenConnection { remote: String, via: Vec<String> },
    CloseConnection { id: ConnectionId },
    Send { id: ConnectionId, bytes: Vec<u8> },
    /// One-shot unconnected (UI) frame, e.g. a beacon. Backends that have no
    /// notion of unproto traffic (Telnet, SSH) silently ignore this. `via` is
    /// an optional digipeater path, same as `OpenConnection`.
    SendUnproto { dest: String, via: Vec<String>, bytes: Vec<u8> },
}

pub struct PortHandle {
    pub cmd_tx: mpsc::Sender<PortCommand>,
    pub events: async_channel::Receiver<PortEvent>,
    pub join: JoinHandle<()>,
}

/// Implemented by each transport/protocol backend (telnet, ssh, AGWPE, AX.25
/// raw socket, ...). `run` owns a dedicated OS thread and does blocking I/O;
/// all UI interaction happens exclusively through the channels it's given.
pub trait PortRunner: Send + 'static {
    fn run(self: Box<Self>, cmd_rx: mpsc::Receiver<PortCommand>, event_tx: async_channel::Sender<PortEvent>);
}

pub fn spawn_port<R: PortRunner>(runner: R) -> PortHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (event_tx, event_rx) = async_channel::unbounded();
    let join = std::thread::spawn(move || {
        Box::new(runner).run(cmd_rx, event_tx);
    });
    PortHandle {
        cmd_tx,
        events: event_rx,
        join,
    }
}
