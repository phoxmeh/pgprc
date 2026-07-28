//! AGWPE ("AGW Packet Engine") TCP/IP API frame codec.
//!
//! Every frame is a 36-byte header optionally followed by a data payload.
//! Layout (all multi-byte integers little-endian):
//!
//! ```text
//! +00       Port      (1 byte)
//! +01..03   reserved  (3 bytes, zero)
//! +04       DataKind  (1 byte, ASCII command letter)
//! +05       reserved  (1 byte, zero)
//! +06       PID       (1 byte)
//! +07       reserved  (1 byte, zero)
//! +08..17   CallFrom  (10 bytes, null-padded ASCII)
//! +18..27   CallTo    (10 bytes, null-padded ASCII)
//! +28..31   DataLen   (4 bytes, u32 LE)
//! +32..35   reserved  (4 bytes, zero)
//! +36..     Data      (DataLen bytes)
//! ```

pub const HEADER_LEN: usize = 36;
const CALL_FIELD_LEN: usize = 10;

#[derive(Debug, Clone)]
pub struct AgwFrame {
    pub port: u8,
    pub data_kind: u8,
    pub pid: u8,
    pub call_from: String,
    pub call_to: String,
    pub data: Vec<u8>,
}

impl AgwFrame {
    pub fn new(port: u8, data_kind: char, call_from: &str, call_to: &str, data: Vec<u8>) -> Self {
        AgwFrame {
            port,
            data_kind: data_kind as u8,
            pid: 0xF0,
            call_from: call_from.to_string(),
            call_to: call_to.to_string(),
            data,
        }
    }

    pub fn kind(&self) -> char {
        self.data_kind as char
    }

    /// Build a 'P' (Application Login) frame: two 255-byte null-padded
    /// fields for username and password.
    pub fn login(username: &str, password: &str) -> Self {
        let mut data = vec![0u8; 510];
        write_padded(&mut data[0..255], username.as_bytes());
        write_padded(&mut data[255..510], password.as_bytes());
        AgwFrame::new(0, 'P', "", "", data)
    }

    /// Build a 'v' (Connect, Via Digipeater(s)) frame. `digis` is the ordered
    /// digipeater path.
    pub fn connect_via(port: u8, call_from: &str, call_to: &str, digis: &[String]) -> Self {
        AgwFrame::new(port, 'v', call_from, call_to, encode_digi_path(digis))
    }

    /// Build a 'V' (Send UNPROTO Information, Via Digipeater(s)) frame:
    /// same digipeater-path prefix as 'v', followed by the info payload.
    pub fn unproto_via(port: u8, call_from: &str, call_to: &str, digis: &[String], info: Vec<u8>) -> Self {
        let mut data = encode_digi_path(digis);
        data.extend_from_slice(&info);
        AgwFrame::new(port, 'V', call_from, call_to, data)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(HEADER_LEN + self.data.len());
        buf.push(self.port);
        buf.extend_from_slice(&[0, 0, 0]);
        buf.push(self.data_kind);
        buf.push(0);
        buf.push(self.pid);
        buf.push(0);
        let mut from_field = [0u8; CALL_FIELD_LEN];
        write_padded(&mut from_field, self.call_from.as_bytes());
        buf.extend_from_slice(&from_field);
        let mut to_field = [0u8; CALL_FIELD_LEN];
        write_padded(&mut to_field, self.call_to.as_bytes());
        buf.extend_from_slice(&to_field);
        buf.extend_from_slice(&(self.data.len() as u32).to_le_bytes());
        buf.extend_from_slice(&[0, 0, 0, 0]);
        buf.extend_from_slice(&self.data);
        buf
    }

    /// Try to decode a single frame from the front of `buf`. Returns the
    /// frame and the number of bytes it consumed, or `None` if `buf` doesn't
    /// yet contain a complete frame.
    pub fn decode(buf: &[u8]) -> Option<(AgwFrame, usize)> {
        if buf.len() < HEADER_LEN {
            return None;
        }
        let port = buf[0];
        let data_kind = buf[4];
        let pid = buf[6];
        let call_from = read_padded(&buf[8..18]);
        let call_to = read_padded(&buf[18..28]);
        let data_len = u32::from_le_bytes(buf[28..32].try_into().unwrap()) as usize;
        if buf.len() < HEADER_LEN + data_len {
            return None;
        }
        let data = buf[HEADER_LEN..HEADER_LEN + data_len].to_vec();
        Some((
            AgwFrame { port, data_kind, pid, call_from, call_to, data },
            HEADER_LEN + data_len,
        ))
    }
}

