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
  protocol reference, read in full (see Phase 1 findings below).
- `/home/phox/Nextcloud/Projects/lora-kiss-tnc/src/transport/
  ble_nus_transport.h` / `ble_nimble_impl.cpp` — firmware's BLE transport:
  RX characteristic is `WRITE_NR | WRITE_ENC` (write-without-response,
  encrypted-link-required), TX notifications chunked at a fixed 180 bytes
  with no explicit MTU-negotiation call.
- `/home/phox/Nextcloud/Projects/lora-kiss-tnc/tools/ble_kiss_client.py` —
  Python/bleak reference client, read in full — confirms the same 180-byte
  chunking + `response=False` write scheme works end-to-end against real
  hardware over BlueZ (per that repo's README verification notes).
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

### Phase 1 - Research & wire-protocol confirmation - status: done

1. [x] Read `docs/kiss_protocol.md` and the BLE-relevant parts of
   `lora-kiss-tnc/src` (or `tools/ble_kiss_client.py`) to confirm:
   - exact GATT write type used for the RX characteristic (Write vs
     WriteWithoutResponse) — affects btleplug call and latency
   - whether the firmware negotiates/expects a larger ATT MTU, or assumes
     the default ~20-byte payload and expects the client to chunk
   - confirm TX (notify) characteristic requires no special subscribe
     dance beyond standard CCCD write (btleplug's `subscribe()` should
     cover this, but confirm nothing TNC-specific is needed)
   - => RX characteristic is `WRITE_NR | WRITE_ENC` — write-without-response,
     encrypted-link-required. btleplug's `Peripheral::write()` must be
     called with `WriteType::WithoutResponse`
     (`ble_nimble_impl.cpp:117-118`)
   - => firmware does no explicit MTU negotiation; `sendFrame()` chunks TX
     notifications at a fixed 180 bytes regardless of negotiated MTU
     (`ble_nimble_impl.cpp:163-172`) and relies on the far side's streaming
     `KissDecoder` to reassemble across chunk boundaries — matches this
     app's existing `KissDecoder`, no changes needed there
   - => TX subscribe is standard NUS notify, nothing TNC-specific beyond
     the UUIDs already documented
2. [x] Confirm btleplug's current version and Linux/BlueZ backend
   requirements (D-Bus session, any system packages/permissions needed
   beyond what BlueZ pairing already requires)
   - check whether connecting to an already-bonded/encrypted GATT
     characteristic "just works" once BlueZ holds the bond (expected, since
     encryption is a link-layer property BlueZ manages), or needs anything
     explicit from btleplug
   - => **MTU resolved**: on Linux, btleplug's BlueZ backend goes through
     BlueZ's D-Bus API (`bluez_async` crate → `WriteValue`/notify), and
     BlueZ auto-negotiates the ATT MTU while transparently
     fragmenting/reassembling `WriteValue`/notification payloads at the
     D-Bus level regardless of the negotiated MTU (confirmed via the
     linux-bluetooth mailing list: "MTU is negotiated automatically to the
     maximum value possible but it doesn't really matter with WriteValue
     and ReadValue since they will fragment and reassemble the data
     automatically"). This is also already empirically verified: the
     bleak-based `ble_kiss_client.py` uses the identical 180-byte-chunk +
     write-without-response scheme against real lora-kiss-tnc hardware,
     confirmed working end-to-end. **No explicit MTU-negotiation step
     needed in `run_ble`** — chunk writes at 180 bytes to match the
     firmware's own convention and let BlueZ/btleplug handle the rest.
   - => **Scan resolved**: btleplug's Linux backend (`src/bluez/adapter.rs`,
     `peripherals()`/`peripheral()`) queries BlueZ's live D-Bus device list
     (`session.get_devices_on_adapter()`) rather than a locally-cached
     scan-discovered set — a bonded device already known to BlueZ shows up
     via `adapter.peripherals()` with **no active `start_scan()` required**.
     `run_ble` can call `peripherals()`, match by `Peripheral::address()`
     (`BDAddr`) against the configured address, and `connect()` directly.
   - => write type: `Peripheral::write()` takes `btleplug::api::WriteType`
     (`WithoutResponse`/`WithResponse`), maps directly to the RX
     characteristic's `WRITE_NR` property — use `WriteType::WithoutResponse`.

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
     writes to the RX characteristic via `WriteType::WithoutResponse`,
     chunked at 180 bytes (matches the firmware's own chunk size; no
     explicit MTU negotiation needed — BlueZ fragments/reassembles
     `WriteValue`/notify transparently, see Phase 1 findings)
   - unlike TCP/serial, BLE reader/writer aren't naturally the same
     `try_clone`-able handle — write `run_ble` directly rather than forcing
     it through `run_tcp`/`run_serial`'s clone pattern
4. [ ] Implement `run_ble()`: call `adapter.peripherals()` (queries BlueZ's
   live device list, no `start_scan()` needed for an already-bonded
   device — see Phase 1), match by `Peripheral::address()` against the
   configured address, `connect()`, discover NUS service
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

- 2608211917 — [[planreview - 2608211917 - BLE port type for lora-kiss-tnc]]
  written after starting Phase 1; user asked to resolve the MTU and scan
  questions it raised before continuing. Phase 1 completed via firmware
  source reading + external research (btleplug/BlueZ source, mailing list),
  answers folded into Phase 1 (now `status: done`) and Phase 2 actions 3-4.
  Review's other open items (unbonded-connect error handling, Context
  links) not addressed — user asked specifically for MTU/scan.

## Progress Log

- 2608211912 — Plan created after reading lora-kiss-tnc sibling-project
  memory and packet-radio's port/KISS/UI architecture; clarified pairing
  scope (manual first), BLE crate (btleplug), platform scope (Linux only)
  with the user before drafting phases.
- 2608211917 — Plan review written; user asked to resolve MTU/scan
  questions. Read lora-kiss-tnc firmware BLE transport source
  (`ble_nus_transport.h`, `ble_nimble_impl.cpp`) and `ble_kiss_client.py`,
  researched btleplug's BlueZ backend source and BlueZ D-Bus MTU behavior.
  Phase 1 completed: write type is `WriteType::WithoutResponse`, no MTU
  negotiation needed (BlueZ fragments/reassembles transparently), no scan
  needed to reach an already-bonded device (`adapter.peripherals()` queries
  BlueZ live). Phase 2 actions 3-4 updated to reflect these answers.
