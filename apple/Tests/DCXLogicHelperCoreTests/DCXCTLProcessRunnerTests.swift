import DCXLogicBridge
@testable import DCXLogicHelperCore
import Foundation
import XCTest

final class DCXCTLProcessRunnerTests: XCTestCase {
    func testConcurrentPipesDrainCompletelyAndPreserveNonzeroExit() throws {
        let fixture = try OwnedChildFixture()
        defer { fixture.remove() }
        let result = try fixture.run("dual-pipes")
        XCTAssertEqual(result.terminationStatus, 23)
        XCTAssertEqual(result.stdout, Data(repeating: 79, count: 16 * 16_384))
        XCTAssertEqual(result.stderr, Data(repeating: 69, count: 16 * 16_384))
        XCTAssertNotNil(result.failurePayload())
        XCTAssertEqual(result.receipt(for: .snapshotCapture).exitCode, 23)
    }

    func testExactOutputCeilingIsAcceptedAndEachPipeOverflowFails() throws {
        let fixture = try OwnedChildFixture()
        defer { fixture.remove() }
        let exact = try fixture.run("exact-limit")
        XCTAssertEqual(exact.terminationStatus, 0)
        XCTAssertEqual(exact.stdout, Data(repeating: 76, count: DCXCTLProcessRunner.maximumOutputBytes))
        for mode in ["overflow-stdout", "overflow-stderr"] {
            XCTAssertThrowsError(try fixture.run(mode)) { error in
                XCTAssertEqual(error as? DCXCTLRunnerError, .outputTooLarge)
            }
        }
    }

    func testDeadlineBoundsOwnedChildIgnoringTermination() throws {
        let fixture = try OwnedChildFixture()
        defer { fixture.remove() }
        let start = DispatchTime.now().uptimeNanoseconds
        XCTAssertThrowsError(try fixture.run("deadline", timeout: 1)) { error in
            XCTAssertEqual(error as? DCXCTLRunnerError, .timedOut)
        }
        let seconds = Double(DispatchTime.now().uptimeNanoseconds - start) / 1_000_000_000
        // Wide bound verifies bounded completion, not scheduler performance.
        XCTAssertLessThan(seconds, 12)
    }

    func testExitedChildWithDescendantHoldingPipesStillHonorsDeadline() throws {
        let fixture = try OwnedChildFixture()
        defer { fixture.remove() }
        let done = fixture.path("descendant-done")
        let start = DispatchTime.now().uptimeNanoseconds
        XCTAssertThrowsError(try fixture.run("retained-pipes", arguments: [done.path], timeout: 1)) { error in
            XCTAssertEqual(error as? DCXCTLRunnerError, .timedOut)
        }
        let seconds = Double(DispatchTime.now().uptimeNanoseconds - start) / 1_000_000_000
        XCTAssertLessThan(seconds, 12)
        // No signal is sent to that descendant; it has its own four-second
        // voluntary exit. Preserve its owned directory until it finishes.
        try fixture.waitForMarker(done)
    }

    func testMissingExecutableFailsBeforeAnyChildLaunch() throws {
        let fixture = try OwnedChildFixture()
        defer { fixture.remove() }
        let invocation = DCXCTLInvocation(operation: .snapshotCapture,
            executableURL: fixture.path("absent-executable"), arguments: [])
        XCTAssertThrowsError(try DCXCTLProcessRunner().run(invocation, timeoutSeconds: 1)) { error in
            XCTAssertEqual(error as? DCXCTLRunnerError, .launchFailed)
        }
    }

    func testUnconfirmedExitRemainsNonretryableAndRecoveryAuthoritySurvives() throws {
        let recovery = try MutationRecoveryFixture()
        defer { recovery.remove() }
        let backend = RecoveryFixtureBackend(outcome: .failure(DCXCTLRunnerError.terminationUnconfirmed))
        let response = recovery.coordinator(backend).handle(try recovery.applyRequest())
        XCTAssertEqual(response.error?.code, .childTimedOut)
        XCTAssertFalse(try XCTUnwrap(response.error).retryable)
        XCTAssertTrue(try XCTUnwrap(response.error).message.contains("child exit is unconfirmed"))
        XCTAssertEqual(try recovery.store.load(), recovery.lease)
        XCTAssertNil(try recovery.store.loadCompletion())
        let status = try recovery.status(recovery.coordinator(RecoveryFixtureBackend()))
        XCTAssertEqual(status.recovery?.transactionID, recovery.lease.transactionID)
    }

    func testInheritedChildStdinKeepsLeaseLockUntilLastDescriptorCloses() throws {
        let fixture = try OwnedChildFixture()
        defer { fixture.remove() }
        let recovery = try MutationRecoveryFixture(persistPlans: false)
        defer { recovery.remove() }
        try recovery.admitLease()
        let lock = try recovery.store.acquireExclusiveLock()
        let input = try lock.childStandardInput()
        let ready = fixture.path("stdin-ready")
        let release = fixture.path("stdin-release")
        let done = fixture.path("stdin-done")
        let child = try fixture.start("hold-stdin", arguments: [ready.path, release.path, done.path], input: input)
        try input.close()
        defer {
            try? fixture.mark(release)
            lock.release()
            try? fixture.waitForMarker(done)
            try? fixture.waitForExit(child)
        }
        try fixture.waitForMarker(ready)
        lock.release()
        XCTAssertThrowsError(try recovery.store.acquireExclusiveLock()) { error in
            XCTAssertEqual(error as? MutationRecoveryLeaseError, .lockBusy)
        }
        try fixture.mark(release)
        try fixture.waitForMarker(done)
        try fixture.waitForExit(child)
        XCTAssertEqual(child.terminationStatus, 0)
        let next = try recovery.store.acquireExclusiveLock()
        next.release()
        XCTAssertEqual(try recovery.store.load(), recovery.lease)
    }

