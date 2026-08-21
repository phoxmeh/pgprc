---
tldr: Pre-implementation review of [[plan - 2608211912 - BLE port type for lora-kiss-tnc]]
---

# Plan Review: BLE port type for lora-kiss-tnc

Plan: [[plan - 2608211912 - BLE port type for lora-kiss-tnc]]

Reviewed against `docs/kiss_protocol.md`, `src/transport/ble_nus_transport.h`,
and `src/transport/ble_nimble_impl.cpp` in the lora-kiss-tnc repo (read
partway through Phase 1 before this review was requested — findings below
fold that reading in).

# Phase 1: Research & wire-protocol confirmation — MTU/scan questions resolved — status: resolved

1. RX characteristic property is `WRITE_NR | WRITE_ENC` (write-without-response,
   encrypted-link-required) — not plain `WRITE`. btleplug's write call takes
   a `WriteType` parameter; Phase 2's writer must pass
   `WriteType::WithoutResponse`, or the write will likely fail against a
   characteristic that doesn't advertise the with-response property.
   - [ ]
2. The firmware does not call any explicit MTU-negotiation API in
   `ble_nimble_impl.cpp` — grepped, nothing found. TX notifications are
   chunked at a fixed 180 bytes regardless of the actual negotiated MTU
   (`ble_nimble_impl.cpp:166`, `kChunkSize = 180`), which only works if the
   link's negotiated MTU is ≥183 bytes (180 + 3-byte ATT header). NimBLE's
   default max MTU is high (up to 517) but the *actual* negotiated value
   depends on what the central (btleplug/BlueZ) requests — if BlueZ doesn't
   request an MTU bump, the link may stay at the default 23-byte MTU and
   every 180-byte notify would silently get truncated or fail.
   - Action 2 in this phase already flags "whether the firmware
     negotiates/expects a larger ATT MTU" but frames it as a firmware
     question — it's actually a central-side (btleplug/BlueZ) question:
     does btleplug/BlueZ auto-request a larger MTU on connect, or does the
     plan need an explicit MTU-request step in Phase 2's `run_ble`?
   - [x] Resolved: no explicit MTU request needed. On Linux, btleplug's
     BlueZ backend goes through BlueZ's D-Bus API, which auto-negotiates
     MTU and transparently fragments/reassembles `WriteValue`/notify
     payloads regardless of the negotiated value (linux-bluetooth mailing
     list). Empirically confirmed too: `ble_kiss_client.py` (bleak, same
     BlueZ backend) already does the identical 180-byte-chunk +
     write-without-response scheme against real hardware, verified
     working per the lora-kiss-tnc README.
3. `tools/ble_kiss_client.py` (bleak-based reference client) was not read
   during this session's Phase 1 work despite being named in the plan's
   Context — bleak's own MTU/write-type handling there would be a fast,
   already-working cross-check before writing the btleplug equivalent.
   - [x] Read. Connects directly via `BleakClient(address)` (no scan when
     address is already known), writes RX in 180-byte chunks with
     `response=False` — matches the write-type/chunking findings above.

# Phase 2: BLE transport core (pr-ax25) — status: open

1. Action 3's chunking note says "chunked to the negotiated/assumed ATT
   MTU (per Phase 1 findings)" but as of this review Phase 1 hasn't landed
   a concrete MTU number or negotiation strategy — this action can't
   actually be implemented as worded until Phase 1's finding #2 above is
   resolved (either "btleplug/BlueZ auto-negotiates a usable MTU, use
   `characteristic.max_write_len` or equivalent" or "we must explicitly
   request MTU=X before writing").
   - [x] Resolved, plan action 3 updated: fixed 180-byte chunks +
     `WriteType::WithoutResponse`, no MTU negotiation step. See Phase 1
     above.
2. No action addresses what happens if `run_ble` is asked to connect while
   BlueZ has no bond for the given address (i.e. Phase 4's manual-pairing
   precondition wasn't actually met) — should probably surface as a clear
   `PortError` rather than an opaque write failure or hang. Worth an
   explicit action or at least a note under action 4.
   - [ ]
3. Address discovery/connect: does btleplug require an active scan to
   resolve a `PeripheralId` from a known BlueZ address string, or can it
   connect directly given the address (since the device is already
   bonded/known to BlueZ)? This affects whether `run_ble` needs a
   scan-then-match step before `connect()`, and belongs in Phase 1's
   research rather than being discovered mid-Phase-2.
   - [x] Resolved: no scan needed. btleplug's BlueZ backend queries BlueZ's
     live D-Bus device list on `peripherals()`, which already includes
     bonded devices. Plan action 4 updated to match/connect directly. See
     Phase 1 above.

# Phase 3: Config & UI wiring (pr-core, pr-app) — status: open

No issues found — matches the existing `PortConfig`/`ports_dialog.rs`
patterns closely enough that action granularity looks right.

# Phase 4: Hardware verification — status: open

No issues found.

# Phase 5: App-triggered pairing (deferred) — status: open

No issues found — correctly marked as requiring explicit go-ahead, not
auto-started after Phase 4.

# General

1. The plan's Context section doesn't yet link the three lora-kiss-tnc
   source files actually read this session (`docs/kiss_protocol.md`,
   `src/transport/ble_nus_transport.h`, `src/transport/
   ble_nimble_impl.cpp`) — only the two sibling-project memory names and
   the not-yet-read Python client. Since these files directly shaped Phase
   1/2 findings above, they should be added as durable Context links, not
   left implicit in this review.
   - [ ]
2. Cross-check: the firmware's `sendFrame` chunks at a byte boundary with
   no regard for KISS frame boundaries ("it's safe to split arbitrarily
   across notifications — the far side's streaming decoder reassembles
   it" — `ble_nimble_impl.cpp:163-165`), which matches the plan's assumption
   that `KissDecoder` (chunk-boundary agnostic) needs no changes. No action
   needed, just confirms Phase 2 action choice is sound.
   - [ ]
