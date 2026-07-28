//! Linux `AF_AX25` raw socket support.
//!
//! No maintained Rust crate wraps this kernel API, so we bind the small
//! amount of it we need directly via `libc`. Struct layouts below mirror
//! `/usr/include/linux/ax25.h` exactly.
//!
//! Using `SOCK_SEQPACKET` means the *kernel* AX.25 stack performs connected
//! mode ARQ (retransmission/acknowledgement) for us — we only need to
//! bind/connect/read/write, not reimplement the modulus-8 state machine.

use std::io;
use std::mem;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

pub const AX25_MAX_DIGIS: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum Ax25Error {
    #[error("invalid callsign '{0}': must be 1-6 alphanumeric characters")]
    InvalidCallsign(String),
    #[error("invalid SSID {0}: must be 0-15")]
    InvalidSsid(u32),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Matches kernel `ax25_address`: 6 callsign bytes + SSID byte, each shifted
/// left by one bit ("shifted ASCII"), the same encoding used in over-the-air
/// AX.25 address fields.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ax25Address {
    pub ax25_call: [u8; 7],
}

/// Matches kernel `struct sockaddr_ax25`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SockaddrAx25 {
    pub sax25_family: libc::sa_family_t,
    pub sax25_call: Ax25Address,
    pub sax25_ndigis: libc::c_int,
}

/// Matches kernel `struct full_sockaddr_ax25`, used when digipeaters are
/// specified.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FullSockaddrAx25 {
    pub fsa_ax25: SockaddrAx25,
    pub fsa_digipeater: [Ax25Address; AX25_MAX_DIGIS],
}

/// Encode a human callsign like `"N0CALL-5"` (or bare `"N0CALL"`, SSID 0)
/// into the kernel's shifted-ASCII `ax25_address` representation.
pub fn encode_callsign(input: &str) -> Result<Ax25Address, Ax25Error> {
    let (call, ssid) = match input.split_once('-') {
        Some((call, ssid_str)) => {
            let ssid: u32 = ssid_str
                .parse()
                .map_err(|_| Ax25Error::InvalidSsid(u32::MAX))?;
            (call, ssid)
        }
        None => (input, 0),
    };
    let call = call.to_uppercase();
    if call.is_empty() || call.len() > 6 || !call.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(Ax25Error::InvalidCallsign(input.to_string()));
    }
    if ssid > 15 {
        return Err(Ax25Error::InvalidSsid(ssid));
    }

    let mut bytes = [0u8; 7];
    let call_bytes = call.as_bytes();
    for (i, slot) in bytes[..6].iter_mut().enumerate() {
        let c = *call_bytes.get(i).unwrap_or(&b' ');
        *slot = c << 1;
    }
    bytes[6] = ((ssid as u8) << 1) | 0x60;
    Ok(Ax25Address { ax25_call: bytes })
}

/// The inverse of [`encode_callsign`], e.g. for logging/monitor display.
pub fn decode_callsign(addr: &Ax25Address) -> String {
    let mut call = String::new();
    for &b in &addr.ax25_call[..6] {
        let c = (b >> 1) as char;
        if c != ' ' {
            call.push(c);
        }
    }
    let ssid = (addr.ax25_call[6] >> 1) & 0x0F;
    if ssid > 0 {
        format!("{call}-{ssid}")
    } else {
        call
    }
}

/// A single `AF_AX25`/`SOCK_SEQPACKET` socket: one connected-mode session.
/// Open a fresh one per outgoing connection (analogous to opening a new TCP
/// socket per connection); the kernel routes by the bound local callsign.
pub struct RawAx25Socket {
    fd: OwnedFd,
}

