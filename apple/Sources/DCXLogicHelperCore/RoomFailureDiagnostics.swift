import CryptoKit
import Darwin
import DCXLogicBridge
import Foundation

/// One bounded private record for a failed attempt. Readiness never depends on
/// successful diagnostic storage, and storage never retries a device command.
struct RoomFailureAttempt {
    let id = UUID()
    let observedAt = Date()
    var stage: RoomFailureDiagnosticV1.Stage = .admission
    var child: DCXCTLProcessResult?
    var closeKnown: RoomFailureDiagnosticV1.CloseKnown = .notApplicable

    func metadata(error: Error, pointer: String?, persistenceUnavailable: Bool) -> RoomFailureDiagnosticV1 {
        let kind: RoomFailureDiagnosticV1.Failure
        switch error {
        case DCXCTLRunnerError.launchFailed: kind = .launchFailed
        case DCXCTLRunnerError.timedOut: kind = .timedOut
        case DCXCTLRunnerError.outputReadFailed: kind = .outputReadFailed
        case DCXCTLRunnerError.outputTooLarge: kind = .outputTooLarge
        case DCXCTLRunnerError.terminationUnconfirmed: kind = .terminationUnconfirmed
        case RoomVerificationError.invalidExpectation: kind = .invalidExpectation
        case RoomVerificationError.invalidComparison: kind = .invalidComparison
        case RoomVerificationError.unavailable: kind = .unavailable
        case RoomVerificationError.busy: kind = .busy
        case is DecodingError: kind = .decoding
        default: kind = .other
        }
        func digest(_ data: Data) -> String { "sha256/" + SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined() }
        return .init(attemptID: id, observedAt: observedAt, stage: stage,
            failure: child.map { $0.terminationStatus != 0 } == true && [RoomFailureDiagnosticV1.Stage.validationChild, .refreshChild, .snapshotChild, .comparisonChild].contains(stage) ? .childExit : kind,
            childExitCode: child?.terminationStatus, childDurationMilliseconds: child?.durationMilliseconds,
            stdoutDigest: child.map { digest($0.stdout) }, stdoutByteCount: child?.stdout.count,
            stderrDigest: child.map { digest($0.stderr) }, stderrByteCount: child?.stderr.count,
            stderrRetainedByteCount: persistenceUnavailable ? 0 : min(child?.stderr.count ?? 0, RoomFailureDiagnosticStore.maximumStderrBytes),
            closeKnown: closeKnown, privateRecord: pointer, persistenceUnavailable: persistenceUnavailable)
    }
}

final class RoomFailureDiagnosticStore: @unchecked Sendable {
    static let maximumRecords = 8
    static let maximumRecordBytes = 32 * 1024
    static let maximumStderrBytes = 16 * 1024
    static let directoryName = "RoomDiagnostics"
    private let root: URL
    private let lock = NSLock()
    private let beforePersist: @Sendable () -> Void
    init(parent: URL, beforePersist: @escaping @Sendable () -> Void = {}) {
        root = parent.appendingPathComponent(Self.directoryName, isDirectory: true)
        self.beforePersist = beforePersist
    }
    private enum StoreError: Error { case custody, bounds, io }

    func record(_ attempt: RoomFailureAttempt, error: Error) -> RoomFailureDiagnosticV1 {
        lock.lock(); defer { lock.unlock() }
        beforePersist()
        let leaf = "failure-" + attempt.id.uuidString.lowercased() + ".json"
        let pointer = Self.directoryName + "/" + leaf
        let metadata = attempt.metadata(error: error, pointer: pointer, persistenceUnavailable: false)
        struct PrivateRecord: Encodable { let metadata: RoomFailureDiagnosticV1; let stderr: Data }
        let originalError = error
        do {
            let data = try BridgeJSONCodec.encoder().encode(PrivateRecord(metadata: metadata,
                stderr: Data((attempt.child?.stderr ?? Data()).prefix(Self.maximumStderrBytes))))
            guard data.count <= Self.maximumRecordBytes else { throw StoreError.bounds }
            try persist(data, leaf: leaf)
            return metadata
        } catch {
            return attempt.metadata(error: originalError, pointer: nil, persistenceUnavailable: true)
        }
    }

