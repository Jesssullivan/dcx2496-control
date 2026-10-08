# A4: team-signed install and first named-device O4 PEQ round trips

Authority: dec-autonomous-muted-bench-20261007 / R-HOOK-CONVERGENCE-20261004
(R-N11, R-N13). Host petting-zoo-mini, source `44adf4eb04fe5fcb4828c10acf0c2cc8d8ec60d8`
(tree `0fccbff6edcc519cc7fe5a71c2b049fa359c5ebb`), source and build output on
`/Volumes/LegalabCache`, every build/install/serial step under the shared
pzm-bench lockf, 2026-10-08T01:12Z-01:33Z. No lab-1 switch was running. All
downstream speakers muted per the operator ruling; no audio was played and no
acoustic claim is made. Raw snapshots, plans and receipts stay private on the
host under `~/.local/state/legalab/dcx2496/a4-20261007/`; none enter Git.

## Build, sign, install

- `just apple-team-signed-bundle` with `DCX_CODESIGN_KEYCHAIN` set to a
  transient keychain materialized from the lab sops store (Developer ID
  Application, team `QP994XQKNH`); the keychain was deleted and the user search
  list restored afterwards. ARCHIVE SUCCEEDED; deep strict verify passes.
  The sops `apple_certificate_password` no longer opens the p12; the empty
  password does (same fallback as lab `probe-darwin-codesign-identity.sh`).
- Artifact SHA-256: helper `d07a26b6...db6f5`, AU `43117edb...a3a74`, embedded
  `dcxctl` `811e2f2e...19725`.
- Installed to `~/Applications/DCXLogicHelper.app`; the previous Sept 1 install
  (helper `f8d3470f...337dd`) is kept as
  `install-rollback/DCXLogicHelper.before-44adf4e.app`. `pluginkit` lists
  `io.tinyland.dcx2496.logic.midi(0.1.1)` at the new path.
- `auval -strict -v aumi DcxC TnLd` in the Aqua user context
  (`launchctl asuser`): AU VALIDATION SUCCEEDED, component version 1.0.1. Over
  plain SSH (Background session) auval cannot find the component, as before.
- Gap: `MARKETING_VERSION`/`CURRENT_PROJECT_VERSION` are still 0.1.1 (2), so the
  new install is distinguishable only by hash. Bump before the next install.

## Serial: Status, Search, Snapshot (unsandboxed `live-control` dcxctl, same source)

- Status: exactly one FTDI callout, no holder, Logic/helper/DCXServer absent.
- Search: ten identities, complete; 100 s. Snapshot: Dump0 1015 B, Dump1 911 B,
  95-101 s each; digest `sha256/531ca64a...2352`, byte-identical to the
  2026-08-30 baseline. The device has not been reconfigured since August.
- Decoded with the DuinoDCX `00b9d70` offsets (unverified except where noted):
  Input C gain = line (Dump0 121 = 0); setup Mute Outs off; input sum off.
  Mutes O1-O6 all read 0 (unmuted). Sources O1 A, O2 B, O3 B, O4 B, O5 C, O6 C.
- Auto Align and +15 V have no direct parameter or dump field in the serial
  protocol (setup parameters are 0x02-0x0B and 0x14-0x18). +15 V is engaged only
  while an Auto Align run forces Input C to mic, so "Input C = line" is the only
  observable proxy; it is not an independent phantom readback.
- **Not as designed:** O5/O6 do not read as muted and O4 is sourced from B, not C.
  Either the front panel was never set as described on Sept 23, or the mute
  offsets/polarity are wrong. Practice routing needs front-panel confirmation,
  or an extension of the reviewed address set to the O5/O6 mutes and O4 source.

## O4 PEQ round trips (first named-device O4 writes)

Each run: fresh rollback snapshot, `control diff`, `control apply` with
complete readback, `control rollback`, then an independent full readback.
The write gate was scoped to "this O4-PEQ-only write leaves every other byte
unchanged" because O5/O6 read unmuted at baseline; that was checked at every stage.

| Run | Change | Apply | Changed Dump1 bytes (offset: before->after) | Rollback | Final readback |
| --- | --- | --- | --- | --- | --- |
| p1 inactive band | O4 band1 frequency code 180->181 (band inactive, PEQ off) | verified, exact, 96 s | 259: 52->53; 909 (trailer): 78->77 | rolled_back, equals baseline, ~1 s | digest = original |
| p2 synthetic notch | one bell 2.5 kHz (code 223), Q 10, -3 dB, slope 0; eq_count 0->1; PEQ on | verified, exact, 102 s | 230: 0->1 (PEQ on); 232: 0->1 (count); 259: 52->95; 262: 20->40; 264: 22->120; 268 (carrier): 8->0; 269: 1->0; 909: 78->52 | rolled_back, equals baseline, ~1 s | digest = original |

Hypotheses from `docs/feedback-suppression.md`, now observed on the named device:

1. Dump1 keeps the modulo-128 trailer balance (byte 909), as Dump0 does.
2. Raising the band count after writing band values does not reset the bands;
   readback was exact.
3. O4 PEQ on/off (Dump1 230), band count (232) and band 1 frequency (259),
   Q (262), gain (264) and slope (269), with the shared 7-of-8 carrier (268),
   match the transcribed layout.

Band 1 kind was unchanged, so its location is still unexercised. O4 mute, bands
2-9 and the other outputs' PEQ remain transcription-only.