impl RawAx25Socket {
    /// Create the socket and bind it to `local_call` (the callsign
    /// associated with the target device/port in `/etc/ax25/axports`).
    pub fn bind(local_call: &str) -> Result<Self, Ax25Error> {
        let raw = unsafe { libc::socket(libc::AF_AX25, libc::SOCK_SEQPACKET, 0) };
        if raw < 0 {
            return Err(Ax25Error::Io(io::Error::last_os_error()));
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let socket = RawAx25Socket { fd };

        let local = encode_callsign(local_call)?;
        let addr = SockaddrAx25 {
            sax25_family: libc::AF_AX25 as libc::sa_family_t,
            sax25_call: local,
            sax25_ndigis: 0,
        };
        let ret = unsafe {
            libc::bind(
                socket.fd.as_raw_fd(),
                &addr as *const SockaddrAx25 as *const libc::sockaddr,
                mem::size_of::<SockaddrAx25>() as libc::socklen_t,
            )
        };
        if ret < 0 {
            return Err(Ax25Error::Io(io::Error::last_os_error()));
        }
        Ok(socket)
    }

    /// Connect to `remote_call`, optionally via up to 8 digipeaters.
    pub fn connect(&self, remote_call: &str, digis: &[String]) -> Result<(), Ax25Error> {
        let remote = encode_callsign(remote_call)?;
        let mut addr = FullSockaddrAx25 {
            fsa_ax25: SockaddrAx25 {
                sax25_family: libc::AF_AX25 as libc::sa_family_t,
                sax25_call: remote,
                sax25_ndigis: digis.len() as libc::c_int,
            },
            fsa_digipeater: [Ax25Address { ax25_call: [0; 7] }; AX25_MAX_DIGIS],
        };
        for (i, digi) in digis.iter().enumerate().take(AX25_MAX_DIGIS) {
            addr.fsa_digipeater[i] = encode_callsign(digi)?;
        }
        let len = if digis.is_empty() {
            mem::size_of::<SockaddrAx25>()
        } else {
            mem::size_of::<FullSockaddrAx25>()
        };
        let ret = unsafe {
            libc::connect(
                self.fd.as_raw_fd(),
                &addr as *const FullSockaddrAx25 as *const libc::sockaddr,
                len as libc::socklen_t,
            )
        };
        if ret < 0 {
            return Err(Ax25Error::Io(io::Error::last_os_error()));
        }
        Ok(())
    }

    /// Mark this (bound) socket as a listener for incoming connections —
    /// used for the personal mailbox, so other stations can connect to us
    /// instead of only ever the other way around.
    pub fn listen(&self, backlog: i32) -> Result<(), Ax25Error> {
        let ret = unsafe { libc::listen(self.fd.as_raw_fd(), backlog) };
        if ret < 0 {
            return Err(Ax25Error::Io(io::Error::last_os_error()));
        }
        Ok(())
    }

    /// Block until a station connects to this listening socket, returning a
    /// fresh socket for that session plus the remote station's callsign.
    pub fn accept(&self) -> Result<(RawAx25Socket, String), Ax25Error> {
        let mut addr: FullSockaddrAx25 = unsafe { mem::zeroed() };
        let mut len = mem::size_of::<FullSockaddrAx25>() as libc::socklen_t;
        let raw = unsafe {
            libc::accept(self.fd.as_raw_fd(), &mut addr as *mut FullSockaddrAx25 as *mut libc::sockaddr, &mut len)
        };
        if raw < 0 {
            return Err(Ax25Error::Io(io::Error::last_os_error()));
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let remote = decode_callsign(&addr.fsa_ax25.sax25_call);
        Ok((RawAx25Socket { fd }, remote))
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let n = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        let n = unsafe { libc::write(self.fd.as_raw_fd(), buf.as_ptr().cast(), buf.len()) };
        if n < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }

    pub fn try_clone(&self) -> io::Result<RawAx25Socket> {
        let new_fd = unsafe { libc::dup(self.fd.as_raw_fd()) };
        if new_fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(RawAx25Socket {
            fd: unsafe { OwnedFd::from_raw_fd(new_fd) },
        })
    }

    pub fn shutdown(&self) {
        unsafe {
            libc::shutdown(self.fd.as_raw_fd(), libc::SHUT_RDWR);
        }
    }
}

impl AsRawFd for RawAx25Socket {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

/// One entry from `/etc/ax25/axports`: `name callsign speed paclen window description`.
#[derive(Debug, Clone)]
pub struct AxPort {
    pub device: String,
    pub callsign: String,
    pub description: String,
}

/// Parse an axports file (defaults to `/etc/ax25/axports`), skipping blank
/// lines and `#`-comments.
pub fn parse_axports(text: &str) -> Vec<AxPort> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let device = fields.next()?.to_string();
            let callsign = fields.next()?.to_string();
            let _speed = fields.next();
            let _paclen = fields.next();
            let _window = fields.next();
            let description = fields.collect::<Vec<_>>().join(" ");
            Some(AxPort { device, callsign, description })
        })
        .collect()
}

pub fn read_axports() -> io::Result<Vec<AxPort>> {
    let text = std::fs::read_to_string("/etc/ax25/axports")?;
    Ok(parse_axports(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_sizes_match_kernel_layout() {
        // ax25_address: 7 bytes, no padding expected.
        assert_eq!(mem::size_of::<Ax25Address>(), 7);
        // sockaddr_ax25: sa_family_t (2) + ax25_address (7) + padding to
        // align the following c_int (4) + c_int (4) = 16 bytes total.
        assert_eq!(mem::size_of::<SockaddrAx25>(), 16);
        assert_eq!(
            mem::size_of::<FullSockaddrAx25>(),
            mem::size_of::<SockaddrAx25>() + AX25_MAX_DIGIS * mem::size_of::<Ax25Address>()
        );
    }

    #[test]
    fn encode_decode_round_trip_with_ssid() {
        let addr = encode_callsign("N0CALL-5").unwrap();
        assert_eq!(decode_callsign(&addr), "N0CALL-5");
    }

    #[test]
    fn encode_decode_round_trip_without_ssid() {
        let addr = encode_callsign("KD3BFP").unwrap();
        assert_eq!(decode_callsign(&addr), "KD3BFP");
    }

    #[test]
    fn shifted_ascii_matches_known_encoding() {
        // 'N' = 0x4E, shifted left = 0x9C.
        let addr = encode_callsign("N").unwrap();
        assert_eq!(addr.ax25_call[0], 0x9C);
        // Trailing unused callsign bytes are shifted spaces (0x20 << 1 = 0x40).
        assert_eq!(addr.ax25_call[1], 0x40);
        // SSID 0 -> reserved bits only: 0b0110_0000.
        assert_eq!(addr.ax25_call[6], 0x60);
    }

    #[test]
    fn rejects_bad_callsigns() {
        assert!(encode_callsign("").is_err());
        assert!(encode_callsign("TOOLONGCALL").is_err());
        assert!(encode_callsign("N0CALL-16").is_err());
    }

    #[test]
    fn parses_axports_format() {
        let sample = "\
# comment
wl2k\tKD3BFP-9\t19200\t255\t7\tMobilinkd
#1\tOH2BNS-1\t1200\t255\t2\t144.675 MHz (1200  bps)
";
        let ports = parse_axports(sample);
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].device, "wl2k");
        assert_eq!(ports[0].callsign, "KD3BFP-9");
        assert_eq!(ports[0].description, "Mobilinkd");
    }
}
