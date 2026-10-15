# Read-only live meter and the Input A/B source field

Authority: the 2026-10-14 operator rulings recorded on Linear TIN-5379 (receipt
comment `9f4ae802`): DCX control is a priority lane. This lane changed only
source; it opened no serial device. R-HOOK-CONVERGENCE-20261004 (R-N13).

## What landed on `feat/dcx-live-meter-20261014`

- `dcx_core::meter::MeterReadingV1` decodes one exact 25-byte function-`0x04`
  reply to `Query::Ping` (`F0 00 20 32 <dev> 0E 44 00 00 F7`):
  - frame bytes 8-10 are inputs A/B/C, bytes 11-16 are outputs 1-6;
  - the low five bits are a level step 0-6 (5 warn, 6 clip), bit 5 is limiter;
  - a level above 6, a set bit 6, a wrong length, function or address fails
    closed. Opaque bytes are never serialized.
- `dcx_transport::meter::execute_live_meter`: one valid Search identity (at
  most three attempts, only empty timeouts replay, 5 s cadence), then ten
  meter queries each preceded by 500 ms, 30 s budget. No remote-mode or
  direct-parameter write exists on this path. The Darwin carrier needed no
  change; a fake-backend test proves the eleven wire writes are exactly one
  Search plus ten meter queries.
- `dcxctl discovery live-meter --tty ... --expected-device 0` (live-control
  feature) prints `dcx.live-meter-result/v1`, including readings validated
  before any stop. `dcxctl discovery validate-meter FILE` decodes offline.
- `LabProfileV1.input_ab_source` is optional (`analog` | `aes_ebu`) so the
  pinned `safe-muted-v1.json` digest is unchanged. The Dump0 index 119 code map
  (0 analog, 1 AES/EBU) is recorded as a constant only; no snapshot decoder or
  apply path reads or writes it yet. The operator set the front panel to Analog
  at about 22:03 ET on 2026-10-14; the synthetic control-room profile now says
  `analog`.
- Compatibility: `LabProfileV1` keeps `deny_unknown_fields` and the same
  `schema_version`/`profile_revision`, so a dcxctl built before this field
  (for example a legalab-pinned binary) rejects any profile that sets
  `input_ab_source`, including `SYNTHETIC-control-room-v1.json`. The pinned
  `safe-muted-v1.json` omits it and is unaffected. Bump the revision when a
  consumer starts emitting the field.

## Evidence class

The meter layout and the Dump0[119] map are lane hypotheses. Every fixture is
synthetic. Nothing here is a named-device observation until a live-meter run
returns readings that agree with audible signal.

## Review fixups (2026-10-14)

- AGENTS.md now lists `discovery live-meter` among the closed live-control
  operations.
- `MeterError::LevelOutOfRange` carries the raw byte, like `ReservedBit`.
- The result's write fields are renamed `designRemoteModeWrites` and
  `designParameterWrites`: design properties, not measured receipt counts.
- `tests/fixture_contract.sh` now refuses `live-meter` in default-build help.
- Left as is: each meter reply is parsed twice (once in `validate_response`,
  once in `meter_body`); harmless.

## Open

- First named-device run of `live-meter` (main session only; hardware lock).
- If the device answers Ping only after transmit remote mode, the meter will
  time out at sequence 1; that is a finding, not a reason to add a mode write.
- Legalab runbook `docs/runbooks/dcx-runtime-evidence-rehearsal.md` line 238
  still says `control snapshot` is query-only; it writes `3F 08` before Dump0
  and `3F 04` after Dump1. Legalab owns that edit.
