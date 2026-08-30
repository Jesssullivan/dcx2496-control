import DCXLogicBridge
import Foundation

public final class DCXHelperCoordinator: @unchecked Sendable {
    private let locations: AppGroupLocations
    private var configuration: HelperConfigurationV1?
    private var configurationError: Error?
    private let coreMIDI: CoreMIDIPresentation
    private let runner = DCXCTLProcessRunner()
    private let snapshotStore: RawSnapshotStore
    private let planStore: RawPlanStore
    private let stateLock = NSLock()
    private var foreground = false
    private var activeTransactionID: String?

    public init(
        locations: AppGroupLocations,
        configuration: Result<HelperConfigurationV1, Error>,
        coreMIDI: CoreMIDIPresentation
    ) {
        self.locations = locations
        snapshotStore = .init(root: locations.snapshotRootURL)
        planStore = .init(root: locations.planRootURL)
        switch configuration {
        case let .success(value):
            self.configuration = value
            configurationError = nil
        case let .failure(error):
            self.configuration = nil
            configurationError = error
        }
        self.coreMIDI = coreMIDI
    }

    public func setForeground(_ value: Bool) {
        stateLock.lock()
        foreground = value
        stateLock.unlock()
    }

    func replaceConfiguration(_ result: Result<HelperConfigurationV1, Error>) {
        stateLock.lock()
        switch result {
        case let .success(value):
            configuration = value
            configurationError = nil
        case let .failure(error):
            configuration = nil
            configurationError = error
        }
        stateLock.unlock()
    }

    func currentConfiguration() -> HelperConfigurationV1? {
        stateLock.lock()
        defer { stateLock.unlock() }
        return configuration
    }

    public func handle(_ request: BridgeRequest) -> BridgeResponse {
        do {
            try request.body.validate()
        } catch {
            return failure(request, .invalidRequest, "request failed bridge validation", false)
        }
        if case .helperStatus = request.body {
            return BridgeResponse(requestID: request.requestID, body: .helperStatus(status()))
        }
        guard isForeground else {
            return failure(request, .helperNotForeground, "helper must be open and foreground", true)
        }
        guard let configuration = configurationSnapshot() else {
            return failure(request, .helperNotConfigured, "helper configuration is unavailable", false)
        }

        let target: DCXTargetReference
        switch request.body {
        case .helperStatus:
            target = configuration.target
        case let .identitySearch(value): target = value.target
        case let .snapshotCapture(value): target = value.target
        case let .diffPreview(value): target = value.target
        case let .apply(value): target = value.target
        case let .readback(value): target = value.target
        case let .rollback(value): target = value.target
        }

        do {
            try configuration.require(request.operation, target: target)
        } catch HelperConfigurationError.targetNotAllowlisted {
            return failure(request, .targetNotAllowlisted, "requested target is not configured", false)
        } catch {
            return failure(request, .operationUnavailable, "operation is not enabled", false)
        }

        let activeID = UUID().uuidString
        stateLock.lock()
        guard activeTransactionID == nil else {
            stateLock.unlock()
            return failure(request, .operationInFlight, "one helper transaction is already active", true)
        }
        activeTransactionID = activeID
        stateLock.unlock()
        defer {
            stateLock.lock()
            activeTransactionID = nil
            stateLock.unlock()
        }

        do {
            let workspace = try TransactionWorkspace(root: locations.transactionRootURL)
            defer { workspace.remove() }
            let invocation = try buildInvocation(
                request: request,
                configuration: configuration,
                workspace: workspace
            )
            let result = try runner.run(invocation, timeoutSeconds: configuration.childTimeoutSeconds)
            guard result.terminationStatus == 0 else {
                return failure(
                    request,
                    .childFailed,
                    "dcxctl exited with status \(result.terminationStatus)",
                    false
                )
            }
            return try decode(request: request, result: result)
        } catch DCXCTLRunnerError.timedOut {
            return failure(request, .childTimedOut, "dcxctl exceeded its configured deadline", true)
        } catch ChildResponseError.requestPlanBinding {
            return failure(
                request,
                .invalidRequest,
                "stored plan does not match the request bindings",
                false
            )
        } catch HelperConfigurationError.executableUnavailable {
            return failure(request, .helperNotConfigured, "bundled dcxctl is unavailable", false)
        } catch {
            return failure(request, .malformedChildResponse, "bounded command did not return its expected schema", false)
        }
    }

