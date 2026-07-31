//! Decoder for NET/ROM "NODES" routing broadcasts: periodic AX.25 UI-frames
//! (PID `0xCF`, destination callsign `NODES`) a NET/ROM node sends to
//! advertise the other nodes it knows about.
//!
//! Byte layout verified against a primary source (F6FBB/AA4RE's *NET/ROM
//! Protocol* reference, "Automatic Routing Table Updates" section):
//!
//! ```text
//! +0       Signature: 0xFF
//! +1..7    Mnemonic alias of the sending node (6 bytes, space-padded ASCII)
//! +7..     Repeated per destination (up to 11 per frame), 21 bytes each:
//!            +0..7   Callsign of destination node (7-byte AX.25-shifted address field)
//!            +7..13  Mnemonic alias of destination node (6 bytes, space-padded ASCII)
//!            +13..20 Callsign of best-quality neighbor (7-byte AX.25-shifted address field)
//!            +20     Best-quality value (1 byte, unused here)
//! ```
//!
//! We only need `(destination callsign, destination alias)` pairs — the
//! neighbor/quality fields are for routing-table maintenance, not relevant
//! to an address book.

use crate::wire;

pub const NODES_SIGNATURE: u8 = 0xFF;

const HEADER_LEN: usize = 7; // signature (1) + sender alias (6)
const ENTRY_LEN: usize = 21; // dest call (7) + dest alias (6) + neighbor call (7) + quality (1)

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetRomNodeEntry {
    pub alias: String,
    pub callsign: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetRomNodesBroadcast {
    pub sender_alias: String,
    pub entries: Vec<NetRomNodeEntry>,
}

/// Parse a NODES broadcast info field. `None` if the signature byte is
/// missing/wrong or the payload is too short to even carry a sender alias.
/// An individual destination entry with an undecodable callsign field is
/// skipped rather than aborting the whole broadcast; a truncated trailing
/// entry is simply not included.
pub fn parse_nodes_broadcast(payload: &[u8]) -> Option<NetRomNodesBroadcast> {
    if payload.first() != Some(&NODES_SIGNATURE) || payload.len() < HEADER_LEN {
        return None;
    }
    let sender_alias = decode_mnemonic(&payload[1..HEADER_LEN]);

    let mut entries = Vec::new();
    let mut offset = HEADER_LEN;
    while offset + ENTRY_LEN <= payload.len() {
        let chunk = &payload[offset..offset + ENTRY_LEN];
        offset += ENTRY_LEN;
        let Some(callsign) = wire::decode_call_field(&chunk[0..7]) else { continue };
        let alias = decode_mnemonic(&chunk[7..13]);
        entries.push(NetRomNodeEntry { alias, callsign: callsign.to_string() });
    }

    Some(NetRomNodesBroadcast { sender_alias, entries })
}

fn decode_mnemonic(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_call(callsign: &str, ssid: u8) -> [u8; 7] {
        let addr = ax25::frame::Address::from_parts(callsign.to_string(), ssid).expect("valid address");
        wire::encode_address(&addr, false, false)
    }

    fn encode_mnemonic(text: &str) -> [u8; 6] {
        let mut out = [b' '; 6];
        for (i, b) in text.as_bytes().iter().take(6).enumerate() {
            out[i] = *b;
        }
        out
    }

    fn push_entry(payload: &mut Vec<u8>, dest_call: &str, dest_ssid: u8, dest_alias: &str, neighbor_call: &str, quality: u8) {
        payload.extend_from_slice(&encode_call(dest_call, dest_ssid));
        payload.extend_from_slice(&encode_mnemonic(dest_alias));
        payload.extend_from_slice(&encode_call(neighbor_call, 0));
        payload.push(quality);
    }

    #[test]
    fn decodes_a_single_entry_broadcast() {
        let mut payload = vec![NODES_SIGNATURE];
        payload.extend_from_slice(&encode_mnemonic("MYNODE"));
        push_entry(&mut payload, "N0CALL", 5, "REMOTE", "N1CALL", 200);

        let broadcast = parse_nodes_broadcast(&payload).expect("decodes");
        assert_eq!(broadcast.sender_alias, "MYNODE");
        assert_eq!(broadcast.entries, vec![NetRomNodeEntry { alias: "REMOTE".to_string(), callsign: "N0CALL-5".to_string() }]);
    }

    #[test]
    fn decodes_multiple_entries_in_order() {
        let mut payload = vec![NODES_SIGNATURE];
        payload.extend_from_slice(&encode_mnemonic("HUB"));
        push_entry(&mut payload, "AAAAAA", 0, "ALPHA", "N1CALL", 100);
        push_entry(&mut payload, "BBBBBB", 1, "BRAVO", "N1CALL", 150);
        push_entry(&mut payload, "CCCCCC", 2, "", "N1CALL", 255);

        let broadcast = parse_nodes_broadcast(&payload).expect("decodes");
        assert_eq!(broadcast.entries.len(), 3);
        assert_eq!(broadcast.entries[0].callsign, "AAAAAA");
        assert_eq!(broadcast.entries[1].callsign, "BBBBBB-1");
        assert_eq!(broadcast.entries[2].callsign, "CCCCCC-2");
        assert_eq!(broadcast.entries[2].alias, "");
    }

    #[test]
    fn rejects_wrong_signature_byte() {
        let mut payload = vec![0x00];
        payload.extend_from_slice(&encode_mnemonic("MYNODE"));
        push_entry(&mut payload, "N0CALL", 0, "REMOTE", "N1CALL", 200);
        assert!(parse_nodes_broadcast(&payload).is_none());
    }

    #[test]
    fn empty_payload_does_not_panic() {
        assert!(parse_nodes_broadcast(&[]).is_none());
    }

    #[test]
    fn header_only_payload_yields_no_entries() {
        let mut payload = vec![NODES_SIGNATURE];
        payload.extend_from_slice(&encode_mnemonic("LONELY"));
        let broadcast = parse_nodes_broadcast(&payload).expect("decodes");
        assert_eq!(broadcast.sender_alias, "LONELY");
        assert!(broadcast.entries.is_empty());
    }

    #[test]
    fn truncated_trailing_entry_is_simply_omitted() {
        let mut payload = vec![NODES_SIGNATURE];
        payload.extend_from_slice(&encode_mnemonic("MYNODE"));
        push_entry(&mut payload, "N0CALL", 0, "REMOTE", "N1CALL", 200);
        payload.extend_from_slice(&[0xAA; 10]); // partial second entry
        let broadcast = parse_nodes_broadcast(&payload).expect("decodes");
        assert_eq!(broadcast.entries.len(), 1);
    }

    #[test]
    fn entry_with_undecodable_callsign_is_skipped_but_later_entries_still_decode() {
        let mut payload = vec![NODES_SIGNATURE];
        payload.extend_from_slice(&encode_mnemonic("MYNODE"));
        // A callsign field that decodes to non-alphanumeric characters fails
        // `Address::from_parts` and should be skipped, not abort the parse.
        let mut bad_entry = Vec::new();
        bad_entry.extend_from_slice(&[0x00u8; 7]); // all-zero -> not valid callsign chars
        bad_entry.extend_from_slice(&encode_mnemonic("BAD"));
        bad_entry.extend_from_slice(&encode_call("N1CALL", 0));
        bad_entry.push(0);
        payload.extend_from_slice(&bad_entry);
        push_entry(&mut payload, "GOODCL", 3, "GOOD", "N1CALL", 100);

        let broadcast = parse_nodes_broadcast(&payload).expect("decodes");
        assert_eq!(broadcast.entries, vec![NetRomNodeEntry { alias: "GOOD".to_string(), callsign: "GOODCL-3".to_string() }]);
    }
}
