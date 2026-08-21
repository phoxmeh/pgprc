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

### Phase 2 - BLE transport core (pr-ax25) - status: done

1. [x] Add `btleplug` to `Cargo.toml` workspace deps and `pr-ax25/Cargo.toml`
   - => also added `tokio` (`rt-multi-thread`, `sync`, `time` features),
     `uuid`, `futures` as workspace deps — btleplug's async API and the
     reader/writer bridge need all four
2. [x] Add `KissTransport::Ble { address: String }` to
   [pr-ax25/src/kiss_runner.rs](pr-ax25/src/kiss_runner.rs)
   - `address` is the BLE device address/identifier BlueZ uses (platform
     peripheral id), entered by the user after pairing via OS settings
3. [x] Implement a reader/writer bridge from btleplug's async API into the
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
   - => implemented as `BleReader`/`BleWriter` structs. Runtime is a
     **multi-thread** tokio runtime (not current-thread): the
     notify-forward/write-drain/disconnect-watcher tasks (action 4/5) must
     keep making progress on their own worker threads even while the port
     thread isn't inside `block_on` (e.g. while blocked in
     `command_loop`'s synchronous `cmd_rx.recv_timeout`) — a current-thread
     runtime would stall those tasks the moment `block_on` returns from
     the initial connect setup
   - => `BleReader::read` maps a closed channel (`RecvTimeoutError::
     Disconnected`) to `Ok(0)`, matching TCP/serial's EOF convention, so
     `kiss_read_loop` needed no BLE-specific changes
4. [x] Implement `run_ble()`: call `adapter.peripherals()` (queries BlueZ's
   live device list, no `start_scan()` needed for an already-bonded
   device — see Phase 1), match by `Peripheral::address()` against the
   configured address, `connect()`, discover NUS service
   (`6E400001-...`) and RX/TX characteristics (`6E400002-`/`6E400003-...`),
   subscribe to TX, send `send_kiss_params` over the writer, spawn
   `kiss_read_loop` over the reader, run `command_loop` over the writer —
   same lifecycle events (`PortConnected`/`PortDisconnected`/`PortError`) as
   `run_tcp`/`run_serial`
   - => implemented as `ble_connect()` (async setup) + `run_ble()` (sync
     shell matching `run_tcp`/`run_serial`'s shape); characteristics
     matched by both UUID and `service_uuid` (not just UUID) to be safe if
     the device ever exposes more than the NUS service
5. [x] Handle a BLE-initiated disconnect (peripheral drops the link, e.g.
   TNC reboot or out of range) → `PortEvent::PortDisconnected`
   - => a dropped write only fails once something is actively being sent,
     so added a dedicated disconnect-watcher task (watches
     `Central::events()` for `CentralEvent::DeviceDisconnected` matching
     our peripheral id) that reports through a new `InternalEvent::
     Disconnected` variant — reuses the existing internal-event channel
     `kiss_read_loop` already uses to hand decoded frames to
     `command_loop`, so `command_loop` tears down via the same "return
     false from `handle_internal_event`" path a write failure already uses
   - => verified: `cargo check --workspace` clean, `cargo test -p pr-ax25`
     64/64 passing (existing tests untouched, no BLE-specific tests added
     yet — hardware verification is Phase 4)

### Phase 3 - Config & UI wiring (pr-core, pr-app) - status: done

1. [x] Add `PortConfig::Ble { address: String, name: Option<String>, my_call:
   String, kiss_params: KissParams, kiss_arq: KissArqParams }` to
   [pr-core/src/config.rs](pr-core/src/config.rs); `kind_label()` →
   `"KISS (BLE)"`
   - `name` is a display-only label (device name at pairing time), not used
     for connecting — `address` is authoritative
   - => named `PortConfig::KissBle` (not bare `Ble`) to match the existing
     `KissTcp`/`KissSerial` naming convention
2. [x] Wire `KissRunner`'s `PortConfig` → `KissTransport` dispatch (wherever
   that mapping lives — check `pr-app` where `PortRunner`s are constructed
   from `PortConfig`) to build `KissTransport::Ble`
   - => lives in `pr-app/src/app_state.rs`'s `spawn_for_config()`
3. [x] Add a `KIND_NAMES` entry and per-kind field block in
   [pr-app/src/ports_dialog.rs](pr-app/src/ports_dialog.rs): address field
   (manual entry — paste the BlueZ device address after pairing in OS
   settings), reuse existing my_call/kiss_params/kiss_arq rows
   - => added a dim-label hint above the address field pointing at OS
     Bluetooth settings, since this is the one field with no sensible
     default/placeholder that actually works
