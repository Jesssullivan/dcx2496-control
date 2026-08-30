import Foundation

public enum BridgeOperation: String, Codable, CaseIterable, Hashable, Sendable {
    case helperStatus = "helper.status"
    case identitySearch = "device.identity.search"
    case snapshotCapture = "device.snapshot.capture"
    case diffPreview = "profile.diff.preview"
    case apply = "device.apply"
    case readback = "device.readback"
    case rollback = "device.rollback"

    /// Device mutation is exposed only with both verification and recovery.
    public static let mutationCapabilities: Set<BridgeOperation> = [.apply, .readback, .rollback]

    public static func hasValidMutationCapabilities(_ capabilities: Set<BridgeOperation>) -> Bool {
        let selected = capabilities.intersection(mutationCapabilities)
        return selected.isEmpty || selected == mutationCapabilities
    }
}

public struct HelperStatusRequest: Codable, Equatable, Sendable {
    public init() {}
}

public struct IdentitySearchRequest: Codable, Equatable, Sendable {
    public let target: DCXTargetReference
    public init(target: DCXTargetReference) { self.target = target }
}

public struct SnapshotCaptureRequest: Codable, Equatable, Sendable {
    public let target: DCXTargetReference
    public init(target: DCXTargetReference) { self.target = target }
}

public struct DiffPreviewRequest: Codable, Equatable, Sendable {
    public let target: DCXTargetReference
    public let baseline: SnapshotV1
    public let desired: DesiredProfileV1

    public init(target: DCXTargetReference, baseline: SnapshotV1, desired: DesiredProfileV1) {
        self.target = target
        self.baseline = baseline
        self.desired = desired
    }
}

public struct ApplyRequest: Codable, Equatable, Sendable {
    public let target: DCXTargetReference
    public let plan: ApplyPlanV1
    public init(target: DCXTargetReference, plan: ApplyPlanV1) {
        self.target = target
        self.plan = plan
    }
}

public struct ReadbackRequest: Codable, Equatable, Sendable {
    public let target: DCXTargetReference
    public let transactionID: String
    public let expectedDesiredDigest: String

    public init(target: DCXTargetReference, transactionID: String, expectedDesiredDigest: String) throws {
        guard !transactionID.isEmpty, transactionID.utf8.count <= 128 else {
            throw BridgeValidationError.invalidApplyBinding
        }
        try BridgeDigest.validate(expectedDesiredDigest)
        self.target = target
        self.transactionID = transactionID
        self.expectedDesiredDigest = expectedDesiredDigest
    }
}

public struct RollbackRequest: Codable, Equatable, Sendable {
    public let target: DCXTargetReference
    public let plan: RollbackPlanV1
    public init(target: DCXTargetReference, plan: RollbackPlanV1) {
        self.target = target
        self.plan = plan
    }
}

public enum BridgeRequestBody: Equatable, Sendable {
    case helperStatus(HelperStatusRequest)
    case identitySearch(IdentitySearchRequest)
    case snapshotCapture(SnapshotCaptureRequest)
    case diffPreview(DiffPreviewRequest)
    case apply(ApplyRequest)
    case readback(ReadbackRequest)
    case rollback(RollbackRequest)

    public var operation: BridgeOperation {
        switch self {
        case .helperStatus: .helperStatus
        case .identitySearch: .identitySearch
        case .snapshotCapture: .snapshotCapture
        case .diffPreview: .diffPreview
        case .apply: .apply
        case .readback: .readback
        case .rollback: .rollback
        }
    }

    public func validate() throws {
        switch self {
        case .helperStatus:
            break
        case let .identitySearch(value):
            try value.target.validate()
        case let .snapshotCapture(value):
            try value.target.validate()
        case let .diffPreview(value):
            try value.target.validate()
            try value.baseline.validate()
            try value.desired.validate()
            guard value.baseline.target == value.target else {
                throw BridgeValidationError.invalidTarget("baseline target does not match request")
            }
        case let .apply(value):
            try value.target.validate()
            try value.plan.validate()
            guard value.plan.baseline.target == value.target else {
                throw BridgeValidationError.invalidApplyBinding
            }
        case let .readback(value):
            try value.target.validate()
            guard !value.transactionID.isEmpty, value.transactionID.utf8.count <= 128 else {
                throw BridgeValidationError.invalidApplyBinding
            }
            try BridgeDigest.validate(value.expectedDesiredDigest)
        case let .rollback(value):
            try value.target.validate()
            try value.plan.validate()
            guard value.plan.baseline.target == value.target else {
                throw BridgeValidationError.invalidRollbackBinding
            }
        }
    }
}

