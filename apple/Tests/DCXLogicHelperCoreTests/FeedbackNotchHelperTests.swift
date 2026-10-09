import CryptoKit
import DCXLogicBridge
@testable import DCXLogicHelperCore
import Foundation
import XCTest

/// Helper side of bridge v2 feedback planning, over exact dcxctl outputs of the
/// synthetic ring-out and blank snapshot (`//:apple_feedback_fixture_parity`
/// keeps them byte-identical to the Rust CLI). No tty, serial, or device.
final class FeedbackNotchHelperTests: XCTestCase {
    func testRustProducedV2ProfileValidatesInSwiftAndTamperingFails() throws {
        let fixtures = try FeedbackFixtures()
        let decoder = BridgeJSONCodec.decoder()
        let profile = try decoder.decode(DesiredProfileV2.self, from: fixtures.profile)
        XCTAssertNoThrow(try profile.validate())
        XCTAssertEqual(try decoder.decode(DesiredProfile.self, from: fixtures.profile), .v2(profile))
        let tampered = String(decoding: fixtures.profile, as: UTF8.self)
            .replacingOccurrences(of: "\"value\": 159", with: "\"value\": 158")
        XCTAssertNotEqual(Data(tampered.utf8), fixtures.profile)
        XCTAssertThrowsError(try decoder.decode(DesiredProfileV2.self, from: Data(tampered.utf8)).validate())
    }

    func testCoordinatorPlansOfflineFromRingOutText() throws {
        let fixtures = try FeedbackFixtures()
        defer { fixtures.remove() }
        let backend = ScriptedBackend([
            .success(.ok(fixtures.measurement)),
            .success(.ok(fixtures.plan)),
            .success(.ok(fixtures.profile)),
        ])
        var importedSource: Data?
        backend.onRun = { invocation, timeout, lock in
            XCTAssertEqual(invocation.operation, .feedbackPlan)
            XCTAssertEqual(timeout, DCXHelperCoordinator.offlineStepTimeoutSeconds)
            XCTAssertNil(lock)
            XCTAssertFalse(invocation.arguments.contains("--tty"))
            if invocation.arguments.starts(with: ["feedback", "import"]) {
                importedSource = try Data(contentsOf: URL(fileURLWithPath: invocation.arguments[3]))
            }
        }
        let request = try fixtures.request(.frequencyList(fixtures.ringOutText))
        let response = fixtures.coordinator(backend).handle(request)

        XCTAssertNil(response.error, "\(String(describing: response.error))")
        XCTAssertNoThrow(try response.validate(for: request))
        guard case let .feedbackPlan(result)? = response.body else { return XCTFail("expected plan") }
        XCTAssertEqual(importedSource, fixtures.ringOut)
        XCTAssertEqual(result.measurement.sourceDigest,
                       "sha256/" + SHA256.hash(data: fixtures.ringOut).map { String(format: "%02x", $0) }.joined())
        XCTAssertEqual(result.plan.notches.map(\.band), [1, 2, 3])
        XCTAssertEqual(result.plan.notches.map(\.gainCode), [90, 90, 60])
        XCTAssertEqual(result.plan.baselineSnapshotDigest, fixtures.baseline.digest)
        XCTAssertEqual(result.desired.digest, FeedbackNotchBridgeDigest.rust)

        let calls = backend.invocations.map(\.arguments)
        XCTAssertEqual(calls.count, 3)
        XCTAssertEqual(Array(calls[0].prefix(3)), ["feedback", "import", "--frequency-list"])
        XCTAssertEqual(Array(calls[0].suffix(2)), ["--target-output", "4"])
        XCTAssertEqual(calls[1][0...2], ["feedback", "plan", "--measurement"])
        XCTAssertEqual(calls[1][4], "--snapshot")
        XCTAssertTrue(resolved(calls[1][5]).hasPrefix(resolved(fixtures.locations.snapshotRootURL.path)))
        XCTAssertFalse(calls[1].contains("--prior-plan"))
        let stored = try NotchPlanStore(planRoot: fixtures.locations.planRootURL)
            .load(planDigest: result.plan.planDigest)
        XCTAssertEqual(try Data(contentsOf: stored), fixtures.plan)
        // desired-profile reads this run's plan bytes from the transaction
        // workspace, the same bytes the summary was parsed from.
        XCTAssertEqual(calls[2].count, 8)
        XCTAssertEqual(calls[2][0...2], ["feedback", "desired-profile", "--plan"])
        XCTAssertTrue(resolved(calls[2][3]).hasPrefix(resolved(fixtures.locations.transactionRootURL.path)))
        XCTAssertTrue(calls[2][3].hasSuffix("/plan.json"))
        XCTAssertEqual(Array(calls[2][4...]), ["--profile-id", "o4-feedback", "--revision", "synthetic-1"])
    }

