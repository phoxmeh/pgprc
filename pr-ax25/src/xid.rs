//! AX.25 v2.2 XID (parameter negotiation) frame encode/decode, and the
//! decision logic for responding to a peer-initiated XID. Entirely
//! hand-rolled: `FrameContent` in the `ax25` crate has no `Xid` variant at
//! all (and can't be extended from outside -- it's a closed enum), so
//! there's nothing here to reuse from the crate beyond `Address` and the
//! address/route encoder (via a throwaway scaffold frame, see
//! `build_response_frame`).
//!
//! Scope, per this app's connected-mode design: modulus-8 only. We only
//! ever *respond* to a peer's XID (never send one proactively on outgoing
//! connect -- a plain SABM is sent directly; XID pre-negotiation is
//! optional per spec), and our reply always advertises the same fixed
//! modulus-8 parameter set regardless of what the peer proposed.
//!
//! SPEC-VERIFY: the parameter-identifier (PI) numbers and Classes-of-
//! Procedures/HDLC-Optional-Functions bit assignments below follow the
//! AX.25 2.2 spec (section 4.3.3.6) as commonly documented, but have not
//! been cross-checked against the spec text or a reference implementation
//! in this session (no network access available). A wrong PI/bit here
//! can't cause a hang or crash -- decode failures fail closed to
//! `Decline`, and our own reply always advertises the same fixed,
//! self-consistent parameter set regardless of what was parsed -- but
//! could silently misnegotiate against a real v2.2 peer. Verify before
//! relying on this for interop.

use ax25::frame::{Address, Ax25Frame, CommandResponse, DisconnectedMode, FrameContent, RouteEntry};

use crate::arq::ArqConfig;
use crate::wire;

const FORMAT_IDENTIFIER: u8 = 0x82;
const GROUP_IDENTIFIER: u8 = 0x80;

const PI_CLASSES_OF_PROCEDURES: u8 = 2;
const PI_HDLC_OPTIONAL_FUNCTIONS: u8 = 3;
const PI_N1_BITS: u8 = 6; // I Field Length Receive, in bits
const PI_WINDOW: u8 = 8; // Window Size Receive (k)
const PI_T1_MS: u8 = 9; // Acknowledge Timer
const PI_N2: u8 = 10; // Retries

// Classes of Procedures, octet 1.
const COP_HALF_DUPLEX: u8 = 0b0000_0001;
const COP_FULL_DUPLEX: u8 = 0b0000_0010;

// HDLC Optional Functions, octet 1.
const HOF_REJ: u8 = 0b0000_0010;
const HOF_EXTENDED_ADDRESS: u8 = 0b0100_0000;
// HDLC Optional Functions, octet 2.
const HOF2_SREJ: u8 = 0b0000_0001;
const HOF2_MODULO_128: u8 = 0b0000_0010;

/// A decoded (or our own, via [`our_params`]) XID parameter set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XidParams {
    pub window: u8,
    pub t1_ms: u32,
    pub n2: u8,
    pub n1_bits: u16,
    /// True if the proposal requires anything beyond modulus-8 (REJ-only,
    /// half-duplex, basic addressing) support: modulus-128 window, SREJ,
    /// extended (2-byte) addressing, or full-duplex-only.
    pub requests_extended: bool,
}

/// Our own fixed, modulus-8-only advertised parameter set.
pub fn our_params(cfg: &ArqConfig) -> XidParams {
    XidParams {
        window: cfg.window.min(7),
        t1_ms: cfg.t1.as_millis().min(u128::from(u32::MAX)) as u32,
        n2: cfg.n2.min(u32::from(u8::MAX)) as u8,
        n1_bits: (cfg.n1_bytes.min(usize::from(u16::MAX / 8)) as u16).saturating_mul(8),
        requests_extended: false,
    }
}

