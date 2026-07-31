//! Pure modulus-8 AX.25 connected-mode ARQ state machine, decoupled from
//! any actual I/O -- feed it received frames/send requests/clock ticks, get
//! back a list of [`Action`]s (frames to transmit, data to deliver, state
//! changes to report). This lets the protocol logic be fully unit-tested
//! without a live TNC, matching this codebase's existing pattern for
//! things that can't be live-verified in this dev environment (see
//! `mailbox::should_answer`, `keyboard_mode::should_answer`).
//!
//! Scope: modulus-8 only (see `xid` for why). Ack policy is always an
//! explicit RR, never piggybacked, and there's no delayed-ack (T2) timer --
//! a deliberate simplification favoring a simpler, more obviously-correct
//! state machine over shaving a few frames of airtime. Duplicate/out-of-
//! order handling is a plain go-back-N REJ (no SREJ), and FRMR emission is
//! kept narrow (only "control field invalid for current state") rather
//! than covering every spec-defined violation -- the hard requirement here
//! is "never hang or crash", not full RFC conformance on every edge.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ax25::frame::{
    Address, Ax25Frame, CommandResponse, Disconnect, FrameContent, FrameReject, Information, ProtocolIdentifier, ReceiveNotReady,
    ReceiveReady, Reject, RouteEntry, SetAsynchronousBalancedMode, UnnumberedAcknowledge,
};
use pr_core::ConnState;

fn mod8(x: u8) -> u8 {
    x & 0x07
}

fn seq_add(a: u8, b: u8) -> u8 {
    mod8(a.wrapping_add(b))
}

#[derive(Debug, Clone)]
pub struct ArqConfig {
    /// Window size k, 1-7 (7 is the modulus-8 maximum outstanding count).
    pub window: u8,
    /// T1 acknowledge/retransmit timer.
    pub t1: Duration,
    /// N2, max retries before giving up.
    pub n2: u32,
    /// N1, max I-frame payload size in bytes (paclen).
    pub n1_bytes: usize,
}

impl Default for ArqConfig {
    fn default() -> Self {
        Self { window: 4, t1: Duration::from_millis(4000), n2: 10, n1_bytes: 256 }
    }
}

/// Builds an [`ArqConfig`] from the user-editable, all-`Option` config
/// struct -- `None` fields fall back to the defaults above, so an existing
/// saved config with no `kiss_arq` section behaves identically to a fresh
/// one.
pub fn arq_config_from(params: &pr_core::KissArqParams) -> ArqConfig {
    let default = ArqConfig::default();
    ArqConfig {
        window: params.window.unwrap_or(default.window).clamp(1, 7),
        t1: params.t1_ms.map(|ms| Duration::from_millis(u64::from(ms))).unwrap_or(default.t1),
        n2: params.n2.unwrap_or(default.n2),
        n1_bytes: params.n1_bytes.unwrap_or(default.n1_bytes).max(1),
    }
}

#[derive(Debug)]
pub enum Action {
    /// A frame the `ax25` crate can encode -- most of them.
    Transmit(Ax25Frame),
    /// Full on-wire bytes for a frame the crate can't represent (XID
    /// replies only, produced by `xid::handle_peer_xid`, applied by the
    /// caller alongside these actions -- never produced by `ArqSession`
    /// itself, but kept here so the caller has one uniform `Action` type
    /// to apply for everything an incoming frame can provoke).
    TransmitRaw(Vec<u8>),
    /// Payload to deliver to the application (`PortEvent::Data`).
    Data(Vec<u8>),
    /// Connection state changed (`PortEvent::ConnState`).
    StateChanged(ConnState),
    /// A line to show in the Monitor view.
    Monitor(String),
    /// The connection is fully torn down (`PortEvent::ConnectionClosed`).
    Closed,
}

#[derive(Clone)]
struct PendingIFrame {
    seq: u8,
    info: Vec<u8>,
}

enum SKind {
    Rr,
    Rej,
}

pub struct ArqSession {
    local: Address,
    remote: Address,
    route: Vec<RouteEntry>,
    cfg: ArqConfig,
    state: ConnState,
    vs: u8,
    vr: u8,
    outstanding: VecDeque<PendingIFrame>,
    backlog: VecDeque<Vec<u8>>,
    retry_count: u32,
    t1_deadline: Option<Instant>,
    peer_busy: bool,
}

