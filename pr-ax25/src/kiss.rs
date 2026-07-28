//! KISS framing codec (encode + streaming decode).
//!
//! Frame format: `FEND, command, <escaped payload bytes>, FEND`. The command
//! byte's high nibble is the KISS port (0-15) and low nibble is the frame
//! type (0 = data, which is all we send/expect here). `FEND`/`FESC` bytes
//! inside the payload are escaped as `FESC TFEND`/`FESC TFESC`.

pub const FEND: u8 = 0xC0;
pub const FESC: u8 = 0xDB;
pub const TFEND: u8 = 0xDC;
pub const TFESC: u8 = 0xDD;

/// Encode a single data frame for the given KISS port (0-15).
pub fn encode_data_frame(kiss_port: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 8);
    out.push(FEND);
    out.push((kiss_port & 0x0F) << 4); // low nibble 0 = data frame
    for &b in payload {
        match b {
            FEND => {
                out.push(FESC);
                out.push(TFEND);
            }
            FESC => {
                out.push(FESC);
                out.push(TFESC);
            }
            _ => out.push(b),
        }
    }
    out.push(FEND);
    out
}

/// Incrementally accumulates raw bytes from a KISS TNC and yields decoded
/// `(command_byte, payload)` frames as they complete.
#[derive(Default)]
pub struct KissDecoder {
    buf: Vec<u8>,
    in_frame: bool,
    escaped: bool,
}

impl KissDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<(u8, Vec<u8>)> {
        let mut frames = Vec::new();
        for &b in bytes {
            match b {
                FEND => {
                    if self.in_frame && !self.buf.is_empty() {
                        let cmd = self.buf[0];
                        let payload = self.buf[1..].to_vec();
                        frames.push((cmd, payload));
                    }
                    self.buf.clear();
                    self.in_frame = true;
                    self.escaped = false;
                }
                FESC if self.in_frame => self.escaped = true,
                TFEND if self.in_frame && self.escaped => {
                    self.buf.push(FEND);
                    self.escaped = false;
                }
                TFESC if self.in_frame && self.escaped => {
                    self.buf.push(FESC);
                    self.escaped = false;
                }
                _ if self.in_frame => {
                    self.buf.push(b);
                    self.escaped = false;
                }
                _ => {} // bytes outside a frame are ignored
            }
        }
        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_data_frame() {
        let encoded = encode_data_frame(0, b"hello");
        assert_eq!(encoded, [&[FEND, 0x00][..], b"hello", &[FEND]].concat());

        let mut decoder = KissDecoder::new();
        let frames = decoder.feed(&encoded);
        assert_eq!(frames, vec![(0x00, b"hello".to_vec())]);
    }

    #[test]
    fn escapes_fend_and_fesc_bytes_in_payload() {
        let payload = vec![0x01, FEND, 0x02, FESC, 0x03];
        let encoded = encode_data_frame(0, &payload);

        let mut decoder = KissDecoder::new();
        let frames = decoder.feed(&encoded);
        assert_eq!(frames, vec![(0x00, payload)]);
    }

    #[test]
    fn ignores_repeated_fend_separators() {
        let mut decoder = KissDecoder::new();
        let frames = decoder.feed(&[FEND, FEND, FEND, 0x00, b'h', b'i', FEND]);
        assert_eq!(frames, vec![(0x00, b"hi".to_vec())]);
    }

    #[test]
    fn handles_split_reads() {
        let encoded = encode_data_frame(0, b"split");
        let mut decoder = KissDecoder::new();
        let mut frames = decoder.feed(&encoded[..3]);
        assert!(frames.is_empty());
        frames = decoder.feed(&encoded[3..]);
        assert_eq!(frames, vec![(0x00, b"split".to_vec())]);
    }
}