public struct BridgeRequest: Codable, Equatable, Sendable {
    public let schemaVersion: String
    public let requestID: String
    public let operation: BridgeOperation
    public let body: BridgeRequestBody

    public init(requestID: String = UUID().uuidString, body: BridgeRequestBody) throws {
        guard !requestID.isEmpty, requestID.utf8.count <= 128 else {
            throw BridgeMessageError.invalidRequestID
        }
        try body.validate()
        self.schemaVersion = DCXBridgeContract.schemaVersion
        self.requestID = requestID
        self.operation = body.operation
        self.body = body
    }

    private enum CodingKeys: String, CodingKey {
        case schemaVersion, requestID, operation, payload
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        schemaVersion = try container.decode(String.self, forKey: .schemaVersion)
        guard schemaVersion == DCXBridgeContract.schemaVersion else {
            throw BridgeMessageError.unsupportedSchema(schemaVersion)
        }
        requestID = try container.decode(String.self, forKey: .requestID)
        guard !requestID.isEmpty, requestID.utf8.count <= 128 else {
            throw BridgeMessageError.invalidRequestID
        }
        operation = try container.decode(BridgeOperation.self, forKey: .operation)
        switch operation {
        case .helperStatus:
            body = .helperStatus(try container.decode(HelperStatusRequest.self, forKey: .payload))
        case .identitySearch:
            body = .identitySearch(try container.decode(IdentitySearchRequest.self, forKey: .payload))
        case .snapshotCapture:
            body = .snapshotCapture(try container.decode(SnapshotCaptureRequest.self, forKey: .payload))
        case .diffPreview:
            body = .diffPreview(try container.decode(DiffPreviewRequest.self, forKey: .payload))
        case .apply:
            body = .apply(try container.decode(ApplyRequest.self, forKey: .payload))
        case .readback:
            body = .readback(try container.decode(ReadbackRequest.self, forKey: .payload))
        case .rollback:
            body = .rollback(try container.decode(RollbackRequest.self, forKey: .payload))
        }
        try body.validate()
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(schemaVersion, forKey: .schemaVersion)
        try container.encode(requestID, forKey: .requestID)
        try container.encode(operation, forKey: .operation)
        switch body {
        case let .helperStatus(payload): try container.encode(payload, forKey: .payload)
        case let .identitySearch(payload): try container.encode(payload, forKey: .payload)
        case let .snapshotCapture(payload): try container.encode(payload, forKey: .payload)
        case let .diffPreview(payload): try container.encode(payload, forKey: .payload)
        case let .apply(payload): try container.encode(payload, forKey: .payload)
        case let .readback(payload): try container.encode(payload, forKey: .payload)
        case let .rollback(payload): try container.encode(payload, forKey: .payload)
        }
    }
}

public struct CommandReceiptV1: Codable, Equatable, Sendable {
    public let operation: BridgeOperation
    public let exitCode: Int32
    public let durationMilliseconds: UInt64
    public let stdoutDigest: String

    public init(
        operation: BridgeOperation,
        exitCode: Int32,
        durationMilliseconds: UInt64,
        stdoutDigest: String
    ) {
        self.operation = operation
        self.exitCode = exitCode
        self.durationMilliseconds = durationMilliseconds
        self.stdoutDigest = stdoutDigest
    }
}

public struct CoreMIDIEndpointStatusV1: Codable, Equatable, Sendable {
    public let commandsName: String
    public let commandsUniqueID: Int32
    public let statusName: String
    public let statusUniqueID: Int32
    public let online: Bool

