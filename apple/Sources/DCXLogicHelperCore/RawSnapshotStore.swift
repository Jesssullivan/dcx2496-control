import CryptoKit
import DCXLogicBridge
import Foundation

struct CoreSnapshotCarrierV1: Codable, Equatable, Sendable {
    struct Image: Codable, Equatable, Sendable {
        let section: String
        let frame: [UInt8]
        let digest: String
    }

    let schemaVersion: UInt16
    let deviceID: UInt8
    let identity: Image
    let dump0: Image
    let dump1: Image
    let snapshotDigest: String

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case deviceID = "device_id"
        case identity, dump0, dump1
        case snapshotDigest = "snapshot_digest"
    }

    func validate(expectedDevice: UInt8) throws {
        guard schemaVersion == 1, deviceID == expectedDevice,
              identity.section == "identity", identity.frame.count == 26,
              dump0.section == "dump0", dump0.frame.count == 1_015,
              dump1.section == "dump1", dump1.frame.count == 911 else {
            throw RawSnapshotError.invalidCarrier
        }
        for image in [identity, dump0, dump1] {
            try BridgeDigest.validate(image.digest)
            guard image.digest == Self.digest(Data(image.frame)) else {
                throw RawSnapshotError.digestMismatch
            }
        }
        try BridgeDigest.validate(snapshotDigest)

        var canonical = Data("dcx2496.snapshot/v1\0".utf8)
        var schema = schemaVersion.bigEndian
        canonical.append(Data(bytes: &schema, count: MemoryLayout<UInt16>.size))
        canonical.append(deviceID)
        for (tag, image) in [(UInt8(0), identity), (1, dump0), (2, dump1)] {
            canonical.append(tag)
            var length = UInt32(image.frame.count).bigEndian
            canonical.append(Data(bytes: &length, count: MemoryLayout<UInt32>.size))
            canonical.append(contentsOf: image.frame)
        }
        guard snapshotDigest == Self.digest(canonical) else {
            throw RawSnapshotError.digestMismatch
        }
    }

    func summary(
        target: DCXTargetReference,
        validSearchResponses: UInt8,
        capturedAt: Date = Date()
    ) throws -> SnapshotV1 {
        try validate(expectedDevice: target.expectedDeviceAddress)
        return try SnapshotV1(
            target: target,
            identity: .init(
                manufacturer: "Behringer",
                model: "DCX2496",
                deviceAddress: deviceID,
                selectedBaud: 38_400,
                validSearchResponses: validSearchResponses
            ),
            capturedAt: capturedAt,
            digest: snapshotDigest,
            complete: true,
            sectionDigests: .init(
                identity: identity.digest,
                dump0: dump0.digest,
                dump1: dump1.digest
            )
        )
    }

    private static func digest(_ data: Data) -> String {
        "sha256/" + SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }
}

final class RawSnapshotStore: @unchecked Sendable {
    private static let maximumSnapshotBytes = 64 * 1_024
    private let root: URL
    private let fileManager: FileManager

    init(root: URL, fileManager: FileManager = .default) {
        self.root = root.standardizedFileURL
        self.fileManager = fileManager
    }

    func persist(_ data: Data, carrier: CoreSnapshotCarrierV1) throws -> URL {
        guard data.count <= Self.maximumSnapshotBytes else {
            throw RawSnapshotError.invalidCarrier
        }
        try fileManager.createDirectory(
            at: root,
            withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700]
        )
        let leaf = try digestLeaf(carrier.snapshotDigest)
        let destination = root.appendingPathComponent("\(leaf).json", isDirectory: false)
        if fileManager.fileExists(atPath: destination.path) {
            guard try Data(contentsOf: destination) == data else {
                throw RawSnapshotError.immutableCollision
            }
            return destination
        }
        try data.write(to: destination, options: [.atomic, .completeFileProtection])
        try fileManager.setAttributes([.posixPermissions: 0o600], ofItemAtPath: destination.path)
        return destination
    }

    func load(digest: String, expectedDevice: UInt8) throws -> (URL, CoreSnapshotCarrierV1) {
        let leaf = try digestLeaf(digest)
        let url = root.appendingPathComponent("\(leaf).json", isDirectory: false)
        let values = try url.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey])
        guard values.isRegularFile == true,
              let size = values.fileSize,
              size <= Self.maximumSnapshotBytes else {
            throw RawSnapshotError.missingSnapshot
        }
        let data = try Data(contentsOf: url)
        let carrier = try BridgeJSONCodec.decoder().decode(CoreSnapshotCarrierV1.self, from: data)
        try carrier.validate(expectedDevice: expectedDevice)
        guard carrier.snapshotDigest == digest else { throw RawSnapshotError.digestMismatch }
        return (url, carrier)
    }

    private func digestLeaf(_ digest: String) throws -> String {
        try BridgeDigest.validate(digest)
        let leaf = String(digest.dropFirst("sha256/".count)).lowercased()
        guard leaf.count == 64, leaf.allSatisfy(\.isHexDigit) else {
            throw RawSnapshotError.invalidCarrier
        }
        return leaf
    }
}

enum RawSnapshotError: Error, Equatable, Sendable {
    case invalidCarrier
    case digestMismatch
    case immutableCollision
    case missingSnapshot
}