    private var isForeground: Bool {
        stateLock.lock()
        defer { stateLock.unlock() }
        return foreground
    }

    private func configurationSnapshot() -> HelperConfigurationV1? {
        stateLock.lock()
        defer { stateLock.unlock() }
        _ = configurationError
        return configuration
    }

    private func status() -> HelperStatusResponse {
        stateLock.lock()
        let foreground = foreground
        let active = activeTransactionID
        let configuration = configuration
        stateLock.unlock()
        return .init(
            foreground: foreground,
            configured: configuration != nil,
            target: configuration?.target,
            capabilities: [.helperStatus] + (configuration?.enabledOperations ?? []),
            coreMIDI: coreMIDI.bridgeStatus(),
            activeTransactionID: active
        )
    }

    private func buildInvocation(
        request: BridgeRequest,
        configuration: HelperConfigurationV1,
        workspace: TransactionWorkspace
    ) throws -> DCXCTLInvocation {
        let executable = try configuration.resolveExecutable()
        let targetArguments = [
            "--tty", configuration.ttyPath,
            "--expected-device", String(configuration.target.expectedDeviceAddress),
        ]
        let arguments: [String]
        switch request.body {
        case .helperStatus:
            throw HelperConfigurationError.operationUnavailable
        case .identitySearch:
            arguments = ["discovery", "live-search"] + targetArguments
        case .snapshotCapture:
            arguments = ["control", "snapshot"] + targetArguments
        case let .diffPreview(value):
            let (snapshot, _) = try snapshotStore.load(
                digest: value.baseline.digest,
                expectedDevice: value.target.expectedDeviceAddress
            )
            // dcxctl validates the versioned profile envelope and its strict
            // O1/PEQ9 document. The document remains unchanged inside it.
            let profile = try workspace.write(value.desired, named: "profile.json")
            arguments = [
                "control", "diff",
                "--snapshot", snapshot.path,
                "--profile", profile.path,
            ]
        case let .apply(value):
            let applyPlanDigest = value.plan.diff.applyPlanDigest
            let plan: RawPlanReference
            do {
                plan = try planStore.loadApply(
                    applyPlanDigest: applyPlanDigest,
                    transactionID: applyPlanDigest,
                    expectedDevice: value.target.expectedDeviceAddress,
                    baselineDigest: value.plan.baseline.digest,
                    desiredSnapshotDigest: value.plan.diff.desiredSnapshotDigest
                )
            } catch {
                throw ChildResponseError.requestPlanBinding
            }
            arguments = ["control", "apply"] + targetArguments + ["--plan", plan.url.path]
        case .readback:
            arguments = ["control", "readback"] + targetArguments
        case let .rollback(value):
            let plan: RawPlanReference
            do {
                plan = try planStore.loadRollback(
                    rollbackPlanDigest: value.plan.rollbackPlanDigest,
                    transactionID: value.plan.transactionID,
                    applyPlanDigest: value.plan.transactionID,
                    expectedDevice: value.target.expectedDeviceAddress,
                    baselineDigest: value.plan.baseline.digest
                )
            } catch {
                throw ChildResponseError.requestPlanBinding
            }
            arguments = ["control", "rollback"] + targetArguments + ["--plan", plan.url.path]
        }
        return .init(operation: request.operation, executableURL: executable, arguments: arguments)
    }