    public init(
        commandsName: String,
        commandsUniqueID: Int32,
        statusName: String,
        statusUniqueID: Int32,
        online: Bool
    ) {
        self.commandsName = commandsName
        self.commandsUniqueID = commandsUniqueID
        self.statusName = statusName
        self.statusUniqueID = statusUniqueID
        self.online = online
    }
}

public struct HelperStatusResponse: Codable, Equatable, Sendable {
    public let foreground: Bool
    public let configured: Bool
    public let target: DCXTargetReference?
    public let capabilities: [BridgeOperation]
    public let coreMIDI: CoreMIDIEndpointStatusV1
    public let activeTransactionID: String?

    public init(
        foreground: Bool,
        configured: Bool,
        target: DCXTargetReference?,
        capabilities: [BridgeOperation],
        coreMIDI: CoreMIDIEndpointStatusV1,
        activeTransactionID: String?
    ) {
        self.foreground = foreground
        self.configured = configured
        self.target = target
        self.capabilities = capabilities
        self.coreMIDI = coreMIDI
        self.activeTransactionID = activeTransactionID
    }
}

public struct IdentitySearchResponse: Codable, Equatable, Sendable {
    public let identity: DeviceIdentityV1
    public let receipt: CommandReceiptV1
    public init(identity: DeviceIdentityV1, receipt: CommandReceiptV1) {
        self.identity = identity
        self.receipt = receipt
    }
}

public struct SnapshotCaptureResponse: Codable, Equatable, Sendable {
    public let snapshot: SnapshotV1
    public let receipt: CommandReceiptV1
    public init(snapshot: SnapshotV1, receipt: CommandReceiptV1) {
        self.snapshot = snapshot
        self.receipt = receipt
    }
}

public struct DiffPreviewResponse: Codable, Equatable, Sendable {
    public let diff: SemanticDiffV1
    public let receipt: CommandReceiptV1
    public init(diff: SemanticDiffV1, receipt: CommandReceiptV1) {
        self.diff = diff
        self.receipt = receipt
    }
}

public struct ApplyResponse: Codable, Equatable, Sendable {
    public let transactionID: String
    public let baselineDigest: String
    public let desiredSnapshotDigest: String
    public let readback: SnapshotV1?
    public let readbackMatchesDesired: Bool
    public let rollbackRequired: Bool
    public let rollbackAvailable: Bool
    public let receipt: CommandReceiptV1

    public init(
        transactionID: String,
        baselineDigest: String,
        desiredSnapshotDigest: String,
        readback: SnapshotV1?,
        readbackMatchesDesired: Bool,
        rollbackRequired: Bool,
        rollbackAvailable: Bool,
        receipt: CommandReceiptV1
    ) {
        self.transactionID = transactionID
        self.baselineDigest = baselineDigest
        self.desiredSnapshotDigest = desiredSnapshotDigest
        self.readback = readback
        self.readbackMatchesDesired = readbackMatchesDesired
        self.rollbackRequired = rollbackRequired
        self.rollbackAvailable = rollbackAvailable
        self.receipt = receipt
    }
}

public struct ReadbackResponse: Codable, Equatable, Sendable {
    public let transactionID: String
    public let matchesDesired: Bool
    public let snapshot: SnapshotV1
    public let receipt: CommandReceiptV1

    public init(
        transactionID: String,
        matchesDesired: Bool,
        snapshot: SnapshotV1,
        receipt: CommandReceiptV1
    ) {
        self.transactionID = transactionID
        self.matchesDesired = matchesDesired
        self.snapshot = snapshot
        self.receipt = receipt
    }
}

public struct RollbackResponse: Codable, Equatable, Sendable {
    public let transactionID: String
    public let baselineDigest: String
    public let restored: SnapshotV1?
    public let equalsBaseline: Bool
    public let receipt: CommandReceiptV1

    public init(
        transactionID: String,
        baselineDigest: String,
        restored: SnapshotV1?,
        equalsBaseline: Bool,
        receipt: CommandReceiptV1
    ) {
        self.transactionID = transactionID
        self.baselineDigest = baselineDigest
        self.restored = restored
        self.equalsBaseline = equalsBaseline
        self.receipt = receipt
    }
}

