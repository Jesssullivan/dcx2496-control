import CryptoKit
import Darwin
import DCXLogicBridge
import Foundation

struct DCXCTLInvocation: Sendable {
    let operation: BridgeOperation
    let executableURL: URL
    let arguments: [String]
}

struct DCXCTLProcessResult: Sendable {
    let terminationStatus: Int32
    let stdout: Data
    let stderr: Data
    let durationMilliseconds: UInt64

    func receipt(for operation: BridgeOperation) -> CommandReceiptV1 {
        let digest = SHA256.hash(data: stdout).map { String(format: "%02x", $0) }.joined()
        return .init(
            operation: operation,
            exitCode: terminationStatus,
            durationMilliseconds: durationMilliseconds,
            stdoutDigest: "sha256/\(digest)"
        )
    }
}

/// Internal command boundary for coordinator tests. Production still resolves
/// only the bundled exact dcxctl and executes through the bounded runner.
protocol DCXCTLCommandExecuting: Sendable {
    func resolveExecutable(for configuration: HelperConfigurationV1) throws -> URL
    func run(
        _ invocation: DCXCTLInvocation,
        timeoutSeconds: UInt8,
        mutationLock: MutationRecoveryProcessLock?
    ) throws -> DCXCTLProcessResult
}

final class DCXCTLProcessRunner: DCXCTLCommandExecuting, @unchecked Sendable {
    static let maximumOutputBytes = 1_048_576
    // Cleanup is separate from the original operation deadline: at most two
    // seconds for owned-child TERM, then two for SIGKILL exit confirmation.
    // The four-second maximum fits inside the helper/AU ten-second margin.
    static let terminationGraceSeconds = 2
    static let exitConfirmationSeconds = 2

    func resolveExecutable(for configuration: HelperConfigurationV1) throws -> URL {
        try configuration.resolveExecutable()
    }

    func run(
        _ invocation: DCXCTLInvocation,
        timeoutSeconds: UInt8,
        mutationLock: MutationRecoveryProcessLock? = nil
    ) throws -> DCXCTLProcessResult {
        let process = Process()
        process.executableURL = invocation.executableURL
        process.arguments = invocation.arguments
        process.environment = [
            "LANG": "C",
            "LC_ALL": "C",
            "PATH": "/usr/bin:/bin",
        ]

        let stdoutPipe = Pipe()
        let stderrPipe = Pipe()
        for pipe in [stdoutPipe, stderrPipe] {
            let descriptor = pipe.fileHandleForReading.fileDescriptor
            let flags = Darwin.fcntl(descriptor, F_GETFL)
            guard flags >= 0, Darwin.fcntl(descriptor, F_SETFL, flags | O_NONBLOCK) == 0 else {
                throw DCXCTLRunnerError.outputReadFailed
            }
        }
        let childLockInput = try mutationLock?.childStandardInput()
        process.standardInput = childLockInput ?? FileHandle.nullDevice
        process.standardOutput = stdoutPipe
        process.standardError = stderrPipe

        let stdout = BoundedOutput(limit: Self.maximumOutputBytes)
        let stderr = BoundedOutput(limit: Self.maximumOutputBytes)
        let completion = DispatchSemaphore(value: 0)
        let start = DispatchTime.now().uptimeNanoseconds
        let deadline = DispatchTime(uptimeNanoseconds: start) + .seconds(Int(timeoutSeconds))

        stdoutPipe.fileHandleForReading.readabilityHandler = { handle in
            if stdout.read(from: handle), process.isRunning { process.terminate() }
            if stdout.hasEnded { handle.readabilityHandler = nil }
        }
        stderrPipe.fileHandleForReading.readabilityHandler = { handle in
            if stderr.read(from: handle), process.isRunning { process.terminate() }
            if stderr.hasEnded { handle.readabilityHandler = nil }
        }
        process.terminationHandler = { _ in completion.signal() }
        defer {
            stdoutPipe.fileHandleForReading.readabilityHandler = nil
            stderrPipe.fileHandleForReading.readabilityHandler = nil
            // Serialize close with any final callback before returning. No
            // read-to-EOF call may escape the original operation deadline.
            stdout.stopReading()
            stderr.stopReading()
            try? stdoutPipe.fileHandleForReading.close()
            try? stderrPipe.fileHandleForReading.close()
        }

        do {
            try process.run()
        } catch {
            try? childLockInput?.close()
            throw DCXCTLRunnerError.launchFailed
        }
        try? childLockInput?.close()

        if completion.wait(timeout: deadline) == .timedOut {
            if process.isRunning { process.terminate() }
            if completion.wait(timeout: .now() + .seconds(Self.terminationGraceSeconds)) == .timedOut {
                if process.isRunning { _ = Darwin.kill(process.processIdentifier, SIGKILL) }
                guard completion.wait(timeout: .now() + .seconds(Self.exitConfirmationSeconds)) == .success else {
                    // The inherited mutation stdin still owns flock. Closing
                    // the helper's descriptor must not unlock that child.
                    throw DCXCTLRunnerError.terminationUnconfirmed
                }
            }
            throw DCXCTLRunnerError.timedOut
        }

        // Descendants may retain inherited stdout/stderr after the tracked
        // child exits. Both drains still owe EOF inside the original deadline;
        // timing out here never signals the exited PID or those descendants.
        guard stdout.waitForEnd(until: deadline), stderr.waitForEnd(until: deadline) else {
            throw DCXCTLRunnerError.timedOut
        }
        guard !stdout.readFailed, !stderr.readFailed else {
            throw DCXCTLRunnerError.outputReadFailed
        }
        guard !stdout.exceededLimit, !stderr.exceededLimit else {
            throw DCXCTLRunnerError.outputTooLarge
        }

        let elapsed = (DispatchTime.now().uptimeNanoseconds - start) / 1_000_000
        return .init(
            terminationStatus: process.terminationStatus,
            stdout: stdout.data,
            stderr: stderr.data,
            durationMilliseconds: elapsed
        )
    }
}

