# A Search reply in the Dump0 window on November 15, 2026

Linear: TIN-6964. Source: `origin/main` at `9ff6e73`, helper build 14. Line
numbers below are on commit `c927aba` of this branch. No serial port was opened and no DCX command
was sent for this note. The records were read on PZM from the helper's
`RoomDiagnostics` folder. Each record's base64 `stderr` was decoded there.

## Records

The eight newest failure records on PZM, newest first. All have helper stage
`refreshChild`, child exit 1 and a verified close.

| Time, UTC | Operation | Category | Received | Search attempts | Empty Search timeouts | Elapsed, seconds |
|---|---|---|---|---|---|---|
| 22:32:59 | Dump0 | validation, `InvalidDumpLength { part: Part0, expected: 1015, actual: 26 }` | none | 2 | 1 | 5.857 |
| 22:32:34 | Dump0 | timeout | 0 | 2 | 1 | 7.843 |
| 22:28:04 | Search 1 | transport, `UnexpectedTrailingFrame { received: 26 }` | none | 1 | 0 | 0.032 |
| 06:00:31 | Search 2 | timeout | 10 | 2 | 1 | 5.984 |
| 05:52:46 | Dump1 | timeout | 0 | 2 | 1 | 8.130 |
| 05:41:36 | Dump0 | timeout | 0 | 2 | 1 | 7.845 |
| 05:24:30 | Dump0 | timeout | 0 | 2 | 1 | 7.827 |
| 05:13:24 | Dump0 | timeout | 0 | 2 | 1 | 7.839 |

The earlier 26-byte Dump0 record, 03:08:08Z in
`2026-11-15-TIN-6964-search-recovery.md`, is older than these eight.

## What the session does

| Step | Code |
|---|---|
| An empty Search timeout replays after the fixed 5 second gap. It does not taint input, and no Search reply is stored. | `crates/dcx-transport/src/snapshot.rs:936`, `crates/dcx-darwin-tty/src/lib.rs:1000-1017` |
| Before each write the carrier samples the input queue once. With no accepted Search reply, any queued byte is `PreexistingInput`. After one, only a byte-identical copy is consumed. Anything else is `UnexpectedTrailingFrame`. | `lib.rs:853-884` |
| After a snapshot Search reply, the reader consumes only copies already queued. It does not wait. | `lib.rs:1794-1816` |
| Transmit mode, then Dump0, are written with no wait. Each write runs the queue check above first. | `snapshot.rs:840-855`, `lib.rs:1130-1163` |
| A dump read takes the first complete frame that ends in `f7` as the response, after one request echo. Frames are not matched to requests by header. | `lib.rs:1900-1940`, before this branch |
| The executor checks the dump length first. A 26-byte frame is `InvalidDumpLength`. | `snapshot.rs:1029-1035` |

A Search reply is exactly 26 bytes with function byte `0x00`. A dump frame is
1015 or 911 bytes with function byte `0x10`. So the two can be told apart by
length and by header byte 6 (`crates/dcx-core/src/protocol.rs:13-19`,
`protocol.rs:438-478`, `protocol.rs:571-585`). The carrier did not do that
before this branch.

## Timing

The 7.83 to 7.85 second Dump0 timeouts are Search 1 at 0.475, the 5 second
gap, Search 2, the Transmit write, 1.975 of empty Dump0 window and the close.
So Search 2 plus the close took about 0.37 to 0.39 seconds there. The
22:32:59Z read took 5.857 seconds. Search 1, the gap, Search 2 and the close
leave between about 0 and 0.25 seconds for the 26-byte frame after the Dump0
write. The four helper 7 good reads of 6.14 seconds give the low end of the
Search 2 and close time. Either way the frame ended well inside the 1.975
second Dump0 window.

## Verdict on the hypothesis

The hypothesis says the DCX answered Search 1 late, after its window, and that
answer was taken as Dump0.

- **Confirmed.** A 26-byte frame was taken as the Dump0 frame. The dump
  reader takes the first complete frame and does not match it to the request.
  Any Search reply that arrives after the Dump0 write and before the dump is
  taken this way. That the 26-byte frame was a Search reply is not confirmed.
  The length matches. The record keeps no bytes.
- **Refuted, as a reply arriving on its own time.** A Search 1 reply that came
  any time in the 5 second gap would have failed Search 2 with
  `PreexistingInput`. No record shows that. Search 2 was answered in its
  window in seven of eight records. The 26-byte frame was not queued at the
  check after Search 2, before Transmit or before Dump0. Two of those checks
  would have consumed a byte-identical copy without error. So the frame began
  after the Dump0 write. A reply held for 5.5 seconds that lands within a
  quarter second of Search 2 is unlikely unless Search 2 set it off.
- **Supported, not shown.** The device sent a second Search reply right after
  the one the carrier accepted. It may be its late answer to Search 1 or a copy
  of the answer to Search 2. The boundary checks are single samples about one
  millisecond apart. A second frame a few milliseconds later passes all three
  and lands in the Dump0 window. Byte-identical copies are already a known
  shape, which `reconcile_late_search_response` handles.
