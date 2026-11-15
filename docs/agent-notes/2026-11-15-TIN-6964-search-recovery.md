# Search failures on November 15, 2026: recovery in helper 14

Linear: TIN-6964. Source: `origin/main` at `9e03934`, helper build 13. Line
numbers below are on that commit. PZM facts come from the Legalab goals
ledger, `docs/agent-notes/2026-11-15-goals-ledger/state-pzm-20261115.md` and
`dcx.md`. No serial port was opened and no DCX command was sent for this note.

## Records

| Time, UTC | Record | Close |
|---|---|---|
| 06:00:31 | Periodic read. Category timeout, operation search, 10 bytes received. Build 12 latched for 16 hours. | verified |
| 22:28:04 | Explicit read, 42 milliseconds. `ApprovedTargetCaptureError { failure: Transport { operation: Search { sequence: 1 }, source: UnexpectedTrailingFrame { received: 26 }, finish_error: None }, search_attempts: 1, empty_search_timeouts: 0, elapsed: 31.8ms }` | verified |
| about 22:30 | Explicit read. Matched. | verified |

## What the carrier does

| Step | Code at `9e03934` |
|---|---|
| The open samples the input queue once before and once after `configure`. It does not wait. Any queued byte fails the open with `PreexistingInput`. | `crates/dcx-darwin-tty/src/lib.rs:734-740`, `lib.rs:1560-1573` |
| Before the first Search write, queued input with no accepted Search reply is `PreexistingInput`. Nothing is drained. | `lib.rs:834-865` |
| The Search reader takes the first frame that ends in `f7`. It strips a frame equal to the 8-byte request as an echo. It does not check `f0` or the length here. | `lib.rs:1788-1848`, `lib.rs:1851-1890` |
| A complete read needs 8 to 2048 bytes. Identity checks run later, outside the carrier. | `crates/dcx-transport/src/snapshot.rs:382-390`, `snapshot.rs:898-943` |
| If bytes are queued when the reply ends, the reader reads one more frame. A byte-identical copy is a duplicate. Any other frame is `UnexpectedTrailingFrame`, with its length. | `lib.rs:1763-1785` |
| A Search timeout with bytes is terminal. Only an empty timeout replays. | `snapshot.rs:857-863` |

The kernel logs `IOUSBHostPipe::abortGated` on the adapter's bulk-in endpoint
at every session close, per `2026-11-15-TIN-6964-periodic-read-validation.md`
section 2. So the host does not read the adapter while the port is closed.

## What the records imply

1. At 22:28:04Z no byte was queued at open or before the Search write.
   Otherwise the error would be `PreexistingInput`.
2. After the write a frame of 8 to 26 bytes ending in `f7` arrived. The
   carrier took it as the reply. It was not the 8-byte request echo.
3. A second frame of exactly 26 bytes, ending in `f7`, had begun before the
   first one ended. It differed from the first. A 26-byte partial cut by the
   deadline is ruled out, because the session took 31.8 milliseconds and the
   Search deadline is 475.
4. A byte-identical late duplicate would have been accepted. So a plain
   duplicate Search reply does not explain this record. Either the first frame
   was not this Search's reply, or the device sent two different 26-byte frames.
5. At 06:00:31Z a Search reply stopped 10 bytes in. 10 plus 16 is 26. If the
   other 16 bytes reached the adapter after that close, they would wait there
   until the next open. They would then arrive just after the open's one queue
   sample, inside the first Search window, and be taken as the reply. That
   fits points 1 to 4. The bytes were not kept, so it is not shown.
6. The 22:28:04Z failure set the input discard at close, `lib.rs:818-826`,
   `lib.rs:988-996` and `lib.rs:876`. The next read matched. That fits a one-time residue. It does
   not prove it.

Verdict on the lane's leading hypothesis: a late duplicate Search reply is
refuted for 22:28:04Z by point 4. A late frame of a different shape, held
across the close, is the reading the code supports best. The 26-byte frame in
the Dump0 window at 03:08:08Z is a separate case. It arrived mid-session after
the Dump0 write, where an open drain cannot reach it. Build 13 already retries
it.

## Change in this branch

| Rule | Where |
|---|---|
| After `configure` and before any write, the open discards input without parsing until one continuous 50 millisecond quiet window passes. Input that never goes quiet ends the open at its 500 millisecond deadline. No reply of the new session can be lost, since nothing was written. After the open, queued input still blocks a write. | `crates/dcx-darwin-tty/src/lib.rs` |
| A verified-close Search timeout after some bytes, or a Search transport failure, retries on the next 30 second period with its own count. Three in a row stop the reader with `periodic-search-read-cap-reached` and a resume time from the build 13 cooldown. | `apple/Sources/DCXLogicHelperCore/` |
| A Search failure with an unknown or failed close still maps to `.failed` and latches with no timer. So do Search validation, response limit, budget and qualification failures. | `ForegroundHelperRuntime.swift` |
| Dump0 and Dump1 validation is unchanged. No write is replayed. | none |
| Helper build 14, version 0.1.13. | `apple/project.yml` |

## What stays unknown

- The bytes of all three records. A bench read with raw bytes kept would
  settle point 5.
- Whether 50 milliseconds covers the adapter's delivery delay after open. It
  is a bounded allowance, not a measurement.
- Why Search 1 at 22:28:04Z got any answer at all, when the first Search of a
  session is usually unanswered.
- Why the 06:00:31Z reply stopped after 10 bytes.
- Whether other verified-close failures should also resume after a cooldown.
  Budget, qualification, response limit and Search validation still latch.
  That is a policy question for the lead.

Authority: Legalab studio lead task for TIN-6964. R-N13.