public enum BridgeResponseBody: Equatable, Sendable {
    case helperStatus(HelperStatusResponse)
    case identitySearch(IdentitySearchResponse)
    case snapshotCapture(SnapshotCaptureResponse)
    case diffPreview(DiffPreviewResponse)
    case apply(ApplyResponse)
    case readback(ReadbackResponse)
    case rollback(RollbackResponse)

    public var operation: BridgeOperation {
        switch self {
        case .helperStatus: .helperStatus
        case .identitySearch: .identitySearch
        case .snapshotCapture: .snapshotCapture
        case .diffPreview: .diffPreview
        case .apply: .apply
        case .readback: .readback
        case .rollback: .rollback
        }
    }

    public func validate() throws {
        switch self {
        case let .helperStatus(value):
            try value.target?.validate()
            let capabilities = Set(value.capabilities)
            guard capabilities.count == value.capabilities.count,
                  capabilities.contains(.helperStatus),
                  BridgeOperation.hasValidMutationCapabilities(capabilities),
                  value.configured == (value.target != nil),
                  value.coreMIDI.commandsName == "Tinyland DCX Commands",
                  value.coreMIDI.commandsUniqueID == 0x4443_5843,
                  value.coreMIDI.statusName == "Tinyland DCX Status",
                  value.coreMIDI.statusUniqueID == 0x4443_5853 else {
                throw BridgeMessageError.invalidResponse
            }
            if let transactionID = value.activeTransactionID {
                try Self.validateTransactionID(transactionID)
            }
        case let .identitySearch(value):
            try value.identity.validate()
            try Self.validateReceipt(value.receipt, operation: .identitySearch)
        case let .snapshotCapture(value):
            try value.snapshot.validate()
            try Self.validateReceipt(value.receipt, operation: .snapshotCapture)
        case let .diffPreview(value):
            try value.diff.validate()
            try Self.validateReceipt(value.receipt, operation: .diffPreview)
        case let .apply(value):
            try Self.validateTransactionID(value.transactionID)
            try BridgeDigest.validate(value.baselineDigest)
            try BridgeDigest.validate(value.desiredSnapshotDigest)
            try value.readback?.validate()
            let exact = value.readback?.digest == value.desiredSnapshotDigest
            guard value.readbackMatchesDesired == exact,
                  value.rollbackRequired == !exact,
                  exact || value.rollbackAvailable,
                  value.readback != nil || value.rollbackRequired else {
                throw BridgeMessageError.invalidResponse
            }
            try Self.validateReceipt(value.receipt, operation: .apply)
        case let .readback(value):
            try Self.validateTransactionID(value.transactionID)
            try value.snapshot.validate()
            try Self.validateReceipt(value.receipt, operation: .readback)
        case let .rollback(value):
            try Self.validateTransactionID(value.transactionID)
            try BridgeDigest.validate(value.baselineDigest)
            try value.restored?.validate()
            guard value.equalsBaseline == (value.restored?.digest == value.baselineDigest),
                  value.restored != nil || !value.equalsBaseline else {
                throw BridgeMessageError.invalidResponse
            }
            try Self.validateReceipt(value.receipt, operation: .rollback)
        }
    }

    private static func validateTransactionID(_ value: String) throws {
        guard !value.isEmpty, value.utf8.count <= 128 else {
            throw BridgeMessageError.invalidResponse
        }
    }

    private static func validateReceipt(
        _ value: CommandReceiptV1,
        operation: BridgeOperation
    ) throws {
        guard value.operation == operation, value.exitCode == 0 else {
            throw BridgeMessageError.invalidResponse
        }
        try BridgeDigest.validate(value.stdoutDigest)
    }
}

public struct BridgeErrorPayload: Codable, Equatable, Sendable {
    public enum Code: String, Codable, Sendable {
        case invalidRequest = "invalid_request"
        case unsupportedSchema = "unsupported_schema"
        case helperNotForeground = "helper_not_foreground"
        case helperNotConfigured = "helper_not_configured"
        case targetNotAllowlisted = "target_not_allowlisted"
        case operationUnavailable = "operation_unavailable"
        case operationInFlight = "operation_in_flight"
        case childTimedOut = "child_timed_out"
        case childFailed = "child_failed"
        case malformedChildResponse = "malformed_child_response"
        case ipcUnavailable = "ipc_unavailable"
        case internalFailure = "internal_failure"
    }

