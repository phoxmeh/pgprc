# Pretty Good Packet Radio Client (PGPRC)

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
- **Managed Direwolf process** (optional): a handset-icon header button
  starts/stops a local `direwolf` process directly — green while running,
  yellow if it failed to start. Right-click for the **Direwolf Console**
  (its captured log output, live, plus its own Start/Stop buttons, a
  "Save Log..." export, and a "Settings..." dialog holding the raw
  `direwolf.conf` text — editable in place, or loaded from an existing
  file — and an auto-start-with-this-app checkbox). Entirely separate from
  the ports above — you still add/connect an AGWPE or KISS port pointed at
  it the normal way.
- **Session tabs**: pick a port and (for node-capable ports) a destination
  callsign, connect explicitly, and send/receive text. Tabs persist across
  disconnects so you can reconnect or repoint them at a different node.
  Each tab can also run in **Unproto mode** to send one-shot unconnected
  frames instead of opening a session, with its own digipeater `via` field.
- **Per-node history**: every (port, node, mode) conversation gets its own
  plain-text scrollback file under `history/<port>/`, shown (tail-capped to a
  configurable line count) as a read-only preview whenever that tab isn't
  currently connected. A per-tab **Capture** checkbox (left of "Save...")
  can also continuously append everything shown in that tab to a separate,
  dated capture-log file in the same directory — a running transcript
  distinct from the auto-managed history file.
- **Monitor view**: a live log of all port/frame activity across every
  connected port, with a small substring filter in the header (next to
  "Send Beacon...") and "Save Monitor Log..." export.
- **Configurable highlighting**: callsigns, known (address-book) callsigns,
  your own callsign (the Default Callsign in Preferences) in its own color,
  AX.25 frame-type tags, and user-defined destination-address rules (e.g.
  CQ or a digipeater alias) are colored in both the Monitor and session
  scrollback — each rule's bell toggle can also raise a desktop
  notification on a match.