    func testImportedMeasurementSkipsImportAndRecurrenceNamesTheStoredPlan() throws {
        let fixtures = try FeedbackFixtures()
        defer { fixtures.remove() }
        let first = ScriptedBackend([
            .success(.ok(fixtures.measurement)), .success(.ok(fixtures.plan)), .success(.ok(fixtures.profile)),
        ])
        let firstResponse = fixtures.coordinator(first).handle(
            try fixtures.request(.frequencyList(fixtures.ringOutText)))
        guard case let .feedbackPlan(firstPlan)? = firstResponse.body else { return XCTFail("expected plan") }

        let document = try BridgeJSONCodec.decoder().decode(JSONValue.self, from: fixtures.measurement)
        let second = ScriptedBackend([.success(.ok(fixtures.plan)), .success(.ok(fixtures.profile))])
        let request = try fixtures.request(.measurement(document), prior: firstPlan.plan.planDigest)
        let response = fixtures.coordinator(second).handle(request)
        XCTAssertNil(response.error, "\(String(describing: response.error))")
        guard case let .feedbackPlan(result)? = response.body else { return XCTFail("expected plan") }
        XCTAssertNil(result.importReceipt)
        let plan = try XCTUnwrap(second.invocations.first?.arguments)
        XCTAssertEqual(plan[0...1], ["feedback", "plan"])
        let prior = try XCTUnwrap(plan.firstIndex(of: "--prior-plan"))
        XCTAssertEqual(resolved(plan[prior + 1]), resolved(try NotchPlanStore(
            planRoot: fixtures.locations.planRootURL).load(planDigest: firstPlan.plan.planDigest).path))
    }

    func testMissingPriorPlanIsRefusedBeforeAnyChild() throws {
        let fixtures = try FeedbackFixtures()
        defer { fixtures.remove() }
        let backend = ScriptedBackend([])
        let response = fixtures.coordinator(backend).handle(
            try fixtures.request(.frequencyList(fixtures.ringOutText), prior: "sha256/" + String(repeating: "f", count: 64)))
        XCTAssertEqual(response.error?.code, .invalidRequest)
        XCTAssertTrue(backend.invocations.isEmpty)
    }

    func testChildFailureAndForeignOutputsFailClosed() throws {
        let fixtures = try FeedbackFixtures()
        defer { fixtures.remove() }
        let failing = ScriptedBackend([
            .success(.ok(fixtures.measurement)), .success(.ok(Data(), exit: 1, stderr: "Error: NoFreeBands")),
        ])
        let failed = fixtures.coordinator(failing).handle(try fixtures.request(.frequencyList(fixtures.ringOutText)))
        XCTAssertEqual(failed.error?.code, .childFailed)
        XCTAssertEqual(failing.invocations.count, 2)

        let foreignBaseline = String(decoding: fixtures.plan, as: UTF8.self).replacingOccurrences(
            of: fixtures.baseline.digest, with: "sha256/" + String(repeating: "7", count: 64))
        let otherProfile = String(decoding: fixtures.profile, as: UTF8.self)
            .replacingOccurrences(of: "\"value\": 90", with: "\"value\": 91")
        let otherSource = String(decoding: fixtures.measurement, as: UTF8.self).replacingOccurrences(
            of: "\"source_digest\": \"sha256/2", with: "\"source_digest\": \"sha256/3")
        let cases: [[Data]] = [
            [fixtures.measurement, Data(foreignBaseline.utf8)],
            [fixtures.measurement, fixtures.plan, Data(otherProfile.utf8)],
            [Data(otherSource.utf8)],
            [fixtures.measurement, fixtures.plan, Data("{\"schemaVersion\":\"dcx.desired-profile/v1\"}".utf8)],
        ]
        for outputs in cases {
            let backend = ScriptedBackend(outputs.map { .success(.ok($0)) })
            let response = fixtures.coordinator(backend).handle(
                try fixtures.request(.frequencyList(fixtures.ringOutText)))
            XCTAssertEqual(response.error?.code, .malformedChildResponse)
            XCTAssertNil(response.body)
        }
    }

