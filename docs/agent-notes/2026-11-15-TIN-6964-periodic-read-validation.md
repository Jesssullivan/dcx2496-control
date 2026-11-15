# Periodic room reads on November 15, 2026: the validation stop

Linear: TIN-6964. Source: `origin/main` at `b5b9dd3`, which is helper build 12
(version 0.1.11). Every line number below is on that commit unless a row names
another. App source: asfirewire-legalab `origin/main` at `9eae99ad`.

PZM evidence came from file reads and three bounded `log show` calls. No app
was launched or quit. No serial port was opened and no DCX command was sent.
The PZM hold marker was set at 04:51:05Z for the App 62 reboot watch, so the
Mac unit gate for this branch is owed.

## Answers

| Question | Answer | Confidence |
|---|---|---|
| What does category validation mean on Dump0? | A complete frame ended in `f7` inside the Dump0 window, but it was not a 1015-byte Dump0 frame. The record's stderr names `InvalidDumpLength { expected: 1015, actual: 26 }`. 26 is the Search response length. | High |
| Why was it not retried? | The helper retried only timeouts. Every other typed failure became `.failed`, which latches the reader. | High |
| Do reads fail in bursts? | Partly. Failures are scattered among successes. The rate rose from 2 in 30 sessions to 6 in 16 sessions after about 02:58Z. No USB, sleep or helper event explains the rise. | Medium on the count, unknown on the cause |
| Is a read every 30 seconds cheap enough? | Each read holds the port for about 6.1 seconds of a 40 second cycle. 5 of those seconds wait out a first Search that never answers. The app's 60 second freshness rule rules out a longer period. | High |

## 1. Category validation on Dump0

The reader collects bytes until it sees `f7`. It does not require `f0` first
(`crates/dcx-darwin-tty/src/lib.rs:1844`). So any `f7`-terminated frame ends a
Dump0 read as complete. `exchange_validated` sends a complete read to
`validate_response` (`crates/dcx-transport/src/snapshot.rs:919-929`). For a
dump, the frame must be exactly the response limit, 1015 bytes for Dump0
(`snapshot.rs:955`, `crates/dcx-core/src/protocol.rs:17`). It must then decode
as a dump response from the expected device and part (`snapshot.rs:962-985`).
Any of these four checks maps to `Validation` (`snapshot.rs:925`). The summary
gives validation no byte count (`crates/dcx-transport/src/capture_failure.rs:155-164`).
That is why the record says received null.

The record names the check. `dcxctl` writes the error's Debug text to stderr,
and the helper retains up to 16 KiB of it in the private record
(`RoomFailureDiagnostics.swift:45`). The 03:08:08Z
record decodes to:

```text
Error: ApprovedTargetCaptureError { failure: Validation { operation: Dump0, source: InvalidDumpLength { part: Part0, expected: 1015, actual: 26 }, finish_error: None }, search_attempts: 2, empty_search_timeouts: 1, elapsed: 5.856827292s }
```

A 26-byte frame is the exact Search response length
(`crates/dcx-core/src/protocol.rs:13`). The session took 5.857 seconds, about
0.25 seconds less than a good read, so the frame arrived right after the Dump0
request. The carrier consumes a byte-identical late Search reply only before
each write (`lib.rs:834-865`) and at the Search boundary (`lib.rs:1763-1783`).
A duplicate Search reply that starts after the Dump0 write lands in the dump
window. That is the most likely source, but the bytes were not retained. A
Search-shaped frame sent by the DCX in answer to Dump0 fits the record as well.
The record cannot tell the two apart.

Why only timeouts retried: `ApprovedRoomCaptureFailed` grants a retry for a
verified-close Dump0 or Dump1 timeout only
(`apple/Sources/DCXLogicHelperCore/ApprovedRoomRefresh.swift:115-136`). The
runtime maps every other typed failure to `.failed`
(`ForegroundHelperRuntime.swift:227-235`). `.failed` latches with
`periodic-read-failed` and no timer lifts it
(`PeriodicRoomVerification.swift:196-200`). The docs list malformed failures
as latched (`docs/approved-room-refresh.md:59`). The Dump0 read writes nothing,
the session closed verified, and the next read repeats every admission check.
The source shows no reason this case must latch.

## 2. Timeline and pattern

