---
tldr: Add PortConfig::Ble / KissTransport::Ble so packet-radio can use the lora-kiss-tnc firmware as a KISS TNC over Bluetooth LE (Nordic UART Service), Linux/BlueZ first, manual OS-level pairing assumed.
status: active
---

# Plan: BLE port type for lora-kiss-tnc

## Context

- Sibling project memory (read at plan time, not linked files since they live
  in a different repo's memory store):
  - `ble-modem-integration` (lora-kiss-tnc project memory) — GATT profile
    (NUS UUIDs), pairing requirements (Passkey Entry, DisplayOnly on the
    TNC), single-connection constraint, same KISS byte stream as USB.
  - `lora-tnc-cli-usb-only` (lora-kiss-tnc project memory) — confirms
    packet-radio (this app) is the intended BLE client; the sibling CLI is
    deliberately not one.
- `/home/phox/Nextcloud/Projects/lora-kiss-tnc/docs/kiss_protocol.md` — wire
  protocol reference, not yet read in full; read before Phase 1 research.
- `/home/phox/Nextcloud/Projects/lora-kiss-tnc/tools/ble_kiss_client.py` —
  Python/bleak reference client, useful for cross-checking characteristic
  UUIDs, write type, and chunking behavior.
- Existing packet-radio architecture read this session:
  - [pr-core/src/port.rs](pr-core/src/port.rs) — `PortRunner` trait, one
    blocking OS thread per port.
  - [pr-core/src/config.rs](pr-core/src/config.rs) — `PortConfig` enum
    (serde-tagged), `KissParams`/`KissArqParams`, `kind_label()`.
  - [pr-ax25/src/kiss_runner.rs](pr-ax25/src/kiss_runner.rs) — `KissRunner`/
    `KissTransport` (currently `Tcp`/`Serial`), `run_tcp`/`run_serial` share
    `command_loop`/`kiss_read_loop` over `impl Read`/`impl Write`.
  - [pr-ax25/src/kiss.rs](pr-ax25/src/kiss.rs) — `KissDecoder` (byte-stream,
    chunk-boundary agnostic — reusable as-is for BLE notifications).
  - [pr-app/src/ports_dialog.rs](pr-app/src/ports_dialog.rs) — Ports dialog,
    `KIND_NAMES` dropdown, per-kind field rows, `PortConfig::KissTcp`/
    `KissSerial` build/edit blocks.
  - Other exhaustive `PortConfig::` match sites to update: `pr-app/src/
    window.rs`, `pr-app/src/app_state.rs`, `pr-app/src/session_tab.rs`,
    `pr-app/src/dial_dialog.rs`.

### Decisions (from plan clarification)

- **Pairing scope**: manual first pass. User pairs/bonds the TNC via the
  OS's own Bluetooth settings (BlueZ on Linux) before adding the port in
  packet-radio. App-triggered pairing (BlueZ D-Bus agent, passkey UI) is
  explicitly deferred, not silently dropped — see Phase 5.
- **BLE crate**: `btleplug` (cross-platform, async/tokio, standard Rust BLE
  central crate). Needs a bridge into the existing synchronous
  `PortRunner` thread model.
- **Platform scope**: Linux only for this pass (dev machine, GTK4 app, BlueZ
  backend). macOS/Windows backends exist in btleplug but are unverified here.

### Seed

Add a new TNC — lora-kiss-tnc, a BLE-only-radio firmware — as a new port
type, wiring it up as a KISS TNC over BLE.

## Phases

### Phase 1 - Research & wire-protocol confirmation - status: open

1. [ ] Read `docs/kiss_protocol.md` and the BLE-relevant parts of
   `lora-kiss-tnc/src` (or `tools/ble_kiss_client.py`) to confirm:
   - exact GATT write type used for the RX characteristic (Write vs
     WriteWithoutResponse) — affects btleplug call and latency
   - whether the firmware negotiates/expects a larger ATT MTU, or assumes
     the default ~20-byte payload and expects the client to chunk
   - confirm TX (notify) characteristic requires no special subscribe
     dance beyond standard CCCD write (btleplug's `subscribe()` should
     cover this, but confirm nothing TNC-specific is needed)
2. [ ] Confirm btleplug's current version and Linux/BlueZ backend
   requirements (D-Bus session, any system packages/permissions needed
   beyond what BlueZ pairing already requires)
   - check whether connecting to an already-bonded/encrypted GATT
     characteristic "just works" once BlueZ holds the bond (expected, since
     encryption is a link-layer property BlueZ manages), or needs anything
     explicit from btleplug

### Phase 2 - BLE transport core (pr-ax25) - status: open

1. [ ] Add `btleplug` to `Cargo.toml` workspace deps and `pr-ax25/Cargo.toml`
2. [ ] Add `KissTransport::Ble { address: String }` to
   [pr-ax25/src/kiss_runner.rs](pr-ax25/src/kiss_runner.rs)
   - `address` is the BLE device address/identifier BlueZ uses (platform
     peripheral id), entered by the user after pairing via OS settings
3. [ ] Implement a reader/writer bridge from btleplug's async API into the
   blocking `impl Read` / `impl Write` shapes `kiss_read_loop`/
   `command_loop` expect
   - spin up a `tokio::runtime::Runtime` inside `run_ble`'s dedicated
     thread (mirrors the "one OS thread, blocking I/O" model — the tokio
     runtime is local plumbing, not exposed to the rest of the port
     abstraction)
   - notify → `std::sync::mpsc` channel drained by a `Read` impl
   - `Write` impl pushes bytes to a channel drained by an async task that
     writes to the RX characteristic, chunked to the negotiated/assumed
     ATT MTU (per Phase 1 findings)
   - unlike TCP/serial, BLE reader/writer aren't naturally the same
     `try_clone`-able handle — write `run_ble` directly rather than forcing
     it through `run_tcp`/`run_serial`'s clone pattern
4. [ ] Implement `run_ble()`: connect by address, discover NUS service
   (`6E400001-...`) and RX/TX characteristics (`6E400002-`/`6E400003-...`),
   subscribe to TX, send `send_kiss_params` over the writer, spawn
   `kiss_read_loop` over the reader, run `command_loop` over the writer —
   same lifecycle events (`PortConnected`/`PortDisconnected`/`PortError`) as
   `run_tcp`/`run_serial`
5. [ ] Handle a BLE-initiated disconnect (peripheral drops the link, e.g.
   TNC reboot or out of range) → `PortEvent::PortDisconnected`

### Phase 3 - Config & UI wiring (pr-core, pr-app) - status: open

1. [ ] Add `PortConfig::Ble { address: String, name: Option<String>, my_call:
   String, kiss_params: KissParams, kiss_arq: KissArqParams }` to
   [pr-core/src/config.rs](pr-core/src/config.rs); `kind_label()` →
   `"KISS (BLE)"`
   - `name` is a display-only label (device name at pairing time), not used
     for connecting — `address` is authoritative
2. [ ] Wire `KissRunner`'s `PortConfig` → `KissTransport` dispatch (wherever
   that mapping lives — check `pr-app` where `PortRunner`s are constructed
   from `PortConfig`) to build `KissTransport::Ble`
