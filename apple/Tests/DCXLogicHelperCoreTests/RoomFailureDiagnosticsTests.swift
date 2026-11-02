import CryptoKit
import Darwin
import DCXLogicBridge
@testable import DCXLogicHelperCore
import Foundation
import XCTest

final class RoomFailureDiagnosticsTests: XCTestCase {
    private func directory() throws -> URL {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }
        return root
    }
    private func result(exit: Int32 = 2, stdout: Data = Data("SYNTHETIC stdout".utf8), stderr: Data = Data("SYNTHETIC private stderr".utf8)) -> DCXCTLProcessResult {
        .init(terminationStatus: exit, stdout: stdout, stderr: stderr, durationMilliseconds: 321)
    }
    private func exercise(_ backend: DiagnosticBackend, obstructStorage: Bool = false, observeError: (Error) -> Void = { _ in }) throws -> (RoomFailureDiagnosticV1, URL) {
        let root = try directory()
        let configuration = try HelperConfigurationV1(bindingID: "synthetic-diagnostic", expectedDeviceAddress: 0,
            ttyPath: "/dev/cu.usbserial-SYNTHETIC", enabledFeatures: [.snapshotCapture])
        let publisher = RoomVerificationPublisher(startHeartbeats: false, submit: { _ in }, ttyPresent: { _ in true })
        let coordinator = DCXHelperCoordinator(locations: .init(fixtureRoot: root), configuration: .success(configuration),
            coreMIDI: .init(), runner: backend, roomVerification: publisher)
        try configuration.save(to: root.appendingPathComponent(DCXBridgeContract.helperConfigurationFileName))
        coordinator.setForeground(true)
        if obstructStorage { try Data("SYNTHETIC obstruction".utf8).write(to: root.appendingPathComponent("RoomDiagnostics")) }
        XCTAssertThrowsError(try coordinator.verifyBoundRoom(expectation: Data("SYNTHETIC approval".utf8), forceFresh: true,
            expectedConfiguration: configuration, approvedTargetRefresh: true, bindingIsCurrent: { true })) { observeError($0) }
        XCTAssertNil(publisher.current(expectation: Data("SYNTHETIC approval".utf8)))
        let response = coordinator.handle(try .init(body: .helperStatus(.init())))
        guard case let .helperStatus(status)? = response.body else { throw FixtureError.status }
        XCTAssertNil(status.activeTransactionID)
        return (try XCTUnwrap(status.roomFailureDiagnostic), root)
    }
    func testValidationChildFailureRetainsActualExitWithoutStartingRefresh() throws {
        let backend = DiagnosticBackend(validation: .success(result(exit: 9)), refresh: .success(result()))
        let (value, _) = try exercise(backend)
        XCTAssertEqual(value.stage, .validationChild); XCTAssertEqual(value.failure, .childExit)
        XCTAssertEqual(value.childExitCode, 9); XCTAssertEqual(value.childDurationMilliseconds, 321)
        XCTAssertEqual(value.closeKnown, .notApplicable); XCTAssertEqual(backend.calls, 1)
    }
    func testRefreshChildFailureRetainsPrivateStderrWithoutStatusDisclosure() throws {
        let backend = DiagnosticBackend(refresh: .success(result()))
        let (value, root) = try exercise(backend)
        XCTAssertEqual(value.stage, .refreshChild); XCTAssertEqual(value.failure, .childExit)
        XCTAssertEqual(value.closeKnown, .unknown); XCTAssertEqual(value.childExitCode, 2)
        let metadata = try BridgeJSONCodec.encoder().encode(value)
        XCTAssertFalse(String(decoding: metadata, as: UTF8.self).contains("SYNTHETIC private stderr"))
        let record = try Data(contentsOf: root.appendingPathComponent(try XCTUnwrap(value.privateRecord)))
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: record) as? [String: Any])
        XCTAssertEqual(Data(base64Encoded: try XCTUnwrap(object["stderr"] as? String)), Data("SYNTHETIC private stderr".utf8))
    }
    private func failedEnvelope(closed: Bool = true) -> [String: Any] {
        ["schemaVersion": "dcx.approved-room-refresh/v1", "status": "failed",
         "snapshot": NSNull(), "comparison": NSNull(), "closeVerified": closed,
         "searchAttempts": 2, "searchEmptyTimeouts": 1, "captureElapsedMilliseconds": 7800,
         "failure": ["schemaVersion": "dcx.approved-room-capture-failure/v1", "category": "timeout",
                     "operation": "dump0", "received": 0, "sessionClosedVerified": closed,
                     "closeFailure": closed ? NSNull() as Any : "cleanup" as Any]]
    }
    private func searchContextEnvelope() -> [String: Any] {
        var envelope = failedEnvelope()
        envelope["failure"] = ["schemaVersion": "dcx.approved-room-capture-failure/v1",
            "category": "transport", "operation": "search", "received": NSNull(),
            "sessionClosedVerified": true, "closeFailure": NSNull()]
        envelope["privateReadContext"] = ["schemaVersion": "dcx.search-read-context/v1",
            "subphase": "response", "frameIndex": 0, "prefix": [240] + Array(repeating: 42, count: 25),
            "received": 26, "queued": 6, "delimiter": "absent", "header": "other",
            "observedMonotonicMicroseconds": 100, "deadlineMonotonicMicroseconds": 200]
        return envelope
    }
    func testSearchReadContextIsRetainedOnlyInPrivateRecordAndCannotPermitRetry() throws {
        let envelope = searchContextEnvelope()
        let data = try JSONSerialization.data(withJSONObject: envelope)
        let (value, root) = try exercise(.init(refresh: .success(result(exit: 1, stdout: data))))
        XCTAssertEqual(value.closeKnown, .verified)
        XCTAssertFalse(ApprovedRoomCaptureFailed(failure: try XCTUnwrap(value.captureFailure)).permitsPeriodicRetry)
        let publicData = try BridgeJSONCodec.encoder().encode(value)
        let publicObject = try XCTUnwrap(JSONSerialization.jsonObject(with: publicData) as? [String: Any])
        XCTAssertNil(publicObject["privateReadContext"])
        XCTAssertFalse(String(decoding: publicData, as: UTF8.self).contains("prefix"))
        let record = try Data(contentsOf: root.appendingPathComponent(try XCTUnwrap(value.privateRecord)))
        let stored = try XCTUnwrap(JSONSerialization.jsonObject(with: record) as? [String: Any])
        let context = try XCTUnwrap(stored["privateReadContext"] as? [String: Any])
        XCTAssertEqual(context["prefix"] as? [Int], [240] + Array(repeating: 42, count: 25))
        XCTAssertNil((stored["metadata"] as? [String: Any])?["privateReadContext"])
    }
    func testMalformedSearchPrivateContextCannotSupplyCloseOrRetryAuthority() throws {
        let changes: [(String, Any)] = [("prefix", Array(repeating: 0, count: 27)),
            ("prefix", Array(repeating: 256, count: 26)), ("prefix", Array(repeating: 247, count: 26)),
            ("received", 25), ("queued", 0), ("queued", UInt64(UInt32.max)),
            ("subphase", "prewrite"), ("header", "unknown"), ("header", "search_response"), ("delimiter", "present"),
            ("observedMonotonicMicroseconds", -1), ("schemaVersion", "unknown")]
        for (key, invalid) in changes {
            var object = searchContextEnvelope()
            var context = try XCTUnwrap(object["privateReadContext"] as? [String: Any])
            context[key] = invalid; object["privateReadContext"] = context
            XCTAssertThrowsError(try ApprovedRoomFailureEnvelope.decodeIfFailed(
                JSONSerialization.data(withJSONObject: object), terminationStatus: 1), key)
        }
        var wrongOperation = searchContextEnvelope()
        wrongOperation["failure"] = failedEnvelope()["failure"]
        XCTAssertThrowsError(try ApprovedRoomFailureEnvelope.decodeIfFailed(
            JSONSerialization.data(withJSONObject: wrongOperation), terminationStatus: 1))
        XCTAssertNil(try ApprovedRoomFailureEnvelope.decodeIfFailed(
            JSONSerialization.data(withJSONObject: failedEnvelope()), terminationStatus: 1)?.privateReadContext)
        var lateObservation = searchContextEnvelope()
        var context = try XCTUnwrap(lateObservation["privateReadContext"] as? [String: Any])
        context["observedMonotonicMicroseconds"] = 201
        lateObservation["privateReadContext"] = context
        XCTAssertEqual(try ApprovedRoomFailureEnvelope.decodeIfFailed(
            JSONSerialization.data(withJSONObject: lateObservation), terminationStatus: 1)?.privateReadContext?.observedMonotonicMicroseconds, 201)
        for (function, label) in [(0, "search_response"), (0x40, "search_request")] {
            var classified = searchContextEnvelope()
            var context = try XCTUnwrap(classified["privateReadContext"] as? [String: Any])
            context["prefix"] = [240, 0, 32, 50, 0, 14, function] + Array(repeating: 0, count: 19)
            context["header"] = label
            classified["privateReadContext"] = context
            XCTAssertEqual(try ApprovedRoomFailureEnvelope.decodeIfFailed(
                JSONSerialization.data(withJSONObject: classified), terminationStatus: 1)?.privateReadContext?.header, label)
        }
    }
    func testTypedEmptyDumpTimeoutRetainsActualCloseAndNeverPublishesReadiness() throws {
        let data = try JSONSerialization.data(withJSONObject: failedEnvelope())
        let (value, _) = try exercise(.init(refresh: .success(result(exit: 1, stdout: data))))
        XCTAssertEqual(value.stage, .refreshChild); XCTAssertEqual(value.failure, .childExit)
        XCTAssertEqual(value.childExitCode, 1); XCTAssertEqual(value.closeKnown, .verified)
        XCTAssertEqual(value.captureFailure?.category, .timeout)
        XCTAssertEqual(value.captureFailure?.operation, .dump0)
        XCTAssertEqual(value.captureFailure?.received, 0)
        XCTAssertTrue(try XCTUnwrap(value.captureFailure).sessionClosedVerified)
    }
    func testOnlyTypedVerifiedCloseSearchOverflowCanRearmNewCapture() throws {
        var original = searchContextEnvelope()
        original.removeValue(forKey: "privateReadContext")
        original["carrierFailureKind"] = "overflow"
        func permitted(_ object: [String: Any], exit: Int32 = 1) throws -> Bool {
            let envelope = try XCTUnwrap(ApprovedRoomFailureEnvelope.decodeIfFailed(
                JSONSerialization.data(withJSONObject: object), terminationStatus: exit))
            return ApprovedRoomCaptureFailed(failure: envelope.failure, carrierFailureKind: envelope.carrierFailureKind).permitsPeriodicRetry
        }
        XCTAssertTrue(try permitted(original))
        let (metadata, root) = try exercise(.init(refresh: .success(result(exit: 1,
            stdout: JSONSerialization.data(withJSONObject: original)))))
        let privateData = try Data(contentsOf: root.appendingPathComponent(try XCTUnwrap(metadata.privateRecord)))
        let stored = try XCTUnwrap(JSONSerialization.jsonObject(with: privateData) as? [String: Any])
        XCTAssertEqual(stored["carrierFailureKind"] as? String, "overflow")
        XCTAssertFalse(String(decoding: try BridgeJSONCodec.encoder().encode(metadata), as: UTF8.self).contains("carrierFailureKind"))
        let subtypes: [Any] = [NSNull(), "system", "Overflow { received: 26, queued: 6 }"]
        for subtype in subtypes {
            var changed = original; changed["carrierFailureKind"] = subtype
            XCTAssertFalse((try? permitted(changed)) ?? false)
        }
        let changes: [(String, Any)] = [("category", "validation"), ("category", "remote_mode"),
            ("operation", "dump0"), ("operation", "dump1"), ("operation", "meter"),
            ("received", 1),
            ("sessionClosedVerified", false), ("closeFailure", "cleanup")]
        for (key, invalid) in changes {
            var changed = original
            var failure = try XCTUnwrap(changed["failure"] as? [String: Any])
            failure[key] = invalid; changed["failure"] = failure
            if key == "sessionClosedVerified" {
                changed["closeVerified"] = false
                failure["closeFailure"] = "cleanup"; changed["failure"] = failure
            }
            XCTAssertFalse((try? permitted(changed)) ?? false, key)
        }
        for exit in [Int32(0), 2, -1] { XCTAssertFalse((try? permitted(original, exit: exit)) ?? false) }
        var privateOnly = original; privateOnly.removeValue(forKey: "carrierFailureKind")
        privateOnly["privateReadContext"] = searchContextEnvelope()["privateReadContext"]
        XCTAssertFalse(try permitted(privateOnly), "Private forensic context cannot authorize a retry")
    }
    func testTypedFailedCloseRemainsUnknownAndCannotBecomeFreshCapture() throws {
        let data = try JSONSerialization.data(withJSONObject: failedEnvelope(closed: false))
        let (value, _) = try exercise(.init(refresh: .success(result(exit: 1, stdout: data))))
        XCTAssertEqual(value.closeKnown, .unknown)
        XCTAssertEqual(value.captureFailure?.closeFailure, "cleanup")
        XCTAssertFalse(try XCTUnwrap(value.captureFailure).sessionClosedVerified)
    }
    func testLegacyFailedEnvelopeRetainsUnknownClose() throws {
        var object = failedEnvelope(); object.removeValue(forKey: "failure"); object["closeVerified"] = NSNull()
        let (value, _) = try exercise(.init(refresh: .success(result(exit: 1, stdout: try JSONSerialization.data(withJSONObject: object)))))
        XCTAssertEqual(value.closeKnown, .unknown); XCTAssertNil(value.captureFailure)
    }
    func testConflictingUnknownAndMalformedFailureEvidenceCannotClaimClose() throws {
        let changes: [(inout [String: Any]) -> Void] = [
            { $0["closeVerified"] = false },
            { $0["snapshot"] = ["synthetic": true] },
            { $0["comparison"] = ["matches": true] },
            { $0["searchAttempts"] = 4 },
            { $0["captureElapsedMilliseconds"] = 30_001 },
            { var f = $0["failure"] as! [String: Any]; f["operation"] = "unknown"; $0["failure"] = f },
            { var f = $0["failure"] as! [String: Any]; f["received"] = -1; $0["failure"] = f },
            { var f = $0["failure"] as! [String: Any]; f["received"] = 2049; $0["failure"] = f },
            { var f = $0["failure"] as! [String: Any]; f["received"] = NSNull(); $0["failure"] = f },
            { var f = $0["failure"] as! [String: Any]; f["schemaVersion"] = "unknown"; $0["failure"] = f },
            { var f = $0["failure"] as! [String: Any]; f["category"] = "unknown"; $0["failure"] = f },
            { var f = $0["failure"] as! [String: Any]; f["sessionClosedVerified"] = NSNull(); $0["failure"] = f },
            { var f = $0["failure"] as! [String: Any]; f["closeFailure"] = "cleanup"; $0["failure"] = f },
        ]
        for change in changes {
            var object = failedEnvelope(); change(&object)
            let (value, _) = try exercise(.init(refresh: .success(result(exit: 1, stdout: try JSONSerialization.data(withJSONObject: object)))))
            XCTAssertEqual(value.closeKnown, .unknown); XCTAssertNil(value.captureFailure)
        }
    }
    func testFailedEnvelopeWithSuccessfulChildExitCannotClaimClose() throws {
        let data = try JSONSerialization.data(withJSONObject: failedEnvelope())
        let (value, _) = try exercise(.init(refresh: .success(result(exit: 0, stdout: data))))
        XCTAssertEqual(value.closeKnown, .unknown); XCTAssertNil(value.captureFailure)
    }
    func testSuccessfulStatusCannotSmuggleTypedFailureAuthority() throws {
        var object = failedEnvelope(); object["status"] = "match"
        XCTAssertThrowsError(try ApprovedRoomFailureEnvelope.decodeIfFailed(JSONSerialization.data(withJSONObject: object), terminationStatus: 0))
    }
    func testOnlyVerifiedClosedEmptyDump0OrDump1TimeoutPermitsPeriodicRetry() throws {
        func eligible(_ object: [String: Any]) throws -> Bool {
            let failure = try BridgeJSONCodec.decoder().decode(RoomCaptureFailureV1.self,
                from: JSONSerialization.data(withJSONObject: object))
            return ApprovedRoomCaptureFailed(failure: failure).permitsPeriodicRetry
        }
        for operation in ["dump0", "dump1"] {
            var original = try XCTUnwrap(failedEnvelope()["failure"] as? [String: Any])
            original["operation"] = operation
            XCTAssertTrue(try eligible(original))
            for (key, value) in [("category", "transport" as Any), ("category", "validation" as Any),
                                 ("operation", "search" as Any), ("operation", "meter" as Any),
                                 ("received", 1 as Any), ("received", 903 as Any), ("received", NSNull() as Any),
                                 ("sessionClosedVerified", false as Any), ("closeFailure", "cleanup" as Any),
                                 ("schemaVersion", "unknown" as Any)] {
                var changed = original; changed[key] = value
                XCTAssertFalse(try eligible(changed))
            }
        }
    }
    func testCompletedFailurePropagatesRetryEvidenceWhileReadinessStaysInvalid() throws {
        let data = try JSONSerialization.data(withJSONObject: failedEnvelope())
        var error: Error?
        let (value, _) = try exercise(.init(refresh: .success(result(exit: 1, stdout: data))), observeError: { error = $0 })
        XCTAssertTrue(try XCTUnwrap(error as? ApprovedRoomCaptureFailed).permitsPeriodicRetry)
        XCTAssertEqual(value.closeKnown, .verified)
        XCTAssertEqual(value.failure, .childExit)
    }
    func testMalformedSuccessfulRefreshIsDecodeFailureAndCloseUnknown() throws {
        let (value, _) = try exercise(.init(refresh: .success(result(exit: 0))))
        XCTAssertEqual(value.stage, .refreshDecode); XCTAssertEqual(value.failure, .decoding)
        XCTAssertEqual(value.childExitCode, 0); XCTAssertEqual(value.closeKnown, .unknown)
    }
    func testRefreshTimeoutDoesNotReuseValidationExitOrClaimClose() throws {
        let (value, _) = try exercise(.init(refresh: .failure(DCXCTLRunnerError.timedOut)))
        XCTAssertEqual(value.stage, .refreshChild); XCTAssertEqual(value.failure, .timedOut)
        XCTAssertNil(value.childExitCode); XCTAssertNil(value.childDurationMilliseconds); XCTAssertEqual(value.closeKnown, .unknown)
    }
    func testUnknownTerminationStaysDistinctFromConfirmedTimeout() throws {
        let (value, _) = try exercise(.init(refresh: .failure(DCXCTLRunnerError.terminationUnconfirmed)))
        XCTAssertEqual(value.failure, .terminationUnconfirmed); XCTAssertEqual(value.closeKnown, .unknown)
        XCTAssertNil(value.childExitCode)
    }
    func testPersistenceFailureCannotTurnCaptureFailureIntoReadiness() throws {
        let (value, _) = try exercise(.init(refresh: .failure(DCXCTLRunnerError.timedOut)), obstructStorage: true)
        XCTAssertTrue(value.persistenceUnavailable); XCTAssertNil(value.privateRecord); XCTAssertEqual(value.stderrRetainedByteCount, 0)
        XCTAssertEqual(value.failure, .timedOut); XCTAssertEqual(value.closeKnown, .unknown)
    }
    func testStatusWireWithoutNewOptionalFieldStillDecodes() throws {
        let (value, _) = try exercise(.init(refresh: .success(result())))
        let status = HelperStatusResponse(foreground: true, configured: false, target: nil,
            capabilities: [.helperStatus], coreMIDI: .init(commandsName: "SYNTHETIC commands", commandsUniqueID: 1, statusName: "SYNTHETIC status", statusUniqueID: 2, online: false),
            activeTransactionID: nil, roomFailureDiagnostic: value)
        let encoded = try BridgeJSONCodec.encoder().encode(status)
        var object = try XCTUnwrap(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
        object.removeValue(forKey: "roomFailureDiagnostic")
        let decoded = try BridgeJSONCodec.decoder().decode(HelperStatusResponse.self, from: JSONSerialization.data(withJSONObject: object))
        XCTAssertNil(decoded.roomFailureDiagnostic)
        XCTAssertEqual(try BridgeJSONCodec.decoder().decode(HelperStatusResponse.self, from: encoded).roomFailureDiagnostic?.attemptID, value.attemptID)
    }
    func testRetentionBoundsRecordCountBytesAndPermissions() throws {
        let root = try directory(); let store = RoomFailureDiagnosticStore(parent: root)
        var attempt = RoomFailureAttempt(); attempt.stage = .refreshChild
        attempt.child = result(stderr: Data(repeating: 0x61, count: 100_000)); attempt.closeKnown = .unknown
        for _ in 0..<12 {
            var next = RoomFailureAttempt(); next.stage = attempt.stage; next.child = attempt.child; next.closeKnown = .unknown
            let value = store.record(next, error: RoomVerificationError.unavailable)
            XCTAssertFalse(value.persistenceUnavailable); XCTAssertEqual(value.stderrByteCount, 100_000)
            XCTAssertEqual(value.stderrRetainedByteCount, 16 * 1024)
        }
        let folder = root.appendingPathComponent("RoomDiagnostics")
        let files = try FileManager.default.contentsOfDirectory(at: folder, includingPropertiesForKeys: nil)
        XCTAssertEqual(files.count, 8)
        let mode = try FileManager.default.attributesOfItem(atPath: folder.path)[.posixPermissions] as? NSNumber
        XCTAssertEqual(mode?.intValue, 0o700)
        var total = 0
        for file in files {
            let attributes = try FileManager.default.attributesOfItem(atPath: file.path)
            XCTAssertEqual((attributes[.posixPermissions] as? NSNumber)?.intValue, 0o600)
            let data = try Data(contentsOf: file); total += data.count; XCTAssertLessThanOrEqual(data.count, 32 * 1024)
        }
        XCTAssertLessThanOrEqual(total, 8 * 32 * 1024)
    }
    func testRepeatedAttemptCannotOverwriteItsExclusiveRecord() throws {
        let root = try directory(); let store = RoomFailureDiagnosticStore(parent: root); let attempt = RoomFailureAttempt()
        let first = store.record(attempt, error: RoomVerificationError.busy)
        let file = root.appendingPathComponent(try XCTUnwrap(first.privateRecord)); let data = try Data(contentsOf: file)
        XCTAssertTrue(store.record(attempt, error: RoomVerificationError.unavailable).persistenceUnavailable)
        XCTAssertEqual(try Data(contentsOf: file), data)
    }
    func testRepeatedOldestAttemptCannotEvadeExclusiveCreationThroughRetention() throws {
        let root = try directory(); let store = RoomFailureDiagnosticStore(parent: root)
        let attempt = RoomFailureAttempt(); let first = store.record(attempt, error: RoomVerificationError.busy)
        let file = root.appendingPathComponent(try XCTUnwrap(first.privateRecord)); let data = try Data(contentsOf: file)
        for _ in 0..<7 { XCTAssertFalse(store.record(.init(), error: RoomVerificationError.busy).persistenceUnavailable) }
        XCTAssertTrue(store.record(attempt, error: RoomVerificationError.unavailable).persistenceUnavailable)
        XCTAssertEqual(try Data(contentsOf: file), data)
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: file.deletingLastPathComponent().path).count, 8)
    }
    func testSymlinkedBridgeParentCannotRedirectDiagnosticWrites() throws {
        let outer = try directory(); let target = try directory(); let alias = outer.appendingPathComponent("Bridge")
        try FileManager.default.createSymbolicLink(at: alias, withDestinationURL: target)
        let value = RoomFailureDiagnosticStore(parent: alias).record(.init(), error: RoomVerificationError.busy)
        XCTAssertTrue(value.persistenceUnavailable); XCTAssertFalse(FileManager.default.fileExists(atPath: target.appendingPathComponent("RoomDiagnostics").path))
    }
    func testSymlinkedDiagnosticsDirectoryRefusesWithoutTouchingTarget() throws {
        let root = try directory(); let target = try directory(); let alias = root.appendingPathComponent("RoomDiagnostics")
        try FileManager.default.createSymbolicLink(at: alias, withDestinationURL: target)
        XCTAssertTrue(RoomFailureDiagnosticStore(parent: root).record(.init(), error: RoomVerificationError.busy).persistenceUnavailable)
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: target.path), [])
    }
    func testHardlinkedEntryRefusesBeforeRetentionOrCreation() throws {
        let root = try directory(); let store = RoomFailureDiagnosticStore(parent: root)
        let first = store.record(.init(), error: RoomVerificationError.busy)
        let file = root.appendingPathComponent(try XCTUnwrap(first.privateRecord))
        try FileManager.default.linkItem(at: file, to: root.appendingPathComponent("preserved-hardlink"))
        let before = try Data(contentsOf: file)
        XCTAssertTrue(store.record(.init(), error: RoomVerificationError.busy).persistenceUnavailable)
        XCTAssertEqual(try Data(contentsOf: file), before)
    }
    func testUnsafeDirectoryModeRefusesWithoutRepairingPermissions() throws {
        let root = try directory(); let folder = root.appendingPathComponent("RoomDiagnostics")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o755])
        XCTAssertTrue(RoomFailureDiagnosticStore(parent: root).record(.init(), error: RoomVerificationError.busy).persistenceUnavailable)
        XCTAssertEqual((try FileManager.default.attributesOfItem(atPath: folder.path)[.posixPermissions] as? NSNumber)?.intValue, 0o755)
    }
    private enum FixtureError: Error { case status }
}

private final class DiagnosticBackend: DCXCTLCommandExecuting, @unchecked Sendable {
    let validation: Result<DCXCTLProcessResult, Error>
    let refresh: Result<DCXCTLProcessResult, Error>
    var calls = 0
    init(validation: Result<DCXCTLProcessResult, Error> = .success(.init(terminationStatus: 0, stdout: Data(), stderr: Data(), durationMilliseconds: 1)), refresh: Result<DCXCTLProcessResult, Error>) {
        self.validation = validation; self.refresh = refresh
    }
    func resolveExecutable(for configuration: HelperConfigurationV1) throws -> URL { URL(fileURLWithPath: "/SYNTHETIC/dcxctl") }
    func run(_ invocation: DCXCTLInvocation, timeoutSeconds: UInt8, mutationLock: MutationRecoveryProcessLock?) throws -> DCXCTLProcessResult {
        calls += 1
        XCTAssertNotNil(mutationLock)
        if invocation.arguments.starts(with: ["profile", "room-validate"]) { return try validation.get() }
        XCTAssertTrue(invocation.arguments.starts(with: ["profile", "room-refresh"]))
        return try refresh.get()
    }
}
