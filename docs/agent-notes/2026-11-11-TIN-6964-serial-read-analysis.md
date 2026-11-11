# DCX serial reads on November 11, 2026: source analysis

Linear: TIN-6964. Evidence: TIN-5385 comment `64c91d0e`, TIN-6964 comment
`4f0dc0fc`. Source: `origin/main` at `7b2af6c`, which carries the Helper 9
lineage `43086d4`. Every line number below is on that commit.

This note answers four questions from the source alone. No hardware was read.
It changes no code, because the source does not support a code change. Each
open cause needs one bench run on the real DCX.

## Answers

| Question | Answer from source | Confidence |
|---|---|---|
| Why does every good read show searchAttempts 2 and searchEmptyTimeouts 1? | The device gave no Search response to the first request of each session. Attempt 2 follows the fixed 5 second gap. The cause is not in host code. | High on the mechanism, unknown on the cause |
| Why can Dump0 or Dump1 time out with 0 or 491 bytes? | The DCX sent no dump frame, or stopped mid-frame, inside a 1.975 second deadline. The 491 figure is not in any record I found. | Medium |
| What does the retry policy do after a zero-byte or partial dump? | Zero bytes with a verified close rearm at 15 seconds, doubling to 300. Any partial count latches the periodic reader until the app leaves the foreground or a binding changes. | High |
| Can six helper windows truncate each other's reads? | No. The windows share one runtime and one timer. Three separate guards serialize every serial open. | High |

## 1. The empty first Search

The capture loop sends attempt 1 at once and waits `REPEAT_SEARCH_GAP`,
5 seconds, before every later attempt
(`crates/dcx-transport/src/snapshot.rs:825-833`, `crates/dcx-transport/src/lib.rs:40`).
A zero-byte timeout continues the loop. Any other error ends it
(`snapshot.rs:857-863`). The approved-target path allows 3 attempts
(`snapshot.rs:62`).

The counter rises only for a Search that ends `TimedOut` with zero received
bytes (`snapshot.rs:143-149`). Two facts narrow what zero means.

- The reader strips an exact echo of the request before it counts bytes
  (`crates/dcx-darwin-tty/src/lib.rs:1867-1876`). An echo with no response is
  still an empty timeout. The test
  `exact_request_echo_without_a_response_is_an_empty_timeout`
  (`crates/dcx-darwin-tty/src/tests.rs:669`) pins this. So the wire may have
  carried the echo. It carried no Search response.
- A late response cannot hide. Before attempt 2 writes, the carrier checks
  the input queue. With no earlier accepted response, any queued byte is a
  terminal `PreexistingInput` (`lib.rs:845-853`, `lib.rs:1008`). Attempt 2
  succeeds on every good read. So nothing arrived in the 5.5 seconds after
  attempt 1.

The host path is the same on every session. Open, termios snapshot, a queue
check, `tcsetattr` with `OptionalActions::Now`, a second queue check, then the
first write (`lib.rs:714-741`, `lib.rs:2155-2176`). It has no delay after the
baud change and no `tcdrain`.

The DCX answered attempt 1 at 22:09:03Z, right after a power cycle. The host
path did not change, so that read argues against a host baud race. It points
at device state left by the previous session. Every capture ends with a
ReceiveDirect write (`snapshot.rs:803-807`). The carrier sends it through
`quiesce_receive_direct_command` (`lib.rs:1058-1098`). A power cycle clears
that state.

The arithmetic matches the receipts. 0.475 seconds of Search deadline, plus
5 seconds of gap, plus 1,926 dump bytes at about 3,840 bytes per second, gives
about 6.0 seconds. The Helper 7 receipt on TIN-6964 records 6,141 to 6,145
milliseconds over four captures.

## 2. Dump timeouts

Each dump gets `SNAPSHOT_DUMP_TIMEOUT`, 2 seconds (`snapshot.rs:56`), less the
25 millisecond cleanup reserve (`lib.rs:45`, `lib.rs:1521-1524`). A full Dump0
is 1,015 bytes and a full Dump1 is 911 bytes
(`crates/dcx-core/src/protocol.rs:17-19`). Either takes about 0.26 seconds on
the wire. Transmit mode is written and Dump0 follows with no gap
(`snapshot.rs:766-781`).

- **0 bytes.** No dump frame began inside 1.975 seconds. An echo may have
  come back and been stripped. Both recorded cases, 21:58:53Z on Dump0 and
  22:09:03Z on Dump1, came within minutes of a host reboot or a DCX power
  cycle.
- **491 bytes.** That count does not appear in TIN-5385 `64c91d0e`, in
  TIN-6964, or in the estate-push notes. If it was seen, the DCX began a frame
  and then went silent for at least 1.7 seconds. 491 bytes is less than half
  of either frame and takes about 0.13 seconds to send. A host reader cannot
  lose bytes on this path. It reads one byte at a time against the kernel
  queue until `f7` or the deadline (`lib.rs:1788-1848`).

Any dump timeout marks the session for input recovery (`lib.rs:988-996`). At
finish, the carrier writes ReceiveDirect, discards input and waits for a
25 millisecond quiet window, then restores termios and closes
(`lib.rs:867-922`, `lib.rs:1190-1277`). The test
`snapshot_dump_failure_is_quiesced_once_before_finish_restores_and_closes`
(`tests.rs:1549`) pins this. So a zero-byte and a partial dump timeout both
reach a verified close by the same route.

## 3. Retry policy after a failed dump

