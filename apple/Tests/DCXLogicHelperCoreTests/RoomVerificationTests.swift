import CryptoKit
import DCXLogicBridge
@testable import DCXLogicHelperCore
import Foundation
import XCTest

final class RoomVerificationTests: XCTestCase {
    func testSavedApprovalIsOptInAndPinsTargetCalloutAndExactBytes() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let first = try HelperConfigurationV1(bindingID: "synthetic-room", expectedDeviceAddress: 0,
            ttyPath: "/dev/cu.usbserial-SYNTHETIC", enabledFeatures: [.snapshotCapture])
        let approved = try ApprovedRoomConfiguration(configuration: first, expectation: Data("SYNTHETIC approved manifest".utf8))
        XCTAssertFalse(approved.verifyOnForegroundLaunch)
        XCTAssertFalse(approved.allowExplicitRefreshRequests)
        let file = root.appendingPathComponent("approved-room-configuration.json")
        try approved.save(to: file)
        let restored = try ApprovedRoomConfiguration.load(from: file)
        XCTAssertEqual(restored, approved)
        XCTAssertNoThrow(try restored.requireBinding(to: first))
        let other = try HelperConfigurationV1(bindingID: "synthetic-room", expectedDeviceAddress: 0,
            ttyPath: "/dev/cu.usbserial-OTHER", enabledFeatures: [.snapshotCapture])
        XCTAssertThrowsError(try restored.requireBinding(to: other))
        var tampered = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: file)) as? [String: Any])
        tampered["expectation"] = Data("different".utf8).base64EncodedString()
        try JSONSerialization.data(withJSONObject: tampered).write(to: file)
        XCTAssertThrowsError(try ApprovedRoomConfiguration.load(from: file))
    }

    func testRefreshURLAcceptsNoCallerSuppliedDeviceOrOperation() {
        XCTAssertTrue(ApprovedRoomConfiguration.acceptsRefreshURL(URL(string: "tinyland-dcx://verify-approved-room")!))
        for value in ["tinyland-dcx://verify-approved-room/", "tinyland-dcx://verify-approved-room?tty=other",
                      "tinyland-dcx://verify-approved-room#apply", "tinyland-dcx://verify-approved-room:1",
                      "tinyland-dcx://apply", "https://verify-approved-room"] {
            XCTAssertFalse(ApprovedRoomConfiguration.acceptsRefreshURL(URL(string: value)!))
        }
    }

    func testHeartbeatNeverExtendsActualCaptureLifetime() throws {
        let capture = ReceiptCapture()
        let publisher = RoomVerificationPublisher(startHeartbeats: false, submit: { value in capture.append(value) })
        let expectation = Data("SYNTHETIC approved expectation".utf8)
        let at = Date()
        let receipt = try publisher.install(comparison(expectation), expectation: expectation, capturedAt: at, ttyPath: "/dev/cu.usbserial-SYNTHETIC")
        for seconds in stride(from: 0.0, to: 60.0, by: 0.5) {
            let now = at.addingTimeInterval(seconds)
            let renewed = try XCTUnwrap(receipt.renewed(at: now, capturedAt: at, ttyPresent: true))
            let formatter = ISO8601DateFormatter()
            formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
            let expiry = try XCTUnwrap(formatter.date(from: renewed.expiresAt))
            XCTAssertLessThanOrEqual(expiry.timeIntervalSince(at), 60.001)
            XCTAssertLessThanOrEqual(expiry.timeIntervalSince(now), 2.001)
            XCTAssertEqual(renewed.verifiedAt, receipt.verifiedAt)
        }
        XCTAssertNil(receipt.renewed(at: at.addingTimeInterval(60), capturedAt: at, ttyPresent: true))
        XCTAssertNil(receipt.renewed(at: at.addingTimeInterval(-1), capturedAt: at, ttyPresent: true))
        XCTAssertNil(receipt.renewed(at: at, capturedAt: at, ttyPresent: false))
        publisher.invalidate("helper-left-foreground")
        let invalidated = try XCTUnwrap(capture.values.last)
        XCTAssertEqual(invalidated.status, "invalidated")
        XCTAssertFalse(invalidated.matches)
        XCTAssertEqual(invalidated.invalidationReason, "helper-left-foreground")
    }

    func testWrongExpectationOrSnapshotBindingCannotPublishMatch() throws {
        let expectation = Data("SYNTHETIC approved expectation".utf8)
        let capture = ReceiptCapture()
        let publisher = RoomVerificationPublisher(startHeartbeats: false, submit: { value in capture.append(value) })
        XCTAssertThrowsError(try publisher.install(comparison(expectation), expectation: Data("different".utf8), capturedAt: Date(), ttyPath: "/dev/cu.usbserial-SYNTHETIC"))
        let wrong = comparison(expectation, expectedSnapshot: "sha256/" + String(repeating: "b", count: 64))
        XCTAssertThrowsError(try publisher.install(wrong, expectation: expectation, capturedAt: Date(), ttyPath: "/dev/cu.usbserial-SYNTHETIC"))
        XCTAssertTrue(capture.values.isEmpty)
    }

    func testExplicitForegroundActionCapturesThenVerifiesAndExitInvalidates() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let capture = ReceiptCapture()
        let publisher = RoomVerificationPublisher(startHeartbeats: false, submit: { value in capture.append(value) })
        let expectation = Data("SYNTHETIC approved expectation".utf8)
        let snapshotURL = try XCTUnwrap(Bundle.module.url(forResource: "SYNTHETIC-blank-snapshot", withExtension: "json", subdirectory: "feedback"))
        let snapshot = try Data(contentsOf: snapshotURL)
        let carrier = try BridgeJSONCodec.decoder().decode(CoreSnapshotCarrierV1.self, from: snapshot)
        let output = comparison(expectation, snapshot: carrier.snapshotDigest)
        let backend = RoomBackend(snapshot: snapshot, comparison: try JSONEncoder().encode(output))
        let configuration = try HelperConfigurationV1(bindingID: "synthetic-room", expectedDeviceAddress: 0, ttyPath: "/dev/cu.usbserial-SYNTHETIC", enabledFeatures: [.snapshotCapture])
        let coordinator = DCXHelperCoordinator(locations: .init(fixtureRoot: root), configuration: .success(configuration), coreMIDI: .init(), runner: backend, roomVerification: publisher)
        XCTAssertThrowsError(try coordinator.verifyRoom(expectation: expectation))
        XCTAssertTrue(backend.arguments.isEmpty)
        coordinator.setForeground(true)
        let receipt = try coordinator.verifyRoom(expectation: expectation)
        XCTAssertTrue(receipt.matches)
        XCTAssertEqual(backend.arguments.count, 3)
        XCTAssertEqual(Array(backend.arguments[0].prefix(2)), ["profile", "room-validate"])
        XCTAssertFalse(backend.arguments[0].contains("--tty"))
        XCTAssertEqual(Array(backend.arguments[1].prefix(2)), ["control", "snapshot"])
        XCTAssertTrue(backend.arguments[1].contains("--tty"))
        XCTAssertEqual(Array(backend.arguments[2].prefix(2)), ["profile", "room-verify"])
        XCTAssertFalse(backend.arguments[2].contains("--tty"))
        coordinator.setForeground(false)
        XCTAssertEqual(capture.values.last?.status, "invalidated")
        XCTAssertFalse(capture.values.last?.matches ?? true)
    }

    func testCachedCoordinatorReceiptCannotBypassChangedRecoveryAuthority() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let locations = DCXHelperStorageLocations(fixtureRoot: root)
        let manifest = Data("SYNTHETIC recovery cache admission".utf8)
        let snapshotURL = try XCTUnwrap(Bundle.module.url(forResource: "SYNTHETIC-blank-snapshot", withExtension: "json", subdirectory: "feedback"))
        let snapshot = try Data(contentsOf: snapshotURL)
        let carrier = try BridgeJSONCodec.decoder().decode(CoreSnapshotCarrierV1.self, from: snapshot)
        let backend = RoomBackend(snapshot: snapshot, comparison: try JSONEncoder().encode(comparison(manifest, snapshot: carrier.snapshotDigest)))
        let configuration = try HelperConfigurationV1(bindingID: "synthetic-room", expectedDeviceAddress: 0,
            ttyPath: "/dev/cu.usbserial-SYNTHETIC", enabledFeatures: [.snapshotCapture])
        try configuration.save(to: locations.helperConfigurationURL)
        let receipts = ReceiptCapture()
        let coordinator = DCXHelperCoordinator(locations: locations, configuration: .success(configuration),
            coreMIDI: .init(), runner: backend,
            roomVerification: .init(startHeartbeats: false, submit: { receipts.append($0) }, ttyPresent: { _ in true }))
        coordinator.setForeground(true)
        let first = try coordinator.verifyRoom(expectation: manifest)
        XCTAssertEqual(try coordinator.verifyRoom(expectation: manifest).verifiedAt, first.verifiedAt)
        XCTAssertEqual(backend.arguments.count, 3)
        let brokenLease = locations.planRootURL.appendingPathComponent("mutation-recovery.json")
        try Data("SYNTHETIC malformed foreign recovery lease".utf8).write(to: brokenLease)
        XCTAssertThrowsError(try coordinator.verifyRoom(expectation: manifest))
        XCTAssertEqual(backend.arguments.count, 3)
        XCTAssertEqual(receipts.values.last?.status, "invalidated")
    }

    func testCoordinatorConcurrentTriggersReachOneSerialCaptureBeforeProcessLock() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let manifest = Data("SYNTHETIC shared coordinator capture".utf8)
        let snapshotURL = try XCTUnwrap(Bundle.module.url(forResource: "SYNTHETIC-blank-snapshot", withExtension: "json", subdirectory: "feedback"))
        let snapshot = try Data(contentsOf: snapshotURL)
        let carrier = try BridgeJSONCodec.decoder().decode(CoreSnapshotCarrierV1.self, from: snapshot)
        let backend = RoomBackend(snapshot: snapshot, comparison: try JSONEncoder().encode(comparison(manifest, snapshot: carrier.snapshotDigest)))
        let started = DispatchSemaphore(value: 0), release = DispatchSemaphore(value: 0)
        backend.beforeSnapshot = {
            started.signal()
            guard release.wait(timeout: .now() + 3) == .success else { throw RoomVerificationError.unavailable }
        }
        let gate = RoomVerificationPreparation()
        let configuration = try HelperConfigurationV1(bindingID: "synthetic-room", expectedDeviceAddress: 0,
            ttyPath: "/dev/cu.usbserial-SYNTHETIC", enabledFeatures: [.snapshotCapture])
        let coordinator = DCXHelperCoordinator(locations: .init(fixtureRoot: root), configuration: .success(configuration),
            coreMIDI: .init(), runner: backend, roomVerification: .init(startHeartbeats: false, submit: { _ in }), roomPreparation: gate)
        coordinator.setForeground(true)
        let done = expectation(description: "coordinator triggers returned")
        done.expectedFulfillmentCount = 2
        let results = PreparationResults()
        DispatchQueue.global().async {
            results.append(Result { try coordinator.verifyRoom(expectation: manifest) }); done.fulfill()
        }
        XCTAssertEqual(started.wait(timeout: .now() + 3), .success)
        DispatchQueue.global().async {
            results.append(Result { try coordinator.verifyRoom(expectation: manifest) }); done.fulfill()
        }
        waitForJoin(gate)
        release.signal()
        wait(for: [done], timeout: 3)
        XCTAssertEqual(results.values.count, 2)
        for result in results.values { XCTAssertTrue(try result.get().matches) }
        XCTAssertEqual(backend.arguments.count, 3)
        XCTAssertEqual(backend.arguments.filter { $0.starts(with: ["control", "snapshot"]) }.count, 1)
    }

    func testConcurrentMatchingTriggersShareOneCaptureAndResult() throws {
        let gate = RoomVerificationPreparation()
        let started = DispatchSemaphore(value: 0), release = DispatchSemaphore(value: 0)
        let done = expectation(description: "both triggers completed")
        done.expectedFulfillmentCount = 2
        let receipt = try fixtureReceipt()
        let results = PreparationResults()
        DispatchQueue.global().async {
            results.append(Result { try gate.run(key: Data([1]), reuse: { nil }) { _ in
                started.signal()
                guard release.wait(timeout: .now() + 3) == .success else { throw RoomVerificationError.unavailable }
                return receipt
            } })
            done.fulfill()
        }
        XCTAssertEqual(started.wait(timeout: .now() + 3), .success)
        DispatchQueue.global().async {
            results.append(Result { try gate.run(key: Data([1]), reuse: { nil }) { _ in
                XCTFail("Matching trigger started a second capture")
                throw RoomVerificationError.unavailable
            } })
            done.fulfill()
        }
        waitForJoin(gate)
        release.signal()
        wait(for: [done], timeout: 3)
        XCTAssertEqual(results.values.count, 2)
        for value in results.values { XCTAssertEqual(try value.get(), receipt) }
    }

    func testDifferentBindingRefusesAndInvalidationSuppressesInFlightPublication() throws {
        let gate = RoomVerificationPreparation()
        let started = DispatchSemaphore(value: 0), release = DispatchSemaphore(value: 0)
        let done = expectation(description: "invalidated trigger returned")
        let receipt = try fixtureReceipt()
        let results = PreparationResults()
        DispatchQueue.global().async {
            results.append(Result { try gate.run(key: Data([1]), reuse: { nil }) { attempt in
                started.signal()
                guard release.wait(timeout: .now() + 3) == .success else { throw RoomVerificationError.unavailable }
                return try gate.publish(attempt) { XCTFail("Invalidated capture published"); return receipt }
            } })
            done.fulfill()
        }
        XCTAssertEqual(started.wait(timeout: .now() + 3), .success)
        XCTAssertThrowsError(try gate.run(key: Data([2]), reuse: { nil }) { _ in
            XCTFail("Different binding started capture"); return receipt
        })
        gate.invalidate {}
        release.signal()
        wait(for: [done], timeout: 3)
        XCTAssertThrowsError(try XCTUnwrap(results.values.first).get())
    }

    func testConcurrentFailureIsSharedAndDoesNotBecomeCachedSuccess() throws {
        let gate = RoomVerificationPreparation()
        let started = DispatchSemaphore(value: 0), release = DispatchSemaphore(value: 0)
        let done = expectation(description: "both failed triggers returned")
        done.expectedFulfillmentCount = 2
        let results = PreparationResults()
        DispatchQueue.global().async {
            results.append(Result { try gate.run(key: Data([1]), reuse: { nil }) { _ in
                started.signal()
                _ = release.wait(timeout: .now() + 3)
                throw RoomVerificationError.invalidComparison
            } })
            done.fulfill()
        }
        XCTAssertEqual(started.wait(timeout: .now() + 3), .success)
        DispatchQueue.global().async {
            results.append(Result { try gate.run(key: Data([1]), reuse: { nil }) { _ in
                XCTFail("Concurrent failure retried capture"); throw RoomVerificationError.unavailable
            } })
            done.fulfill()
        }
        waitForJoin(gate)
        release.signal()
        wait(for: [done], timeout: 3)
        XCTAssertEqual(results.values.count, 2)
        for result in results.values {
            XCTAssertThrowsError(try result.get()) { XCTAssertEqual($0 as? RoomVerificationError, .invalidComparison) }
        }
        let receipt = try fixtureReceipt()
        XCTAssertEqual(try gate.run(key: Data([1]), reuse: { XCTFail("Failure was cached"); return receipt }) { _ in receipt }, receipt)
    }

    func testFreshCompletedResultReusesCaptureButStaleOrInvalidatedRequiresRead() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        try Data().write(to: root)
        let publisher = RoomVerificationPublisher(startHeartbeats: false, submit: { _ in })
        let manifest = Data("SYNTHETIC freshness".utf8)
        let at = Date()
        let receipt = try publisher.install(comparison(manifest), expectation: manifest, capturedAt: at, ttyPath: root.path)
        let gate = RoomVerificationPreparation()
        _ = try gate.run(key: manifest, reuse: { nil }) { _ in receipt }
        let reused = try gate.run(key: manifest, reuse: { publisher.current(expectation: manifest, now: at.addingTimeInterval(59)) }) { _ in
            XCTFail("Fresh completed capture reread serial"); return receipt
        }
        XCTAssertEqual(reused.verifiedAt, receipt.verifiedAt)
        XCTAssertNil(publisher.current(expectation: manifest, now: at.addingTimeInterval(60)))
        XCTAssertNil(publisher.current(expectation: Data("different".utf8), now: at))
        var capturedAgain = false
        _ = try gate.run(key: manifest, reuse: { publisher.current(expectation: manifest, now: at.addingTimeInterval(60)) }) { _ in
            capturedAgain = true; return receipt
        }
        XCTAssertTrue(capturedAgain)
        gate.invalidate { publisher.invalidate("device-operation") }
        XCTAssertNil(publisher.current(expectation: manifest, now: at))
        _ = try gate.run(key: manifest, reuse: { XCTFail("Invalidated result reused"); return receipt }) { _ in receipt }
        try FileManager.default.removeItem(at: root)
        XCTAssertNil(publisher.current(expectation: manifest, now: at))
    }

    func testOldApprovalSchemaDoesNotAuthorizePeriodicReads() throws {
        let configuration = try HelperConfigurationV1(bindingID: "synthetic-room", expectedDeviceAddress: 0,
            ttyPath: "/dev/cu.usbserial-SYNTHETIC", enabledFeatures: [.snapshotCapture])
        let value = try ApprovedRoomConfiguration(configuration: configuration, expectation: Data("SYNTHETIC".utf8),
            periodicVerificationEnabled: true)
        var object = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(value)) as? [String: Any])
        object.removeValue(forKey: "periodicVerificationEnabled")
        let old = try JSONDecoder().decode(ApprovedRoomConfiguration.self, from: JSONSerialization.data(withJSONObject: object))
        XCTAssertFalse(old.periodicVerificationEnabled)
        XCTAssertTrue(try JSONDecoder().decode(ApprovedRoomConfiguration.self, from: JSONEncoder().encode(value)).periodicVerificationEnabled)
        object["periodicVerificationEnabled"] = "true"
        XCTAssertThrowsError(try JSONDecoder().decode(ApprovedRoomConfiguration.self, from: JSONSerialization.data(withJSONObject: object)))
    }

    func testPeriodicPathRequiresRealReadInsteadOfCompletedCache() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let locations = DCXHelperStorageLocations(fixtureRoot: root)
        let configuration = try HelperConfigurationV1(bindingID: "synthetic-room", expectedDeviceAddress: 0,
            ttyPath: "/dev/cu.usbserial-SYNTHETIC", enabledFeatures: [.snapshotCapture])
        try configuration.save(to: locations.helperConfigurationURL)
        let manifest = Data("SYNTHETIC periodic real capture".utf8)
        let url = try XCTUnwrap(Bundle.module.url(forResource: "SYNTHETIC-blank-snapshot", withExtension: "json", subdirectory: "feedback"))
        let snapshot = try Data(contentsOf: url)
        let carrier = try BridgeJSONCodec.decoder().decode(CoreSnapshotCarrierV1.self, from: snapshot)
        let backend = RoomBackend(snapshot: snapshot, comparison: try JSONEncoder().encode(comparison(manifest, snapshot: carrier.snapshotDigest)))
        let coordinator = DCXHelperCoordinator(locations: locations, configuration: .success(configuration), coreMIDI: .init(),
            runner: backend, roomVerification: .init(startHeartbeats: false, submit: { _ in }, ttyPresent: { _ in true }))
        coordinator.setForeground(true)
        _ = try coordinator.verifyRoom(expectation: manifest)
        _ = try coordinator.verifyRoom(expectation: manifest)
        XCTAssertEqual(backend.arguments.filter { $0.starts(with: ["control", "snapshot"]) }.count, 1)
        _ = try coordinator.verifyBoundRoom(expectation: manifest, forceFresh: true, bindingIsCurrent: { true })
        XCTAssertEqual(backend.arguments.filter { $0.starts(with: ["control", "snapshot"]) }.count, 2)
        XCTAssertFalse(backend.arguments.contains { $0.contains("apply") || $0.contains("rollback") })
    }

    func testChangedSavedBindingDuringComparisonCannotPublishInitialMatch() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let configuration = try HelperConfigurationV1(bindingID: "synthetic-room", expectedDeviceAddress: 0,
            ttyPath: "/dev/cu.usbserial-SYNTHETIC", enabledFeatures: [.snapshotCapture])
        let manifest = Data("SYNTHETIC changed approval".utf8)
        let url = try XCTUnwrap(Bundle.module.url(forResource: "SYNTHETIC-blank-snapshot", withExtension: "json", subdirectory: "feedback"))
        let snapshot = try Data(contentsOf: url)
        let carrier = try BridgeJSONCodec.decoder().decode(CoreSnapshotCarrierV1.self, from: snapshot)
        let backend = RoomBackend(snapshot: snapshot, comparison: try JSONEncoder().encode(comparison(manifest, snapshot: carrier.snapshotDigest)))
        let allowed = BindingFlag()
        backend.beforeComparison = { allowed.disable() }
        let receipts = ReceiptCapture()
        let coordinator = DCXHelperCoordinator(locations: .init(fixtureRoot: root), configuration: .success(configuration), coreMIDI: .init(),
            runner: backend, roomVerification: .init(startHeartbeats: false, submit: { receipts.append($0) }))
        coordinator.setForeground(true)
        XCTAssertThrowsError(try coordinator.verifyBoundRoom(expectation: manifest, forceFresh: true,
            bindingIsCurrent: { allowed.value }))
        XCTAssertFalse(receipts.values.contains { $0.matches })
    }

    func testCoalescedTriggerAddsBindingConstraintBeforePublication() throws {
        let gate = RoomVerificationPreparation()
        let started = DispatchSemaphore(value: 0), release = DispatchSemaphore(value: 0)
        let done = expectation(description: "coalesced saved binding returned")
        done.expectedFulfillmentCount = 2
        let results = PreparationResults(), allowed = BindingFlag()
        let receipt = try fixtureReceipt()
        DispatchQueue.global().async {
            results.append(Result { try gate.run(key: Data([9]), reuse: { nil }) { attempt in
                started.signal(); _ = release.wait(timeout: .now() + 3)
                return try gate.publish(attempt) { receipt }
            } }); done.fulfill()
        }
        XCTAssertEqual(started.wait(timeout: .now() + 3), .success)
        DispatchQueue.global().async {
            results.append(Result { try gate.run(key: Data([9]), reuse: { nil }, bindingIsCurrent: { allowed.value }) { _ in
                XCTFail("joined periodic trigger started another child"); return receipt
            } }); done.fulfill()
        }
        waitForJoin(gate)
        allowed.disable(); release.signal()
        wait(for: [done], timeout: 3)
        for result in results.values { XCTAssertThrowsError(try result.get()) }
    }

    func testSavedConfigurationCannotReadTheDifferentInMemoryTarget() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let memory = try HelperConfigurationV1(bindingID: "synthetic-room", expectedDeviceAddress: 0,
            ttyPath: "/dev/cu.usbserial-SYNTHETIC-A", enabledFeatures: [.snapshotCapture])
        let saved = try HelperConfigurationV1(bindingID: "synthetic-room", expectedDeviceAddress: 0,
            ttyPath: "/dev/cu.usbserial-SYNTHETIC-B", enabledFeatures: [.snapshotCapture])
        let locations = DCXHelperStorageLocations(fixtureRoot: root)
        try saved.save(to: locations.helperConfigurationURL)
        let backend = RoomBackend(snapshot: Data(), comparison: Data())
        let coordinator = DCXHelperCoordinator(locations: locations, configuration: .success(memory),
            coreMIDI: .init(), runner: backend, roomVerification: .init(startHeartbeats: false, submit: { _ in }))
        coordinator.setForeground(true)
        XCTAssertThrowsError(try coordinator.verifyBoundRoom(expectation: Data("SYNTHETIC".utf8), forceFresh: true,
            expectedConfiguration: saved, bindingIsCurrent: { true }))
        XCTAssertTrue(backend.arguments.isEmpty)
    }

    func testSavedBindingChangeStopsCachedHeartbeatWithoutRestamping() throws {
        let allowed = BindingFlag()
        let expectation = Data("SYNTHETIC heartbeat binding".utf8)
        let publisher = RoomVerificationPublisher(startHeartbeats: false, submit: { _ in }, ttyPresent: { _ in true })
        let capturedAt = Date()
        let receipt = try publisher.install(comparison(expectation), expectation: expectation, capturedAt: capturedAt,
            ttyPath: "/SYNTHETIC", bindingIsCurrent: { allowed.value })
        XCTAssertEqual(publisher.current(expectation: expectation)?.verifiedAt, receipt.verifiedAt)
        allowed.disable()
        XCTAssertNil(publisher.current(expectation: expectation))
        XCTAssertNil(publisher.captureDate(expectation: expectation))
    }

    private func waitForJoin(_ gate: RoomVerificationPreparation) {
        let deadline = Date().addingTimeInterval(2)
        while gate.waitingRequestCount == 0 && Date() < deadline { Thread.sleep(forTimeInterval: 0.001) }
        XCTAssertEqual(gate.waitingRequestCount, 1)
    }

    private func fixtureReceipt() throws -> RoomVerificationReceipt {
        let manifest = Data("SYNTHETIC concurrent expectation".utf8)
        return try RoomVerificationPublisher(startHeartbeats: false, submit: { _ in }).install(
            comparison(manifest), expectation: manifest, capturedAt: Date(), ttyPath: "/SYNTHETIC")
    }

    private func comparison(_ expectation: Data, snapshot: String = "sha256/" + String(repeating: "a", count: 64), expectedSnapshot: String? = nil) -> RoomComparison {
        .init(schemaVersion: "dcx.room-comparison/v1", sceneId: "practice-ready", sceneRevision: 1,
              compiledDigest: "sha256/" + String(repeating: "c", count: 64),
              presetDigest: "sha256/" + String(repeating: "d", count: 64),
              expectationDigest: "sha256/" + SHA256.hash(data: expectation).map { String(format: "%02x", $0) }.joined(),
              snapshotDigest: snapshot, expectedSnapshotDigest: expectedSnapshot ?? snapshot,
              matches: true, readable: true, comparedFields: 67)
    }
}

