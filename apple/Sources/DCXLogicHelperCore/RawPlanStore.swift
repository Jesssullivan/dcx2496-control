import DCXLogicBridge
import Darwin
import Foundation

enum RawPlanKind: String, Sendable {
    case apply
    case rollback
}

struct RawPlanReference: Sendable {
    let kind: RawPlanKind
    let digest: String
    let url: URL
}

final class RawPlanStore: @unchecked Sendable {
    private static let maximumPlanBytes = 256 * 1_024
    private let root: URL
    private let fileManager: FileManager

    init(root: URL, fileManager: FileManager = .default) {
        self.root = root.standardizedFileURL
        self.fileManager = fileManager
    }

    func persistApply(
        _ value: JSONValue,
        expectedDevice: UInt8,
        baselineDigest: String,
        desiredSnapshotDigest: String
    ) throws -> RawPlanReference {
        let data = try BridgeJSONCodec.encoder().encode(value)
        let metadata = try BridgeJSONCodec.decoder().decode(ApplyMetadata.self, from: data)
        try metadata.validate(
            expectedDevice: expectedDevice,
            baselineDigest: baselineDigest,
            desiredSnapshotDigest: desiredSnapshotDigest
        )
        return try persist(data, digest: metadata.planDigest, kind: .apply)
    }

    func persistRollback(
        _ value: JSONValue,
        expectedDevice: UInt8,
        baselineDigest: String,
        applyPlanDigest: String
    ) throws -> RawPlanReference {
        let data = try BridgeJSONCodec.encoder().encode(value)
        let metadata = try BridgeJSONCodec.decoder().decode(RollbackMetadata.self, from: data)
        let nestedApplyData = try BridgeJSONCodec.encoder().encode(metadata.applyPlan)
        let nestedApply = try BridgeJSONCodec.decoder().decode(ApplyMetadata.self, from: nestedApplyData)
        guard metadata.schemaVersion == 1,
              metadata.applyPlanDigest == applyPlanDigest,
              nestedApply.planDigest == applyPlanDigest else {
            throw RawPlanStoreError.bindingMismatch
        }
        try nestedApply.validate(
            expectedDevice: expectedDevice,
            baselineDigest: baselineDigest,
            desiredSnapshotDigest: nestedApply.desiredSnapshotDigest
        )
        let storedApply = try loadApply(
            applyPlanDigest: applyPlanDigest,
            transactionID: applyPlanDigest,
            expectedDevice: expectedDevice,
            baselineDigest: baselineDigest,
            desiredSnapshotDigest: nestedApply.desiredSnapshotDigest
        )
        guard try Data(contentsOf: storedApply.url) == nestedApplyData else {
            throw RawPlanStoreError.bindingMismatch
        }
        return try persist(data, digest: metadata.planDigest, kind: .rollback)
    }

    /// Load an apply plan only when every request-carried identifier matches the
    /// immutable raw plan before a child process can be created.
    func loadApply(
        applyPlanDigest: String,
        transactionID: String,
        expectedDevice: UInt8,
        baselineDigest: String,
        desiredSnapshotDigest: String
    ) throws -> RawPlanReference {
        guard transactionID == applyPlanDigest else {
            throw RawPlanStoreError.bindingMismatch
        }
        let loaded = try loadData(digest: applyPlanDigest, kind: .apply)
        let metadata = try BridgeJSONCodec.decoder().decode(ApplyMetadata.self, from: loaded.data)
        guard metadata.planDigest == applyPlanDigest else {
            throw RawPlanStoreError.bindingMismatch
        }
        try metadata.validate(
            expectedDevice: expectedDevice,
            baselineDigest: baselineDigest,
            desiredSnapshotDigest: desiredSnapshotDigest
        )
        return loaded.reference
    }

    /// Load a rollback only when it matches the request baseline, transaction,
    /// apply-plan identifier, and the exact separately persisted apply carrier.
    /// The latter comparison binds the desired snapshot even though bridge v1's
    /// rollback request does not repeat that digest.
    func loadRollback(
        rollbackPlanDigest: String,
        transactionID: String,
        applyPlanDigest: String,
        expectedDevice: UInt8,
        baselineDigest: String
    ) throws -> RawPlanReference {
        guard transactionID == applyPlanDigest else {
            throw RawPlanStoreError.bindingMismatch
        }
        let loaded = try loadData(digest: rollbackPlanDigest, kind: .rollback)
        let metadata = try BridgeJSONCodec.decoder().decode(RollbackMetadata.self, from: loaded.data)
        let nestedApplyData = try BridgeJSONCodec.encoder().encode(metadata.applyPlan)
        let nestedApply = try BridgeJSONCodec.decoder().decode(ApplyMetadata.self, from: nestedApplyData)
        guard metadata.schemaVersion == 1,
              metadata.planDigest == rollbackPlanDigest,
              metadata.applyPlanDigest == applyPlanDigest,
              nestedApply.planDigest == applyPlanDigest else {
            throw RawPlanStoreError.bindingMismatch
        }
        try nestedApply.validate(
            expectedDevice: expectedDevice,
            baselineDigest: baselineDigest,
            desiredSnapshotDigest: nestedApply.desiredSnapshotDigest
        )
        let storedApply = try loadApply(
            applyPlanDigest: applyPlanDigest,
            transactionID: transactionID,
            expectedDevice: expectedDevice,
            baselineDigest: baselineDigest,
            desiredSnapshotDigest: nestedApply.desiredSnapshotDigest
        )
        guard try Data(contentsOf: storedApply.url) == nestedApplyData else {
            throw RawPlanStoreError.bindingMismatch
        }
        return loaded.reference
    }