impl ArqSession {
    pub fn new(local: Address, remote: Address, route: Vec<RouteEntry>, cfg: ArqConfig) -> Self {
        Self {
            local,
            remote,
            route,
            cfg,
            state: ConnState::Disconnected,
            vs: 0,
            vr: 0,
            outstanding: VecDeque::new(),
            backlog: VecDeque::new(),
            retry_count: 0,
            t1_deadline: None,
            peer_busy: false,
        }
    }

    pub fn state(&self) -> ConnState {
        self.state
    }

    pub fn remote(&self) -> &Address {
        &self.remote
    }

    /// We're initiating. Sends SABM, starts T1. No-op if not currently
    /// `Disconnected` (a fresh session is always constructed per attempt,
    /// so this should only ever be called once per session in practice).
    pub fn open(&mut self, now: Instant) -> Vec<Action> {
        if self.state != ConnState::Disconnected {
            return vec![];
        }
        self.reset_sequence_state();
        self.state = ConnState::Connecting;
        self.t1_deadline = Some(now + self.cfg.t1);
        vec![Action::Transmit(self.build_sabm(true)), Action::StateChanged(ConnState::Connecting)]
    }

    /// Peer sent SABM and we're accepting it for a brand-new session.
    /// Resets sequence state, replies UA, and goes straight to `Connected`
    /// (no `Connecting` step) -- matches the raw-socket backend's accept
    /// path.
    pub fn accept_incoming(&mut self, poll: bool, _now: Instant) -> Vec<Action> {
        self.reset_sequence_state();
        self.state = ConnState::Connected;
        vec![Action::Transmit(self.build_ua(poll)), Action::StateChanged(ConnState::Connected)]
    }

    /// Local close request. No-op if already `Disconnected`.
    pub fn close(&mut self, now: Instant) -> Vec<Action> {
        if self.state == ConnState::Disconnected {
            return vec![];
        }
        self.state = ConnState::Disconnecting;
        self.retry_count = 0;
        self.outstanding.clear();
        self.backlog.clear();
        self.t1_deadline = Some(now + self.cfg.t1);
        vec![Action::Transmit(self.build_disc(true)), Action::StateChanged(ConnState::Disconnecting)]
    }

    /// Application bytes to send, fragmented to `cfg.n1_bytes` and queued;
    /// as much as the current window allows goes out immediately, the rest
    /// waits in a backlog drained as the window frees up. Never blocks --
    /// matches `PortCommand::Send` already being fire-and-forget. A no-op
    /// while not `Connected`.
    pub fn send(&mut self, bytes: Vec<u8>, now: Instant) -> Vec<Action> {
        if self.state != ConnState::Connected {
            return vec![];
        }
        for chunk in fragment(&bytes, self.cfg.n1_bytes) {
            self.backlog.push_back(chunk);
        }
        self.push_backlog_into_window(now)
    }

    /// Dispatch a frame already known to be addressed to us and relevant to
    /// this session's remote callsign.
    pub fn on_frame(&mut self, frame: &Ax25Frame, now: Instant) -> Vec<Action> {
        match &frame.content {
            FrameContent::Information(i) => self.on_information(i, now),
            FrameContent::ReceiveReady(rr) => self.on_rr(rr, now),
            FrameContent::ReceiveNotReady(rnr) => self.on_rnr(rnr, now),
            FrameContent::Reject(rej) => self.on_reject(rej, now),
            FrameContent::SetAsynchronousBalancedMode(s) => self.on_sabm(s),
            FrameContent::Disconnect(d) => self.on_disc(d),
            FrameContent::UnnumberedAcknowledge(_) => self.on_ua(now),
            FrameContent::DisconnectedMode(_) => self.on_dm(),
            FrameContent::FrameReject(_) => self.on_frmr(),
            FrameContent::UnnumberedInformation(_) | FrameContent::UnknownContent(_) => vec![],
        }
    }