private final class BoundedOutput: @unchecked Sendable {
    private static let readChunkBytes = 16 * 1_024
    private let limit: Int
    private let lock = NSLock()
    private let end = DispatchGroup()
    private var storage = Data()
    private var didExceedLimit = false
    private var didFailRead = false
    private var reading = true

    init(limit: Int) {
        self.limit = limit
        end.enter()
    }

    /// Return true only on the first overflow. Continue draining/discarding
    /// after it so the bounded child can exit without blocking on a full pipe.
    func read(from handle: FileHandle) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard reading else { return false }
        var bytes = Data(count: Self.readChunkBytes)
        var count: Int
        repeat {
            count = bytes.withUnsafeMutableBytes {
                Darwin.read(handle.fileDescriptor, $0.baseAddress, $0.count)
            }
        } while count < 0 && errno == EINTR
        if count < 0 {
            if errno == EAGAIN || errno == EWOULDBLOCK { return false }
            didFailRead = true
            finishReading()
            return false
        }
        guard count != 0 else {
            finishReading()
            return false
        }
        guard !didExceedLimit else { return false }
        guard count <= limit - storage.count else {
            didExceedLimit = true
            return true
        }
        storage.append(bytes.prefix(count))
        return false
    }

    private func finishReading() {
        guard reading else { return }
        reading = false
        end.leave()
    }

    func waitForEnd(until deadline: DispatchTime) -> Bool {
        end.wait(timeout: deadline) == .success
    }

    func stopReading() {
        lock.lock()
        defer { lock.unlock() }
        finishReading()
    }

    var data: Data {
        lock.lock()
        defer { lock.unlock() }
        return storage
    }

    var exceededLimit: Bool {
        lock.lock()
        defer { lock.unlock() }
        return didExceedLimit
    }

    var readFailed: Bool {
        lock.lock()
        defer { lock.unlock() }
        return didFailRead
    }

    var hasEnded: Bool {
        lock.lock()
        defer { lock.unlock() }
        return !reading
    }
}

enum DCXCTLRunnerError: Error, Equatable, Sendable {
    case launchFailed
    case timedOut
    case outputTooLarge
    case outputReadFailed
    case terminationUnconfirmed
}
