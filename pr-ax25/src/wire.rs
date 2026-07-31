//! Hand-rolled AX.25 address-field and control-byte parsing for frame types
//! the `ax25` crate (v0.4.0) cannot decode at all: SABME (extended-mode
//! connect) and XID (parameter negotiation). Both control bytes make
//! `ax25::frame::Ax25Frame::from_bytes` fail outright
//! (`FrameParseError::UnrecognisedUFieldType`), with no address info
//! recovered through the crate's public API. This module duplicates the
//! small amount of address-field-walking logic the crate keeps private, so
//! these two frame types can still be addressed and declined gracefully
//! (a DM reply) instead of silently dropped -- required since this app
//! only ever supports modulus-8 connected mode (see `arq`/`xid`).

use ax25::frame::{Address, Ax25Frame, CommandResponse, DisconnectedMode, FrameContent, RouteEntry};

/// SABME's control byte with the P/F bit masked out (`c & 0xEF`).
pub const CTRL_SABME: u8 = 0b0110_1111;
/// XID's control byte with the P/F bit masked out (`c & 0xEF`).
pub const CTRL_XID: u8 = 0b1010_1111;

pub fn is_sabme(control: u8) -> bool {
    control & 0xEF == CTRL_SABME
}

pub fn is_xid(control: u8) -> bool {
    control & 0xEF == CTRL_XID
}

/// Just enough of a decoded frame header to identify who it's from/to and
/// where the control byte sits, recovered independent of whether the
/// trailing content is something the `ax25` crate can parse.
pub struct RawHeader {
    pub destination: Address,
    pub source: Address,
    pub route: Vec<RouteEntry>,
    /// `Some(Command)`/`Some(Response)` from the dest/src high bits, same
    /// derivation `Ax25Frame::from_bytes` uses.
    pub command_or_response: Option<CommandResponse>,
    /// Byte offset of the control field within the original slice.
    pub control_offset: usize,
}

/// Mirrors `Ax25Frame::from_bytes`'s address-field walk (private upstream),
/// without requiring the trailing content to parse. `None` on the same
/// malformed-address conditions `from_bytes` itself would reject.
pub fn parse_header(bytes: &[u8]) -> Option<RawHeader> {
    let addr_start = bytes.iter().position(|&c| c != 0)?;
    let addr_end = bytes.iter().position(|&c| c & 0x01 == 0x01)?;
    let control_offset = addr_end + 1;
    if addr_end - addr_start + 1 < 14 || control_offset >= bytes.len() {
        return None;
    }

    let (destination, dest_high) = decode_address_field(&bytes[addr_start..addr_start + 7])?;
    let (source, src_high) = decode_address_field(&bytes[addr_start + 7..addr_start + 14])?;

    let rpt_count = (addr_end + 1 - addr_start - 14) / 7;
    let mut route = Vec::with_capacity(rpt_count);
    for i in 0..rpt_count {
        let start = addr_start + 14 + i * 7;
        let (repeater, has_repeated) = decode_address_field(&bytes[start..start + 7])?;
        route.push(RouteEntry { repeater, has_repeated });
    }

    let command_or_response = match (dest_high, src_high) {
        (true, false) => Some(CommandResponse::Command),
        (false, true) => Some(CommandResponse::Response),
        _ => None,
    };

    Some(RawHeader { destination, source, route, command_or_response, control_offset })
}

pub fn control_byte(bytes: &[u8], header: &RawHeader) -> Option<u8> {
    bytes.get(header.control_offset).copied()
}

/// Decode one 7-byte AX.25 address field (6 shifted-ASCII callsign bytes +
/// SSID byte). Returns the address and the field's high bit -- meaning is
/// context-dependent (command/response for dest/source, has-repeated for a
/// digipeater entry), matching the crate's own dual-purpose `high_bit`.
fn decode_address_field(bytes: &[u8]) -> Option<(Address, bool)> {
    if bytes.len() != 7 {
        return None;
    }
    let mut callsign: Vec<u8> = bytes[..6].iter().rev().map(|&c| c >> 1).skip_while(|&c| c == b' ').collect();
    callsign.reverse();
    let callsign = String::from_utf8(callsign).ok()?;
    let ssid = (bytes[6] >> 1) & 0x0F;
    let address = Address::from_parts(callsign, ssid).ok()?;
    let high_bit = bytes[6] & 0b1000_0000 > 0;
    Some((address, high_bit))
}