- **Address Book**: tracks "last heard" automatically as callsigns show up
  on the air, plus manually-entered name/alias/location/notes/**via path**.
  Picking an entry from a session tab's address-book dropdown fills in both
  its callsign and via path, since a station usually needs the same
  digipeater route every time. Includes an online **QRZ.com lookup** and
  **ADIF export** of logged QSOs.
- **Scheduled beacons**: configure one or more periodic unproto beacons
  (port, destination, via path, message, interval) that fire automatically
  while their port is connected.
- **Personal packet mailbox** (off by default): a minimal BBS-style
  auto-responder (`L`ist / `R`ead / `S`end / `B`ye) for unsolicited incoming
  connections — a local message store, not real Winlink/RMS interop.
- **Status bar**: shows the currently selected tab's connect/disconnect
  state (icon + subtle text) on the left and its packet count/byte totals,
  sent and received, on the right.
- **KISS TNC parameter** configuration (TXDELAY/persistence/slot-time/
  full-duplex) and **PID labeling** of monitored frames (NET/ROM, ARPA IP,
  etc.).
- **Pinning**: pin a tab to have its shell (port + node prefilled,
  disconnected) recreated automatically the next time the app starts. It
  never auto-connects — you still press Connect.
- **Desktop notifications** (off by default): an unsolicited incoming
  connection, a monitored frame addressed to your Default Callsign, or a
  frame matching a Custom Rule with its bell toggle on, each raise a
  notification. Every notified packet is also kept in the **Notified
  Packets** list (menu) — useful since these can be bulletins — with the
  same highlighting as the Monitor and a two-click delete per entry.
- **Help** and **About** (bottom of the menu): a quick in-app reference for
  basic usage and keyboard shortcuts, and standard version/license info.

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
sudo pacman -U pgprc-*.pkg.tar.zst
```

Optional dependencies: `direwolf` (sound-card TNC — required for the
header's managed Direwolf process button to do anything) and `linux-lts`
(a Linux kernel build with AX.25 support, for the `AF_AX25` raw-socket port
kind — the mainline `linux` kernel package doesn't enable it).

## Building a portable AppImage

`appimage/build.sh` builds a self-contained `pgprc-*-x86_64.AppImage` that
runs on most x86_64 Linux distros without installing anything:

```sh
./appimage/build.sh
```

It needs `linuxdeploy`, `linuxdeploy-plugin-gtk.sh`, and `appimagetool` —
use copies already on `PATH`, or drop them (executable) into
`appimage/tools/` next to the script:

- <https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage>
- <https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh>
- <https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage>

The script works around two issues seen building on a bleeding-edge glibc/
binutils toolchain (both documented inline): the `gtk` plugin unconditionally
copying a `gtk-4.0` module directory that GTK4 doesn't ship on every distro
(Arch included), and `patchelf` corrupting bundled libraries that carry RELR
relocations (`DT_RELR`/`.relr.dyn`) when rewriting their runpath, which
crashes the dynamic linker at runtime. Neither should affect older/more
conservative toolchains, but the workarounds are harmless either way.

## Workspace layout

| Crate      | Purpose                                                          |
|------------|-------------------------------------------------------------------|
| `pr-core`  | Config model (`~/.config/packet-radio/`, split across several files), the `Port` trait, and shared event/command types. |
| `pr-ax25`  | AX.25 raw-socket (`AF_AX25`) and KISS (TCP/serial) transports.   |
| `pr-agwpe` | AGWPE frame codec and client actor.                              |
| `pr-app`   | The GTK4/libadwaita UI — builds the `pgprc` binary.              |

## Configuration

State is split across `~/.config/packet-radio/` so it stays human-navigable —
managed entirely through the UI, but each piece is easy to find or back up
individually if you ever need to:

| File                    | Contents                                              |
|-------------------------|--------------------------------------------------------|
| `config.toml`            | General preferences only (font, timestamps, QRZ creds, highlight colors, mailbox/notify toggles). |
| `ports.toml`             | Configured ports.                                      |
| `address_book.toml`      | Address book entries.                                  |
| `qso_log.toml`           | Logged connected-mode QSOs (for ADIF export).           |
| `notified_packets.toml`  | Packets that raised a desktop notification.             |
| `rules.toml`             | Custom highlight/notification destination rules.        |
| `pinned_sessions.toml`   | Pinned tab shells recreated at startup.                  |
| `beacons.toml`           | Scheduled beacons.                                      |
| `mailbox.toml`           | Stored personal-mailbox messages (received only).        |
| `history/<port>/`        | One plain-text file per (node, mode) — the auto-managed scrollback archive — plus any dated capture-log files from the per-tab Capture checkbox (`<node>_<date>_<time>.txt`). |

Upgrading from an older single-`config.toml` install migrates automatically
and losslessly the first time the app starts, including converting the old
in-file node history into these per-node text files.

## Basic usage

1. Open the menu (top-left, hamburger icon) → **Ports...** and add a port.
   Telnet/SSH need a host; AGWPE/KISS-TCP need a host and port; KISS-Serial
   needs a device and baud rate. Running a local Direwolf instance? Right-
   click the handset icon in the header → **Settings...** to paste in its
   config and (optionally) have this app start it for you.
2. Click **+** on the tab bar (or the empty-state's **+ New Tab** button) to
   open a new session tab, pick the port, and (for AGWPE/AX.25/KISS ports)
   enter a destination callsign — either type it in or pick one from the
   Address Book via the small arrow next to the node field (also fills in
   its via path, if it has one).
3. Press **Connect**. Type in the input box at the bottom and press Enter
   or click **Send**.
4. Check **Unproto** in a tab to send one-shot unconnected frames instead of
   opening a session, optionally via a digipeater path.
5. Use the menu for **Address Book**, **Mailbox**, **Beacons**, and
   **Preferences** (fonts, QRZ credentials, highlighting colors, the
   personal mailbox and notification toggles, and the **Custom Rules**
   list — destination addresses to highlight, each with a bell toggle to
   also raise a desktop notification on a match). **Help** and **About**
   are at the bottom of the same menu.

## Keyboard shortcuts

| Shortcut       | Action                                           |
|----------------|--------------------------------------------------|
| `Escape`       | Close the frontmost dialog (Ports, Address Book, Preferences, etc.) — always safe, since none of them save on close, only on an explicit Save/Send click. |
| `Ctrl+N`       | New session tab.                                  |
| `Ctrl+W`       | Close the current session tab.                    |
| `Ctrl+,`       | Open Preferences.                                 |
| `Ctrl+F`       | Focus the Monitor filter.                         |
| `Ctrl+Q`       | Quit.                                             |

## License

MIT — see [LICENSE](LICENSE).