    /// Checked periodically (not on a dedicated timer thread -- see
    /// `kiss_runner`) to drive T1 expiry: retransmit, or give up past N2.
    pub fn tick(&mut self, now: Instant) -> Vec<Action> {
        let Some(deadline) = self.t1_deadline else { return vec![] };
        if now < deadline {
            return vec![];
        }
        self.retry_count += 1;
        if self.retry_count > self.cfg.n2 {
            return self.give_up();
        }
        self.t1_deadline = Some(now + self.cfg.t1);
        match self.state {
            ConnState::Connecting => vec![Action::Transmit(self.build_sabm(true))],
            ConnState::Disconnecting => vec![Action::Transmit(self.build_disc(true))],
            ConnState::Connected => {
                if self.outstanding.is_empty() {
                    // Shouldn't happen (t1_deadline is only armed alongside
                    // a nonempty `outstanding`) -- defensive, avoids ever
                    // retry-looping toward a false give-up over nothing.
                    self.t1_deadline = None;
                    self.retry_count = 0;
                    vec![]
                } else {
                    self.retransmit_outstanding(true)
                }
            }
            ConnState::Disconnected => vec![],
        }
    }

    fn reset_sequence_state(&mut self) {
        self.vs = 0;
        self.vr = 0;
        self.retry_count = 0;
        self.outstanding.clear();
        self.backlog.clear();
        self.peer_busy = false;
        self.t1_deadline = None;
    }

    fn on_information(&mut self, i: &Information, now: Instant) -> Vec<Action> {
        if self.state != ConnState::Connected {
            // A data frame while we're not in a state to accept one is the
            // one case we treat as a genuine protocol violation worth a
            // FRMR (w: "control field invalid or not implemented").
            let control = (i.receive_sequence << 5) | (u8::from(i.poll) << 4) | (i.send_sequence << 1);
            return vec![Action::Transmit(self.build_frmr(true, false, false, false, control))];
        }
        let mut actions = self.ack_up_to(i.receive_sequence, now);
        if i.send_sequence == self.vr {
            self.vr = seq_add(self.vr, 1);
            actions.push(Action::Data(i.info.clone()));
            actions.push(Action::Transmit(self.build_s_frame(SKind::Rr, false)));
        } else {
            // Anything other than exactly the expected N(S) -- whether an
            // out-of-order gap or the peer replaying something we already
            // saw -- gets a uniform go-back-N REJ. Safe either way: at
            // worst a genuine duplicate gets asked to resend again, which
            // is spec-legal and just mildly wasteful, not incorrect.
            actions.push(Action::Transmit(self.build_s_frame(SKind::Rej, false)));
        }
        actions
    }

    fn on_rr(&mut self, rr: &ReceiveReady, now: Instant) -> Vec<Action> {
        if self.state != ConnState::Connected {
            return vec![];
        }
        self.peer_busy = false;
        let mut actions = self.ack_up_to(rr.receive_sequence, now);
        if rr.poll_or_final {
            actions.push(Action::Transmit(self.build_s_frame(SKind::Rr, true)));
        }
        actions
    }

    fn on_rnr(&mut self, rnr: &ReceiveNotReady, now: Instant) -> Vec<Action> {
        if self.state != ConnState::Connected {
            return vec![];
        }
        self.peer_busy = true;
        let mut actions = self.ack_up_to(rnr.receive_sequence, now);
        if rnr.poll_or_final {
            actions.push(Action::Transmit(self.build_s_frame(SKind::Rr, true)));
        }
        actions
    }

    fn on_reject(&mut self, rej: &Reject, now: Instant) -> Vec<Action> {
        if self.state != ConnState::Connected {
            return vec![];
        }
        self.peer_busy = false;
        let mut actions = self.ack_up_to(rej.receive_sequence, now);
        actions.extend(self.retransmit_outstanding(false));
        if rej.poll_or_final {
            actions.push(Action::Transmit(self.build_s_frame(SKind::Rr, true)));
        }
        actions
    }

    /// Spec "link reset": peer (re)sends SABM for a session we already
    /// have. Ack and reset sequence state, but -- deliberately -- stay on
    /// this same session/`ConnectionId` rather than tearing down and
    /// reopening, to avoid flapping the UI tab for what's often just the
    /// peer's TNC recovering from a missed UA.
    fn on_sabm(&mut self, s: &SetAsynchronousBalancedMode) -> Vec<Action> {
        let was_connected = self.state == ConnState::Connected;
        self.reset_sequence_state();
        self.state = ConnState::Connected;
        let mut actions = vec![Action::Transmit(self.build_ua(s.poll))];
        if was_connected {
            actions.push(Action::Monitor(format!("{} reset the link", self.remote)));
        } else {
            actions.push(Action::StateChanged(ConnState::Connected));
        }
        actions
    }