4. [x] Update the other exhaustive `PortConfig::` matches found this
   session (`window.rs`, `app_state.rs`, `session_tab.rs`, `dial_dialog.rs`)
   — compiler will catch these as non-exhaustive-match errors, use that as
   the checklist
   - => `window.rs`'s `line_ending()` and `dial_dialog.rs`'s Telnet/SSH
     check both already used wildcard/negative matches, correct for BLE
     as-is (added a BLE case to `line_ending`'s test anyway for coverage)
   - => `session_tab.rs`'s `port_supports_connect`/`port_supports_unproto`
     are **allowlists**, not exhaustive matches — compiler didn't catch
     these, had to be found by inspection. Missing `KissBle` here would
     have silently left BLE ports unable to open connected-mode sessions
     or send unproto traffic despite being otherwise fully wired up. Added
     `KissBle` to both, plus a `KissBle` case in the table-driven tests
     covering them (`all_variants()`) — the existing tests didn't exercise
     the new variant at all until this
   - => `cargo test --workspace`: 113/113 passing

### Phase 4 - Hardware verification - status: open

1. [x] Pair the lora-kiss-tnc device via OS Bluetooth settings (GNOME
   Settings / `bluetoothctl`), confirm bonded
   - => device was already paired/bonded from earlier work on this
     hardware (address `34:B7:DA:57:5F:1D`, name `LoRa-KISS-TNC`);
     confirmed via `bluetoothctl info` (`Paired: yes`, `Bonded: yes`)
2. [x] Add a BLE port in packet-radio pointing at the paired device, connect
   - => connected successfully through the real GUI (`target/debug/pgprc`,
     built from this branch)
3. [ ] Verify KISS traffic round-trip: send unproto/UI frame from
   packet-radio, confirm received on the TNC side (or a second radio/SDR
   per lora-kiss-tnc's own verified setup); receive an inbound frame,
   confirm it decodes and shows in Monitor
   - => TX half confirmed: sent a UI frame from packet-radio over the new
     BLE port, visible on the user's SDR waterfall at the configured
     frequency -- confirms connect, KISS encode, the BLE write path
     (`WriteType::WithoutResponse`, 180-byte chunking), and the firmware's
     TX-to-radio path all work end to end over a real link
   - RX half (inbound LoRa frame -> BLE notify -> Monitor) not yet
     verified -- needs a second LoRa device/SDR that can transmit on the
     TNC's frequency; matches lora-kiss-tnc's own README, which lists RX
     as its one still-unverified path. Staying `[ ]` until this is done or
     the user decides to accept TX-only verification for now.
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
- 2608211934 — Noticed mid-Phase-3 that lora-kiss-tnc's
  `ble_nimble_impl.cpp` changed on disk since Phase 1's reading (a
  30-second pairing timeout that force-drops a connected-but-never-paired
  device, plus `setLinkEncrypted()` gating `sendFrame()` on the link
  actually being encrypted, not just connected). No action needed on this
  side — doesn't change the GATT UUIDs, write type, or chunking this plan
  relies on — but worth knowing about before Phase 4: if a device connects
  and gets dropped ~30s later, that's this timeout, not a packet-radio bug.
- 2608211949 — User requested (mid-Phase-4, not part of the original plan)
  that the BLE port's device-address field be a dropdown of paired
  Bluetooth devices instead of manual entry. Implemented immediately
  (explicit direction, not a silent scope addition) — see
  `paired_ble_devices()` / dropdown + Refresh button in
  [pr-app/src/ports_dialog.rs](pr-app/src/ports_dialog.rs).
- 2608211949 — A `Gtk-CRITICAL **: gtk_box_append: assertion
  'gtk_widget_get_parent (child) == NULL' failed` appeared in the app's
  log ~13s after launch, during first manual testing. User confirmed the
  Ports dialog looked fine visually, so not blocking; not chased down
  further this session -- likely pre-existing (unrelated to this branch's
  changes) but not confirmed either way. Worth a `/eidos:observe` or
  separate investigation if it recurs.

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
- 2608211929 — Phase 2 implemented: `btleplug`/`tokio`/`uuid`/`futures`
  deps added; `KissTransport::Ble`, `BleReader`/`BleWriter`, `ble_connect()`
  async setup, `run_ble()`, and `InternalEvent::Disconnected` added to
  [pr-ax25/src/kiss_runner.rs](pr-ax25/src/kiss_runner.rs). `cargo check
  --workspace` clean, `cargo test -p pr-ax25` 64/64 passing.
- 2608211934 — Phase 3 implemented: `PortConfig::KissBle` in
  [pr-core/src/config.rs](pr-core/src/config.rs), dispatch in
  [pr-app/src/app_state.rs](pr-app/src/app_state.rs), "KISS (BLE)" tab in
  [pr-app/src/ports_dialog.rs](pr-app/src/ports_dialog.rs), allowlists in
  [pr-app/src/session_tab.rs](pr-app/src/session_tab.rs) (compiler-silent —
  found by inspection, not a build error). `cargo test --workspace`
  113/113 passing. Noticed lora-kiss-tnc firmware source changed since
  Phase 1 (pairing timeout, link-encryption gate) — no plan impact, logged
  for Phase 4 awareness.
- 2608211949 — Phase 4 started against real hardware (already-paired
  `LoRa-KISS-TNC` at `34:B7:DA:57:5F:1D`). Built and launched
  `target/debug/pgprc`; user added a BLE port, connected successfully, sent
  a UI frame that showed up on their SDR waterfall — confirms the full TX
  path end to end. RX path (inbound LoRa frame -> Monitor) still
  unverified, needs a second transmitting device. Also implemented, on
  request, a paired-devices dropdown for the address field (see
  Adjustments) — `cargo test --workspace` still 113/113 after that change.