    func testFeedbackPlanningIsGatedByConfigurationAndRecovery() throws {
        let fixtures = try FeedbackFixtures(features: [.snapshotCapture, .diffPreview])
        defer { fixtures.remove() }
        let backend = ScriptedBackend([])
        let response = fixtures.coordinator(backend).handle(try fixtures.request(.frequencyList(fixtures.ringOutText)))
        XCTAssertEqual(response.error?.code, .operationUnavailable)

        let recovery = try MutationRecoveryFixture()
        defer { recovery.remove() }
        try recovery.admitLease()
        let snapshot = try recovery.baseline.summary(target: recovery.configuration.target, validSearchResponses: 10)
        let blocked = recovery.coordinator(RecoveryFixtureBackend()).handle(try .init(body: .feedbackPlan(.init(
            target: recovery.configuration.target, baseline: snapshot,
            measurement: .frequencyList("630"), profileID: "o4-feedback", revision: "r"))))
        XCTAssertEqual(blocked.error?.code, .operationUnavailable)
    }

    func testV2DiffPreviewDecodesFieldChangesBoundToTheProfile() throws {
        let fixtures = try FeedbackFixtures()
        defer { fixtures.remove() }
        let profile = try BridgeJSONCodec.decoder().decode(DesiredProfileV2.self, from: fixtures.profile)
        let backend = ScriptedBackend([.success(.ok(fixtures.diff))])
        let request = try BridgeRequest(body: .diffPreview(.init(
            target: fixtures.configuration.target, baseline: fixtures.baseline, desired: .v2(profile))))
        let response = fixtures.coordinator(backend).handle(request)
        XCTAssertNil(response.error, "\(String(describing: response.error))")
        XCTAssertNoThrow(try response.validate(for: request))
        guard case let .diffPreview(result)? = response.body else { return XCTFail("expected diff") }
        XCTAssertEqual(result.diff.schemaVersion, SemanticDiff.v2SchemaVersion)
        XCTAssertEqual(result.diff.fieldChanges.count, 14)
        XCTAssertTrue(result.diff.fieldChanges.contains {
            $0.field == "band3.gain" && $0.after == 60 && $0.parameter == 0x1f
        })
        XCTAssertEqual(result.diff.fieldChanges.last?.field, "eq_enabled")
        XCTAssertNoThrow(try result.diff.validate(against: .v2(profile)))
    }

    func testV2DiffMustShowEveryActionApplyWrites() throws {
        let fixtures = try FeedbackFixtures()
        defer { fixtures.remove() }
        let profile = try BridgeJSONCodec.decoder().decode(DesiredProfileV2.self, from: fixtures.profile)
        guard case var .object(output) = try BridgeJSONCodec.decoder().decode(JSONValue.self, from: fixtures.diff),
              case var .array(changes)? = output["changes"] else {
            return XCTFail("expected a diff fixture with changes")
        }
        let request = try BridgeRequest(body: .diffPreview(.init(
            target: fixtures.configuration.target, baseline: fixtures.baseline, desired: .v2(profile))))
        // A stale `before` (what Rollback restores) is caught as well.
        var staleBefore = changes
        guard case var .object(first) = staleBefore[0] else { return XCTFail("expected a change") }
        first["before"] = .number(60)
        staleBefore[0] = .object(first)
        changes.removeLast()
        for candidate in [changes, staleBefore] {
            output["changes"] = .array(candidate)
            let backend = ScriptedBackend([.success(.ok(try BridgeJSONCodec.encoder().encode(JSONValue.object(output))))])
            let response = fixtures.coordinator(backend).handle(request)
            XCTAssertEqual(response.error?.code, .malformedChildResponse)
            XCTAssertNil(response.body)
        }
    }

    func testRequestSideFailuresAreInvalidRequestsBeforeAnyChild() throws {
        let fixtures = try FeedbackFixtures()
        defer { fixtures.remove() }
        // A well-labelled document without peaks never came from a child.
        let document: JSONValue = .object([
            "schema_version": .string("dcx.feedback-measurement/v1"),
            "target_output": .number(4), "digest": .string("sha256/" + String(repeating: "d", count: 64)),
        ])
        let backend = ScriptedBackend([])
        let malformed = fixtures.coordinator(backend).handle(try fixtures.request(.measurement(document)))
        XCTAssertEqual(malformed.error?.code, .invalidRequest)
        XCTAssertTrue(backend.invocations.isEmpty)

        // A recalled baseline this helper never captured.
        let foreign = try SnapshotV1(
            target: fixtures.configuration.target, identity: fixtures.baseline.identity,
            capturedAt: Date(timeIntervalSince1970: 0), digest: "sha256/" + String(repeating: "7", count: 64),
            complete: true, sectionDigests: fixtures.baseline.sectionDigests)
        let missing = fixtures.coordinator(backend).handle(try .init(body: .feedbackPlan(.init(
            target: fixtures.configuration.target, baseline: foreign,
            measurement: .frequencyList(fixtures.ringOutText), profileID: "o4-feedback", revision: "r"))))
        XCTAssertEqual(missing.error?.code, .invalidRequest)
        XCTAssertTrue(backend.invocations.isEmpty)
    }

