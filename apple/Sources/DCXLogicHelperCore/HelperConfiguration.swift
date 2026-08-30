import Darwin
import DCXLogicBridge
import Foundation

/// Closed helper capabilities exposed by the foreground configuration UI.
///
/// Raw values intentionally match the existing bridge operations. This type
/// keeps the containing app from needing a direct dependency on bridge internals
/// and cannot represent `helper.status`, which is always locally available.
public enum HelperFeature: String, CaseIterable, Hashable, Identifiable, Sendable {
    case identitySearch = "device.identity.search"
    case snapshotCapture = "device.snapshot.capture"
    case diffPreview = "profile.diff.preview"
    case apply = "device.apply"
    case readback = "device.readback"
    case rollback = "device.rollback"

    public var id: String { rawValue }

    public var displayName: String {
        switch self {
        case .identitySearch: "Identity search"
        case .snapshotCapture: "Snapshot capture"
        case .diffPreview: "Profile diff preview"
        case .apply: "Apply"
        case .readback: "Readback"
        case .rollback: "Rollback"
        }
    }

    fileprivate init?(_ operation: BridgeOperation) {
        switch operation {
        case .helperStatus: return nil
        case .identitySearch: self = .identitySearch
        case .snapshotCapture: self = .snapshotCapture
        case .diffPreview: self = .diffPreview
        case .apply: self = .apply
        case .readback: self = .readback
        case .rollback: self = .rollback
        }
    }

    fileprivate var operation: BridgeOperation {
        switch self {
        case .identitySearch: .identitySearch
        case .snapshotCapture: .snapshotCapture
        case .diffPreview: .diffPreview
        case .apply: .apply
        case .readback: .readback
        case .rollback: .rollback
        }
    }
}

public struct HelperConfigurationV1: Codable, Equatable, Sendable {
    public static let schemaVersion = "dcx.helper-configuration/v1"
    public static let defaultChildTimeoutSeconds: UInt8 = 90
    public static let minimumReadOnlyChildTimeoutSeconds: UInt8 = 60
    public static let minimumMutationChildTimeoutSeconds: UInt8 = 90
    public static let maximumChildTimeoutSeconds: UInt8 = 120

    private static let maximumConfigurationBytes = 32 * 1024

    public let schemaVersion: String
    public let target: DCXTargetReference
    public let ttyPath: String
    public let enabledOperations: [BridgeOperation]
    public let childTimeoutSeconds: UInt8

    public init(
        target: DCXTargetReference,
        ttyPath: String,
        enabledOperations: [BridgeOperation],
        childTimeoutSeconds: UInt8 = Self.defaultChildTimeoutSeconds
    ) throws {
        try target.validate()
        let calloutPrefix = "/dev/cu.usbserial-"
        let calloutSuffix = ttyPath.dropFirst(calloutPrefix.count)
        guard ttyPath.hasPrefix(calloutPrefix),
              !calloutSuffix.isEmpty,
              !calloutSuffix.contains("/"),
              !calloutSuffix.utf8.contains(0) else {
            throw HelperConfigurationError.invalidTTY
        }
        let permitted = Set(BridgeOperation.allCases).subtracting([.helperStatus])
        guard Set(enabledOperations).isSubset(of: permitted),
              Set(enabledOperations).count == enabledOperations.count else {
            throw HelperConfigurationError.invalidCapabilities
        }
        let mutationEnabled = enabledOperations.contains(.apply) || enabledOperations.contains(.rollback)
        let minimumTimeout = mutationEnabled
            ? Self.minimumMutationChildTimeoutSeconds
            : Self.minimumReadOnlyChildTimeoutSeconds
        guard (minimumTimeout...Self.maximumChildTimeoutSeconds).contains(childTimeoutSeconds) else {
            throw HelperConfigurationError.invalidTimeout
        }
        self.schemaVersion = Self.schemaVersion
        self.target = target
        self.ttyPath = ttyPath
        self.enabledOperations = enabledOperations
        self.childTimeoutSeconds = childTimeoutSeconds
    }

    /// Construct one checked configuration from foreground-UI values.
    public init(
        bindingID: String,
        expectedDeviceAddress: UInt8,
        ttyPath: String,
        enabledFeatures: [HelperFeature],
        childTimeoutSeconds: UInt8 = Self.defaultChildTimeoutSeconds
    ) throws {
        try self.init(
            target: DCXTargetReference(
                bindingID: bindingID,
                expectedDeviceAddress: expectedDeviceAddress
            ),
            ttyPath: ttyPath,
            enabledOperations: enabledFeatures.map(\.operation),
            childTimeoutSeconds: childTimeoutSeconds
        )
    }

    public var bindingID: String { target.bindingID }
    public var expectedDeviceAddress: UInt8 { target.expectedDeviceAddress }
    public var enabledFeatures: [HelperFeature] { enabledOperations.compactMap(HelperFeature.init) }

