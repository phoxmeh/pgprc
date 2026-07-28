pub mod kiss;
pub mod kiss_runner;
pub mod raw_socket;
pub mod runner;

pub use kiss_runner::{KissRunner, KissTransport};
pub use runner::Ax25RawSocketRunner;

/// A human-readable label for a standard AX.25 PID (Protocol Identifier)
/// byte, for Monitor display — pure decode/labeling, not routing. `None` for
/// 0xF0 ("No layer 3 protocol implemented", i.e. our own plain-text
/// traffic) so ordinary sessions don't get a label at all.
pub fn pid_label(pid: u8) -> Option<&'static str> {
    match pid {
        0xF0 => None,
        0x01 => Some("X.25 PLP"),
        0x06 => Some("Compressed TCP/IP"),
        0x07 => Some("Uncompressed TCP/IP"),
        0x08 => Some("Segmentation Fragment"),
        0xC3 => Some("TEXNET"),
        0xC4 => Some("Link Quality"),
        0xCA => Some("Appletalk"),
        0xCB => Some("Appletalk ARP"),
        0xCC => Some("ARPA IP"),
        0xCD => Some("ARPA ARP"),
        0xCE => Some("FlexNet"),
        0xCF => Some("NET/ROM"),
        0xFF => Some("Escape"),
        pid if pid & 0b0011_0000 == 0b0001_0000 || pid & 0b0011_0000 == 0b0010_0000 => Some("Layer 3"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_known_pids() {
        assert_eq!(pid_label(0xCF), Some("NET/ROM"));
        assert_eq!(pid_label(0xCC), Some("ARPA IP"));
    }

    #[test]
    fn no_layer3_and_unknown_pids_are_unlabeled() {
        assert_eq!(pid_label(0xF0), None);
        assert_eq!(pid_label(0x42), None);
    }
}
