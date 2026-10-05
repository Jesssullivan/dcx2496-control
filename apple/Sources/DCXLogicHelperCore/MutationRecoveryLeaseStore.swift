import CryptoKit
import Darwin
import DCXLogicBridge
import Foundation

// Darwin also exports `struct flock`; bind the libc function under an
// unambiguous Swift name while retaining open-file-description semantics.
@_silgen_name("flock")
private func systemFlock(_ descriptor: CInt, _ operation: CInt) -> CInt

/// Durable authority for one admitted mutation ceremony. The lease is written
/// before dcxctl can start, so its absence means this helper did not admit a
/// potentially-writing Apply. Recovery always reconstructs the exact checked
/// configuration captured here rather than trusting the mutable config file.
struct MutationRecoveryLeaseV1: Codable, Equatable, Sendable {
    static let schemaVersion = "dcx.mutation-recovery-lease/v1"

    let schemaVersion: String
    let transactionID: String
    let target: DCXTargetReference
    let ttyPath: String
    let enabledOperations: [BridgeOperation]
    let childTimeoutSeconds: UInt8
    let configurationRevision: String
    let baseline: SnapshotV1
    let baselineDigest: String
    let desiredSnapshotDigest: String
    let applyPlanDigest: String
    let rollbackPlanDigest: String

    init(configuration: HelperConfigurationV1, apply: ApplyRequest) throws {
        try configuration.require(.apply, target: apply.target)
        schemaVersion = Self.schemaVersion
        transactionID = apply.plan.diff.applyPlanDigest
        target = apply.target
        ttyPath = configuration.ttyPath
        enabledOperations = configuration.enabledOperations
        childTimeoutSeconds = configuration.childTimeoutSeconds
        configurationRevision = try configuration.recoveryRevision()
        baseline = apply.plan.baseline
        baselineDigest = apply.plan.baseline.digest
        desiredSnapshotDigest = apply.plan.diff.desiredSnapshotDigest
        applyPlanDigest = apply.plan.diff.applyPlanDigest
        rollbackPlanDigest = apply.plan.diff.rollbackPlanDigest
        try validate()
    }

    func validate() throws {
        guard schemaVersion == Self.schemaVersion,
              transactionID == applyPlanDigest else {
            throw MutationRecoveryLeaseError.invalidLease
        }
        try target.validate()
        try BridgeDigest.validate(transactionID)
        try BridgeDigest.validate(configurationRevision)
        try baseline.validate()
        try BridgeDigest.validate(baselineDigest)
        try BridgeDigest.validate(desiredSnapshotDigest)
        try BridgeDigest.validate(applyPlanDigest)
        try BridgeDigest.validate(rollbackPlanDigest)
        let configuration = try pinnedConfiguration()
        guard baseline.target == target,
              baseline.digest == baselineDigest,
              configurationRevision == (try configuration.recoveryRevision()),
              BridgeOperation.mutationCapabilities.isSubset(of: Set(enabledOperations)) else {
            throw MutationRecoveryLeaseError.invalidLease
        }
    }

    func pinnedConfiguration() throws -> HelperConfigurationV1 {
        try HelperConfigurationV1(
            target: target,
            ttyPath: ttyPath,
            enabledOperations: enabledOperations,
            childTimeoutSeconds: childTimeoutSeconds
        )
    }

    func matches(readback: ReadbackRequest) -> Bool {
        readback.target == target
            && readback.transactionID == transactionID
            && readback.expectedDesiredDigest == desiredSnapshotDigest
    }

    func matches(rollback: RollbackRequest) -> Bool {
        rollback.target == target
            && rollback.plan.transactionID == transactionID
            && rollback.plan.baseline == baseline
            && rollback.plan.rollbackPlanDigest == rollbackPlanDigest
    }
}

/// The one bounded terminal proof retained after exact baseline verification.
/// It carries both the immutable admission baseline and the newly observed
/// snapshot so a recreated AU can reconcile a response lost after completion.
struct MutationRecoveryCompletionV1: Codable, Equatable, Sendable {
    static let schemaVersion = "dcx.mutation-recovery-completion/v1"

    let schemaVersion: String
    let transactionID: String
    let target: DCXTargetReference
    let configurationRevision: String
    let baseline: SnapshotV1
    let verifiedBaseline: SnapshotV1
    let baselineDigest: String
    let desiredSnapshotDigest: String
    let applyPlanDigest: String
    let rollbackPlanDigest: String

    init(lease: MutationRecoveryLeaseV1, verifiedBaseline: SnapshotV1) throws {
        schemaVersion = Self.schemaVersion
        transactionID = lease.transactionID
        target = lease.target
        configurationRevision = lease.configurationRevision
        baseline = lease.baseline
        self.verifiedBaseline = verifiedBaseline
        baselineDigest = lease.baselineDigest
        desiredSnapshotDigest = lease.desiredSnapshotDigest
        applyPlanDigest = lease.applyPlanDigest
        rollbackPlanDigest = lease.rollbackPlanDigest
        try validate()
    }