/// Decode an XID frame's information field (everything after the control
/// byte -- no PID, unlike I/UI frames). Unknown/unrecognized PIs are
/// skipped (length-prefixed, so safe even without understanding them --
/// forward-compatible with a fuller real-world parameter list). Missing
/// numeric parameters fall back to this app's own defaults rather than
/// failing outright, since a peer is free to omit anything it doesn't
/// want to negotiate.
pub fn decode(payload: &[u8]) -> Result<XidParams, String> {
    if payload.len() < 4 {
        return Err("XID payload too short for FI/GI/GL".to_string());
    }
    if payload[0] != FORMAT_IDENTIFIER {
        return Err(format!("unexpected XID format identifier 0x{:02X}", payload[0]));
    }
    if payload[1] != GROUP_IDENTIFIER {
        return Err(format!("unexpected XID group identifier 0x{:02X}", payload[1]));
    }
    let group_len = u16::from_be_bytes([payload[2], payload[3]]) as usize;
    let params_start = 4;
    let params_end = (params_start + group_len).min(payload.len());
    let params = &payload[params_start..params_end];

    let mut window = None;
    let mut t1_ms = None;
    let mut n2 = None;
    let mut n1_bits = None;
    let mut requests_extended = false;

    let mut i = 0;
    while i + 2 <= params.len() {
        let pi = params[i];
        let pl = params[i + 1] as usize;
        let start = i + 2;
        let end = start + pl;
        if end > params.len() {
            break; // truncated parameter -- stop, keep whatever already parsed
        }
        let pv = &params[start..end];
        match pi {
            PI_CLASSES_OF_PROCEDURES if !pv.is_empty() => {
                if pv[0] & COP_FULL_DUPLEX != 0 && pv[0] & COP_HALF_DUPLEX == 0 {
                    requests_extended = true;
                }
            }
            PI_HDLC_OPTIONAL_FUNCTIONS if pv.len() >= 2 => {
                if pv[0] & HOF_EXTENDED_ADDRESS != 0 || pv[1] & (HOF2_MODULO_128 | HOF2_SREJ) != 0 {
                    requests_extended = true;
                }
            }
            PI_N1_BITS if pv.len() == 2 => n1_bits = Some(u16::from_be_bytes([pv[0], pv[1]])),
            PI_WINDOW if pv.len() == 1 => window = Some(pv[0]),
            PI_T1_MS if pv.len() == 2 => t1_ms = Some(u16::from_be_bytes([pv[0], pv[1]]) as u32),
            PI_N2 if pv.len() == 1 => n2 = Some(pv[0]),
            _ => {} // unrecognized or wrong-length-for-what-we-know PI: skip
        }
        i = end;
    }

    let window = window.unwrap_or(4);
    if window > 7 {
        requests_extended = true;
    }

    Ok(XidParams {
        window,
        t1_ms: t1_ms.unwrap_or(4000),
        n2: n2.unwrap_or(10),
        n1_bits: n1_bits.unwrap_or(2048),
        requests_extended,
    })
}

