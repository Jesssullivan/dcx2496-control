import CryptoKit
import DCXLogicBridge
@testable import DCXLogicHelperCore
import Foundation
import XCTest

final class MutationRecoveryCoordinatorTests: XCTestCase {
    func testMissingImmutablePlansRejectApplyBeforeAdmissionOrExecution() throws {
        let fixture = try MutationRecoveryFixture(persistPlans: false)
        defer { fixture.remove() }
        let backend = RecoveryFixtureBackend()
        let coordinator = fixture.coordinator(backend)

        let response = coordinator.handle(try fixture.applyRequest())

        XCTAssertEqual(response.error?.code, .mutationNotAdmitted)
        XCTAssertTrue(backend.invocations.isEmpty)
        XCTAssertNil(try fixture.store.load())
    }

    func testAdmittedChildFailuresRetainLeaseAcrossCoordinatorRecreation() throws {
        let outcomes: [(Result<DCXCTLProcessResult, Error>, BridgeErrorPayload.Code)] = [
            (.failure(DCXCTLRunnerError.launchFailed), .childFailed),
            (.failure(DCXCTLRunnerError.timedOut), .childTimedOut),
            (.failure(DCXCTLRunnerError.outputTooLarge), .childFailed),
            (.success(RecoveryFixtureBackend.result(exit: 1, stderr: "private /dev/cu.usbserial-SECRET")), .childFailed),
            (.success(RecoveryFixtureBackend.result(stdout: Data("{invalid private output}".utf8))), .malformedChildResponse),
        ]
        for (outcome, code) in outcomes {
            let fixture = try MutationRecoveryFixture()
            defer { fixture.remove() }
            let backend = RecoveryFixtureBackend(outcome: outcome)
            backend.onRun = { _, timeout, processLock in
                XCTAssertEqual(try fixture.store.load(), fixture.lease)
                XCTAssertEqual(timeout, fixture.configuration.childTimeoutSeconds)
                XCTAssertNotNil(processLock)
                XCTAssertThrowsError(try fixture.store.acquireExclusiveLock()) { error in
                    XCTAssertEqual(error as? MutationRecoveryLeaseError, .lockBusy)
                }
            }
            let coordinator = fixture.coordinator(backend)
            let response = coordinator.handle(try fixture.applyRequest())

            XCTAssertEqual(response.error?.code, code)
            XCTAssertEqual(backend.invocations.count, 1)
            XCTAssertEqual(try fixture.store.load(), fixture.lease)
            XCTAssertNil(try fixture.store.loadCompletion())
            let encoded = try BridgeJSONCodec.encoder().encode(response)
            XCTAssertFalse(String(decoding: encoded, as: UTF8.self).contains("SECRET"))
            XCTAssertFalse(String(decoding: encoded, as: UTF8.self).contains("private"))

            let restarted = fixture.coordinator(RecoveryFixtureBackend())
            let status = try fixture.status(restarted)
            XCTAssertEqual(status.recovery?.transactionID, fixture.lease.transactionID)
            XCTAssertEqual(status.recovery?.baseline, fixture.lease.baseline)
            XCTAssertEqual(status.capabilities, [.helperStatus])
            XCTAssertNil(status.completion)
            let blocked = restarted.handle(try fixture.applyRequest())
            XCTAssertEqual(blocked.error?.code, .mutationNotAdmitted)
            XCTAssertEqual(try fixture.store.load(), fixture.lease)
        }
    }

    func testSuccessfulApplyWithoutPostWriteCaptureKeepsRecovery() throws {
        let fixture = try MutationRecoveryFixture()
        defer { fixture.remove() }
        let output: JSONValue = .object([
            "transaction_id": .string(fixture.lease.transactionID),
            "baseline_snapshot_digest": .string(fixture.baseline.snapshotDigest),
            "desired_snapshot_digest": .string(fixture.desired.snapshotDigest),
            "readback": .null,
            "rollback_available": .bool(true),
            "readback_matches_desired": .bool(false),
        ])
        let backend = RecoveryFixtureBackend(outcome: .success(
            RecoveryFixtureBackend.result(stdout: try BridgeJSONCodec.encoder().encode(output))
        ))
        let response = fixture.coordinator(backend).handle(try fixture.applyRequest())

        XCTAssertNil(response.error)
        guard case let .apply(result)? = response.body else {
            return XCTFail("expected sanitized Apply result")
        }
        XCTAssertNil(result.readback)
        XCTAssertTrue(result.rollbackRequired)
        XCTAssertTrue(result.rollbackAvailable)
        XCTAssertEqual(try fixture.store.load(), fixture.lease)
        XCTAssertNil(try fixture.store.loadCompletion())
    }