    public static func load(from url: URL, fileManager: FileManager = .default) throws -> Self {
        let values = try url.resourceValues(
            forKeys: [.isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey]
        )
        guard values.isRegularFile == true,
              values.isSymbolicLink != true,
              let size = values.fileSize,
              size <= Self.maximumConfigurationBytes,
              try hasOwnerOnlyPermissions(at: url, fileManager: fileManager) else {
            throw HelperConfigurationError.invalidConfigurationFile
        }
        let decoded = try BridgeJSONCodec.decoder().decode(Self.self, from: Data(contentsOf: url))
        guard decoded.schemaVersion == Self.schemaVersion else {
            throw HelperConfigurationError.unsupportedSchema
        }
        // Re-enter the checked initializer because Codable synthesis does not
        // execute it while decoding.
        try decoded.target.validate()
        return try Self(
            target: decoded.target,
            ttyPath: decoded.ttyPath,
            enabledOperations: decoded.enabledOperations,
            childTimeoutSeconds: decoded.childTimeoutSeconds
        )
    }

    /// Atomically replace the fixed App Group configuration with a checked,
    /// owner-only regular file. The temporary file is created beside the final
    /// path so the final rename cannot cross filesystems.
    public func save(to url: URL, fileManager: FileManager = .default) throws {
        guard schemaVersion == Self.schemaVersion else {
            throw HelperConfigurationError.unsupportedSchema
        }
        let checked = try Self(
            target: target,
            ttyPath: ttyPath,
            enabledOperations: enabledOperations,
            childTimeoutSeconds: childTimeoutSeconds
        )
        let data = try BridgeJSONCodec.encoder().encode(checked)
        guard data.count <= Self.maximumConfigurationBytes else {
            throw HelperConfigurationError.invalidConfigurationFile
        }

        let destination = url.standardizedFileURL
        let directory = destination.deletingLastPathComponent()
        try fileManager.createDirectory(
            at: directory,
            withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700]
        )
        try fileManager.setAttributes([.posixPermissions: 0o700], ofItemAtPath: directory.path)

        let temporary = directory.appendingPathComponent(
            ".\(destination.lastPathComponent).\(UUID().uuidString).tmp",
            isDirectory: false
        )
        var removeTemporary = true
        defer {
            if removeTemporary {
                try? fileManager.removeItem(at: temporary)
            }
        }
        guard fileManager.createFile(
            atPath: temporary.path,
            contents: nil,
            attributes: [.posixPermissions: 0o600]
        ) else {
            throw HelperConfigurationError.configurationWriteFailed
        }

        let handle = try FileHandle(forWritingTo: temporary)
        do {
            try handle.write(contentsOf: data)
            try handle.synchronize()
            try handle.close()
        } catch {
            try? handle.close()
            throw error
        }
        guard try Self.hasOwnerOnlyPermissions(at: temporary, fileManager: fileManager) else {
            throw HelperConfigurationError.configurationWriteFailed
        }

        let renameResult = temporary.path.withCString { source in
            destination.path.withCString { target in Darwin.rename(source, target) }
        }
        guard renameResult == 0 else {
            throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
        }
        removeTemporary = false
    }

    public func resolveExecutable(
        bundle: Bundle = .main,
        fileManager: FileManager = .default
    ) throws -> URL {
        let helpers = bundle.bundleURL
            .appendingPathComponent("Contents", isDirectory: true)
            .appendingPathComponent("Helpers", isDirectory: true)
            .standardizedFileURL
        let expected = helpers.appendingPathComponent("dcxctl", isDirectory: false)
        let resolved = expected.resolvingSymlinksInPath()
        guard resolved.deletingLastPathComponent() == helpers.resolvingSymlinksInPath(),
              resolved.lastPathComponent == "dcxctl",
              fileManager.isExecutableFile(atPath: resolved.path) else {
            throw HelperConfigurationError.executableUnavailable
        }
        return resolved
    }

    public func require(_ operation: BridgeOperation, target requested: DCXTargetReference) throws {
        guard requested == target else { throw HelperConfigurationError.targetNotAllowlisted }
        guard enabledOperations.contains(operation) else {
            throw HelperConfigurationError.operationUnavailable
        }
    }

    private static func hasOwnerOnlyPermissions(
        at url: URL,
        fileManager: FileManager
    ) throws -> Bool {
        let attributes = try fileManager.attributesOfItem(atPath: url.path)
        guard attributes[.type] as? FileAttributeType == .typeRegular,
              let permissions = attributes[.posixPermissions] as? NSNumber else {
            return false
        }
        return permissions.intValue & 0o077 == 0
    }
}

public enum HelperConfigurationError: Error, Equatable, Sendable {
    case executableUnavailable
    case invalidTTY
    case invalidTimeout
    case invalidCapabilities
    case invalidConfigurationFile
    case configurationWriteFailed
    case unsupportedSchema
    case targetNotAllowlisted
    case operationUnavailable
}