    fn on_disc(&mut self, d: &Disconnect) -> Vec<Action> {
        let ua = self.build_ua(d.poll);
        let was_disconnected = self.state == ConnState::Disconnected;
        self.state = ConnState::Disconnected;
        self.outstanding.clear();
        self.backlog.clear();
        self.t1_deadline = None;
        let mut actions = vec![Action::Transmit(ua)];
        if !was_disconnected {
            actions.push(Action::StateChanged(ConnState::Disconnected));
            actions.push(Action::Closed);
        }
        actions
    }

    fn on_ua(&mut self, now: Instant) -> Vec<Action> {
        match self.state {
            ConnState::Connecting => {
                self.state = ConnState::Connected;
                self.retry_count = 0;
                self.t1_deadline = None;
                let mut actions = vec![Action::StateChanged(ConnState::Connected)];
                actions.extend(self.push_backlog_into_window(now));
                actions
            }
            ConnState::Disconnecting => {
                self.state = ConnState::Disconnected;
                self.t1_deadline = None;
                self.retry_count = 0;
                vec![Action::StateChanged(ConnState::Disconnected), Action::Closed]
            }
            _ => vec![], // unexpected UA -- ignore rather than risk misbehaving
        }
    }

    fn on_dm(&mut self) -> Vec<Action> {
        let was_disconnected = self.state == ConnState::Disconnected;
        self.state = ConnState::Disconnected;
        self.outstanding.clear();
        self.backlog.clear();
        self.t1_deadline = None;
        self.retry_count = 0;
        if was_disconnected {
            vec![]
        } else {
            vec![
                Action::Monitor(format!("{} refused or reset the link (DM)", self.remote)),
                Action::StateChanged(ConnState::Disconnected),
                Action::Closed,
            ]
        }
    }

    fn on_frmr(&mut self) -> Vec<Action> {
        // No recovery path for a peer-reported protocol violation in this
        // from-scratch implementation -- treat it like a DM (the peer is
        // done with this link) rather than attempting to resync.
        let was_disconnected = self.state == ConnState::Disconnected;
        self.state = ConnState::Disconnected;
        self.outstanding.clear();
        self.backlog.clear();
        self.t1_deadline = None;
        self.retry_count = 0;
        if was_disconnected {
            vec![]
        } else {
            vec![
                Action::Monitor(format!("{} reported a protocol error (FRMR), disconnecting", self.remote)),
                Action::StateChanged(ConnState::Disconnected),
                Action::Closed,
            ]
        }
    }

    fn give_up(&mut self) -> Vec<Action> {
        let reason = match self.state {
            ConnState::Connecting => format!("connect to {} failed: no response after {} retries", self.remote, self.cfg.n2),
            ConnState::Disconnecting => {
                format!("{} did not confirm disconnect after {} retries, closing locally", self.remote, self.cfg.n2)
            }
            _ => format!("link to {} timed out after {} retries, disconnecting", self.remote, self.cfg.n2),
        };
        let was_disconnected = self.state == ConnState::Disconnected;
        self.state = ConnState::Disconnected;
        self.outstanding.clear();
        self.backlog.clear();
        self.t1_deadline = None;
        self.retry_count = 0;
        let mut actions = vec![Action::Monitor(reason)];
        if !was_disconnected {
            actions.push(Action::StateChanged(ConnState::Disconnected));
            actions.push(Action::Closed);
        }
        actions
    }

    /// Acks every outstanding frame strictly before `nr` (the peer's N(R):
    /// "I have everything up to but not including this"), restarts/stops
    /// T1 accordingly, and drains as much backlog as the now-larger window
    /// allows.
    fn ack_up_to(&mut self, nr: u8, now: Instant) -> Vec<Action> {
        while let Some(front) = self.outstanding.front() {
            if front.seq == nr {
                break;
            }
            self.outstanding.pop_front();
        }
        self.retry_count = 0;
        if self.outstanding.is_empty() {
            self.t1_deadline = None;
        } else {
            self.t1_deadline = Some(now + self.cfg.t1);
        }
        self.push_backlog_into_window(now)
    }

    fn push_backlog_into_window(&mut self, now: Instant) -> Vec<Action> {
        let mut actions = Vec::new();
        if self.peer_busy {
            return actions;
        }
        while self.outstanding.len() < self.cfg.window as usize {
            let Some(chunk) = self.backlog.pop_front() else { break };
            let seq = self.vs;
            self.vs = seq_add(self.vs, 1);
            let frame = self.build_i_frame(seq, chunk.clone(), false);
            self.outstanding.push_back(PendingIFrame { seq, info: chunk });
            if self.t1_deadline.is_none() {
                self.t1_deadline = Some(now + self.cfg.t1);
            }
            actions.push(Action::Transmit(frame));
        }
        actions
    }