private final class ReceiptCapture: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: [RoomVerificationReceipt] = []
    var values: [RoomVerificationReceipt] { lock.lock(); defer { lock.unlock() }; return storage }
    func append(_ value: RoomVerificationReceipt) { lock.lock(); storage.append(value); lock.unlock() }
}

private final class RoomBackend: DCXCTLCommandExecuting, @unchecked Sendable {
    private let snapshot: Data
    private let comparison: Data
    var arguments: [[String]] = []
    var beforeSnapshot: (@Sendable () throws -> Void)?
    var beforeComparison: (@Sendable () throws -> Void)?
    init(snapshot: Data, comparison: Data) { self.snapshot = snapshot; self.comparison = comparison }
    func resolveExecutable(for configuration: HelperConfigurationV1) throws -> URL { URL(fileURLWithPath: "/SYNTHETIC/dcxctl") }
    func run(_ invocation: DCXCTLInvocation, timeoutSeconds: UInt8, mutationLock: MutationRecoveryProcessLock?) throws -> DCXCTLProcessResult {
        XCTAssertNotNil(mutationLock)
        arguments.append(invocation.arguments)
        if invocation.arguments.starts(with: ["control", "snapshot"]) { try beforeSnapshot?() }
        if invocation.arguments.starts(with: ["profile", "room-verify"]) { try beforeComparison?() }
        let stdout = invocation.arguments.starts(with: ["profile", "room-validate"]) ? Data("{}".utf8)
            : invocation.arguments.starts(with: ["control", "snapshot"]) ? snapshot : comparison
        return .init(terminationStatus: 0, stdout: stdout, stderr: Data(), durationMilliseconds: 1)
    }
}

private final class PreparationResults: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: [Result<RoomVerificationReceipt, Error>] = []
    var values: [Result<RoomVerificationReceipt, Error>] { lock.lock(); defer { lock.unlock() }; return storage }
    func append(_ value: Result<RoomVerificationReceipt, Error>) { lock.lock(); storage.append(value); lock.unlock() }
}

private final class BindingFlag: @unchecked Sendable {
    private let lock = NSLock()
    private var allowed = true
    var value: Bool { lock.lock(); defer { lock.unlock() }; return allowed }
    func disable() { lock.lock(); allowed = false; lock.unlock() }
}