    private func decode(
        request: BridgeRequest,
        result: DCXCTLProcessResult
    ) throws -> BridgeResponse {
        let decoder = BridgeJSONCodec.decoder()
        let receipt = result.receipt(for: request.operation)
        let body: BridgeResponseBody
        switch request.body {
        case .helperStatus:
            throw HelperConfigurationError.operationUnavailable
        case let .identitySearch(value):
            let output = try decoder.decode(LiveSearchOutput.self, from: result.stdout)
            guard output.status == "identified", output.selectedBaud == 38_400,
                  output.validResponses == 10,
                  output.protocolIdentity.deviceAddress == value.target.expectedDeviceAddress else {
                throw ChildResponseError.invalidSearch
            }
            let identity = DeviceIdentityV1(
                manufacturer: output.protocolIdentity.manufacturer,
                model: output.protocolIdentity.model,
                deviceAddress: output.protocolIdentity.deviceAddress,
                selectedBaud: output.selectedBaud,
                validSearchResponses: output.validResponses
            )
            try identity.validate()
            body = .identitySearch(.init(identity: identity, receipt: receipt))
        case let .snapshotCapture(value):
            let snapshot = try ingestSnapshot(
                result.stdout,
                target: value.target,
                validSearchResponses: 10
            )
            body = .snapshotCapture(.init(snapshot: snapshot, receipt: receipt))
        case let .diffPreview(value):
            let output = try decoder.decode(DiffCommandOutput.self, from: result.stdout)
            guard output.baselineSnapshotDigest == value.baseline.digest,
                  output.desiredProfileDigest == value.desired.digest else {
                throw ChildResponseError.bindingMismatch
            }
            let apply = try planStore.persistApply(
                output.applyPlan,
                expectedDevice: value.target.expectedDeviceAddress,
                baselineDigest: value.baseline.digest,
                desiredSnapshotDigest: output.desiredSnapshotDigest
            )
            let rollback = try planStore.persistRollback(
                output.rollbackPlan,
                expectedDevice: value.target.expectedDeviceAddress,
                baselineDigest: value.baseline.digest,
                applyPlanDigest: apply.digest
            )
            let diff = try SemanticDiffV1(
                baselineSnapshotDigest: output.baselineSnapshotDigest,
                desiredProfileDigest: output.desiredProfileDigest,
                desiredSnapshotDigest: output.desiredSnapshotDigest,
                applyPlanDigest: apply.digest,
                rollbackPlanDigest: rollback.digest,
                changes: output.changes
            )
            body = .diffPreview(.init(diff: diff, receipt: receipt))
        case let .apply(value):
            let output = try decoder.decode(ApplyCommandOutput.self, from: result.stdout)
            let transactionID = value.plan.diff.applyPlanDigest
            guard output.transactionID == transactionID,
                  output.baselineSnapshotDigest == value.plan.baseline.digest,
                  output.desiredSnapshotDigest == value.plan.diff.desiredSnapshotDigest else {
                throw ChildResponseError.bindingMismatch
            }
            let readback = try output.readback.map {
                try ingestSnapshot($0, target: value.target, validSearchResponses: 1)
            }
            let matches = readback?.digest == output.desiredSnapshotDigest
            guard output.readbackMatchesDesired == matches,
                  matches || output.rollbackAvailable,
                  readback != nil || !matches else {
                throw ChildResponseError.bindingMismatch
            }
            body = .apply(.init(
                transactionID: transactionID,
                baselineDigest: output.baselineSnapshotDigest,
                desiredSnapshotDigest: output.desiredSnapshotDigest,
                readback: readback,
                readbackMatchesDesired: matches,
                rollbackRequired: !matches,
                rollbackAvailable: output.rollbackAvailable,
                receipt: receipt
            ))
        case let .readback(value):
            let snapshot = try ingestSnapshot(
                result.stdout,
                target: value.target,
                validSearchResponses: 10
            )
            body = .readback(.init(
                transactionID: value.transactionID,
                matchesDesired: snapshot.digest == value.expectedDesiredDigest,
                snapshot: snapshot,
                receipt: receipt
            ))
        case let .rollback(value):
            let output = try decoder.decode(RollbackCommandOutput.self, from: result.stdout)
            guard output.transactionID == value.plan.transactionID,
                  output.baselineSnapshotDigest == value.plan.baseline.digest else {
                throw ChildResponseError.bindingMismatch
            }
            let restored = try output.restored.map {
                try ingestSnapshot($0, target: value.target, validSearchResponses: 1)
            }
            let equalsBaseline = restored?.digest == output.baselineSnapshotDigest
            guard output.equalsBaseline == equalsBaseline else {
                throw ChildResponseError.bindingMismatch
            }
            body = .rollback(.init(
                transactionID: value.plan.transactionID,
                baselineDigest: output.baselineSnapshotDigest,
                restored: restored,
                equalsBaseline: equalsBaseline,
                receipt: receipt
            ))
        }
        return BridgeResponse(requestID: request.requestID, body: body)
    }

    private func ingestSnapshot(
        _ data: Data,
        target: DCXTargetReference,
        validSearchResponses: UInt8
    ) throws -> SnapshotV1 {
        let carrier = try BridgeJSONCodec.decoder().decode(CoreSnapshotCarrierV1.self, from: data)
        return try ingestSnapshot(
            carrier,
            target: target,
            validSearchResponses: validSearchResponses
        )
    }