    private func checked(_ fd: Int32, directory: Bool, mode: mode_t) throws -> stat {
        var value = stat()
        guard fstat(fd, &value) == 0, value.st_uid == geteuid(),
              value.st_mode & S_IFMT == (directory ? S_IFDIR : S_IFREG),
              value.st_mode & 0o777 == mode, directory || value.st_nlink == 1 else { throw StoreError.custody }
        return value
    }

    private func persist(_ data: Data, leaf: String) throws {
        // Refuse a redirected bridge leaf before resolving macOS ancestor aliases.
        let configuredParent = root.deletingLastPathComponent()
        var originalParent = stat()
        guard lstat(configuredParent.path, &originalParent) == 0,
              originalParent.st_mode & S_IFMT == S_IFDIR else { throw StoreError.custody }
        let parent = configuredParent.resolvingSymlinksInPath()
        let parentFD = open(parent.path, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
        guard parentFD >= 0 else { throw StoreError.custody }
        defer { close(parentFD) }
        var parentInfo = stat()
        guard fstat(parentFD, &parentInfo) == 0, parentInfo.st_dev == originalParent.st_dev,
              parentInfo.st_ino == originalParent.st_ino, parentInfo.st_uid == geteuid(),
              parentInfo.st_mode & S_IFMT == S_IFDIR, parentInfo.st_mode & 0o022 == 0 else { throw StoreError.custody }
        if mkdirat(parentFD, Self.directoryName, 0o700) != 0 && errno != EEXIST { throw StoreError.io }
        let directory = openat(parentFD, Self.directoryName, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
        guard directory >= 0 else { throw StoreError.custody }
        defer { close(directory) }
        _ = try checked(directory, directory: true, mode: 0o700)
        // fdopendir owns its duplicated descriptor. Every child check and unlink
        // remains relative to the pinned directory, including during retention.
        let copy = dup(directory)
        guard copy >= 0 else { throw StoreError.io }
        guard let listing = fdopendir(copy) else { close(copy); throw StoreError.io }
        defer { closedir(listing) }
        var files: [(String, Int64, Int64)] = []
        while true {
            errno = 0
            guard let entry = readdir(listing) else {
                guard errno == 0 else { throw StoreError.io }
                break
            }
            let name = withUnsafePointer(to: entry.pointee.d_name) {
                $0.withMemoryRebound(to: CChar.self, capacity: Int(MAXNAMLEN) + 1) { String(cString: $0) }
            }
            if name == "." || name == ".." { continue }
            guard name.hasPrefix("failure-"), name.hasSuffix(".json"),
                  UUID(uuidString: String(name.dropFirst(8).dropLast(5))) != nil else { throw StoreError.custody }
            let fd = openat(directory, name, O_RDONLY | O_NOFOLLOW | O_CLOEXEC)
            guard fd >= 0 else { throw StoreError.custody }
            let info: stat
            do { info = try checked(fd, directory: false, mode: 0o600) } catch { close(fd); throw error }
            close(fd)
            guard info.st_size <= Self.maximumRecordBytes else { throw StoreError.bounds }
            files.append((name, Int64(info.st_mtimespec.tv_sec), Int64(info.st_mtimespec.tv_nsec)))
            guard files.count <= Self.maximumRecords else { throw StoreError.bounds }
        }
        guard !files.contains(where: { $0.0 == leaf }) else { throw StoreError.custody }
        // No writes occur until all retained entries have passed custody checks.
        files.sort { ($0.1, $0.2, $0.0) < ($1.1, $1.2, $1.0) }
        for file in files.prefix(max(0, files.count - Self.maximumRecords + 1)) {
            guard unlinkat(directory, file.0, 0) == 0 else { throw StoreError.io }
        }
        let fd = openat(directory, leaf, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0o600)
        guard fd >= 0 else { throw StoreError.io }
        defer { close(fd) }
        _ = try checked(fd, directory: false, mode: 0o600)
        try data.withUnsafeBytes { raw in
            var offset = 0
            while offset < raw.count {
                let count = Darwin.write(fd, raw.baseAddress!.advanced(by: offset), raw.count - offset)
                if count < 0 && errno == EINTR { continue }
                guard count > 0 else { throw StoreError.io }
                offset += count
            }
        }
        guard fsync(fd) == 0 else { throw StoreError.io }
    }
}