    func testEqualDigestPlanBytesKeepTheFirstStoredSerialization() throws {
        let fixtures = try FeedbackFixtures()
        defer { fixtures.remove() }
        let store = NotchPlanStore(planRoot: fixtures.locations.planRootURL)
        guard case let .object(plan) = try BridgeJSONCodec.decoder().decode(JSONValue.self, from: fixtures.plan),
              case let .string(digest)? = plan["plan_digest"] else {
            return XCTFail("fixture plan lacks a digest")
        }
        let first = try store.persist(fixtures.plan, planDigest: digest)
        let compact = try BridgeJSONCodec.encoder().encode(
            BridgeJSONCodec.decoder().decode(JSONValue.self, from: fixtures.plan))
        XCTAssertNotEqual(compact, fixtures.plan)
        XCTAssertEqual(try store.persist(compact, planDigest: digest), first)
        XCTAssertEqual(try Data(contentsOf: try store.load(planDigest: digest)), fixtures.plan)
        XCTAssertThrowsError(try store.persist(fixtures.plan, planDigest: "sha256/" + String(repeating: "0", count: 64)))
        // A damaged entry heals on the next persist instead of wedging the digest.
        try Data("{".utf8).write(to: first)
        XCTAssertThrowsError(try store.load(planDigest: digest))
        XCTAssertEqual(try store.persist(fixtures.plan, planDigest: digest), first)
        XCTAssertEqual(try Data(contentsOf: try store.load(planDigest: digest)), fixtures.plan)
    }

    /// Cross-language end to end with the real dcxctl under test, when the
    /// native lane supplies it (`DCX_TEST_DCXCTL`). Offline subcommands only.
    func testRealDcxctlProducesTheFixturePlan() throws {
        guard let path = ProcessInfo.processInfo.environment["DCX_TEST_DCXCTL"], !path.isEmpty else {
            throw XCTSkip("real dcxctl end to end requires DCX_TEST_DCXCTL from the native lane")
        }
        let fixtures = try FeedbackFixtures()
        defer { fixtures.remove() }
        let backend = RealDcxctlBackend(executable: URL(fileURLWithPath: path))
        let request = try fixtures.request(.frequencyList(fixtures.ringOutText))
        let response = fixtures.coordinator(backend).handle(request)
        XCTAssertNil(response.error, "\(String(describing: response.error))")
        XCTAssertNoThrow(try response.validate(for: request))
        guard case let .feedbackPlan(result)? = response.body else { return XCTFail("expected plan") }
        let expected = try BridgeJSONCodec.decoder().decode(JSONValue.self, from: fixtures.plan)
        guard case let .object(plan) = expected, case let .string(digest)? = plan["plan_digest"] else {
            return XCTFail("fixture plan lacks a digest")
        }
        XCTAssertEqual(result.plan.planDigest, digest)
        XCTAssertEqual(result.desired.digest, FeedbackNotchBridgeDigest.rust)

        let recurrence = fixtures.coordinator(backend).handle(try fixtures.request(
            .frequencyList(fixtures.ringOutText), prior: result.plan.planDigest))
        // The blank baseline does not hold the prior plan's notches, so the
        // real planner refuses the recurrence rather than stacking notches.
        XCTAssertEqual(recurrence.error?.code, .childFailed)
    }
}

/// Compare paths across the /tmp -> /private/tmp spelling.
private func resolved(_ path: String) -> String {
    path.hasPrefix("/") ? URL(fileURLWithPath: path).resolvingSymlinksInPath().path : path
}

enum FeedbackNotchBridgeDigest {
    static let rust = "sha256/1a18205e2d89cf0d094da31337c04ae5328a62b9c4fe387a614c89aef6aaa16c"
}

final class FeedbackFixtures {
    let root: URL
    let locations: DCXHelperStorageLocations
    let configuration: HelperConfigurationV1
    let baseline: SnapshotV1
    let ringOut: Data
    let measurement: Data
    let plan: Data
    let profile: Data
    let diff: Data
    var ringOutText: String { String(decoding: ringOut, as: UTF8.self) }

