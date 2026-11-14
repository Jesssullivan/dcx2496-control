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
        XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: now.addingTimeInterval(10000)).attempt)
        XCTAssertEqual(schedule.blockReason,
            "periodic-partial-read-cap-reached: 3 partial dumps in a row, last received 491 bytes")
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
        XCTAssertNil(schedule.poll(binding: first, capturedAt: nil, now: at.addingTimeInterval(10000)).attempt)
        XCTAssertEqual(schedule.blockReason,
            "periodic-partial-read-cap-reached: 3 partial dumps in a row, last received 7 bytes")
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
