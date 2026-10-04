# Child diagnostics source advance

Authority: R-N13, `dec-native-studio-20261004`, and
`dec-legalab-reset-20261004`. Work is confined to the isolated sprint clone on
`fix/dcx-child-diagnostics-20261004`; original candidate and owner checkouts are
preserved.

Observed from source: Identity Search emits `LiveSearchFailed` with the
`qualification_incomplete` phase only after its 20-attempt loop exhausts.
Snapshot failures in mutation orchestration may be wrapped once by
`TransactionExecutionError::Capture`. The inherited classifier recognized
neither format. The coordinator also grouped launch and output-limit exceptions
with malformed child responses.

Changes recognize the exact incomplete Identity Search message and unwrap one
snapshot-only `Capture(...)` envelope. Inspection remains capped at 4 KiB;
the digest and byte count describe the original bounded stderr buffer. Raw
stderr and stdout do not enter diagnostic IPC. Unsupported phases, wrappers,
extra text, malformed input, and oversized input remain unknown. Fixed launch
and output-limit messages reuse the existing bridge vocabulary. Admitted
mutation leases remain unresolved on these failures; no automatic retry or
apply is introduced.

Synthetic XCTest cases cover source-emitted formats, bare/wrapped parity,
bridge encoding, whole-buffer digest preservation, private-text rejection,
and unsupported envelopes. No injected runner/coordinator test boundary exists;
this change does not introduce one solely to test exception-message mapping.

Unverified in this source lane: compilation, native XCTest execution,
coordinator integration, foreground lifecycle, installed helper IPC, Logic
hosting, and hardware behavior. Root owns validation and the resulting exact
revision receipts. No builds, tests, host commands, device commands, staging,
or commits were executed by this lane.

## Root verification follow-up

Under the same source rulings and `dec-local-first-reapi-20261004`, Sting
validated exact `b4d65378f2758de456d5cf9ed0d674e91365e2c9` at
2026-10-04T06:47:13Z: formatting, Clippy, Cargo workspace tests, CLI fixtures,
eight Bazel test targets, CLI/schema build targets and history secret scan
passed. The original-owner tracked-state digest was unchanged. Private receipt
SHA-256: `84423c6f039f7fd48af13b0e8e761734266419a6eb19789778f3fefad66d96e3`.

PZM's offline native check of `40b8bfb89f15611c12ba21450a0b2dbc6fc9b8cf`
compiled the shared package and passed exactly ten pure diagnostic fixtures;
no runner/timeout fixture ran. Independent log readback corrected a missing
xUnit postcondition at 06:35:16Z. Its Apple tree
`f695baf818784f3794674e452f7bb4feb69bf364` equals the final Rust-test head's
Apple tree. No signing, install, device I/O, durable-lease integration or Logic
qualification follows. Legalab's dated native sprint note retains limitations.

Independent review also approved the EOF regression. Regular advisory workflow
action references now use observed canonical `xoxd-ai/ci-templates` at the same
reviewed revision; GitHub action calls do not follow the old owner redirect.
This metadata correction changes neither protocol nor Apple source behavior.
The exact new head still requires its own source receipt before merge.