- **Supported by 22:28:04Z.** That read failed in Search 1 with
  `UnexpectedTrailingFrame { received: 26 }` after 32 milliseconds.
  `finish_queued_snapshot_search_replays` consumes a queued frame that is
  byte-identical to the accepted reply and refuses any other
  (`lib.rs:1796-1818`). A 26-byte timeout is ruled out at 32 milliseconds. So
  the device sent a complete 26-byte frame that differed from the reply just
  accepted. Two 26-byte frames back to back are possible, and they need not be
  identical. A byte-identity rule would miss such a frame. The discard
  therefore parses the frame as a Search reply for the bound device.
- **Refuted, for these records.** The first Search window is too short. Search
  1 never got a reply inside 5.5 seconds, and Search 2 always got one inside
  0.475. A longer window would not have caught a reply that came after 5.5
  seconds or never came. This change keeps the window.

## Change in this branch

| Rule | Where |
|---|---|
| In a Dump0 or Dump1 window, after this session accepted a Search reply, the carrier discards one complete frame that parses as a Search reply from the bound device. It then keeps reading inside the same deadline. | `crates/dcx-darwin-tty/src/lib.rs:1886-1940`, `lib.rs:1044-1052` |
| A second such frame in the same window becomes the response. The executor refuses it with `InvalidDumpLength`. Any other frame, such as another device's reply, a wrong function byte or a byte above `0x7f`, is refused the same way. | `crates/dcx-transport/src/snapshot.rs:65-73` |
| Dump validation is unchanged. No request is written again. The dump deadline and the 20 second capture budget are unchanged. | none |
| `SnapshotRead` carries the discard count. When the exchange then fails with a carrier error, such as `UnexpectedTrailingFrame` or `EndOfFile`, the session keeps the count and the approved capture reads it through `last_exchange_stale_search_replies`. The approved capture counts it per window. Success and failure stdout carry `staleSearchRepliesDiscarded` with `dump0` and `dump1`. Failure stderr shows `stale_search_replies`. | `snapshot.rs`, `crates/dcxctl/src/live_control.rs`, `crates/dcxctl/src/profile_cli.rs` |
| The schema stays `dcx.approved-room-refresh/v1`. The helper decodes the field when present and refuses a count above one. Older helpers ignore the key. Output without it still decodes. | `apple/Sources/DCXLogicHelperCore/ApprovedRoomRefresh.swift` |
| Helper build 14 and version 0.1.13 are unchanged. Build 14 at `9ff6e73` does not emit the field. A helper built from this branch keeps the number 14 and does. | `apple/project.yml` |

The discard also applies to the dump reads in apply readback, which use the
same carrier. Only the approved capture reports the count.

Approved capture also writes two remote-mode frames, Transmit and
ReceiveDirect. It writes no parameter. This branch adds no write.

## Tests

| Test | Fixture |
|---|---|
| `november_15_search_reply_ahead_of_dump0_is_discarded_and_dump0_still_validates` | Search 1 empty, then a valid Search reply ahead of Dump0. The capture succeeds. Dump0 is written once. |
| `november_15_wrong_frame_ahead_of_dump0_still_fails_as_recorded` | Another device's reply, a wrong function byte or a non-seven-bit byte. Each fails with the 22:32:59Z error text. |
| `november_15_second_search_reply_ahead_of_dump0_is_refused_by_validation` | Two Search replies ahead of Dump0. The second fails with the same text. |
| `dump_reader_reports_a_discarded_search_reply_and_keeps_it_when_not_enabled` | The count on a complete and a timed-out read, and no discard without an accepted Search. |
| `november_15_record_shape_is_unchanged_when_the_carrier_returns_the_26_byte_frame` | The executor reproduces the 22:32:59Z record: 2 attempts, 1 empty timeout. |
| `november_15_discarded_search_reply_is_counted_on_the_dump0_window` and `a_discard_before_a_dump_timeout_is_still_reported_on_failure` | The count reaches the capture and its error. |
| `a_discard_before_a_carrier_error_is_still_reported_on_failure`, `dump_reader_keeps_the_discard_count_when_a_carrier_error_follows` and `carrier_reports_a_discard_ahead_of_a_dump0_carrier_error` | A discard followed by `UnexpectedTrailingFrame` or `EndOfFile` still reports `dump0: 1`. |
| `testStaleSearchReplyCountsAreOptionalAndBounded` and `testFailedEnvelopeCarriesBoundedStaleSearchReplyCounts` | Swift decoding with and without the field, and a count of 2 refused. |

## What stays unknown

- The bytes of the 26-byte frame. A valid Search reply from the bound device
  is the reading the length supports. The next receipts settle it: a match
  with `dump0: 1` shows a discarded reply. A Dump0 validation failure with
  `dump0: 0` shows a frame that was not one.
- Whether the device sends the dump after the stale reply. On November 15 the
  carrier returned before any dump byte was queued. A Dump0 timeout with
  `dump0: 1` would show it did not.
- Whether the frame is Search 1's answer or a copy of Search 2's. The
  22:28:04Z record shows the device can follow an accepted reply with a
  26-byte frame that is not identical to it, so equal bytes cannot be
  assumed. Only wire timestamps from a bench read with raw bytes kept would
  separate the two.
- Why Search 1 goes unanswered. The 2026-11-11 note points at device state
  after the previous session's ReceiveDirect. That is not shown.
- The Dump0 and Dump1 timeouts with 0 bytes. This change does not address
  them. Build 14 retries them.

Authority: Legalab studio lead task for TIN-6964. R-N13. Lexical: XL-ACC-004