/// Decode one 7-byte AX.25-shifted callsign field on its own, discarding the
/// high bit (meaningless outside a real address field's dest/source/repeater
/// context) -- needed by `netrom` to recover the callsign fields inside a
/// NET/ROM NODES broadcast, which reuses this same 7-byte encoding for
/// destination/neighbor callsigns even though it isn't a standard AX.25
/// address field.
pub fn decode_call_field(bytes: &[u8]) -> Option<Address> {
    decode_address_field(bytes).map(|(addr, _)| addr)
}

/// Encode one 7-byte AX.25 address field -- the inverse of
/// `decode_address_field`, needed to hand-build XID frames (whose content
/// the `ax25` crate can't encode either, so the whole frame is built raw).
pub fn encode_address(addr: &Address, high_bit: bool, final_in_address: bool) -> [u8; 7] {
    let mut out = [0u8; 7];
    let call = addr.callsign().as_bytes();
    for (i, slot) in out[..6].iter_mut().enumerate() {
        let c = call.get(i).copied().unwrap_or(b' ');
        *slot = c << 1;
    }
    let high = if high_bit { 0b1000_0000 } else { 0 };
    let low = if final_in_address { 0b0000_0001 } else { 0 };
    out[6] = (addr.ssid() << 1) | 0b0110_0000 | high | low;
    out
}

/// A received frame's digipeater path, reversed and with every
/// `has_repeated` bit cleared -- the correct outgoing route for a reply
/// sent back the way the request came (each digipeater is a point-to-point
/// relay, so returning via the same chain means walking it backwards).
pub fn reverse_route(route: &[RouteEntry]) -> Vec<RouteEntry> {
    route.iter().rev().map(|e| RouteEntry { repeater: e.repeater.clone(), has_repeated: false }).collect()
}

