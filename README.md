# Packet Radio

A Linux-native packet radio client built with Rust and GTK4/libadwaita,
supporting AGWPE, AX.25 (raw kernel sockets), and bare KISS TNCs.

> **Status:** early (`0.1.0`), no published releases yet. Built and tested
> against [Direwolf](https://github.com/wb2osz/direwolf) on a dummy audio
> load. Treat it as a working prototype, not a finished product.

## Features

- **Multiple port types**, each independently configurable:
  - **Telnet** and **SSH** — plain terminal sessions (e.g. a DX cluster or a
    remote shell), no packet-radio concepts involved.
  - **AGWPE** — connects to an AGWPE-speaking TNC host (Direwolf, UZ7HO
    SoundModem, etc.) over TCP. Supports connected-mode sessions and
    unproto/UI frames, with an optional digipeater `via` path.
  - **AX.25 raw socket** (`AF_AX25`) — talks directly to a kernel AX.25
    interface (`kissattach`, etc.). Connected-mode only.
  - **KISS (TCP or serial)** — talks directly to a bare KISS TNC.
    Unproto/UI traffic only — a bare KISS TNC has no connected-mode ARQ
    state machine of its own, and this app doesn't implement one.
- **Session tabs**: pick a port and (for node-capable ports) a destination
  callsign, connect explicitly, and send/receive text. Tabs persist across
  disconnects so you can reconnect or repoint them at a different node.
  Each tab can also run in **Unproto mode** to send one-shot unconnected
  frames instead of opening a session, with its own digipeater `via` field.
- **Per-node history**: every (port, node, mode) conversation gets its own
  persisted scrollback, capped to a configurable line count, shown as a
  read-only preview whenever that tab isn't currently connected.
- **Monitor view**: a live log of all port/frame activity across every
  connected port, with a substring filter and "Save Monitor Log..." export.
- **Configurable highlighting**: callsigns, known (address-book) callsigns,
  AX.25 frame-type tags, and user-defined keyword/bulletin rules (regex or
  plain substring) are colored in both the Monitor and session scrollback.
- **Address Book**: tracks "last heard" automatically as callsigns show up
  on the air, plus manually-entered name/alias/location/notes. Includes an
  online **QRZ.com lookup** and **ADIF export** of logged QSOs.
- **Scheduled beacons**: configure one or more periodic unproto beacons
  (port, destination, via path, message, interval) that fire automatically
  while their port is connected.
- **Personal packet mailbox** (off by default): a minimal BBS-style
  auto-responder (`L`ist / `R`ead / `S`end / `B`ye) for unsolicited incoming
  connections — a local message store, not real Winlink/RMS interop.
- **Per-tab TX/RX byte counters**, **KISS TNC parameter** configuration
  (TXDELAY/persistence/slot-time/full-duplex), and **PID labeling** of
  monitored frames (NET/ROM, ARPA IP, etc.).
- **Pinning**: pin a tab to have its shell (port + node prefilled,
  disconnected) recreated automatically the next time the app starts. It
  never auto-connects — you still press Connect.

## Not supported (by design, for now)

- **APRS.**
- **Digipeating** (auto-relaying other stations' frames) — out of scope for
  a client application.
- **Connected-mode AX.25 over bare KISS** — would require reimplementing
  the modulus-8 ARQ state machine that AGWPE hosts and the Linux kernel's
  `AF_AX25` stack otherwise provide.
- **AX.25 v2.2 XID negotiation** — inherently owned by whichever side runs
  the ARQ state machine (the AGWPE host or the kernel), not by this app.

## Building from source

Requires a stable Rust toolchain (edition 2021), plus GTK4 (≥ 4.12) and
libadwaita (≥ 1.5) development packages, and `libudev` (for KISS-over-serial
support via the `serialport` crate).

```sh
cargo build --release --workspace
cargo run -p pr-app
```

Run `cargo test --workspace` and `cargo clippy --workspace --all-targets`
before submitting changes.

## Installing on Arch Linux

A `PKGBUILD` is included; it builds directly from this working directory
(there's no tagged release/remote yet):

```sh
makepkg -f
sudo pacman -U packet-radio-*.pkg.tar.zst
```

## Workspace layout

| Crate      | Purpose                                                          |
|------------|-------------------------------------------------------------------|
| `pr-core`  | Config model (`~/.config/packet-radio/config.toml`), the `Port` trait, and shared event/command types. |
| `pr-ax25`  | AX.25 raw-socket (`AF_AX25`) and KISS (TCP/serial) transports.   |
| `pr-agwpe` | AGWPE frame codec and client actor.                              |
| `pr-app`   | The GTK4/libadwaita UI.                                          |

## Configuration

All state (ports, address book, history, preferences, beacons, mailbox
messages) lives in a single TOML file at `~/.config/packet-radio/config.toml`,
managed entirely through the UI — there's normally no need to hand-edit it.

## Basic usage

1. Open the menu (top-left, hamburger icon) → **Ports...** and add a port.
   Telnet/SSH need a host; AGWPE/KISS-TCP need a host and port; KISS-Serial
   needs a device and baud rate.
2. Click **+** on the tab bar (or the empty-state's **+ New Tab** button) to
   open a new session tab, pick the port, and (for AGWPE/AX.25/KISS ports)
   enter a destination callsign — either type it in or pick one from the
   Address Book via the small arrow next to the node field.
3. Press **Connect**. Type in the input box at the bottom and press Enter
   or click **Send**.
4. Check **Unproto** in a tab to send one-shot unconnected frames instead of
   opening a session, optionally via a digipeater path.
5. Use the menu for **Address Book**, **Mailbox**, **Beacons**, and
   **Preferences** (fonts, QRZ credentials, highlighting colors, the
   personal mailbox toggle, and keyword/bulletin highlight rules).

## License

MIT — see [LICENSE](LICENSE).
