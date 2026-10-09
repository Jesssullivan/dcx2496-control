import DCXLogicBridge
import Foundation

/// Helper-owned store of raw `dcx.notch-plan/v1` documents keyed by
/// their plan digest. The Audio Unit only ever names a plan by digest; a later
/// round's `--prior-plan` reads the exact bytes dcxctl produced, and dcxctl
/// re-verifies them. Nothing here opens a device.
final class NotchPlanStore: @unchecked Sendable {
    static let maximumPlanBytes = FeedbackNotchContract.maximumDocumentBytes
    private let directory: URL
    private let fileManager: FileManager

    init(planRoot: URL, fileManager: FileManager = .default) {
        directory = planRoot.standardizedFileURL.appendingPathComponent("notch", isDirectory: true)
        self.fileManager = fileManager
    }

    func persist(_ data: Data, planDigest: String) throws -> URL {
        guard data.count <= Self.maximumPlanBytes,
              try Self.claimedDigest(data) == planDigest else {
            throw NotchPlanStoreError.invalidPlan
        }
        try fileManager.createDirectory(
            at: directory,
            withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700]
        )
        let destination = try url(for: planDigest)
        // First stored serialization wins: the bytes behind a staged plan
        // never change. An equal-digest re-serialization (for example after a
        // formatting change) reuses them; dcxctl re-verifies on every read.
        if fileManager.fileExists(atPath: destination.path) {
            return destination
        }
        try data.write(to: destination, options: [.atomic, .completeFileProtection])
        try fileManager.setAttributes([.posixPermissions: 0o600], ofItemAtPath: destination.path)
        return destination
    }

    func load(planDigest: String) throws -> URL {
        let url = try url(for: planDigest)
        guard let values = try? url.resourceValues(
            forKeys: [.isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey]
        ), values.isRegularFile == true, values.isSymbolicLink != true,
              let size = values.fileSize, size <= Self.maximumPlanBytes else {
            throw NotchPlanStoreError.missingPlan
        }
        guard try Self.claimedDigest(Data(contentsOf: url)) == planDigest else {
            throw NotchPlanStoreError.invalidPlan
        }
        return url
    }

    private func url(for planDigest: String) throws -> URL {
        try BridgeDigest.validate(planDigest)
        let leaf = String(planDigest.dropFirst("sha256/".count))
        return directory.appendingPathComponent("\(leaf).json", isDirectory: false)
    }

    private static func claimedDigest(_ data: Data) throws -> String {
        struct Claim: Decodable {
            let schemaVersion: String
            let planDigest: String
            enum CodingKeys: String, CodingKey {
                case schemaVersion = "schema_version"
                case planDigest = "plan_digest"
            }
        }
        let claim = try BridgeJSONCodec.decoder().decode(Claim.self, from: data)
        guard claim.schemaVersion == FeedbackNotchContract.notchPlanSchemaVersion else {
            throw NotchPlanStoreError.invalidPlan
        }
        try BridgeDigest.validate(claim.planDigest)
        return claim.planDigest
    }
}

enum NotchPlanStoreError: Error, Equatable, Sendable {
    case invalidPlan
    case missingPlan
}