`ApprovedRoomCaptureFailed.permitsPeriodicRetry` accepts a failure only when
all of these hold (`apple/Sources/DCXLogicHelperCore/ApprovedRoomRefresh.swift:115-121`).

- The category is timeout and the operation is Dump0 or Dump1.
- `received` is 0.
- The close was verified and no close failure is recorded.

That maps to `.retryableReadFailure`
(`apple/Sources/DCXLogicHelperCore/ForegroundHelperRuntime.swift:201-203`).
The next read is due after 15 seconds, then 30, 60 and so on, capped at 300
(`apple/Sources/DCXLogicHelperCore/PeriodicRoomVerification.swift:98-101`).

Every other failure maps to `.failed`. That sets `blocked` and clears the due
time (`PeriodicRoomVerification.swift:102-105`). The periodic reader then
stays silent until `stop()` runs on leaving the foreground, a binding change,
or a newer capture from the button, the URL or startup
(`PeriodicRoomVerification.swift:38-46`, `48-72`). A 491-byte dump timeout
would land here, even with a verified close.

Commit `1905d14` widened retry from Dump0 to Dump1 and kept the zero-byte
rule. Widening it to partial counts is a policy change. It is Swift, and its
tests need the native gate.

## 4. Six windows and concurrent reads

Six windows cannot run six readers.

- **One runtime per process.** `HelperAppModel` is a `@StateObject` on the
  `App` struct, not on a window
  (`apple/DCXLogicHelper/DCXLogicHelperApp.swift:7`). It builds one
  `ForegroundHelperRuntime` (`DCXLogicHelperApp.swift:59`). That runtime owns
  one `PeriodicRoomVerification` and one timer
  (`ForegroundHelperRuntime.swift:14-15`, `153-170`). `enterForeground`
  returns at once when already running (`ForegroundHelperRuntime.swift:33-38`).
  `handleRoomRefreshURL` refuses while `verifyingRoom` is set
  (`DCXLogicHelperApp.swift:239`).
- **Single flight.** `RoomVerificationPreparation.run` lets one capture run.
  A caller with the same key waits for it. A caller with another key gets
  `unavailable` (`apple/Sources/DCXLogicHelperCore/RoomVerification.swift:234-267`).
- **Process lock.** Every capture takes `flock(LOCK_EX | LOCK_NB)` on
  `mutation-recovery.lock` in the App Group, or fails `busy`
  (`DCXHelperCoordinator.swift:247-249`,
  `MutationRecoveryLeaseStore.swift:199`, `232`). The child `dcxctl` gets a
  duplicate of that descriptor as stdin, so the lock lives as long as the
  child (`MutationRecoveryLeaseStore.swift:155-169`,
  `DCXCTLProcessRunner.swift:75-76`). This also covers a second helper
  process.
- **Exclusive tty.** The carrier opens the callout and sets `TIOCEXCL`
  (`lib.rs:2121-2141`). A second non-root open gets `EBUSY`, a terminal open
  error, not a split stream.

`dcx-v1.sock.lock` is a different lock. It guards the App Group bridge
listener so that only one process serves `dcx-v1.sock`
(`apple/Sources/DCXLogicBridge/AppGroupBoundary.swift:185-212`). It does not
guard the serial port.

Two gaps remain, neither one a window problem. `TIOCEXCL` is set just after
`open` returns, so a second opener in that window shares the port. A root
process ignores `TIOCEXCL`. Only a `dcxctl` started outside the helper, as
root or in that window, could split a read.

The six windows come from `WindowGroup` with `.onOpenURL`
(`DCXLogicHelperApp.swift:10-14`). macOS opens a new window of a
`WindowGroup` for each external URL event. That is a UI defect and costs no
serial safety. A single `Window` scene or `handlesExternalEvents` would fix
it. That fix is Swift and needs the native gate.

## Recorded lead

On November 2, Helper 10 failed at open with `PreexistingInput { queued: 11 }`
(TIN-6964 comment `6d42560c`). A Dump request is 11 bytes: 8 bytes of
envelope plus 3 data bytes (`protocol.rs:201-204`). A Search request is 8
bytes (`crates/dcx-transport/src/lib.rs:24`). So the queued bytes may be a
late echo of the previous session's Dump request. That fits a link that echoes
requests. It is a lead, not a finding.

## Bench runs that would close the causes

Each is one read-only session on the muted bench under
`legalab-hardware-gate`. Watch `searchAttempts`, `searchEmptyTimeouts` and the
dump byte counts.

| Run | Change | Prediction if device state is the cause |
|---|---|---|
| A | Skip the closing ReceiveDirect for one read, then read again | The next read answers attempt 1 |
| B | Add 250 milliseconds after `configure` | No change in the empty count |
| C | Log raw byte counts before echo stripping | Shows whether the empty attempt carried an echo |

Run A needs an operator ruling. The closing ReceiveDirect is part of the
reviewed close contract. A shorter first-replay gap departs from the
DuinoDCX 5 second cadence (`crates/dcx-transport/src/lib.rs:35-40`) and also
needs a ruling.

## Checks

On sting, inside `nix develop` with rustc 1.98.0 and `CARGO_TARGET_DIR`
under `$TMPDIR`:

- `cargo test -p dcx-transport`: 66 and 6 passed, 0 failed.
- `cargo test -p dcx-darwin-tty`: 59 passed, 0 failed.

Not run: `just check`, the Swift tests and any hardware read.
