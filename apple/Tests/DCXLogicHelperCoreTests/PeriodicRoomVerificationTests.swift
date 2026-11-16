@testable import DCXLogicHelperCore
import Foundation
import XCTest

final class PeriodicRoomVerificationTests: XCTestCase {
    private let at = Date(timeIntervalSince1970: 1_000)
    private let first = Data("SYNTHETIC approved binding".utf8)
    /// The capture time of a synthetic kept receipt, thirty seconds before `at`.
    private let kept = Date(timeIntervalSince1970: 970)

    func testDisabledOrAbsentBindingNeverSchedulesDeviceRead() {
        let schedule = PeriodicRoomVerification()
        XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: at).attempt)
        schedule.start()
        for seconds in [0.0, 5, 60, 1000] {
            XCTAssertNil(schedule.poll(binding: nil, capturedAt: nil, now: at.addingTimeInterval(seconds)).attempt)
        }
    }

    func testOneLongCaptureCannotCreateQueueOrSecondReader() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at).attempt)
        for seconds in stride(from: 5.0, through: 101, by: 5) {
            XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(seconds)).attempt)
        }
        schedule.complete(attempt, outcome: .success(capturedAt: at.addingTimeInterval(101)), now: at.addingTimeInterval(101))
        XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(130)).attempt)
        XCTAssertNotNil(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(131)).attempt)
    }

    func testRealManualCaptureDefersDeadlineButRepeatedHeartbeatCannotSlideIt() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        let capture = at.addingTimeInterval(-10)
        XCTAssertNil(schedule.poll(binding: first, capturedAt: capture, now: at).attempt)
        for seconds in [5.0, 29] {
            XCTAssertNil(schedule.poll(binding: first, capturedAt: capture, now: at.addingTimeInterval(seconds)).attempt)
        }
        XCTAssertNotNil(schedule.poll(binding: first, capturedAt: capture, now: at.addingTimeInterval(30)).attempt)
    }

    func testBusyBackoffIsBoundedAndDoesNotBecomeTransportRetry() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        var now = at
        for delay in [15.0, 30, 60, 120, 240, 300, 300] {
            let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
            schedule.complete(attempt, outcome: .busy, now: now)
            XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: now.addingTimeInterval(delay - 0.01)).attempt)
            now = now.addingTimeInterval(delay)
        }
    }

    func testFailedTransportLatchesUntilExplicitNewCapture() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at).attempt)
        schedule.complete(attempt, outcome: .failed, now: at)
        XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(10000)).attempt)
        XCTAssertNil(schedule.poll(binding: first, capturedAt: at.addingTimeInterval(10001), now: at.addingTimeInterval(10001)).attempt)
        XCTAssertNotNil(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(10031)).attempt)
    }

    func testOptOutOrChangedBindingCancelsOldCompletionAndRearm() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at).attempt)
        XCTAssertTrue(schedule.poll(binding: nil, capturedAt: nil, now: at).invalidatesPriorBinding)
        schedule.complete(attempt, outcome: .success(capturedAt: at), now: at)
        XCTAssertNil(schedule.poll(binding: nil, capturedAt: nil, now: at.addingTimeInterval(10000)).attempt)
        let changed = Data("SYNTHETIC changed target".utf8)
        XCTAssertNotNil(schedule.poll(binding: changed, capturedAt: nil, now: at).attempt)
    }

    func testForegroundExitNeverRearmsOldAttempt() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at).attempt)
        schedule.stop()
        schedule.complete(attempt, outcome: .success(capturedAt: at), now: at)
        XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(10000)).attempt)
    }
    func testQueuedOldWorkerCannotAdmitOrPublishAfterStopAndReentry() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        let old = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at).attempt)
        XCTAssertTrue(schedule.isCurrent(old))
        schedule.stop(); schedule.start()
        XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: at).attempt)
        XCTAssertFalse(schedule.isCurrent(old))
        schedule.complete(old, outcome: .failed, now: at)
        let current = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at).attempt)
        XCTAssertTrue(schedule.isCurrent(current))
        schedule.complete(current, outcome: .success(capturedAt: at), now: at)
        XCTAssertTrue(schedule.isCurrent(current), "successful heartbeat binding remains current after completion")
        XCTAssertFalse(schedule.isCurrent(old))
    }

    func testInitialPeriodicBindingDoesNotInvalidateConcurrentStartupFlight() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        let step = schedule.poll(binding: first, capturedAt: nil, now: at)
        XCTAssertFalse(step.invalidatesPriorBinding, "first adoption must join the startup flight")
        XCTAssertNotNil(step.attempt)
        let startupFirst = PeriodicRoomVerification(); startupFirst.start()
        let fresh = startupFirst.poll(binding: first, capturedAt: at, now: at)
        XCTAssertFalse(fresh.invalidatesPriorBinding)
        XCTAssertNil(fresh.attempt, "a completed real startup capture defers periodic reading")
        XCTAssertNotNil(startupFirst.poll(binding: first, capturedAt: at, now: at.addingTimeInterval(30)).attempt)
    }

    func testVerifiedReadFailureBackoffCapsWithoutQueueOrEarlyRetry() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        var now = at
        for delay in [15.0, 30, 60, 120, 240, 300, 300] {
            let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
            XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: now.addingTimeInterval(1000)).attempt)
            schedule.complete(attempt, outcome: .retryableReadFailure, now: now)
            XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: now.addingTimeInterval(delay - 0.001)).attempt)
            now = now.addingTimeInterval(delay)
        }
    }
    func testFreshSuccessResetsReadBackoffAndResumesRefreshAcrossProofLifetime() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        let failed = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at).attempt)
        schedule.complete(failed, outcome: .retryableReadFailure, now: at)
        let recovery = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(15)).attempt)
        schedule.complete(recovery, outcome: .success(capturedAt: at.addingTimeInterval(20)), now: at.addingTimeInterval(20))
        XCTAssertNil(schedule.poll(binding: first, capturedAt: at.addingTimeInterval(20), now: at.addingTimeInterval(49)).attempt)
        let next = try XCTUnwrap(schedule.poll(binding: first, capturedAt: at.addingTimeInterval(20), now: at.addingTimeInterval(50)).attempt)
        schedule.complete(next, outcome: .success(capturedAt: at.addingTimeInterval(55)), now: at.addingTimeInterval(55))
        let later = try XCTUnwrap(schedule.poll(binding: first, capturedAt: at.addingTimeInterval(55), now: at.addingTimeInterval(85)).attempt)
        schedule.complete(later, outcome: .retryableReadFailure, now: at.addingTimeInterval(85))
        XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(99)).attempt)
        XCTAssertNotNil(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(100)).attempt)
    }
    func testUnknownFailureStillLatchesAfterVerifiedReadFailure() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        let firstAttempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at).attempt)
        schedule.complete(firstAttempt, outcome: .retryableReadFailure, now: at)
        let retry = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(15)).attempt)
        schedule.complete(retry, outcome: .failed, now: at.addingTimeInterval(15))
        XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(10000)).attempt)
    }
    func testChangedBindingAndStopDiscardOldRetryCompletion() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        let old = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at).attempt)
        let changed = Data("SYNTHETIC replacement approved binding".utf8)
        XCTAssertTrue(schedule.poll(binding: changed, capturedAt: nil, now: at).invalidatesPriorBinding)
        schedule.complete(old, outcome: .retryableReadFailure, now: at)
        let current = try XCTUnwrap(schedule.poll(binding: changed, capturedAt: nil, now: at).attempt)
        schedule.stop(); schedule.complete(current, outcome: .retryableReadFailure, now: at)
        XCTAssertNil(schedule.poll(binding: changed, capturedAt: nil, now: at.addingTimeInterval(10000)).attempt)
    }
    func testBusyDoesNotResetReadFailureBackoff() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        let firstAttempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at).attempt)
        schedule.complete(firstAttempt, outcome: .retryableReadFailure, now: at)
        let busy = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(15)).attempt)
        schedule.complete(busy, outcome: .busy, now: at.addingTimeInterval(15))
        let again = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(30)).attempt)
        schedule.complete(again, outcome: .retryableReadFailure, now: at.addingTimeInterval(30))
        XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(59)).attempt)
        XCTAssertNotNil(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(60)).attempt)
    }

    // TIN-6964: the November 13 Dump1 timeout stopped after 491 bytes with a verified close.
    func testPartialReadRearmsOnNextPeriodUntilTheCap() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        var now = at
        for _ in 1..<PeriodicRoomVerification.partialReadCap {
            let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
            schedule.complete(attempt, outcome: .partialReadFailure(received: 491), now: now)
            XCTAssertNil(schedule.blockReason)
            XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: now.addingTimeInterval(29.999)).attempt)
            now = now.addingTimeInterval(PeriodicRoomVerification.successDelay)
        }
        let last = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
        schedule.complete(last, outcome: .partialReadFailure(received: 491), now: now)
        XCTAssertEqual(schedule.blockReason,
            "periodic-partial-read-cap-reached: 3 partial dumps in a row, last received 491 bytes; resumes after 300 seconds")
        XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: now.addingTimeInterval(299.999)).attempt)
        XCTAssertNotNil(schedule.poll(binding: first, capturedAt: nil, now: now.addingTimeInterval(300)).attempt)
        XCTAssertNil(schedule.blockReason)
    }
    func testSuccessResetsThePartialReadCount() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        var now = at
        for round in 0..<3 {
            for _ in 1..<PeriodicRoomVerification.partialReadCap {
                let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
                schedule.complete(attempt, outcome: .partialReadFailure(received: UInt32(100 + round)), now: now)
                now = now.addingTimeInterval(PeriodicRoomVerification.successDelay)
            }
            let good = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
            schedule.complete(good, outcome: .success(capturedAt: now), now: now)
            XCTAssertNil(schedule.blockReason)
            now = now.addingTimeInterval(PeriodicRoomVerification.successDelay)
        }
        XCTAssertNotNil(schedule.poll(binding: first, capturedAt: now.addingTimeInterval(-30), now: now).attempt)
    }
    func testEmptyTimeoutAndBusyDoNotResetThePartialReadCount() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        let one = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at).attempt)
        schedule.complete(one, outcome: .partialReadFailure(received: 491), now: at)
        let empty = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(30)).attempt)
        schedule.complete(empty, outcome: .retryableReadFailure, now: at.addingTimeInterval(30))
        let two = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(45)).attempt)
        schedule.complete(two, outcome: .partialReadFailure(received: 12), now: at.addingTimeInterval(45))
        let busy = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(75)).attempt)
        schedule.complete(busy, outcome: .busy, now: at.addingTimeInterval(75))
        let three = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(90)).attempt)
        schedule.complete(three, outcome: .partialReadFailure(received: 7), now: at.addingTimeInterval(90))
        XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(389)).attempt)
        XCTAssertEqual(schedule.blockReason,
            "periodic-partial-read-cap-reached: 3 partial dumps in a row, last received 7 bytes; resumes after 300 seconds")
    }
    func testCapReasonClearsOnExplicitCaptureOrChangedBinding() throws {
        for clearByCapture in [true, false] {
            let schedule = PeriodicRoomVerification(); schedule.start()
            var now = at
            for _ in 0..<PeriodicRoomVerification.partialReadCap {
                let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
                schedule.complete(attempt, outcome: .partialReadFailure(received: 491), now: now)
                now = now.addingTimeInterval(PeriodicRoomVerification.successDelay)
            }
            XCTAssertNotNil(schedule.blockReason)
            if clearByCapture {
                XCTAssertNil(schedule.poll(binding: first, capturedAt: now, now: now).attempt)
                XCTAssertNil(schedule.blockReason)
                XCTAssertNotNil(schedule.poll(binding: first, capturedAt: now, now: now.addingTimeInterval(30)).attempt)
            } else {
                XCTAssertNotNil(schedule.poll(binding: Data("SYNTHETIC changed target".utf8), capturedAt: nil, now: now).attempt)
                XCTAssertNil(schedule.blockReason)
            }
        }
    }
    func testFailedOutcomeStillLatchesWithItsOwnReason() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at).attempt)
        schedule.complete(attempt, outcome: .partialReadFailure(received: 491), now: at)
        let retry = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(30)).attempt)
        schedule.complete(retry, outcome: .failed, now: at.addingTimeInterval(30))
        XCTAssertEqual(schedule.blockReason, PeriodicRoomVerification.failedReason)
        XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(10000)).attempt)
    }

    // TIN-6964: on November 15 a 26-byte frame arrived where the 1015-byte
    // Dump0 belongs, with a verified close. Build 12 stopped on that one read.
    func testValidationFailureRearmsOnNextPeriodUntilItsOwnCap() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        var now = at
        for _ in 1..<PeriodicRoomVerification.validationReadCap {
            let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
            schedule.complete(attempt, outcome: .validationReadFailure, now: now)
            XCTAssertNil(schedule.blockReason)
            XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: now.addingTimeInterval(29.999)).attempt)
            now = now.addingTimeInterval(PeriodicRoomVerification.successDelay)
        }
        let last = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
        schedule.complete(last, outcome: .validationReadFailure, now: now)
        XCTAssertEqual(schedule.blockReason,
            "periodic-validation-read-cap-reached: 3 dump frames in a row failed validation; resumes after 300 seconds")
        XCTAssertEqual(schedule.resumesAt, now.addingTimeInterval(300))
        XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: now.addingTimeInterval(299.999)).attempt)
    }

    func testTheNovemberFifteenSequenceNoLongerStopsTheReader() throws {
        // Five empty timeouts, each followed by a success, then one validation failure.
        let schedule = PeriodicRoomVerification(); schedule.start()
        var now = at
        for _ in 0..<5 {
            let failed = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
            schedule.complete(failed, outcome: .retryableReadFailure, now: now)
            now = now.addingTimeInterval(15)
            let good = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
            schedule.complete(good, outcome: .success(capturedAt: now), now: now)
            now = now.addingTimeInterval(PeriodicRoomVerification.successDelay)
        }
        let validation = try XCTUnwrap(schedule.poll(binding: first, capturedAt: now.addingTimeInterval(-30), now: now).attempt)
        schedule.complete(validation, outcome: .validationReadFailure, now: now)
        XCTAssertNil(schedule.blockReason)
        XCTAssertNotNil(schedule.poll(binding: first, capturedAt: nil, now: now.addingTimeInterval(30)).attempt)
    }

    func testValidationAndPartialCountsAreSeparateAndOnlySuccessResetsThem() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        var now = at
        let outcomes: [PeriodicRoomVerification.Outcome] = [
            .validationReadFailure, .partialReadFailure(received: 491), .retryableReadFailure,
            .validationReadFailure, .partialReadFailure(received: 12), .busy,
        ]
        for outcome in outcomes {
            let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
            schedule.complete(attempt, outcome: outcome, now: now)
            XCTAssertNil(schedule.blockReason, "two of each kind stay under both caps")
            now = now.addingTimeInterval(PeriodicRoomVerification.maximumCapCooldown)
        }
        let third = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
        schedule.complete(third, outcome: .validationReadFailure, now: now)
        XCTAssertEqual(schedule.blockReason?.hasPrefix("periodic-validation-read-cap-reached"), true)

        let reset = PeriodicRoomVerification(); reset.start()
        now = at
        for _ in 0..<2 {
            let attempt = try XCTUnwrap(reset.poll(binding: first, capturedAt: nil, now: now).attempt)
            reset.complete(attempt, outcome: .validationReadFailure, now: now)
            now = now.addingTimeInterval(PeriodicRoomVerification.successDelay)
        }
        let good = try XCTUnwrap(reset.poll(binding: first, capturedAt: nil, now: now).attempt)
        reset.complete(good, outcome: .success(capturedAt: now), now: now)
        now = now.addingTimeInterval(PeriodicRoomVerification.successDelay)
        for _ in 0..<2 {
            let attempt = try XCTUnwrap(reset.poll(binding: first, capturedAt: nil, now: now).attempt)
            reset.complete(attempt, outcome: .validationReadFailure, now: now)
            XCTAssertNil(reset.blockReason)
            now = now.addingTimeInterval(PeriodicRoomVerification.successDelay)
        }
    }

    func testCapStopResumesAfterABoundedDoublingCooldown() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        var now = at
        for cooldown in [300.0, 600, 1200, 1800, 1800] {
            for _ in 0..<PeriodicRoomVerification.validationReadCap {
                let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
                schedule.complete(attempt, outcome: .validationReadFailure, now: now)
                if schedule.blockReason == nil { now = now.addingTimeInterval(PeriodicRoomVerification.successDelay) }
            }
            XCTAssertEqual(schedule.blockReason?.hasSuffix("; resumes after \(Int(cooldown)) seconds"), true)
            XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: now.addingTimeInterval(cooldown - 0.001)).attempt)
            now = now.addingTimeInterval(cooldown)
        }
        // A success restores the first cooldown.
        let good = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
        schedule.complete(good, outcome: .success(capturedAt: now), now: now)
        now = now.addingTimeInterval(PeriodicRoomVerification.successDelay)
        for _ in 0..<PeriodicRoomVerification.validationReadCap {
            let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
            schedule.complete(attempt, outcome: .validationReadFailure, now: now)
            if schedule.blockReason == nil { now = now.addingTimeInterval(PeriodicRoomVerification.successDelay) }
        }
        XCTAssertEqual(schedule.resumesAt, now.addingTimeInterval(300))
    }

    func testLatchedFailureNeverResumesByItself() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        let validation = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at).attempt)
        schedule.complete(validation, outcome: .validationReadFailure, now: at)
        let unknown = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(30)).attempt)
        schedule.complete(unknown, outcome: .failed, now: at.addingTimeInterval(30))
        XCTAssertEqual(schedule.blockReason, PeriodicRoomVerification.failedReason)
        XCTAssertNil(schedule.resumesAt)
        for seconds in [300.0, 1800, 86_400] {
            XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(seconds)).attempt)
        }
    }

    // TIN-6964: on November 15 one verified-close Search timeout after 10 bytes
    // latched build 12 for 16 hours. A Search failure now has its own capped
    // retry, and its cap stop lifts after the cooldown.
    func testSearchFailureRearmsOnNextPeriodUntilItsOwnCapThenResumes() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        var now = at
        for _ in 1..<PeriodicRoomVerification.searchReadCap {
            let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
            schedule.complete(attempt, outcome: .searchReadFailure, now: now)
            XCTAssertNil(schedule.blockReason)
            XCTAssertNil(schedule.resumesAt)
            XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: now.addingTimeInterval(29.999)).attempt)
            now = now.addingTimeInterval(PeriodicRoomVerification.successDelay)
        }
        let last = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
        schedule.complete(last, outcome: .searchReadFailure, now: now)
        XCTAssertEqual(schedule.blockReason,
            "periodic-search-read-cap-reached: 3 Search reads in a row failed with a verified close; resumes after 300 seconds")
        XCTAssertEqual(schedule.resumesAt, now.addingTimeInterval(300))
        XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: now.addingTimeInterval(299.999)).attempt)
        XCTAssertNotNil(schedule.poll(binding: first, capturedAt: nil, now: now.addingTimeInterval(300)).attempt)
        XCTAssertNil(schedule.blockReason)
    }

    func testSearchCountIsSeparateAndOnlySuccessResetsIt() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        var now = at
        let outcomes: [PeriodicRoomVerification.Outcome] = [
            .searchReadFailure, .validationReadFailure, .partialReadFailure(received: 10),
            .searchReadFailure, .retryableReadFailure, .busy, .validationReadFailure,
        ]
        for outcome in outcomes {
            let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
            schedule.complete(attempt, outcome: outcome, now: now)
            XCTAssertNil(schedule.blockReason, "two of each kind stay under every cap")
            now = now.addingTimeInterval(PeriodicRoomVerification.maximumCapCooldown)
        }
        let third = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
        schedule.complete(third, outcome: .searchReadFailure, now: now)
        XCTAssertEqual(schedule.blockReason?.hasPrefix("periodic-search-read-cap-reached"), true)

        let reset = PeriodicRoomVerification(); reset.start()
        now = at
        for _ in 0..<2 {
            let attempt = try XCTUnwrap(reset.poll(binding: first, capturedAt: nil, now: now).attempt)
            reset.complete(attempt, outcome: .searchReadFailure, now: now)
            now = now.addingTimeInterval(PeriodicRoomVerification.successDelay)
        }
        let good = try XCTUnwrap(reset.poll(binding: first, capturedAt: nil, now: now).attempt)
        reset.complete(good, outcome: .success(capturedAt: now), now: now)
        now = now.addingTimeInterval(PeriodicRoomVerification.successDelay)
        for _ in 0..<2 {
            let attempt = try XCTUnwrap(reset.poll(binding: first, capturedAt: nil, now: now).attempt)
            reset.complete(attempt, outcome: .searchReadFailure, now: now)
            XCTAssertNil(reset.blockReason)
            now = now.addingTimeInterval(PeriodicRoomVerification.successDelay)
        }
    }

    func testSearchCapStopsShareTheDoublingCooldownAndNeverLatch() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        var now = at
        for cooldown in [300.0, 600, 1200, 1800, 1800] {
            for _ in 0..<PeriodicRoomVerification.searchReadCap {
                let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
                schedule.complete(attempt, outcome: .searchReadFailure, now: now)
                if schedule.blockReason == nil { now = now.addingTimeInterval(PeriodicRoomVerification.successDelay) }
            }
            XCTAssertEqual(schedule.blockReason?.hasSuffix("; resumes after \(Int(cooldown)) seconds"), true)
            XCTAssertEqual(schedule.resumesAt, now.addingTimeInterval(cooldown))
            now = now.addingTimeInterval(cooldown)
        }
        // A Search failure that closed unknown maps to .failed and still latches.
        let unknown = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
        schedule.complete(unknown, outcome: .failed, now: now)
        XCTAssertEqual(schedule.blockReason, PeriodicRoomVerification.failedReason)
        XCTAssertNil(schedule.resumesAt)
        XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: now.addingTimeInterval(86_400)).attempt)
    }

    // MARK: TIN-6964, ruling 70

    /// A retry under a kept receipt waits three seconds, never back to back.
    func testKeptReceiptRetriesAfterTheSpacingForEachCleanReadKind() throws {
        XCTAssertEqual(PeriodicRoomVerification.keptRetrySpacing, 3)
        let kinds: [PeriodicRoomVerification.Outcome] = [
            .retryableReadFailure, .partialReadFailure(received: 491), .searchReadFailure,
        ]
        let spacing = PeriodicRoomVerification.keptRetrySpacing
        for outcome in kinds {
            let schedule = PeriodicRoomVerification(); schedule.start()
            let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at).attempt)
            XCTAssertEqual(schedule.complete(attempt, outcome: outcome, now: at, keptCapturedAt: kept), .keep)
            XCTAssertNil(schedule.blockReason)
            XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: at).attempt, "not back to back")
            XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(spacing - 0.001)).attempt)
            XCTAssertNotNil(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(spacing)).attempt)
        }
    }

    /// A burst of partial or Search failures under a kept receipt spends the
    /// cap of three no sooner than two spacings after the first failure, and
    /// each spaced retry still counts toward the cap.
    func testSpacedKeptRetriesStillCountTowardTheirCaps() throws {
        let spacing = PeriodicRoomVerification.keptRetrySpacing
        for outcome: PeriodicRoomVerification.Outcome in [.partialReadFailure(received: 491), .searchReadFailure] {
            let schedule = PeriodicRoomVerification(); schedule.start()
            var now = at
            for index in 1...3 {
                let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
                let disposition = schedule.complete(attempt, outcome: outcome, now: now, keptCapturedAt: kept)
                if index < 3 {
                    XCTAssertEqual(disposition, .keep)
                    XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: now.addingTimeInterval(spacing - 0.001)).attempt)
                    now = now.addingTimeInterval(spacing)
                } else {
                    XCTAssertEqual(disposition,
                        .invalidate(RoomVerificationPublisher.periodicStopReason(try XCTUnwrap(schedule.blockReason))))
                }
            }
            XCTAssertEqual(now.timeIntervalSince(at), 2 * spacing)
        }
    }

    func testWithoutAKeptReceiptTheOldScheduleStands() throws {
        let cases: [(PeriodicRoomVerification.Outcome, TimeInterval)] = [
            (.retryableReadFailure, 15), (.partialReadFailure(received: 491), 30), (.searchReadFailure, 30),
        ]
        for (outcome, delay) in cases {
            let schedule = PeriodicRoomVerification(); schedule.start()
            let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at).attempt)
            XCTAssertEqual(schedule.complete(attempt, outcome: outcome, now: at), .none)
            XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(delay - 0.01)).attempt)
            XCTAssertNotNil(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(delay)).attempt)
        }
    }

    func testKeptReceiptEndsOnValidationFailedLatchBusyAndSuccessOutcomes() throws {
        let refused = PeriodicRoomVerification.KeptReceipt.invalidate(PeriodicRoomVerification.keptReceiptRefusedReason)
        let cases: [(PeriodicRoomVerification.Outcome, PeriodicRoomVerification.KeptReceipt)] = [
            (.validationReadFailure, refused),
            (.busy, refused),
            (.success(capturedAt: at), refused),
            (.failed, .invalidate(RoomVerificationPublisher.periodicStopReason(PeriodicRoomVerification.failedReason))),
        ]
        for (outcome, expected) in cases {
            let schedule = PeriodicRoomVerification(); schedule.start()
            let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at).attempt)
            XCTAssertEqual(schedule.complete(attempt, outcome: outcome, now: at, keptCapturedAt: kept), expected)
        }
    }

    func testKeptReceiptEndsWhenTheRetryCapIsSpent() throws {
        let caps: [(PeriodicRoomVerification.Outcome, Int)] = [
            (.partialReadFailure(received: 10), PeriodicRoomVerification.partialReadCap),
            (.searchReadFailure, PeriodicRoomVerification.searchReadCap),
        ]
        for (outcome, cap) in caps {
            let schedule = PeriodicRoomVerification(); schedule.start()
            var now = at
            for index in 1...cap {
                let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
                let disposition = schedule.complete(attempt, outcome: outcome, now: now, keptCapturedAt: kept)
                if index < cap {
                    XCTAssertEqual(disposition, .keep)
                } else {
                    let reason = try XCTUnwrap(schedule.blockReason)
                    XCTAssertEqual(disposition, .invalidate(RoomVerificationPublisher.periodicStopReason(reason)))
                    XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
                }
                now = now.addingTimeInterval(PeriodicRoomVerification.keptRetrySpacing)
            }
        }
    }

    func testKeptReceiptEndsForAStaleOrUnknownAttempt() throws {
        let refused = PeriodicRoomVerification.KeptReceipt.invalidate(PeriodicRoomVerification.keptReceiptRefusedReason)
        let schedule = PeriodicRoomVerification(); schedule.start()
        let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: at).attempt)
        _ = schedule.poll(binding: Data("SYNTHETIC changed binding".utf8), capturedAt: nil, now: at)
        XCTAssertEqual(schedule.complete(attempt, outcome: .searchReadFailure, now: at, keptCapturedAt: kept), refused)
        XCTAssertEqual(schedule.complete(attempt, outcome: .searchReadFailure, now: at, keptCapturedAt: kept), refused,
            "a completion repeated for an attempt no longer active")
        let stopped = PeriodicRoomVerification(); stopped.start()
        let old = try XCTUnwrap(stopped.poll(binding: first, capturedAt: nil, now: at).attempt)
        stopped.stop()
        XCTAssertEqual(stopped.complete(old, outcome: .searchReadFailure, now: at, keptCapturedAt: kept), refused)
    }

    func testAKeptFailureNeitherResetsCountsNorMovesTheCapture() throws {
        let schedule = PeriodicRoomVerification(); schedule.start()
        let capture = at.addingTimeInterval(-30)
        XCTAssertNil(schedule.poll(binding: first, capturedAt: capture, now: at.addingTimeInterval(-30)).attempt)
        let one = try XCTUnwrap(schedule.poll(binding: first, capturedAt: capture, now: at).attempt)
        XCTAssertEqual(schedule.complete(one, outcome: .searchReadFailure, now: at, keptCapturedAt: kept), .keep)
        // The heartbeat keeps reporting the same capture. It never defers or resets anything.
        let two = try XCTUnwrap(schedule.poll(binding: first, capturedAt: capture, now: at.addingTimeInterval(5)).attempt)
        XCTAssertEqual(schedule.complete(two, outcome: .searchReadFailure, now: at.addingTimeInterval(5), keptCapturedAt: kept), .keep)
        let three = try XCTUnwrap(schedule.poll(binding: first, capturedAt: capture, now: at.addingTimeInterval(10)).attempt)
        XCTAssertEqual(schedule.complete(three, outcome: .searchReadFailure, now: at.addingTimeInterval(10), keptCapturedAt: kept),
            .invalidate(RoomVerificationPublisher.periodicStopReason(try XCTUnwrap(schedule.blockReason))))
    }

    /// Three empty dump retries run under one kept receipt and leave the
    /// backoff count alone. The fourth empty timeout ends the receipt and
    /// takes the first backoff build 14 used, 15 seconds, not an inflated one.
    func testKeptEmptyDumpRetriesStopAtTheCapAndDoNotInflateTheBackoff() throws {
        XCTAssertEqual(PeriodicRoomVerification.keptEmptyRetryCap, 3)
        let schedule = PeriodicRoomVerification(); schedule.start()
        var now = at
        for _ in 0..<PeriodicRoomVerification.keptEmptyRetryCap {
            let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
            XCTAssertEqual(schedule.complete(attempt, outcome: .retryableReadFailure, now: now, keptCapturedAt: kept), .keep)
            now = now.addingTimeInterval(PeriodicRoomVerification.keptRetrySpacing)
        }
        var attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
        XCTAssertEqual(schedule.complete(attempt, outcome: .retryableReadFailure, now: now, keptCapturedAt: kept),
            .invalidate(PeriodicRoomVerification.keptReceiptRefusedReason), "the fourth empty timeout ends the receipt")
        XCTAssertNil(schedule.blockReason, "an empty timeout never stops the reader")
        XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: now.addingTimeInterval(14.999)).attempt)
        now = now.addingTimeInterval(15)
        attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
        XCTAssertEqual(schedule.complete(attempt, outcome: .retryableReadFailure, now: now), .none)
        XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: now.addingTimeInterval(29.999)).attempt)
        XCTAssertNotNil(schedule.poll(binding: first, capturedAt: nil, now: now.addingTimeInterval(30)).attempt)
    }

    /// The empty retry cap counts per kept receipt. A newer kept capture
    /// starts a new count, and a success clears it.
    func testKeptEmptyRetryCapCountsPerKeptReceipt() throws {
        let spacing = PeriodicRoomVerification.keptRetrySpacing
        let schedule = PeriodicRoomVerification(); schedule.start()
        var now = at
        func emptyTimeout(_ capture: Date?) throws -> PeriodicRoomVerification.KeptReceipt {
            let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
            defer { now = now.addingTimeInterval(spacing) }
            return schedule.complete(attempt, outcome: .retryableReadFailure, now: now, keptCapturedAt: capture)
        }
        for _ in 0..<3 { XCTAssertEqual(try emptyTimeout(kept), .keep) }
        let newer = kept.addingTimeInterval(20)
        for _ in 0..<3 { XCTAssertEqual(try emptyTimeout(newer), .keep, "a newer kept capture has its own count") }
        XCTAssertEqual(try emptyTimeout(newer), .invalidate(PeriodicRoomVerification.keptReceiptRefusedReason))
        now = now.addingTimeInterval(60)
        let success = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
        schedule.complete(success, outcome: .success(capturedAt: now), now: now)
        now = now.addingTimeInterval(PeriodicRoomVerification.successDelay)
        for _ in 0..<3 { XCTAssertEqual(try emptyTimeout(newer), .keep, "a success clears the count") }
    }

    /// Partial and Search retries under a kept receipt still count toward
    /// their caps, so the third in a row stops the reader as in build 14.
    func testKeptPartialAndSearchRetriesStillCountTowardTheirCaps() throws {
        for outcome: PeriodicRoomVerification.Outcome in [.partialReadFailure(received: 491), .searchReadFailure] {
            let schedule = PeriodicRoomVerification(); schedule.start()
            var now = at
            for _ in 0..<2 {
                let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
                XCTAssertEqual(schedule.complete(attempt, outcome: outcome, now: now, keptCapturedAt: kept), .keep)
                now = now.addingTimeInterval(10)
            }
            let attempt = try XCTUnwrap(schedule.poll(binding: first, capturedAt: nil, now: now).attempt)
            XCTAssertEqual(schedule.complete(attempt, outcome: outcome, now: now), .none)
            XCTAssertNotNil(schedule.blockReason, "the third failure spends the cap with or without a kept receipt")
        }
    }

    func testOnlyOnePeriodicReaderOwnerPerProcessClaim() {
        let claim = PeriodicRoomReaderClaim()
        let firstOwner = NSObject(), secondOwner = NSObject()
        XCTAssertTrue(claim.acquire(firstOwner))
        XCTAssertTrue(claim.acquire(firstOwner))
        for _ in 0..<6 { XCTAssertFalse(claim.acquire(secondOwner)) }
        claim.release(secondOwner)
        XCTAssertFalse(claim.acquire(secondOwner))
        claim.release(firstOwner)
        XCTAssertTrue(claim.acquire(secondOwner))
        XCTAssertFalse(claim.acquire(firstOwner))
    }

    func testConcurrentClaimsAdmitExactlyOneReader() {
        let claim = PeriodicRoomReaderClaim()
        let owners = (0..<32).map { _ in NSObject() }
        let admitted = NSLock()
        var count = 0
        DispatchQueue.concurrentPerform(iterations: owners.count) { index in
            if claim.acquire(owners[index]) {
                admitted.lock(); count += 1; admitted.unlock()
            }
        }
        XCTAssertEqual(count, 1)
    }

    func testFreedOwnerDoesNotHoldTheClaim() {
        let claim = PeriodicRoomReaderClaim()
        autoreleasepool {
            let gone = NSObject()
            XCTAssertTrue(claim.acquire(gone))
        }
        XCTAssertTrue(claim.acquire(NSObject()))
    }

    func testClockStartStopRestartAndStaleGeneration() {
        let claim = PeriodicRoomReaderClaim()
        let clock = PeriodicRoomClockOwnership(claim: claim)
        let runtime = NSObject(), other = NSObject()
        guard let first = clock.begin(owner: runtime) else { return XCTFail("start must take the claim") }
        XCTAssertTrue(clock.isCurrent(first))
        XCTAssertFalse(claim.acquire(other), "a started clock holds the claim")
        clock.end(owner: runtime, generation: first)
        XCTAssertFalse(clock.isCurrent(first), "a stopped clock's late event must not tick")
        XCTAssertTrue(claim.acquire(other), "stop releases the claim")
        claim.release(other)
        guard let second = clock.begin(owner: runtime) else { return XCTFail("restart must take the claim again") }
        XCTAssertNotEqual(first, second)
        XCTAssertFalse(clock.isCurrent(first), "an old generation stays stale after restart")
        XCTAssertTrue(clock.isCurrent(second))
    }

    func testStaleStopDoesNotRetireANewerClock() {
        let claim = PeriodicRoomReaderClaim()
        let clock = PeriodicRoomClockOwnership(claim: claim)
        let runtime = NSObject()
        guard let first = clock.begin(owner: runtime) else { return XCTFail("start must take the claim") }
        guard let second = clock.begin(owner: runtime) else { return XCTFail("the owner may start again") }
        clock.end(owner: runtime, generation: first)
        XCTAssertTrue(clock.isCurrent(second), "a stop for an older generation must not retire the newer one")
        XCTAssertFalse(claim.acquire(NSObject()), "the newer clock still holds the claim")
        clock.end(owner: runtime, generation: second)
        XCTAssertFalse(clock.isCurrent(second))
        XCTAssertTrue(claim.acquire(NSObject()))
    }

    func testClockRefusedWhileAnotherOwnerHoldsTheClaim() {
        let claim = PeriodicRoomReaderClaim()
        let holder = NSObject()
        XCTAssertTrue(claim.acquire(holder))
        let clock = PeriodicRoomClockOwnership(claim: claim)
        XCTAssertNil(clock.begin(owner: NSObject()))
    }
}