/// Build a DM response frame -- fully within what the `ax25` crate's own
/// encoder supports, used to decline both an unwanted SABME and an
/// unroutable/unrecognized DISC.
pub fn build_dm(local: &Address, remote: &Address, route: Vec<RouteEntry>, final_bit: bool) -> Ax25Frame {
    Ax25Frame {
        source: local.clone(),
        destination: remote.clone(),
        route,
        command_or_response: Some(CommandResponse::Response),
        content: FrameContent::DisconnectedMode(DisconnectedMode { final_bit }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ax25::frame::SetAsynchronousBalancedMode;

    fn addr(s: &str) -> Address {
        s.parse().unwrap()
    }

    #[test]
    fn address_round_trips() {
        let a = addr("KD3BFP-9");
        let bytes = encode_address(&a, true, false);
        let (decoded, high) = decode_address_field(&bytes).unwrap();
        assert_eq!(decoded, a);
        assert!(high);
    }

    #[test]
    fn address_round_trips_no_ssid() {
        let a = addr("N0CALL");
        let bytes = encode_address(&a, false, true);
        let (decoded, high) = decode_address_field(&bytes).unwrap();
        assert_eq!(decoded, a);
        assert!(!high);
    }

    #[test]
    fn parse_header_matches_crate_addressing_on_a_frame_the_crate_can_decode() {
        let frame = Ax25Frame {
            source: addr("KD3BFP-9"),
            destination: addr("N0CALL-1"),
            route: vec![],
            command_or_response: Some(CommandResponse::Command),
            content: FrameContent::SetAsynchronousBalancedMode(SetAsynchronousBalancedMode { poll: true }),
        };
        let bytes = frame.to_bytes();
        let header = parse_header(&bytes).expect("header parses");
        assert_eq!(header.source, addr("KD3BFP-9"));
        assert_eq!(header.destination, addr("N0CALL-1"));
        assert!(header.route.is_empty());
        assert_eq!(header.command_or_response, Some(CommandResponse::Command));

        let control = control_byte(&bytes, &header).unwrap();
        assert_eq!(control & 0xEF, 0b0010_1111); // SABM, which the crate CAN decode
        assert!(!is_sabme(control));
        assert!(!is_xid(control));

        // Cross-check against the crate's own parse for the same bytes.
        let via_crate = Ax25Frame::from_bytes(&bytes).unwrap();
        assert_eq!(via_crate.source, header.source);
        assert_eq!(via_crate.destination, header.destination);
    }

    #[test]
    fn parse_header_recovers_addressing_for_sabme_which_the_crate_cannot_decode_at_all() {
        // Build valid SABM bytes via the crate's own encoder, then splice in
        // the SABME control byte -- addressing is independent of content
        // type, so this yields genuine "SABME frame" bytes for test purposes.
        let frame = Ax25Frame {
            source: addr("KD3BFP-9"),
            destination: addr("N0CALL-1"),
            route: vec![],
            command_or_response: Some(CommandResponse::Command),
            content: FrameContent::SetAsynchronousBalancedMode(SetAsynchronousBalancedMode { poll: false }),
        };
        let mut bytes = frame.to_bytes();
        let header = parse_header(&bytes).unwrap();
        bytes[header.control_offset] = CTRL_SABME;

        // The crate itself genuinely can't parse this at all.
        assert!(Ax25Frame::from_bytes(&bytes).is_err());

        // But wire::parse_header still recovers full addressing.
        let header = parse_header(&bytes).expect("header still parses");
        assert_eq!(header.source, addr("KD3BFP-9"));
        assert_eq!(header.destination, addr("N0CALL-1"));
        let control = control_byte(&bytes, &header).unwrap();
        assert!(is_sabme(control));
        assert!(!is_xid(control));
    }

    #[test]
    fn parse_header_recovers_addressing_for_xid_which_the_crate_cannot_decode_at_all() {
        let frame = Ax25Frame {
            source: addr("KD3BFP-9"),
            destination: addr("N0CALL-1"),
            route: vec![],
            command_or_response: Some(CommandResponse::Command),
            content: FrameContent::SetAsynchronousBalancedMode(SetAsynchronousBalancedMode { poll: false }),
        };
        let mut bytes = frame.to_bytes();
        let header = parse_header(&bytes).unwrap();
        bytes[header.control_offset] = CTRL_XID;

        assert!(Ax25Frame::from_bytes(&bytes).is_err());
        let header = parse_header(&bytes).expect("header still parses");
        let control = control_byte(&bytes, &header).unwrap();
        assert!(is_xid(control));
        assert!(!is_sabme(control));
    }

    #[test]
    fn parse_header_recovers_digipeater_route() {
        let frame = Ax25Frame {
            source: addr("KD3BFP-9"),
            destination: addr("N0CALL-1"),
            route: vec![
                RouteEntry { repeater: addr("WIDE1-1"), has_repeated: true },
                RouteEntry { repeater: addr("WIDE2-1"), has_repeated: false },
            ],
            command_or_response: Some(CommandResponse::Command),
            content: FrameContent::SetAsynchronousBalancedMode(SetAsynchronousBalancedMode { poll: false }),
        };
        let bytes = frame.to_bytes();
        let header = parse_header(&bytes).unwrap();
        assert_eq!(header.route.len(), 2);
        assert_eq!(header.route[0].repeater, addr("WIDE1-1"));
        assert!(header.route[0].has_repeated);
        assert_eq!(header.route[1].repeater, addr("WIDE2-1"));
        assert!(!header.route[1].has_repeated);
    }

    #[test]
    fn reverse_route_flips_order_and_clears_has_repeated() {
        let route = vec![
            RouteEntry { repeater: addr("WIDE1-1"), has_repeated: true },
            RouteEntry { repeater: addr("WIDE2-1"), has_repeated: false },
        ];
        let reversed = reverse_route(&route);
        assert_eq!(reversed.len(), 2);
        assert_eq!(reversed[0].repeater, addr("WIDE2-1"));
        assert!(!reversed[0].has_repeated);
        assert_eq!(reversed[1].repeater, addr("WIDE1-1"));
        assert!(!reversed[1].has_repeated);
    }

    #[test]
    fn build_dm_matches_the_crates_own_encoder() {
        let local = addr("N0CALL-1");
        let remote = addr("KD3BFP-9");
        let dm = build_dm(&local, &remote, vec![], true);
        let expected = Ax25Frame {
            source: local,
            destination: remote,
            route: vec![],
            command_or_response: Some(CommandResponse::Response),
            content: FrameContent::DisconnectedMode(DisconnectedMode { final_bit: true }),
        };
        assert_eq!(dm.to_bytes(), expected.to_bytes());
    }
}