3. [ ] Add a `KIND_NAMES` entry and per-kind field block in
   [pr-app/src/ports_dialog.rs](pr-app/src/ports_dialog.rs): address field
   (manual entry — paste the BlueZ device address after pairing in OS
   settings), reuse existing my_call/kiss_params/kiss_arq rows
4. [ ] Update the other exhaustive `PortConfig::` matches found this
   session (`window.rs`, `app_state.rs`, `session_tab.rs`, `dial_dialog.rs`)
   — compiler will catch these as non-exhaustive-match errors, use that as
   the checklist

### Phase 4 - Hardware verification - status: open

1. [ ] Pair the lora-kiss-tnc device via OS Bluetooth settings (GNOME
   Settings / `bluetoothctl`), confirm bonded
2. [ ] Add a BLE port in packet-radio pointing at the paired device, connect
3. [ ] Verify KISS traffic round-trip: send unproto/UI frame from
   packet-radio, confirm received on the TNC side (or a second radio/SDR
   per lora-kiss-tnc's own verified setup); receive an inbound frame,
   confirm it decodes and shows in Monitor
4. [ ] Fix issues found; note any deviations from Phase 1's protocol
   assumptions

### Phase 5 - App-triggered pairing (deferred, needs go-ahead) - status: open

_Deferred by explicit choice during plan clarification — manual pairing
ships first. Only start this phase on explicit confirmation, not
automatically after Phase 4._

1. [ ] Research BlueZ's D-Bus pairing agent API (register an agent,
   `DisplayYesNo`/`KeyboardOnly` vs what the TNC's `DisplayOnly` IO
   capability actually requires from the central) and whether `bluer`
   (BlueZ-specific) is a better fit here than `btleplug` for this one piece
2. [ ] Design in-app pairing UX: trigger pair, prompt user for the 6-digit
   code shown on the TNC's OLED, surface bonding success/failure
3. [ ] Implement, verify against hardware

## Verification

- `cargo build` / `cargo test` pass across the workspace with the new
  variant wired through every match site.
- Manual hardware test (Phase 4) confirms a real round-trip KISS
  send/receive over BLE against the lora-kiss-tnc firmware, with the port
  reporting `PortConnected`/`PortDisconnected` correctly including on TNC
  reboot / out-of-range disconnect.

## Adjustments

_None yet._

## Progress Log

- 2608211912 — Plan created after reading lora-kiss-tnc sibling-project
  memory and packet-radio's port/KISS/UI architecture; clarified pairing
  scope (manual first), BLE crate (btleplug), platform scope (Linux only)
  with the user before drafting phases.