    func testVoluntaryHelperExitLeavesDescendantOwningLockAndRecoveryRehydrates() throws {
        let fixture = try OwnedChildFixture()
        defer { fixture.remove() }
        let recovery = try MutationRecoveryFixture(persistPlans: false)
        defer { recovery.remove() }
        try recovery.admitLease()
        let ready = fixture.path("owner-child-ready")
        let release = fixture.path("owner-child-release")
        let done = fixture.path("owner-child-done")
        let lockPath = recovery.locations.planRootURL.appendingPathComponent("mutation-recovery.lock")
        let owner = try fixture.start("voluntary-owner-exit",
            arguments: [lockPath.path, ready.path, release.path, done.path])
        defer {
            try? fixture.mark(release)
            try? fixture.waitForMarker(done)
        }
        try fixture.waitForExit(owner)
        XCTAssertEqual(owner.terminationStatus, 0)
        try fixture.waitForMarker(ready)
        XCTAssertThrowsError(try recovery.store.acquireExclusiveLock()) { error in
            XCTAssertEqual(error as? MutationRecoveryLeaseError, .lockBusy)
        }
        try fixture.mark(release)
        try fixture.waitForMarker(done)
        let next = try recovery.store.acquireExclusiveLock()
        next.release()
        let status = try recovery.status(recovery.coordinator(RecoveryFixtureBackend()))
        XCTAssertEqual(status.recovery?.transactionID, recovery.lease.transactionID)
        XCTAssertEqual(status.recovery?.baseline, recovery.lease.baseline)
        XCTAssertNil(status.completion)
    }
}

/// Fixed, native-test-only binary. No executable reference or environment
/// setting in this fixture is visible to the production helper initializer.
private final class OwnedChildFixture {
    let root: URL
    private let executable: URL

    init() throws {
        let environment = ProcessInfo.processInfo.environment
        guard let temporary = environment["TEST_TMPDIR"], temporary.hasPrefix("/"),
              let binary = environment["DCX_PROCESS_FIXTURE_EXECUTABLE"], binary.hasPrefix("/") else {
            throw XCTSkip("real child fixtures require the declared native runner and owned TEST_TMPDIR")
        }
        let temporaryRoot = URL(fileURLWithPath: temporary, isDirectory: true).standardizedFileURL
        executable = URL(fileURLWithPath: binary).standardizedFileURL
        guard executable.path.hasPrefix(temporaryRoot.path + "/"),
              executable.lastPathComponent == "dcx-child-fixture",
              executable.resolvingSymlinksInPath() == executable,
              FileManager.default.isExecutableFile(atPath: executable.path) else {
            throw FixtureError.unownedExecutable
        }
        root = temporaryRoot.appendingPathComponent("child-case-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false,
                                               attributes: [.posixPermissions: 0o700])
    }

    func path(_ leaf: String) -> URL { root.appendingPathComponent(leaf) }
    func remove() { try? FileManager.default.removeItem(at: root) }

    func run(_ mode: String, arguments: [String] = [], timeout: UInt8 = 10) throws -> DCXCTLProcessResult {
        try DCXCTLProcessRunner().run(.init(operation: .snapshotCapture,
            executableURL: executable, arguments: [mode] + arguments), timeoutSeconds: timeout)
    }

    func start(_ mode: String, arguments: [String], input: FileHandle? = nil) throws -> Process {
        let child = Process()
        child.executableURL = executable
        child.arguments = [mode] + arguments
        child.environment = ["LANG": "C", "LC_ALL": "C", "PATH": "/usr/bin:/bin"]
        child.standardInput = input ?? FileHandle.nullDevice
        child.standardOutput = FileHandle.nullDevice
        child.standardError = FileHandle.nullDevice
        try child.run()
        return child
    }

    func mark(_ url: URL) throws {
        guard url.deletingLastPathComponent() == root else { throw FixtureError.unownedMarker }
        if !FileManager.default.fileExists(atPath: url.path) {
            try Data("fixture\n".utf8).write(to: url, options: .withoutOverwriting)
        }
    }

    func waitForMarker(_ url: URL) throws {
        guard url.deletingLastPathComponent() == root else { throw FixtureError.unownedMarker }
        let deadline = DispatchTime.now().uptimeNanoseconds + 12_000_000_000
        while !FileManager.default.fileExists(atPath: url.path) {
            guard DispatchTime.now().uptimeNanoseconds < deadline else { throw FixtureError.markerDeadline }
            Thread.sleep(forTimeInterval: 0.01)
        }
    }

    func waitForExit(_ child: Process) throws {
        let deadline = DispatchTime.now().uptimeNanoseconds + 12_000_000_000
        while child.isRunning {
            guard DispatchTime.now().uptimeNanoseconds < deadline else { throw FixtureError.childDeadline }
            Thread.sleep(forTimeInterval: 0.01)
        }
    }

    private enum FixtureError: Error {
        case unownedExecutable, unownedMarker, markerDeadline, childDeadline
    }
}