    func testRecoveryUsesPinnedConfigurationAfterMutableConfigChanges() throws {
        let fixture = try MutationRecoveryFixture()
        defer { fixture.remove() }
        try fixture.admitLease()
        let changed = try HelperConfigurationV1(
            bindingID: "fixture-successor",
            expectedDeviceAddress: 1,
            ttyPath: "/dev/cu.usbserial-FIXTURE-SUCCESSOR",
            enabledFeatures: [.identitySearch]
        )
        try changed.save(to: fixture.locations.helperConfigurationURL)
        let backend = RecoveryFixtureBackend(outcome: .success(
            RecoveryFixtureBackend.result(stdout: try BridgeJSONCodec.encoder().encode(fixture.baseline))
        ))
        let restarted = fixture.coordinator(backend, configuration: .success(changed))
        XCTAssertThrowsError(try restarted.installConfiguration(changed) { changed }) { error in
            XCTAssertEqual(error as? HelperConfigurationError, .recoveryConfigurationPinned)
        }
        let response = restarted.handle(try fixture.readbackRequest())

        XCTAssertNil(response.error)
        let invocation = try XCTUnwrap(backend.invocations.first)
        XCTAssertEqual(invocation.arguments, [
            "control", "readback", "--tty", fixture.configuration.ttyPath,
            "--expected-device", "0",
        ])
        XCTAssertEqual(backend.resolvedConfigurations, [fixture.configuration])
        XCTAssertNil(try fixture.store.load())
        XCTAssertEqual(try fixture.store.loadCompletion()?.transactionID, fixture.lease.transactionID)
        let again = fixture.coordinator(RecoveryFixtureBackend(), configuration: .success(changed))
        let status = try fixture.status(again)
        XCTAssertNil(status.recovery)
        XCTAssertEqual(status.completion?.verifiedBaseline.digest, fixture.baseline.snapshotDigest)
        XCTAssertEqual(status.target, changed.target)
    }

    func testMismatchedReadbackAndRollbackNeverExecuteOrClearLease() throws {
        let fixture = try MutationRecoveryFixture()
        defer { fixture.remove() }
        try fixture.admitLease()
        let backend = RecoveryFixtureBackend()
        let coordinator = fixture.coordinator(backend)
        let wrongTransaction = try fixture.readbackRequest(transactionID: MutationRecoveryFixture.digest("9"))
        let wrongDesired = try fixture.readbackRequest(expectedDesiredDigest: MutationRecoveryFixture.digest("8"))
        let wrongRollback = try BridgeRequest(body: .rollback(.init(
            target: fixture.configuration.target,
            plan: .init(
                transactionID: fixture.lease.transactionID,
                baseline: fixture.lease.baseline,
                rollbackPlanDigest: MutationRecoveryFixture.digest("7")
            )
        )))
        for request in [wrongTransaction, wrongDesired, wrongRollback] {
            XCTAssertEqual(coordinator.handle(request).error?.code, .invalidRequest)
            XCTAssertEqual(try fixture.store.load(), fixture.lease)
        }
        XCTAssertTrue(backend.invocations.isEmpty)
        XCTAssertNil(try fixture.store.loadCompletion())
    }

    func testLeaseAllowsRecoveryWhenMutableConfigurationIsMissing() throws {
        let fixture = try MutationRecoveryFixture()
        defer { fixture.remove() }
        try fixture.admitLease()
        try FileManager.default.removeItem(at: fixture.locations.helperConfigurationURL)
        let backend = RecoveryFixtureBackend(outcome: .success(
            RecoveryFixtureBackend.result(stdout: try BridgeJSONCodec.encoder().encode(fixture.baseline))
        ))
        let coordinator = fixture.coordinator(
            backend, configuration: .failure(HelperConfigurationError.invalidConfigurationFile)
        )
        let status = try fixture.status(coordinator)
        XCTAssertFalse(status.configured)
        XCTAssertFalse(status.recoveryUnavailable)
        XCTAssertEqual(status.recovery?.transactionID, fixture.lease.transactionID)

        XCTAssertNil(coordinator.handle(try fixture.readbackRequest()).error)
        XCTAssertEqual(backend.resolvedConfigurations, [fixture.configuration])
        XCTAssertNil(try fixture.store.load())
        XCTAssertEqual(try fixture.store.loadCompletion()?.transactionID, fixture.lease.transactionID)
    }