/// Encode our own parameter set as an XID information field (no PID --
/// unlike I/UI, XID has no upper-layer payload; the value that would be a
/// PID position is start of the FI/GI/GL envelope itself).
pub fn encode(params: &XidParams) -> Vec<u8> {
    let mut body = Vec::new();
    body.push(PI_CLASSES_OF_PROCEDURES);
    body.push(2);
    body.extend_from_slice(&[COP_HALF_DUPLEX, 0]);

    body.push(PI_HDLC_OPTIONAL_FUNCTIONS);
    body.push(3);
    body.extend_from_slice(&[HOF_REJ, 0, 0]); // REJ implemented; no SREJ/modulo-128/extended-address

    body.push(PI_N1_BITS);
    body.push(2);
    body.extend_from_slice(&params.n1_bits.to_be_bytes());

    body.push(PI_WINDOW);
    body.push(1);
    body.push(params.window.min(7));

    body.push(PI_T1_MS);
    body.push(2);
    body.extend_from_slice(&(params.t1_ms.min(u32::from(u16::MAX)) as u16).to_be_bytes());

    body.push(PI_N2);
    body.push(1);
    body.push(params.n2);

    let mut out = Vec::with_capacity(4 + body.len());
    out.push(FORMAT_IDENTIFIER);
    out.push(GROUP_IDENTIFIER);
    out.extend_from_slice(&(body.len() as u16).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

pub enum XidDecision {
    /// Full on-wire bytes (address fields + control byte + parameter list)
    /// ready to KISS-encode and transmit as-is.
    Reply(Vec<u8>),
    /// Peer requested something beyond modulus-8, or the payload didn't
    /// decode at all -- same graceful-decline path as an unwanted SABME.
    Decline(Ax25Frame),
}

pub fn handle_peer_xid(local: &Address, remote: &Address, route: &[RouteEntry], payload: &[u8], cfg: &ArqConfig) -> XidDecision {
    let reply_route = wire::reverse_route(route);
    match decode(payload) {
        Ok(peer) if !peer.requests_extended => XidDecision::Reply(build_response_frame(local, remote, &reply_route, cfg)),
        _ => XidDecision::Decline(wire::build_dm(local, remote, reply_route, true)),
    }
}

/// Builds a throwaway `Ax25Frame` purely to reuse the crate's own address/
/// route encoder (its content is discarded), then splices in the real XID
/// control byte + parameter payload, which the crate has no `FrameContent`
/// variant for at all.
fn build_response_frame(local: &Address, remote: &Address, route: &[RouteEntry], cfg: &ArqConfig) -> Vec<u8> {
    let scaffold = Ax25Frame {
        source: local.clone(),
        destination: remote.clone(),
        route: route.to_vec(),
        command_or_response: Some(CommandResponse::Response),
        content: FrameContent::DisconnectedMode(DisconnectedMode { final_bit: true }),
    };
    let scaffold_bytes = scaffold.to_bytes();
    let header = wire::parse_header(&scaffold_bytes).expect("scaffold frame is always well-formed");
    let mut out = scaffold_bytes[..header.control_offset].to_vec();
    out.push(wire::CTRL_XID | 0b0001_0000); // F=1: this is our reply
    out.extend_from_slice(&encode(&our_params(cfg)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> Address {
        s.parse().unwrap()
    }

    fn cfg() -> ArqConfig {
        ArqConfig::default()
    }

    #[test]
    fn our_params_round_trip_through_encode_decode() {
        let ours = our_params(&cfg());
        let bytes = encode(&ours);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.window, ours.window);
        assert_eq!(decoded.t1_ms, ours.t1_ms);
        assert_eq!(decoded.n2, ours.n2);
        assert_eq!(decoded.n1_bits, ours.n1_bits);
        assert!(!decoded.requests_extended);
    }

    #[test]
    fn decode_flags_full_duplex_only_as_extended() {
        let mut bytes = encode(&our_params(&cfg()));
        // Classes of Procedures is the first parameter emitted by encode():
        // FI,GI,GL(2) then PI=2,PL=2,PV(2) starting at offset 6.
        assert_eq!(bytes[4], PI_CLASSES_OF_PROCEDURES);
        bytes[6] = COP_FULL_DUPLEX; // full duplex bit set, half duplex bit clear
        let decoded = decode(&bytes).unwrap();
        assert!(decoded.requests_extended);
    }

    #[test]
    fn decode_flags_modulo_128_as_extended() {
        let mut bytes = encode(&our_params(&cfg()));
        // HDLC Optional Functions is the second parameter: offset 8 = PI,
        // 9 = PL, 10/11/12 = PV.
        assert_eq!(bytes[8], PI_HDLC_OPTIONAL_FUNCTIONS);
        bytes[11] |= HOF2_MODULO_128;
        let decoded = decode(&bytes).unwrap();
        assert!(decoded.requests_extended);
    }

    #[test]
    fn decode_flags_srej_as_extended() {
        let mut bytes = encode(&our_params(&cfg()));
        bytes[11] |= HOF2_SREJ;
        let decoded = decode(&bytes).unwrap();
        assert!(decoded.requests_extended);
    }

    #[test]
    fn decode_flags_oversized_window_as_extended() {
        let mut bytes = encode(&our_params(&cfg()));
        // Locate the window parameter by PI byte rather than a hardcoded
        // offset, since that's more robust to encode() ever reordering
        // fields; params start right after the 4-byte FI/GI/GL header.
        let mut i = 4;
        loop {
            let pi = bytes[i];
            let pl = bytes[i + 1] as usize;
            if pi == PI_WINDOW {
                bytes[i + 2] = 8; // invalid for modulus-8 (max 7)
                break;
            }
            i += 2 + pl;
        }
        let decoded = decode(&bytes).unwrap();
        assert!(decoded.requests_extended);
        assert_eq!(decoded.window, 8);
    }

    #[test]
    fn decode_skips_unknown_parameter_ids() {
        let mut bytes = encode(&our_params(&cfg()));
        // Insert an unknown PI=99 with a 1-byte value right after the FI/GI/GL
        // header, and bump GL accordingly.
        let extra = [99u8, 1, 0xAA];
        bytes.splice(4..4, extra);
        let new_group_len = (bytes.len() - 4) as u16;
        bytes[2..4].copy_from_slice(&new_group_len.to_be_bytes());
        let decoded = decode(&bytes).expect("unknown PI should not break decoding");
        assert!(!decoded.requests_extended);
        assert_eq!(decoded.window, our_params(&cfg()).window);
    }

    #[test]
    fn decode_rejects_bad_format_identifier() {
        let mut bytes = encode(&our_params(&cfg()));
        bytes[0] = 0x00;
        assert!(decode(&bytes).is_err());
    }

    #[test]
    fn decode_rejects_too_short_payload() {
        assert!(decode(&[0x82, 0x80]).is_err());
    }

    #[test]
    fn decode_tolerates_truncated_parameter_without_panicking() {
        let mut bytes = encode(&our_params(&cfg()));
        bytes.truncate(bytes.len() - 1); // chop the last byte off a multi-byte PV
        let decoded = decode(&bytes);
        assert!(decoded.is_ok());
    }

    #[test]
    fn handle_peer_xid_replies_to_a_compatible_proposal() {
        let local = addr("N0CALL-1");
        let remote = addr("KD3BFP-9");
        let payload = encode(&our_params(&cfg()));
        match handle_peer_xid(&local, &remote, &[], &payload, &cfg()) {
            XidDecision::Reply(bytes) => {
                let header = wire::parse_header(&bytes).unwrap();
                assert_eq!(header.source, local);
                assert_eq!(header.destination, remote);
                let control = wire::control_byte(&bytes, &header).unwrap();
                assert!(wire::is_xid(control));
                let decoded = decode(&bytes[header.control_offset + 1..]).unwrap();
                assert!(!decoded.requests_extended);
                assert_eq!(decoded.window, our_params(&cfg()).window);
            }
            XidDecision::Decline(_) => panic!("expected a Reply"),
        }
    }

    #[test]
    fn handle_peer_xid_declines_an_extended_mode_request() {
        let local = addr("N0CALL-1");
        let remote = addr("KD3BFP-9");
        let mut payload = encode(&our_params(&cfg()));
        payload[11] |= HOF2_MODULO_128;
        match handle_peer_xid(&local, &remote, &[], &payload, &cfg()) {
            XidDecision::Decline(dm) => {
                assert_eq!(dm.source, local);
                assert_eq!(dm.destination, remote);
                assert!(matches!(dm.content, FrameContent::DisconnectedMode(_)));
            }
            XidDecision::Reply(_) => panic!("expected a Decline"),
        }
    }

    #[test]
    fn handle_peer_xid_declines_undecodable_payload() {
        let local = addr("N0CALL-1");
        let remote = addr("KD3BFP-9");
        match handle_peer_xid(&local, &remote, &[], &[0x00, 0x00, 0x00, 0x00], &cfg()) {
            XidDecision::Decline(_) => {}
            XidDecision::Reply(_) => panic!("expected a Decline"),
        }
    }
}