    fn retransmit_outstanding(&self, poll_last: bool) -> Vec<Action> {
        let len = self.outstanding.len();
        self.outstanding
            .iter()
            .enumerate()
            .map(|(idx, p)| {
                let poll = poll_last && idx + 1 == len;
                Action::Transmit(self.build_i_frame(p.seq, p.info.clone(), poll))
            })
            .collect()
    }

    fn build_i_frame(&self, seq: u8, info: Vec<u8>, poll: bool) -> Ax25Frame {
        Ax25Frame {
            source: self.local.clone(),
            destination: self.remote.clone(),
            route: self.route.clone(),
            command_or_response: Some(CommandResponse::Command),
            content: FrameContent::Information(Information { pid: ProtocolIdentifier::None, info, receive_sequence: self.vr, send_sequence: seq, poll }),
        }
    }

    fn build_s_frame(&self, kind: SKind, poll_or_final: bool) -> Ax25Frame {
        let content = match kind {
            SKind::Rr => FrameContent::ReceiveReady(ReceiveReady { receive_sequence: self.vr, poll_or_final }),
            SKind::Rej => FrameContent::Reject(Reject { receive_sequence: self.vr, poll_or_final }),
        };
        Ax25Frame {
            source: self.local.clone(),
            destination: self.remote.clone(),
            route: self.route.clone(),
            command_or_response: Some(CommandResponse::Response),
            content,
        }
    }

    fn build_sabm(&self, poll: bool) -> Ax25Frame {
        Ax25Frame {
            source: self.local.clone(),
            destination: self.remote.clone(),
            route: self.route.clone(),
            command_or_response: Some(CommandResponse::Command),
            content: FrameContent::SetAsynchronousBalancedMode(SetAsynchronousBalancedMode { poll }),
        }
    }

    fn build_disc(&self, poll: bool) -> Ax25Frame {
        Ax25Frame {
            source: self.local.clone(),
            destination: self.remote.clone(),
            route: self.route.clone(),
            command_or_response: Some(CommandResponse::Command),
            content: FrameContent::Disconnect(Disconnect { poll }),
        }
    }

    fn build_ua(&self, final_bit: bool) -> Ax25Frame {
        Ax25Frame {
            source: self.local.clone(),
            destination: self.remote.clone(),
            route: self.route.clone(),
            command_or_response: Some(CommandResponse::Response),
            content: FrameContent::UnnumberedAcknowledge(UnnumberedAcknowledge { final_bit }),
        }
    }

    fn build_frmr(&self, w: bool, x: bool, y: bool, z: bool, rejected_control_field_raw: u8) -> Ax25Frame {
        Ax25Frame {
            source: self.local.clone(),
            destination: self.remote.clone(),
            route: self.route.clone(),
            command_or_response: Some(CommandResponse::Response),
            content: FrameContent::FrameReject(FrameReject {
                final_bit: false,
                rejected_control_field_raw,
                z,
                y,
                x,
                w,
                receive_sequence: self.vr,
                send_sequence: self.vs,
                command_response: CommandResponse::Response,
            }),
        }
    }
}