    func testOnlyExactBaselineReadbackCompletesRecovery() throws {
        let fixture = try MutationRecoveryFixture()
        defer { fixture.remove() }
        try fixture.admitLease()
        let backend = RecoveryFixtureBackend()
        let coordinator = fixture.coordinator(backend)
        for (carrier, matchesDesired) in [(fixture.desired, true), (try MutationRecoveryFixture.carrier(marker: 2), false)] {
            backend.outcome = .success(RecoveryFixtureBackend.result(
                stdout: try BridgeJSONCodec.encoder().encode(carrier)
            ))
            let response = coordinator.handle(try fixture.readbackRequest())
            guard case let .readback(result)? = response.body else {
                return XCTFail("expected complete synthetic readback")
            }
            XCTAssertEqual(result.matchesDesired, matchesDesired)
            XCTAssertEqual(try fixture.store.load(), fixture.lease)
            XCTAssertNil(try fixture.store.loadCompletion())
        }
        backend.outcome = .success(RecoveryFixtureBackend.result(
            stdout: try BridgeJSONCodec.encoder().encode(fixture.baseline)
        ))
        XCTAssertNil(coordinator.handle(try fixture.readbackRequest()).error)
        XCTAssertNil(try fixture.store.load())
        let completion = try XCTUnwrap(fixture.store.loadCompletion())
        XCTAssertEqual(completion.baseline, fixture.lease.baseline)
        XCTAssertEqual(completion.verifiedBaseline.digest, fixture.lease.baselineDigest)
        XCTAssertNil(try fixture.status(coordinator).recovery)
    }

    func testRollbackWithoutRestoredCaptureRetainsLeaseThenVerifiedRollbackCompletes() throws {
        let fixture = try MutationRecoveryFixture()
        defer { fixture.remove() }
        try fixture.admitLease()
        let backend = RecoveryFixtureBackend()
        let coordinator = fixture.coordinator(backend)
        for restored in [JSONValue.null, try fixture.json(fixture.baseline)] {
            let verified = restored != .null
            let output: JSONValue = .object([
                "transaction_id": .string(fixture.lease.transactionID),
                "baseline_snapshot_digest": .string(fixture.lease.baselineDigest),
                "restored": restored,
                "equals_baseline": .bool(verified),
            ])
            backend.outcome = .success(RecoveryFixtureBackend.result(
                stdout: try BridgeJSONCodec.encoder().encode(output)
            ))
            let response = coordinator.handle(try fixture.rollbackRequest())
            XCTAssertNil(response.error)
            guard case let .rollback(result)? = response.body else {
                return XCTFail("expected sanitized Rollback result")
            }
            XCTAssertEqual(result.equalsBaseline, verified)
            if verified {
                XCTAssertNil(try fixture.store.load())
                XCTAssertNotNil(try fixture.store.loadCompletion())
            } else {
                XCTAssertNil(result.restored)
                XCTAssertEqual(try fixture.store.load(), fixture.lease)
                XCTAssertNil(try fixture.store.loadCompletion())
            }
        }
    }

    func testUnreadableLeaseFailsClosedAfterRestart() throws {
        let fixture = try MutationRecoveryFixture()
        defer { fixture.remove() }
        try fixture.admitLease()
        let leaseURL = fixture.locations.planRootURL.appendingPathComponent("mutation-recovery.json")
        try Data("{broken}".utf8).write(to: leaseURL)
        let backend = RecoveryFixtureBackend()
        let coordinator = fixture.coordinator(backend)
        let status = try fixture.status(coordinator)
        XCTAssertTrue(status.recoveryUnavailable)
        XCTAssertEqual(status.capabilities, [.helperStatus])
        XCTAssertNil(status.recovery)
        XCTAssertNil(status.completion)
        XCTAssertEqual(coordinator.handle(try fixture.applyRequest()).error?.code, .mutationNotAdmitted)
        XCTAssertEqual(coordinator.handle(try fixture.readbackRequest()).error?.code, .helperNotConfigured)
        XCTAssertTrue(backend.invocations.isEmpty)
    }
}