    func validate() throws {
        guard schemaVersion == Self.schemaVersion,
              transactionID == applyPlanDigest else {
            throw MutationRecoveryLeaseError.invalidLease
        }
        try target.validate()
        try BridgeDigest.validate(transactionID)
        try BridgeDigest.validate(configurationRevision)
        try baseline.validate()
        try verifiedBaseline.validate()
        try BridgeDigest.validate(baselineDigest)
        try BridgeDigest.validate(desiredSnapshotDigest)
        try BridgeDigest.validate(applyPlanDigest)
        try BridgeDigest.validate(rollbackPlanDigest)
        guard baseline.target == target,
              verifiedBaseline.target == target,
              baseline.digest == baselineDigest,
              verifiedBaseline.digest == baselineDigest else {
            throw MutationRecoveryLeaseError.invalidLease
        }
    }
}

final class MutationRecoveryProcessLock: @unchecked Sendable {
    private var descriptor: Int32

    fileprivate init(descriptor: Int32) { self.descriptor = descriptor }

    /// Give the mutation child a duplicate of the locked open-file description
    /// as stdin. If the helper crashes, the inherited descriptor keeps flock
    /// ownership alive until the child itself exits.
    func childStandardInput() throws -> FileHandle {
        guard descriptor >= 0 else { throw MutationRecoveryLeaseError.invalidLease }
        let duplicate = Darwin.dup(descriptor)
        guard duplicate >= 0 else {
            throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
        }
        let flags = Darwin.fcntl(duplicate, F_GETFD)
        guard flags >= 0,
              Darwin.fcntl(duplicate, F_SETFD, flags & ~FD_CLOEXEC) == 0 else {
            let code = errno
            Darwin.close(duplicate)
            throw POSIXError(POSIXErrorCode(rawValue: code) ?? .EIO)
        }
        return FileHandle(fileDescriptor: duplicate, closeOnDealloc: true)
    }

    func release() {
        guard descriptor >= 0 else { return }
        // flock belongs to the open-file description shared with child stdin.
        // Explicit LOCK_UN would also release a still-running child's lease.
        // Closing our reference releases the lock only after the final owner
        // closes its descriptor, including an unconfirmed or orphaned child.
        Darwin.close(descriptor)
        descriptor = -1
    }

    deinit { release() }
}

final class MutationRecoveryLeaseStore: @unchecked Sendable {
    private static let maximumLeaseBytes = 32 * 1_024
    private let root: URL
    private let leaseURL: URL
    private let completionURL: URL
    private let lockURL: URL
    private let fileManager: FileManager

    init(root: URL, fileManager: FileManager = .default) {
        self.root = root.standardizedFileURL
        leaseURL = self.root.appendingPathComponent("mutation-recovery.json", isDirectory: false)
        completionURL = self.root.appendingPathComponent(
            "mutation-recovery-completion.json",
            isDirectory: false
        )
        lockURL = self.root.appendingPathComponent("mutation-recovery.lock", isDirectory: false)
        self.fileManager = fileManager
    }

    func load() throws -> MutationRecoveryLeaseV1? {
        try load(MutationRecoveryLeaseV1.self, from: leaseURL) { try $0.validate() }
    }

    func loadCompletion() throws -> MutationRecoveryCompletionV1? {
        try load(MutationRecoveryCompletionV1.self, from: completionURL) { try $0.validate() }
    }

    /// All helpers sharing the App Group serialize mutation authority with this
    /// owner-only lock. Callers retain the returned token through confirmed
    /// child exit and the durable active-to-terminal transition.
    func acquireExclusiveLock() throws -> MutationRecoveryProcessLock {
        try prepareRoot()
        let descriptor = Darwin.open(
            lockURL.path,
            O_CREAT | O_RDWR | O_CLOEXEC | O_NOFOLLOW,
            S_IRUSR | S_IWUSR
        )
        guard descriptor >= 0 else {
            throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
        }
        var metadata = stat()
        guard Darwin.fstat(descriptor, &metadata) == 0,
              (metadata.st_mode & S_IFMT) == S_IFREG,
              metadata.st_uid == geteuid(),
              metadata.st_mode & 0o077 == 0 else {
            Darwin.close(descriptor)
            throw MutationRecoveryLeaseError.invalidLease
        }
        guard systemFlock(descriptor, LOCK_EX | LOCK_NB) == 0 else {
            let code = errno
            Darwin.close(descriptor)
            if code == EWOULDBLOCK || code == EAGAIN {
                throw MutationRecoveryLeaseError.lockBusy
            }
            throw POSIXError(POSIXErrorCode(rawValue: code) ?? .EIO)
        }
        return MutationRecoveryProcessLock(descriptor: descriptor)
    }