fn write_padded(field: &mut [u8], value: &[u8]) {
    let n = value.len().min(field.len().saturating_sub(1));
    field[..n].copy_from_slice(&value[..n]);
}

/// Encodes a digipeater path the way AGWPE's 'v'/'V' (Connect/Unproto, Via
/// Digipeater) frames expect it: a 1-byte count followed by each callsign in
/// a 10-byte null-padded field, matching the header's `CallFrom`/`CallTo`
/// field layout.
fn encode_digi_path(digis: &[String]) -> Vec<u8> {
    let mut data = vec![digis.len() as u8];
    for digi in digis {
        let mut field = [0u8; CALL_FIELD_LEN];
        write_padded(&mut field, digi.as_bytes());
        data.extend_from_slice(&field);
    }
    data
}

fn read_padded(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).to_string()
}

/// Incrementally accumulates bytes from a TCP stream and yields complete
/// frames as they become available.
#[derive(Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    pub fn feed(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn next_frame(&mut self) -> Option<AgwFrame> {
        let (frame, consumed) = AgwFrame::decode(&self.buf)?;
        self.buf.drain(..consumed);
        Some(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_connect_frame() {
        let frame = AgwFrame::new(0, 'C', "MYCALL-1", "N0CALL-2", b"hello".to_vec());
        let bytes = frame.encode();
        assert_eq!(bytes.len(), HEADER_LEN + 5);
        let (decoded, consumed) = AgwFrame::decode(&bytes).expect("decodes");
        assert_eq!(consumed, bytes.len());
        assert_eq!(decoded.port, 0);
        assert_eq!(decoded.kind(), 'C');
        assert_eq!(decoded.call_from, "MYCALL-1");
        assert_eq!(decoded.call_to, "N0CALL-2");
        assert_eq!(decoded.data, b"hello");
    }

    #[test]
    fn connect_via_encodes_digi_count_and_padded_calls() {
        let digis = vec!["WIDE1-1".to_string(), "WIDE2-1".to_string()];
        let frame = AgwFrame::connect_via(0, "MYCALL-1", "N0CALL-2", &digis);
        assert_eq!(frame.kind(), 'v');
        assert_eq!(frame.data[0], 2);
        assert_eq!(read_padded(&frame.data[1..11]), "WIDE1-1");
        assert_eq!(read_padded(&frame.data[11..21]), "WIDE2-1");
        assert_eq!(frame.data.len(), 21);
    }

    #[test]
    fn unproto_via_appends_info_after_digi_path() {
        let digis = vec!["WIDE1-1".to_string()];
        let frame = AgwFrame::unproto_via(0, "MYCALL-1", "BEACON", &digis, b"hello".to_vec());
        assert_eq!(frame.kind(), 'V');
        assert_eq!(frame.data[0], 1);
        assert_eq!(read_padded(&frame.data[1..11]), "WIDE1-1");
        assert_eq!(&frame.data[11..], b"hello");
    }

    #[test]
    fn login_frame_has_two_255_byte_fields() {
        let frame = AgwFrame::login("KI5ABC", "s3cr3t");
        assert_eq!(frame.data.len(), 510);
        assert!(frame.data[0..6].starts_with(b"KI5ABC"));
        assert!(frame.data[255..261].starts_with(b"s3cr3t"));
    }

    #[test]
    fn decoder_handles_partial_and_multiple_frames() {
        let f1 = AgwFrame::new(0, 'G', "", "", vec![]).encode();
        let f2 = AgwFrame::new(0, 'm', "", "", vec![1, 2, 3]).encode();
        let mut decoder = FrameDecoder::default();
        decoder.feed(&f1[..HEADER_LEN - 1]);
        assert!(decoder.next_frame().is_none());
        decoder.feed(&f1[HEADER_LEN - 1..]);
        decoder.feed(&f2);
        let a = decoder.next_frame().expect("first frame");
        assert_eq!(a.kind(), 'G');
        let b = decoder.next_frame().expect("second frame");
        assert_eq!(b.kind(), 'm');
        assert_eq!(b.data, vec![1, 2, 3]);
        assert!(decoder.next_frame().is_none());
    }
}