    public let code: Code
    public let message: String
    public let retryable: Bool

    public init(code: Code, message: String, retryable: Bool) {
        self.code = code
        self.message = String(message.prefix(512))
        self.retryable = retryable
    }

    public func validate() throws {
        guard message.count <= 512 else { throw BridgeMessageError.invalidResponse }
    }
}

public struct BridgeResponse: Codable, Equatable, Sendable {
    public enum Status: String, Codable, Sendable { case ok, error }

    public let schemaVersion: String
    public let requestID: String
    public let operation: BridgeOperation?
    public let status: Status
    public let body: BridgeResponseBody?
    public let error: BridgeErrorPayload?

    public init(requestID: String, body: BridgeResponseBody) {
        schemaVersion = DCXBridgeContract.schemaVersion
        self.requestID = requestID
        operation = body.operation
        status = .ok
        self.body = body
        error = nil
    }

    public init(requestID: String, operation: BridgeOperation?, error: BridgeErrorPayload) {
        schemaVersion = DCXBridgeContract.schemaVersion
        self.requestID = requestID
        self.operation = operation
        status = .error
        body = nil
        self.error = error
    }

    private enum CodingKeys: String, CodingKey {
        case schemaVersion, requestID, operation, status, payload, error
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        schemaVersion = try container.decode(String.self, forKey: .schemaVersion)
        guard schemaVersion == DCXBridgeContract.schemaVersion else {
            throw BridgeMessageError.unsupportedSchema(schemaVersion)
        }
        requestID = try container.decode(String.self, forKey: .requestID)
        guard !requestID.isEmpty, requestID.utf8.count <= 128 else {
            throw BridgeMessageError.invalidRequestID
        }
        operation = try container.decodeIfPresent(BridgeOperation.self, forKey: .operation)
        status = try container.decode(Status.self, forKey: .status)
        if status == .error {
            body = nil
            error = try container.decode(BridgeErrorPayload.self, forKey: .error)
            try error?.validate()
            return
        }
        guard let operation else { throw BridgeMessageError.missingOperation }
        error = nil
        switch operation {
        case .helperStatus:
            body = .helperStatus(try container.decode(HelperStatusResponse.self, forKey: .payload))
        case .identitySearch:
            body = .identitySearch(try container.decode(IdentitySearchResponse.self, forKey: .payload))
        case .snapshotCapture:
            body = .snapshotCapture(try container.decode(SnapshotCaptureResponse.self, forKey: .payload))
        case .diffPreview:
            body = .diffPreview(try container.decode(DiffPreviewResponse.self, forKey: .payload))
        case .apply:
            body = .apply(try container.decode(ApplyResponse.self, forKey: .payload))
        case .readback:
            body = .readback(try container.decode(ReadbackResponse.self, forKey: .payload))
        case .rollback:
            body = .rollback(try container.decode(RollbackResponse.self, forKey: .payload))
        }
        try body?.validate()
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(schemaVersion, forKey: .schemaVersion)
        try container.encode(requestID, forKey: .requestID)
        try container.encodeIfPresent(operation, forKey: .operation)
        try container.encode(status, forKey: .status)
        try container.encodeIfPresent(error, forKey: .error)
        switch body {
        case let .helperStatus(payload): try container.encode(payload, forKey: .payload)
        case let .identitySearch(payload): try container.encode(payload, forKey: .payload)
        case let .snapshotCapture(payload): try container.encode(payload, forKey: .payload)
        case let .diffPreview(payload): try container.encode(payload, forKey: .payload)
        case let .apply(payload): try container.encode(payload, forKey: .payload)
        case let .readback(payload): try container.encode(payload, forKey: .payload)
        case let .rollback(payload): try container.encode(payload, forKey: .payload)
        case nil: break
        }
    }
}

public enum BridgeMessageError: Error, Equatable, Sendable {
    case unsupportedSchema(String)
    case invalidRequestID
    case missingOperation
    case invalidResponse
}