    init(features: [HelperFeature] = [.snapshotCapture, .diffPreview, .feedbackPlan]) throws {
        guard let temporary = ProcessInfo.processInfo.environment["TEST_TMPDIR"], !temporary.isEmpty else {
            throw XCTSkip("feedback fixtures require an owned TEST_TMPDIR")
        }
        guard let directory = Bundle.module.url(forResource: "feedback", withExtension: nil) else {
            throw XCTSkip("feedback fixture resources are missing")
        }
        func read(_ name: String) throws -> Data {
            try Data(contentsOf: directory.appendingPathComponent(name))
        }
        ringOut = try read("SYNTHETIC-ring-out.txt")
        measurement = try read("SYNTHETIC-o4-measurement.json")
        plan = try read("SYNTHETIC-o4-notch-plan.json")
        profile = try read("SYNTHETIC-o4-desired-profile-v2.json")
        diff = try read("SYNTHETIC-o4-diff-v2.json")
        root = URL(fileURLWithPath: temporary, isDirectory: true)
            .appendingPathComponent("dcx-feedback-\(UUID().uuidString)", isDirectory: true)
        locations = .init(fixtureRoot: root)
        configuration = try .init(
            bindingID: "fixture-dcx", expectedDeviceAddress: 0,
            ttyPath: "/dev/cu.usbserial-FIXTURE", enabledFeatures: features)
        try configuration.save(to: locations.helperConfigurationURL)
        let snapshot = try read("SYNTHETIC-blank-snapshot.json")
        let carrier = try BridgeJSONCodec.decoder().decode(CoreSnapshotCarrierV1.self, from: snapshot)
        _ = try RawSnapshotStore(root: locations.snapshotRootURL).persist(snapshot, carrier: carrier)
        baseline = try carrier.summary(target: configuration.target, validSearchResponses: 10,
                                       capturedAt: Date(timeIntervalSince1970: 0))
    }

    func remove() { try? FileManager.default.removeItem(at: root) }

    func request(_ measurement: FeedbackMeasurementInputV1, prior: String? = nil) throws -> BridgeRequest {
        try .init(body: .feedbackPlan(.init(
            target: configuration.target, baseline: baseline, measurement: measurement,
            priorPlanDigest: prior, profileID: "o4-feedback", revision: "synthetic-1")))
    }

    func coordinator(_ backend: any DCXCTLCommandExecuting) -> DCXHelperCoordinator {
        let value = DCXHelperCoordinator(
            locations: locations, configuration: .success(configuration),
            coreMIDI: CoreMIDIPresentation(), runner: backend)
        value.setForeground(true)
        return value
    }
}

/// Replays one scripted child result per invocation, in order.
final class ScriptedBackend: DCXCTLCommandExecuting, @unchecked Sendable {
    private var outcomes: [Result<DCXCTLProcessResult, Error>]
    var onRun: ((DCXCTLInvocation, UInt8, MutationRecoveryProcessLock?) throws -> Void)?
    private(set) var invocations: [DCXCTLInvocation] = []

    init(_ outcomes: [Result<DCXCTLProcessResult, Error>]) { self.outcomes = outcomes }

    func resolveExecutable(for configuration: HelperConfigurationV1) throws -> URL {
        URL(fileURLWithPath: "/fixture-only/dcxctl")
    }

    func run(_ invocation: DCXCTLInvocation, timeoutSeconds: UInt8,
             mutationLock: MutationRecoveryProcessLock?) throws -> DCXCTLProcessResult {
        invocations.append(invocation)
        try onRun?(invocation, timeoutSeconds, mutationLock)
        guard !outcomes.isEmpty else { throw DCXCTLRunnerError.launchFailed }
        return try outcomes.removeFirst().get()
    }
}

/// The bounded production runner against an explicitly supplied dcxctl.
final class RealDcxctlBackend: DCXCTLCommandExecuting, @unchecked Sendable {
    private let executable: URL
    private let runner = DCXCTLProcessRunner()

    init(executable: URL) { self.executable = executable }

    func resolveExecutable(for configuration: HelperConfigurationV1) throws -> URL { executable }

    func run(_ invocation: DCXCTLInvocation, timeoutSeconds: UInt8,
             mutationLock: MutationRecoveryProcessLock?) throws -> DCXCTLProcessResult {
        try runner.run(invocation, timeoutSeconds: timeoutSeconds, mutationLock: mutationLock)
    }
}

extension DCXCTLProcessResult {
    static func ok(_ stdout: Data, exit: Int32 = 0, stderr: String = "") -> DCXCTLProcessResult {
        .init(terminationStatus: exit, stdout: stdout, stderr: Data(stderr.utf8), durationMilliseconds: 1)
    }
}