/// Test-only, synthetic carriers: no captured identity or device bytes.
final class MutationRecoveryFixture {
    let root: URL
    let locations: DCXHelperStorageLocations
    let configuration: HelperConfigurationV1
    let baseline: CoreSnapshotCarrierV1
    let desired: CoreSnapshotCarrierV1
    let apply: ApplyRequest
    let lease: MutationRecoveryLeaseV1
    var store: MutationRecoveryLeaseStore { .init(root: locations.planRootURL) }

    init(persistPlans: Bool = true) throws {
        guard let temporary = ProcessInfo.processInfo.environment["TEST_TMPDIR"], !temporary.isEmpty else {
            throw XCTSkip("recovery fixtures require an owned TEST_TMPDIR")
        }
        root = URL(fileURLWithPath: temporary, isDirectory: true)
            .appendingPathComponent("dcx-recovery-\(UUID().uuidString)", isDirectory: true)
        locations = .init(fixtureRoot: root)
        configuration = try .init(
            bindingID: "fixture-dcx",
            expectedDeviceAddress: 0,
            ttyPath: "/dev/cu.usbserial-FIXTURE",
            enabledFeatures: [.identitySearch, .snapshotCapture, .diffPreview, .apply, .readback, .rollback]
        )
        baseline = try Self.carrier(marker: 0)
        desired = try Self.carrier(marker: 1)
        let profile = try DesiredProfileV1(
            profileID: "pzm-rew-o1-peq9-qualification",
            revision: "2026-09-01",
            digest: "sha256/6135022f405de2d172475865d2ecac3479b898eba209c687a0e6e5d92d774ec4",
            document: .object([
                "target_output": .number(1), "parameter_channel": .number(5), "slot": .number(9),
                "actions": .array(zip([59, 60, 61, 62], [53, 32, 118, 1]).map { parameter, value in
                    .object(["channel": .number(5), "parameter": .number(Double(parameter)), "value": .number(Double(value))])
                }),
            ])
        )
        let diff = try SemanticDiffV1(
            baselineSnapshotDigest: baseline.snapshotDigest,
            desiredProfileDigest: profile.digest,
            desiredSnapshotDigest: desired.snapshotDigest,
            applyPlanDigest: Self.digest("a"),
            rollbackPlanDigest: Self.digest("b"),
            changes: []
        )
        apply = try .init(target: configuration.target, plan: .init(
            baseline: baseline.summary(target: configuration.target, validSearchResponses: 10,
                                       capturedAt: Date(timeIntervalSince1970: 0)),
            desired: profile, diff: diff
        ))
        lease = try .init(configuration: configuration, apply: apply)
        try configuration.save(to: locations.helperConfigurationURL)
        if persistPlans {
            let rawApply: JSONValue = .object([
                "schema_version": .number(1), "device_id": .number(0),
                "baseline": try json(baseline), "desired": try json(desired),
                "baseline_snapshot_digest": .string(baseline.snapshotDigest),
                "desired_snapshot_digest": .string(desired.snapshotDigest),
                "plan_digest": .string(diff.applyPlanDigest),
            ])
            let plans = RawPlanStore(root: locations.planRootURL)
            _ = try plans.persistApply(rawApply, expectedDevice: 0,
                baselineDigest: baseline.snapshotDigest, desiredSnapshotDigest: desired.snapshotDigest)
            _ = try plans.persistRollback(.object([
                "schema_version": .number(1), "apply_plan": rawApply,
                "apply_plan_digest": .string(diff.applyPlanDigest),
                "plan_digest": .string(diff.rollbackPlanDigest),
            ]), expectedDevice: 0, baselineDigest: baseline.snapshotDigest,
                applyPlanDigest: diff.applyPlanDigest)
        }
    }

    func remove() { try? FileManager.default.removeItem(at: root) }

    func coordinator(
        _ backend: RecoveryFixtureBackend,
        configuration initial: Result<HelperConfigurationV1, Error>? = nil
    ) -> DCXHelperCoordinator {
        let value = DCXHelperCoordinator(
            locations: locations, configuration: initial ?? .success(configuration),
            coreMIDI: CoreMIDIPresentation(), runner: backend
        )
        value.setForeground(true)
        return value
    }