fn fragment(bytes: &[u8], n1: usize) -> Vec<Vec<u8>> {
    if bytes.is_empty() {
        return Vec::new();
    }
    bytes.chunks(n1.max(1)).map(<[u8]>::to_vec).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ax25::frame::DisconnectedMode;

    fn addr(s: &str) -> Address {
        s.parse().unwrap()
    }

    fn session() -> ArqSession {
        ArqSession::new(addr("N0CALL-1"), addr("KD3BFP-9"), vec![], ArqConfig { window: 4, t1: Duration::from_millis(1000), n2: 3, n1_bytes: 4 })
    }

    fn transmitted_i_frames(actions: &[Action]) -> Vec<&Information> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Transmit(f) => match &f.content {
                    FrameContent::Information(i) => Some(i),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    fn contains_transmit(actions: &[Action], matches: impl Fn(&FrameContent) -> bool) -> bool {
        actions.iter().any(|a| matches!(a, Action::Transmit(f) if matches(&f.content)))
    }

    fn state_changes(actions: &[Action]) -> Vec<ConnState> {
        actions.iter().filter_map(|a| if let Action::StateChanged(s) = a { Some(*s) } else { None }).collect()
    }

    fn ua_frame(poll: bool) -> Ax25Frame {
        Ax25Frame {
            source: addr("KD3BFP-9"),
            destination: addr("N0CALL-1"),
            route: vec![],
            command_or_response: Some(CommandResponse::Response),
            content: FrameContent::UnnumberedAcknowledge(UnnumberedAcknowledge { final_bit: poll }),
        }
    }

    fn rr_frame(nr: u8, poll: bool) -> Ax25Frame {
        Ax25Frame {
            source: addr("KD3BFP-9"),
            destination: addr("N0CALL-1"),
            route: vec![],
            command_or_response: Some(CommandResponse::Response),
            content: FrameContent::ReceiveReady(ReceiveReady { receive_sequence: nr, poll_or_final: poll }),
        }
    }

    #[test]
    fn open_sends_sabm_and_arms_t1() {
        let mut s = session();
        let now = Instant::now();
        let actions = s.open(now);
        assert!(contains_transmit(&actions, |c| matches!(c, FrameContent::SetAsynchronousBalancedMode(_))));
        assert_eq!(state_changes(&actions), vec![ConnState::Connecting]);
        assert_eq!(s.state(), ConnState::Connecting);
        // Idle before T1 -- no retransmit yet.
        assert!(s.tick(now).is_empty());
    }

    #[test]
    fn successful_outgoing_handshake() {
        let mut s = session();
        let now = Instant::now();
        s.open(now);
        let actions = s.on_frame(&ua_frame(true), now);
        assert_eq!(state_changes(&actions), vec![ConnState::Connected]);
        assert_eq!(s.state(), ConnState::Connected);
    }

    #[test]
    fn connect_timeout_gives_up_after_exactly_n2_retries() {
        let mut s = session();
        let mut now = Instant::now();
        s.open(now);
        let mut sabm_retransmits = 0;
        let mut gave_up = false;
        for _ in 0..10 {
            now += Duration::from_millis(1001);
            let actions = s.tick(now);
            if contains_transmit(&actions, |c| matches!(c, FrameContent::SetAsynchronousBalancedMode(_))) {
                sabm_retransmits += 1;
            }
            if state_changes(&actions).contains(&ConnState::Disconnected) {
                gave_up = true;
                break;
            }
        }
        assert!(gave_up);
        assert_eq!(sabm_retransmits, 3); // n2 == 3 for the test session
        assert_eq!(s.state(), ConnState::Disconnected);
    }

    #[test]
    fn incoming_connect_skips_connecting_state() {
        let mut s = session();
        let now = Instant::now();
        let actions = s.accept_incoming(true, now);
        assert!(contains_transmit(&actions, |c| matches!(c, FrameContent::UnnumberedAcknowledge(_))));
        assert_eq!(state_changes(&actions), vec![ConnState::Connected]);
        assert_eq!(s.state(), ConnState::Connected);
    }

    #[test]
    fn data_transfer_within_window() {
        let mut s = session();
        let now = Instant::now();
        s.open(now);
        s.on_frame(&ua_frame(false), now);
        let actions = s.send(b"hi".to_vec(), now);
        let frames = transmitted_i_frames(&actions);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].send_sequence, 0);
        assert_eq!(frames[0].info, b"hi");
    }

    #[test]
    fn window_full_backpressure_then_drains_on_ack() {
        let mut s = session();
        let now = Instant::now();
        s.open(now);
        s.on_frame(&ua_frame(false), now);
        // window == 4; send 6 single-byte chunks (n1_bytes=4, so each byte
        // is its own fragment only if we send them separately -- send one
        // big buffer that fragments into more pieces than the window.
        let actions = s.send(vec![0u8; 6 * 4], now); // 6 chunks of 4 bytes each
        let frames = transmitted_i_frames(&actions);
        assert_eq!(frames.len(), 4); // only window's worth went out
        assert_eq!(s.outstanding.len(), 4);
        assert_eq!(s.backlog.len(), 2);

        // Ack the first two -- window opens by two, backlog drains by two.
        let actions = s.on_frame(&rr_frame(2, false), now);
        let frames = transmitted_i_frames(&actions);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].send_sequence, 4);
        assert_eq!(frames[1].send_sequence, 5);
        assert!(s.backlog.is_empty());
    }

    #[test]
    fn mismatched_sequence_number_gets_rejected_not_delivered() {
        let mut s = session();
        let now = Instant::now();
        s.accept_incoming(false, now);
        let bad = Ax25Frame {
            source: addr("KD3BFP-9"),
            destination: addr("N0CALL-1"),
            route: vec![],
            command_or_response: Some(CommandResponse::Command),
            content: FrameContent::Information(Information { pid: ProtocolIdentifier::None, info: b"x".to_vec(), receive_sequence: 0, send_sequence: 5, poll: false }),
        };
        let actions = s.on_frame(&bad, now);
        assert!(!actions.iter().any(|a| matches!(a, Action::Data(_))));
        assert!(contains_transmit(&actions, |c| matches!(c, FrameContent::Reject(_))));
    }

    #[test]
    fn reject_from_peer_retransmits_from_its_n_r() {
        let mut s = session();
        let now = Instant::now();
        s.open(now);
        s.on_frame(&ua_frame(false), now);
        s.send(vec![1, 2, 3, 4, 5, 6, 7, 8], now); // 2 I-frames, seq 0 and 1
        let rej = Ax25Frame {
            source: addr("KD3BFP-9"),
            destination: addr("N0CALL-1"),
            route: vec![],
            command_or_response: Some(CommandResponse::Response),
            content: FrameContent::Reject(Reject { receive_sequence: 0, poll_or_final: false }),
        };
        let actions = s.on_frame(&rej, now);
        let frames = transmitted_i_frames(&actions);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].send_sequence, 0);
        assert_eq!(frames[1].send_sequence, 1);
    }

    #[test]
    fn t1_retransmit_then_eventually_acked() {
        let mut s = session();
        let mut now = Instant::now();
        s.open(now);
        s.on_frame(&ua_frame(false), now);
        s.send(b"hi".to_vec(), now);
        now += Duration::from_millis(1001);
        let actions = s.tick(now);
        let frames = transmitted_i_frames(&actions);
        assert_eq!(frames.len(), 1); // retransmitted the one outstanding frame

        let actions = s.on_frame(&rr_frame(1, false), now);
        assert!(transmitted_i_frames(&actions).is_empty());
        assert!(s.outstanding.is_empty());
        // T1 disarmed -- no further retransmit even well past the old deadline.
        now += Duration::from_millis(5000);
        assert!(s.tick(now).is_empty());
    }

    #[test]
    fn peer_disc_gets_ua_and_closes() {
        let mut s = session();
        let now = Instant::now();
        s.accept_incoming(false, now);
        let disc = Ax25Frame {
            source: addr("KD3BFP-9"),
            destination: addr("N0CALL-1"),
            route: vec![],
            command_or_response: Some(CommandResponse::Command),
            content: FrameContent::Disconnect(Disconnect { poll: true }),
        };
        let actions = s.on_frame(&disc, now);
        assert!(contains_transmit(&actions, |c| matches!(c, FrameContent::UnnumberedAcknowledge(_))));
        assert_eq!(state_changes(&actions), vec![ConnState::Disconnected]);
        assert!(actions.iter().any(|a| matches!(a, Action::Closed)));
    }

    #[test]
    fn local_close_success() {
        let mut s = session();
        let now = Instant::now();
        s.accept_incoming(false, now);
        let actions = s.close(now);
        assert!(contains_transmit(&actions, |c| matches!(c, FrameContent::Disconnect(_))));
        assert_eq!(state_changes(&actions), vec![ConnState::Disconnecting]);

        let actions = s.on_frame(&ua_frame(false), now);
        assert_eq!(state_changes(&actions), vec![ConnState::Disconnected]);
        assert!(actions.iter().any(|a| matches!(a, Action::Closed)));
    }

    #[test]
    fn local_close_with_no_reply_gives_up_after_n2() {
        let mut s = session();
        let mut now = Instant::now();
        s.accept_incoming(false, now);
        s.close(now);
        let mut gave_up = false;
        for _ in 0..10 {
            now += Duration::from_millis(1001);
            let actions = s.tick(now);
            if state_changes(&actions).contains(&ConnState::Disconnected) {
                gave_up = true;
                break;
            }
        }
        assert!(gave_up);
        assert_eq!(s.state(), ConnState::Disconnected);
    }

    #[test]
    fn peer_sabm_while_connected_resets_link_without_closing_tab() {
        let mut s = session();
        let now = Instant::now();
        s.accept_incoming(false, now);
        s.send(b"hi".to_vec(), now);
        assert!(!s.outstanding.is_empty());

        let sabm = Ax25Frame {
            source: addr("KD3BFP-9"),
            destination: addr("N0CALL-1"),
            route: vec![],
            command_or_response: Some(CommandResponse::Command),
            content: FrameContent::SetAsynchronousBalancedMode(SetAsynchronousBalancedMode { poll: true }),
        };
        let actions = s.on_frame(&sabm, now);
        assert!(contains_transmit(&actions, |c| matches!(c, FrameContent::UnnumberedAcknowledge(_))));
        assert!(!actions.iter().any(|a| matches!(a, Action::Closed)));
        assert!(!actions.iter().any(|a| matches!(a, Action::StateChanged(_)))); // stays Connected -- no churn
        assert_eq!(s.state(), ConnState::Connected);
        assert!(s.outstanding.is_empty()); // sequence state reset
    }

    #[test]
    fn frmr_on_information_frame_while_not_connected() {
        let mut s = session();
        let now = Instant::now();
        s.open(now); // state is Connecting, not Connected
        let bad = Ax25Frame {
            source: addr("KD3BFP-9"),
            destination: addr("N0CALL-1"),
            route: vec![],
            command_or_response: Some(CommandResponse::Command),
            content: FrameContent::Information(Information { pid: ProtocolIdentifier::None, info: b"x".to_vec(), receive_sequence: 0, send_sequence: 0, poll: false }),
        };
        let actions = s.on_frame(&bad, now);
        assert!(contains_transmit(&actions, |c| matches!(c, FrameContent::FrameReject(_))));
        assert!(!actions.iter().any(|a| matches!(a, Action::Data(_) | Action::StateChanged(_))));
        assert_eq!(s.state(), ConnState::Connecting);
    }

    #[test]
    fn dm_at_any_state_closes_immediately() {
        let mut s = session();
        let now = Instant::now();
        s.open(now);
        let dm = Ax25Frame {
            source: addr("KD3BFP-9"),
            destination: addr("N0CALL-1"),
            route: vec![],
            command_or_response: Some(CommandResponse::Response),
            content: FrameContent::DisconnectedMode(DisconnectedMode { final_bit: true }),
        };
        let actions = s.on_frame(&dm, now);
        assert_eq!(state_changes(&actions), vec![ConnState::Disconnected]);
        assert!(actions.iter().any(|a| matches!(a, Action::Closed)));
        assert_eq!(s.state(), ConnState::Disconnected);
    }

    #[test]
    fn rnr_suppresses_sending_until_rr_clears_it() {
        let mut s = session();
        let now = Instant::now();
        s.accept_incoming(false, now);
        let rnr = Ax25Frame {
            source: addr("KD3BFP-9"),
            destination: addr("N0CALL-1"),
            route: vec![],
            command_or_response: Some(CommandResponse::Response),
            content: FrameContent::ReceiveNotReady(ReceiveNotReady { receive_sequence: 0, poll_or_final: false }),
        };
        s.on_frame(&rnr, now);
        let actions = s.send(b"hi".to_vec(), now);
        assert!(transmitted_i_frames(&actions).is_empty());
        assert_eq!(s.backlog.len(), 1);

        let actions = s.on_frame(&rr_frame(0, false), now);
        let frames = transmitted_i_frames(&actions);
        assert_eq!(frames.len(), 1);
        assert!(s.backlog.is_empty());
    }

    #[test]
    fn tick_on_idle_disconnected_session_is_a_no_op() {
        let mut s = session();
        assert!(s.tick(Instant::now()).is_empty());
    }

    #[test]
    fn fragmentation_splits_and_preserves_order() {
        let mut s = session(); // n1_bytes = 4, window = 4
        let now = Instant::now();
        s.open(now);
        s.on_frame(&ua_frame(false), now);
        let payload: Vec<u8> = (0..10).collect(); // 10 bytes -> 3 fragments of 4,4,2
        let actions = s.send(payload.clone(), now);
        let frames = transmitted_i_frames(&actions);
        assert_eq!(frames.len(), 3);
        let reassembled: Vec<u8> = frames.iter().flat_map(|i| i.info.clone()).collect();
        assert_eq!(reassembled, payload);
        assert_eq!(frames[0].send_sequence, 0);
        assert_eq!(frames[1].send_sequence, 1);
        assert_eq!(frames[2].send_sequence, 2);
    }
}