    private func load<Value: Decodable>(
        _ type: Value.Type,
        from url: URL,
        validate: (Value) throws -> Void
    ) throws -> Value? {
        guard fileManager.fileExists(atPath: url.path) else { return nil }
        let values = try url.resourceValues(
            forKeys: [.isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey]
        )
        let attributes = try fileManager.attributesOfItem(atPath: url.path)
        let permissions = (attributes[.posixPermissions] as? NSNumber)?.intValue
        let owner = (attributes[.ownerAccountID] as? NSNumber)?.uint32Value
        guard values.isRegularFile == true,
              values.isSymbolicLink != true,
              let size = values.fileSize,
              size <= Self.maximumLeaseBytes,
              let permissions,
              permissions & 0o077 == 0,
              owner == geteuid() else {
            throw MutationRecoveryLeaseError.invalidLease
        }
        let value = try BridgeJSONCodec.decoder().decode(
            type,
            from: Data(contentsOf: url)
        )
        try validate(value)
        return value
    }

    /// Publish with an exclusive hard link so two helper processes cannot both
    /// admit Apply for the shared App Group device authority.
    func persistNew(_ lease: MutationRecoveryLeaseV1) throws {
        try lease.validate()
        let data = try BridgeJSONCodec.encoder().encode(lease)
        guard data.count <= Self.maximumLeaseBytes else {
            throw MutationRecoveryLeaseError.invalidLease
        }
        try prepareRoot()
        let temporary = try writeTemporary(data, prefix: ".mutation-recovery")
        defer { try? fileManager.removeItem(at: temporary) }

        guard Darwin.link(temporary.path, leaseURL.path) == 0 else {
            if errno == EEXIST { throw MutationRecoveryLeaseError.activeLease }
            throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
        }
        guard Darwin.unlink(temporary.path) == 0 else {
            throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
        }
        try synchronizeRoot()
    }

    /// Atomically replace the bounded terminal history while the active lease
    /// still owns recovery, make that publication durable, and only then clear
    /// the active lease. The caller holds the process-shared mutation lock, so
    /// the pathname cannot become a successor transaction between comparison
    /// and unlink.
    func complete(
        _ expected: MutationRecoveryLeaseV1,
        verifiedBaseline: SnapshotV1
    ) throws -> MutationRecoveryCompletionV1 {
        guard try load() == expected else {
            throw MutationRecoveryLeaseError.bindingMismatch
        }
        let completion = try MutationRecoveryCompletionV1(
            lease: expected,
            verifiedBaseline: verifiedBaseline
        )
        let data = try BridgeJSONCodec.encoder().encode(completion)
        guard data.count <= Self.maximumLeaseBytes else {
            throw MutationRecoveryLeaseError.invalidLease
        }
        let temporary = try writeTemporary(data, prefix: ".mutation-recovery-completion")
        defer { try? fileManager.removeItem(at: temporary) }
        guard Darwin.rename(temporary.path, completionURL.path) == 0 else {
            throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
        }
        try synchronizeRoot()
        guard try load() == expected else {
            throw MutationRecoveryLeaseError.bindingMismatch
        }
        guard Darwin.unlink(leaseURL.path) == 0 else {
            throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
        }
        try synchronizeRoot()
        return completion
    }

    private func prepareRoot() throws {
        try fileManager.createDirectory(
            at: root,
            withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700]
        )
        try fileManager.setAttributes([.posixPermissions: 0o700], ofItemAtPath: root.path)
        try synchronizeDirectory(root.deletingLastPathComponent())
    }

    private func writeTemporary(_ data: Data, prefix: String) throws -> URL {
        try prepareRoot()
        let temporary = root.appendingPathComponent(
            "\(prefix).\(UUID().uuidString).tmp",
            isDirectory: false
        )
        guard fileManager.createFile(
            atPath: temporary.path,
            contents: nil,
            attributes: [.posixPermissions: 0o600]
        ) else {
            throw MutationRecoveryLeaseError.invalidLease
        }
        do {
            let handle = try FileHandle(forWritingTo: temporary)
            do {
                try handle.write(contentsOf: data)
                try handle.synchronize()
                guard Darwin.fcntl(handle.fileDescriptor, F_FULLFSYNC) == 0 else {
                    throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
                }
                try handle.close()
            } catch {
                try? handle.close()
                throw error
            }
        } catch {
            try? fileManager.removeItem(at: temporary)
            throw error
        }
        return temporary
    }

    private func synchronizeRoot() throws {
        try synchronizeDirectory(root)
    }

    private func synchronizeDirectory(_ url: URL) throws {
        let descriptor = Darwin.open(url.path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW)
        guard descriptor >= 0 else {
            throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
        }
        defer { Darwin.close(descriptor) }
        guard Darwin.fsync(descriptor) == 0 else {
            throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
        }
    }
}

extension HelperConfigurationV1 {
    func recoveryRevision() throws -> String {
        let data = try BridgeJSONCodec.encoder().encode(self)
        let digest = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        return "sha256/\(digest)"
    }
}

enum MutationRecoveryLeaseError: Error, Equatable, Sendable {
    case invalidLease
    case activeLease
    case bindingMismatch
    case lockBusy
}
