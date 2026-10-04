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