The helper keeps 8 failure records and deletes the oldest
(`RoomFailureDiagnostics.swift:43`). The folder held 8 records, all from
02:46Z on November 15. The November 13 and 14 records are gone. So the
premise of a record timeline since November 13 is false. The kernel logs one
`IOUSBHostPipe::abortGated` on endpoint `0x81` of the USB Serial Converter at
each session close. Those lines give every session end, including successes
and pruned failures. A failure's record `observedAt` plus its child duration
matches its close line within one second.

Session signatures from the close intervals:

| Interval to the close | Meaning |
|---|---|
| 40.0 s | A good read. 30 second delay, 5 second tick, about 6.1 second session. |
| 41.7 s then 23.3 s | An empty Dump0 timeout, then the 15 second retry. |
| 42.0 s | An empty Dump1 timeout. |
| 45.5 s | A good read with three Search attempts, two of them empty. |
| 39.7 s | The validation failure, cut short when the 26-byte frame arrived. |

Every failure, November 13 to 15:

| Time, UTC | Build | Category | Operation | Bytes | Session | Source |
|---|---|---|---|---|---|---|
| Nov 13, time not read | 9 | timeout | not recorded | 491 | not recorded | Lead's fact. Record pruned. No log read for that day. |
| Nov 14 14:44:34 | 9 | timeout, inferred | dump0, inferred | 0, inferred | about 7.8 s | 41.7 s then 23.3 s close intervals. Record pruned. |
| Nov 14 15:03:05 | 9 | timeout | dump1 | 264 | about 8 s | Lead's fact, given as 15:02Z. The close line is 15:03:12.8, 41.9 s after the last good close. The reader stopped. |
| Nov 15 02:41:58 | 12 | timeout, inferred | dump0, inferred | 0, inferred | about 7.8 s | 41.7 s then 23.3 s close intervals. Record pruned. |
| Nov 15 02:46:23 | 12 | timeout | dump0 | 0 | 7850 ms | Record `e6e35481`. Retried and recovered. |
| Nov 15 02:55:37 | 12 | publication unavailable | none | none | 6157 ms | Record `ec20a61e`. Child exit 0. The rebind changed the binding during the read. |
| Nov 15 02:59:07 | 12 | timeout | dump0 | 0 | 7856 ms | Record `efae3d20`. |
| Nov 15 03:00:12 | 12 | timeout | dump1 | 0 | 8128 ms | Record `f2d51cfa`. |
| Nov 15 03:01:57 | 12 | timeout | dump0 | 0 | 7842 ms | Record `c50bd602`. |
| Nov 15 03:02:22 | 12 | timeout | dump0 | 0 | 7859 ms | Record `177c6f7f`. Second in a row. |
| Nov 15 03:05:42 | 12 | timeout | dump0 | 0 | 7853 ms | Record `aba7d7b0`. |
| Nov 15 03:08:08 | 12 | validation | dump0 | null, 26-byte frame | 5869 ms | Record `7074b72a`. The reader stopped at 03:08:13.903Z. |

Every November 15 record has search attempts 2, one empty Search timeout, a
verified close and child exit 1 for the read failures.

What the evidence supports:

- The failures are not one burst. Between 02:59Z and 03:08Z, 10 good reads
  separate the 6 failures. Each good read reset the counters.
- The rate changed. From 02:40:04Z to 02:58:33Z, 2 of 30 sessions failed.
  From 02:59:15Z to 03:08:13Z, 6 of 16 failed. Session ends are the close
  lines.
- Failures do not keep an interval. The gaps were 65, 105, 25, 200 and 146
  seconds.
- Time since launch does not explain them. Build 12 ran 37 and 41 minutes
  before its first two failures. After the 02:55Z relaunch, failures began at
  4 minutes. On November 14, build 9 failed 7 and 25 minutes after its
  first read at 14:37:48Z.
- USB showed no fault. In both windows the only USB Serial Converter lines
  are the one abort per close. There is no reset, suspend or re-enumeration
  line.
- The system did not sleep. powerd logged no sleep or wake transition. It
  logged `darkwake_mode 0 -> 0` every three minutes, screen sharing
  assertions and a `caffeinate` assertion renewed every four minutes.
- The search phase is constant. Every record and every 40.0 second interval
  shows two Search attempts. Three-attempt reads appear on November 14. The
  first Search of each session is never answered. The November 11 note
  traced this to the device and left the cause open.