    private func ingestSnapshot(
        _ carrier: CoreSnapshotCarrierV1,
        target: DCXTargetReference,
        validSearchResponses: UInt8
    ) throws -> SnapshotV1 {
        try carrier.validate(expectedDevice: target.expectedDeviceAddress)
        let canonical = try BridgeJSONCodec.encoder().encode(carrier)
        _ = try snapshotStore.persist(canonical, carrier: carrier)
        return try carrier.summary(
            target: target,
            validSearchResponses: validSearchResponses
        )
    }

    private func failure(
        _ request: BridgeRequest,
        _ code: BridgeErrorPayload.Code,
        _ message: String,
        _ retryable: Bool
    ) -> BridgeResponse {
        .init(
            requestID: request.requestID,
            operation: request.operation,
            error: .init(code: code, message: message, retryable: retryable)
        )
    }
}

private struct LiveSearchOutput: Decodable {
    struct Identity: Decodable {
        let manufacturer: String
        let model: String
        let deviceAddress: UInt8
    }

    let status: String
    let protocolIdentity: Identity
    let selectedBaud: UInt32
    let validResponses: UInt8
}

private struct DiffCommandOutput: Decodable {
    let baselineSnapshotDigest: String
    let desiredProfileDigest: String
    let desiredSnapshotDigest: String
    let changes: [SemanticChangeV1]
    let applyPlan: JSONValue
    let rollbackPlan: JSONValue

    enum CodingKeys: String, CodingKey {
        case baselineSnapshotDigest = "baseline_snapshot_digest"
        case desiredProfileDigest = "desired_profile_digest"
        case desiredSnapshotDigest = "desired_snapshot_digest"
        case changes
        case applyPlan = "apply_plan"
        case rollbackPlan = "rollback_plan"
    }
}

private struct ApplyCommandOutput: Decodable {
    let transactionID: String
    let baselineSnapshotDigest: String
    let desiredSnapshotDigest: String
    let readback: CoreSnapshotCarrierV1?
    let rollbackAvailable: Bool
    let readbackMatchesDesired: Bool

    enum CodingKeys: String, CodingKey {
        case transactionID = "transaction_id"
        case baselineSnapshotDigest = "baseline_snapshot_digest"
        case desiredSnapshotDigest = "desired_snapshot_digest"
        case readback
        case rollbackAvailable = "rollback_available"
        case readbackMatchesDesired = "readback_matches_desired"
    }
}

private struct RollbackCommandOutput: Decodable {
    let transactionID: String
    let baselineSnapshotDigest: String
    let restored: CoreSnapshotCarrierV1?
    let equalsBaseline: Bool

    enum CodingKeys: String, CodingKey {
        case transactionID = "transaction_id"
        case baselineSnapshotDigest = "baseline_snapshot_digest"
        case restored
        case equalsBaseline = "equals_baseline"
    }
}

private final class TransactionWorkspace: @unchecked Sendable {
    private let root: URL
    private let directory: URL
    private let fileManager: FileManager

    init(root: URL, fileManager: FileManager = .default) throws {
        self.root = root.standardizedFileURL
        self.fileManager = fileManager
        directory = self.root.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try fileManager.createDirectory(
            at: directory,
            withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700]
        )
    }

    func write<T: Encodable>(_ value: T, named name: String) throws -> URL {
        guard !name.contains("/"), !name.contains("..") else {
            throw ChildResponseError.invalidWorkspaceLeaf
        }
        let url = directory.appendingPathComponent(name, isDirectory: false)
        let data = try BridgeJSONCodec.encoder().encode(value)
        guard data.count <= DCXBridgeContract.maximumFrameBytes else {
            throw AppGroupBoundaryError.frameTooLarge
        }
        try data.write(to: url, options: [.atomic, .completeFileProtection])
        try fileManager.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
        return url
    }

    func remove() {
        let standardized = directory.standardizedFileURL
        guard standardized.path.hasPrefix(root.path + "/"), standardized != root else { return }
        try? fileManager.removeItem(at: standardized)
    }

    deinit { remove() }
}

private enum ChildResponseError: Error {
    case invalidSearch
    case bindingMismatch
    case invalidWorkspaceLeaf
    case requestPlanBinding
}
