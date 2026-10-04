import DCXLogicBridge
@testable import DCXLogicHelperCore
import Foundation
import XCTest

final class MutationRecoveryLeaseStoreTests: XCTestCase {
    func testExclusivePublicationCannotReplaceAnActiveLease() throws {
        let fixture = try MutationRecoveryFixture(persistPlans: false)
        defer { fixture.remove() }
        try fixture.admitLease()
        XCTAssertThrowsError(try fixture.store.persistNew(fixture.lease)) { error in
            XCTAssertEqual(error as? MutationRecoveryLeaseError, .activeLease)
        }
        XCTAssertEqual(try fixture.store.load(), fixture.lease)
        XCTAssertNil(try fixture.store.loadCompletion())
    }

    func testIndependentStoreLocksExcludeThenAdmitAfterRelease() throws {
        let fixture = try MutationRecoveryFixture(persistPlans: false)
        defer { fixture.remove() }
        let first = try fixture.store.acquireExclusiveLock()
        defer { first.release() }
        XCTAssertThrowsError(try fixture.store.acquireExclusiveLock()) { error in
            XCTAssertEqual(error as? MutationRecoveryLeaseError, .lockBusy)
        }
        first.release()
        let second = try fixture.store.acquireExclusiveLock()
        second.release()
    }

    func testNonBaselineCompletionFailsWithoutDiscardingRecoveryAuthority() throws {
        let fixture = try MutationRecoveryFixture(persistPlans: false)
        defer { fixture.remove() }
        try fixture.admitLease()
        let lock = try fixture.store.acquireExclusiveLock()
        defer { lock.release() }
        let desired = try fixture.desired.summary(target: fixture.configuration.target, validSearchResponses: 10)
        XCTAssertThrowsError(try fixture.store.complete(fixture.lease, verifiedBaseline: desired)) { error in
            XCTAssertEqual(error as? MutationRecoveryLeaseError, .invalidLease)
        }
        XCTAssertEqual(try fixture.store.load(), fixture.lease)
        XCTAssertNil(try fixture.store.loadCompletion())
    }

    func testTerminalReceiptSurvivesStoreRecreationAndActiveLeaseSuppressesHistory() throws {
        let fixture = try MutationRecoveryFixture(persistPlans: false)
        defer { fixture.remove() }
        try fixture.admitLease()
        let lock = try fixture.store.acquireExclusiveLock()
        let verified = try fixture.baseline.summary(target: fixture.configuration.target,
                                                   validSearchResponses: 10, capturedAt: Date(timeIntervalSince1970: 1))
        let completed = try fixture.store.complete(fixture.lease, verifiedBaseline: verified)
        lock.release()
        XCTAssertNil(try fixture.store.load())
        XCTAssertEqual(try fixture.store.loadCompletion(), completed)

        // A newly active ceremony must hide an older terminal receipt.
        try fixture.admitLease()
        let status = try fixture.status(fixture.coordinator(RecoveryFixtureBackend()))
        XCTAssertEqual(status.recovery?.transactionID, fixture.lease.transactionID)
        XCTAssertNil(status.completion)
        XCTAssertEqual(try fixture.store.loadCompletion(), completed)
    }

    func testBroadPermissionsAndSymlinkedLeaseFailClosed() throws {
        let fixture = try MutationRecoveryFixture(persistPlans: false)
        defer { fixture.remove() }
        try fixture.admitLease()
        let leaseURL = fixture.locations.planRootURL.appendingPathComponent("mutation-recovery.json")
        try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: leaseURL.path)
        XCTAssertThrowsError(try fixture.store.load()) { error in
            XCTAssertEqual(error as? MutationRecoveryLeaseError, .invalidLease)
        }
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: leaseURL.path)
        let saved = fixture.root.appendingPathComponent("fixture-lease.json")
        try FileManager.default.moveItem(at: leaseURL, to: saved)
        try FileManager.default.createSymbolicLink(at: leaseURL, withDestinationURL: saved)
        XCTAssertThrowsError(try fixture.store.load()) { error in
            XCTAssertEqual(error as? MutationRecoveryLeaseError, .invalidLease)
        }
    }
}