- The helper logged only the stop line. Its subsystem had one entry from
  November 13 through November 15, at 03:08:13.903Z.

What stays unknown:

- Whether the display slept. powerd's `delayDisplayOff` assertion timed out
  at 02:57:31Z, a minute before the rate rose. No display-off line matched
  the predicate, so display sleep is neither shown nor ruled out.
- Whether the other session's workflows touched audio or USB. coreaudiod
  released audio assertions at 02:54:49Z and 03:01:48Z. Their link to the
  failures is not shown.
- Why the DCX gives no dump reply in 2 seconds, and what sent the 26-byte
  frame. Both need a bench read with the bytes kept.
- The November 13 failure time and session. It was not read.

## 3. Cost of each read

| Setting | Value | Source |
|---|---|---|
| Delay after success | 30 s | `PeriodicRoomVerification.swift:82` |
| Tick | 5 s | `ForegroundHelperRuntime.swift:182` |
| Search response timeout | 0.5 s, 3 attempts, 5 s apart | `crates/dcx-transport/src/snapshot.rs:54,62`, `crates/dcx-transport/src/lib.rs:40` |
| Dump timeout | 2 s | `snapshot.rs:56` |
| Session budget | 20 s | `snapshot.rs:64` |
| Empty timeout retry | 15 s doubling to 300 s, no cap | `PeriodicRoomVerification.swift:181-184` |
| Partial dump retry | next period, stop at 3 in a row | `PeriodicRoomVerification.swift:85,185-195` |
| Capture freshness | 60 s, heartbeat expiry 2 s | `RoomVerification.swift:41-45` |
| App admission | age at most 60 s, expiry within 2 s, `invalidationReason` nil | asfirewire-legalab `ASFWControl/LS56PracticeControl.swift:123-131` |

A good read holds the port about 6.1 seconds of each 40 second cycle. That is
about 90 sessions an hour and about 2,160 a day. Each sends 2 or 3 Searches,
a Transmit mode frame, Dump0, Dump1 and the ReceiveDirect quiesce. 5 seconds
of each session wait out the unanswered first Search. If that Search were
answered, a session would take about 1.1 seconds.

A longer period does not fit the app. The app refuses a receipt older than 60
seconds, and the helper stops renewing at 60 seconds. With a 40 second cycle,
a good read leaves about 20 seconds of margin. A period near 60 seconds would
let the receipt lapse before every read. Reading less often once verified, or
keeping a receipt valid longer, needs the app's 60 second rule changed. That
is an app and Legalab decision, so this branch leaves it alone.

A failed read already backs off. An empty timeout waits 15 seconds, then
doubles. Any failed read invalidates the receipt at once
(`DCXHelperCoordinator.swift:214,378`). So each failure in the 03:00Z window
dropped admission for about 23 seconds.

## Change in this branch

- A verified-close Dump0 or Dump1 validation failure with no byte count
  retries on the next 30 second period. It has its own count. Three in a row
  with no good read stop the reader with
  `periodic-validation-read-cap-reached`. Unknown close, a byte count, a
  Search or meter operation and every other category still latch.
- A stop at the partial or validation cap lifts by itself after 300 seconds.
  Each further cap stop before a good read doubles the pause, up to 1800
  seconds. The reason names the pause.
- Every other failure still latches, as `docs/approved-room-refresh.md:59`
  requires. When the reader stops for any reason, the helper resends the
  invalidated receipt with `invalidationReason` set to
  `periodic-reads-stopped:` and the stop reason. An agent can then open the
  fixed refresh URL. A successful read clears the latch. The resend never
  replaces a live receipt.
- Helper build 13, version 0.1.12.

## Open work

| Item | Owner | Next |
|---|---|---|
| Mac unit gate for this branch | lead | Run `wave7` gate script after the PZM hold clears. |
| First Search never answered | bench | One read with raw bytes kept, under the hardware gate. |
| 26-byte frame in the Dump0 window | source | Record whether a dump-window frame equals the last Search reply, as a flag with no bytes. If it does, consume it like the existing late-reply path. |
| Freshness versus failure rate | app, Legalab | Decide whether a verified-close read failure may keep the prior proof until its 60 second expiry. |

Authority: lead session task, TIN-6964. R-N13.