    func admitLease() throws {
        let lock = try store.acquireExclusiveLock()
        defer { lock.release() }
        try store.persistNew(lease)
    }

    func applyRequest() throws -> BridgeRequest { try .init(body: .apply(apply)) }

    func readbackRequest(transactionID: String? = nil, expectedDesiredDigest: String? = nil) throws -> BridgeRequest {
        try .init(body: .readback(.init(
            target: configuration.target, transactionID: transactionID ?? lease.transactionID,
            expectedDesiredDigest: expectedDesiredDigest ?? lease.desiredSnapshotDigest
        )))
    }

    func rollbackRequest() throws -> BridgeRequest {
        try .init(body: .rollback(.init(target: configuration.target, plan: .init(
            transactionID: lease.transactionID, baseline: lease.baseline,
            rollbackPlanDigest: lease.rollbackPlanDigest
        ))))
    }

    func status(_ coordinator: DCXHelperCoordinator) throws -> HelperStatusResponse {
        let response = coordinator.handle(try .init(body: .helperStatus(.init())))
        guard case let .helperStatus(value)? = response.body else {
            throw FixtureError.missingStatus
        }
        return value
    }

    func json<T: Encodable>(_ value: T) throws -> JSONValue {
        try BridgeJSONCodec.decoder().decode(JSONValue.self, from: BridgeJSONCodec.encoder().encode(value))
    }

    static func digest(_ digit: Character) -> String { "sha256/" + String(repeating: String(digit), count: 64) }

    static func carrier(marker: UInt8) throws -> CoreSnapshotCarrierV1 {
        let frames = [Array(repeating: UInt8(0), count: 26),
                      Array(repeating: marker, count: 1_015), Array(repeating: UInt8(0), count: 911)]
        let images = zip(["identity", "dump0", "dump1"], frames).map { section, frame in
            CoreSnapshotCarrierV1.Image(section: section, frame: frame, digest: hash(Data(frame)))
        }
        var canonical = Data("dcx2496.snapshot/v1\0".utf8)
        canonical.append(contentsOf: [0, 1, 0]) // schema UInt16(1), then device 0.
        for (tag, frame) in frames.enumerated() {
            canonical.append(UInt8(tag))
            var length = UInt32(frame.count).bigEndian
            canonical.append(Data(bytes: &length, count: 4))
            canonical.append(contentsOf: frame)
        }
        let carrier = CoreSnapshotCarrierV1(schemaVersion: 1, deviceID: 0,
            identity: images[0], dump0: images[1], dump1: images[2], snapshotDigest: hash(canonical))
        try carrier.validate(expectedDevice: 0)
        return carrier
    }

    private static func hash(_ data: Data) -> String {
        "sha256/" + SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }

    private enum FixtureError: Error { case missingStatus }
}

final class RecoveryFixtureBackend: DCXCTLCommandExecuting, @unchecked Sendable {
    var outcome: Result<DCXCTLProcessResult, Error>
    var onRun: ((DCXCTLInvocation, UInt8, MutationRecoveryProcessLock?) throws -> Void)?
    private(set) var invocations: [DCXCTLInvocation] = []
    private(set) var resolvedConfigurations: [HelperConfigurationV1] = []

    init(outcome: Result<DCXCTLProcessResult, Error> = .failure(DCXCTLRunnerError.launchFailed)) {
        self.outcome = outcome
    }

    func resolveExecutable(for configuration: HelperConfigurationV1) throws -> URL {
        resolvedConfigurations.append(configuration)
        // Never created or executed. Only the fake backend sees this reference.
        return URL(fileURLWithPath: "/fixture-only/dcxctl")
    }

    func run(_ invocation: DCXCTLInvocation, timeoutSeconds: UInt8,
             mutationLock: MutationRecoveryProcessLock?) throws -> DCXCTLProcessResult {
        invocations.append(invocation)
        try onRun?(invocation, timeoutSeconds, mutationLock)
        return try outcome.get()
    }

    static func result(exit: Int32 = 0, stdout: Data = Data(), stderr: String = "") -> DCXCTLProcessResult {
        .init(terminationStatus: exit, stdout: stdout, stderr: Data(stderr.utf8), durationMilliseconds: 1)
    }
}
