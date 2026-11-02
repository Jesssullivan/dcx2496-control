@testable import DCXLogicHelperCore
import Foundation
import XCTest

final class PeriodicRoomVerificationTests: XCTestCase {
    private let at = Date(timeIntervalSince1970: 1_000)
    private let first = Data("SYNTHETIC approved binding".utf8)

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

}