    /// Admission-only durability barrier for a previously persisted immutable
    /// plan. Ordinary reads remain side-effect free.
    func makeDurable(_ reference: RawPlanReference) throws {
        try synchronizeFile(reference.url)
        try synchronizeDirectory(reference.url.deletingLastPathComponent())
        try synchronizeDirectory(root)
        try synchronizeDirectory(root.deletingLastPathComponent())
    }

    private func persist(_ data: Data, digest: String, kind: RawPlanKind) throws -> RawPlanReference {
        guard data.count <= Self.maximumPlanBytes else {
            throw RawPlanStoreError.planTooLarge
        }
        let leaf = try digestLeaf(digest)
        let directory = root.appendingPathComponent(kind.rawValue, isDirectory: true)
        try fileManager.createDirectory(
            at: directory,
            withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700]
        )
        try fileManager.setAttributes([.posixPermissions: 0o700], ofItemAtPath: directory.path)
        let destination = directory.appendingPathComponent("\(leaf).json", isDirectory: false)
        if !fileManager.fileExists(atPath: destination.path) {
            let temporary = directory.appendingPathComponent(
                ".\(leaf).\(UUID().uuidString).tmp",
                isDirectory: false
            )
            defer { try? fileManager.removeItem(at: temporary) }
            guard fileManager.createFile(
                atPath: temporary.path,
                contents: nil,
                attributes: [.posixPermissions: 0o600]
            ) else {
                throw RawPlanStoreError.missingPlan
            }
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
            guard Darwin.link(temporary.path, destination.path) == 0 else {
                if errno != EEXIST {
                    throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
                }
            }
            guard Darwin.unlink(temporary.path) == 0 else {
                throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
            }
        }
        guard try Data(contentsOf: destination) == data else {
            throw RawPlanStoreError.immutableCollision
        }
        try synchronizeFile(destination)
        try synchronizeDirectory(directory)
        try synchronizeDirectory(root)
        try synchronizeDirectory(root.deletingLastPathComponent())
        return .init(kind: kind, digest: digest, url: destination)
    }

    private func digestLeaf(_ digest: String) throws -> String {
        try BridgeDigest.validate(digest)
        return String(digest.dropFirst("sha256/".count)).lowercased()
    }

    private func loadData(digest: String, kind: RawPlanKind) throws -> LoadedRawPlan {
        let leaf = try digestLeaf(digest)
        let url = root
            .appendingPathComponent(kind.rawValue, isDirectory: true)
            .appendingPathComponent("\(leaf).json", isDirectory: false)
        let values = try url.resourceValues(
            forKeys: [.isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey]
        )
        let attributes = try fileManager.attributesOfItem(atPath: url.path)
        let permissions = (attributes[.posixPermissions] as? NSNumber)?.intValue
        guard values.isRegularFile == true,
              values.isSymbolicLink != true,
              let size = values.fileSize,
              size <= Self.maximumPlanBytes,
              let permissions,
              permissions & 0o077 == 0 else {
            throw RawPlanStoreError.missingPlan
        }
        return .init(
            reference: .init(kind: kind, digest: digest, url: url),
            data: try Data(contentsOf: url)
        )
    }

    private func synchronizeFile(_ url: URL) throws {
        let descriptor = Darwin.open(url.path, O_RDWR | O_CLOEXEC | O_NOFOLLOW)
        guard descriptor >= 0 else {
            throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
        }
        defer { Darwin.close(descriptor) }
        guard Darwin.fcntl(descriptor, F_FULLFSYNC) == 0 else {
            throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
        }
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

private struct LoadedRawPlan {
    let reference: RawPlanReference
    let data: Data
}

private struct ApplyMetadata: Decodable {
    let schemaVersion: UInt16
    let deviceID: UInt8
    let baseline: CoreSnapshotCarrierV1
    let desired: CoreSnapshotCarrierV1
    let baselineSnapshotDigest: String
    let desiredSnapshotDigest: String
    let planDigest: String

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case deviceID = "device_id"
        case baseline, desired
        case baselineSnapshotDigest = "baseline_snapshot_digest"
        case desiredSnapshotDigest = "desired_snapshot_digest"
        case planDigest = "plan_digest"
    }

    func validate(
        expectedDevice: UInt8,
        baselineDigest: String,
        desiredSnapshotDigest: String
    ) throws {
        guard schemaVersion == 1,
              deviceID == expectedDevice,
              baselineSnapshotDigest == baselineDigest,
              self.desiredSnapshotDigest == desiredSnapshotDigest,
              baseline.snapshotDigest == baselineDigest,
              desired.snapshotDigest == desiredSnapshotDigest else {
            throw RawPlanStoreError.bindingMismatch
        }
        try baseline.validate(expectedDevice: expectedDevice)
        try desired.validate(expectedDevice: expectedDevice)
        try BridgeDigest.validate(planDigest)
    }
}

private struct RollbackMetadata: Decodable {
    let schemaVersion: UInt16
    let applyPlan: JSONValue
    let applyPlanDigest: String
    let planDigest: String

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case applyPlan = "apply_plan"
        case applyPlanDigest = "apply_plan_digest"
        case planDigest = "plan_digest"
    }
}

enum RawPlanStoreError: Error, Equatable, Sendable {
    case bindingMismatch
    case planTooLarge
    case immutableCollision
    case missingPlan
}
