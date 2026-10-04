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
        let childLockInput = try mutationLock?.childStandardInput()
        process.standardInput = childLockInput ?? FileHandle.nullDevice
        process.standardOutput = stdoutPipe
        process.standardError = stderrPipe

        let stdout = BoundedOutput(limit: Self.maximumOutputBytes)
        let stderr = BoundedOutput(limit: Self.maximumOutputBytes)
        let completion = DispatchSemaphore(value: 0)
        let start = DispatchTime.now().uptimeNanoseconds

        stdoutPipe.fileHandleForReading.readabilityHandler = { handle in
            let bytes = handle.availableData
            if !bytes.isEmpty, !stdout.append(bytes) { process.terminate() }
        }
        stderrPipe.fileHandleForReading.readabilityHandler = { handle in
            let bytes = handle.availableData
            if !bytes.isEmpty, !stderr.append(bytes) { process.terminate() }
        }
        process.terminationHandler = { _ in completion.signal() }

        do {
            try process.run()
        } catch {
            try? childLockInput?.close()
            stdoutPipe.fileHandleForReading.readabilityHandler = nil
            stderrPipe.fileHandleForReading.readabilityHandler = nil
            throw DCXCTLRunnerError.launchFailed
        }
        try? childLockInput?.close()

        let deadline = DispatchTime.now() + .seconds(Int(timeoutSeconds))
        if completion.wait(timeout: deadline) == .timedOut {
            process.terminate()
            if completion.wait(timeout: .now() + .seconds(2)) == .timedOut {
                _ = Darwin.kill(process.processIdentifier, SIGKILL)
                completion.wait()
            }
            stdoutPipe.fileHandleForReading.readabilityHandler = nil
            stderrPipe.fileHandleForReading.readabilityHandler = nil
            throw DCXCTLRunnerError.timedOut
        }

        stdoutPipe.fileHandleForReading.readabilityHandler = nil
        stderrPipe.fileHandleForReading.readabilityHandler = nil
        stdout.append(stdoutPipe.fileHandleForReading.readDataToEndOfFile())
        stderr.append(stderrPipe.fileHandleForReading.readDataToEndOfFile())
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
    private let limit: Int
    private let lock = NSLock()
    private var storage = Data()
    private var didExceedLimit = false

    init(limit: Int) { self.limit = limit }

    @discardableResult
    func append(_ bytes: Data) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard storage.count + bytes.count <= limit else {
            didExceedLimit = true
            return false
        }
        storage.append(bytes)
        return true
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
}

enum DCXCTLRunnerError: Error, Equatable, Sendable {
    case launchFailed
    case timedOut
    case outputTooLarge
}
