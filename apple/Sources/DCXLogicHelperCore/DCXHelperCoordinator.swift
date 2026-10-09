import DCXLogicBridge
import Foundation

/// Helper-only storage boundary. The public coordinator always receives the
/// checked App Group locations; tests may supply an owned fixture directory.
struct DCXHelperStorageLocations: Sendable {
    let helperConfigurationURL: URL
    let transactionRootURL: URL
    let snapshotRootURL: URL
    let planRootURL: URL

    init(_ locations: AppGroupLocations) {
        helperConfigurationURL = locations.helperConfigurationURL
        transactionRootURL = locations.transactionRootURL
        snapshotRootURL = locations.snapshotRootURL
        planRootURL = locations.planRootURL
    }

    init(fixtureRoot: URL) {
        helperConfigurationURL = fixtureRoot.appendingPathComponent(
            DCXBridgeContract.helperConfigurationFileName
        )
        transactionRootURL = fixtureRoot.appendingPathComponent("Transactions", isDirectory: true)
        snapshotRootURL = fixtureRoot.appendingPathComponent("Snapshots", isDirectory: true)
        planRootURL = fixtureRoot.appendingPathComponent("Plans", isDirectory: true)
    }
}

public final class DCXHelperCoordinator: @unchecked Sendable {
    private let locations: DCXHelperStorageLocations
    private var configuration: HelperConfigurationV1?
    private var configurationError: Error?
    private let coreMIDI: CoreMIDIPresentation
    private let runner: any DCXCTLCommandExecuting
    private let snapshotStore: RawSnapshotStore
    private let planStore: RawPlanStore
    private let notchPlanStore: NotchPlanStore
    private let recoveryLeaseStore: MutationRecoveryLeaseStore
    private let stateLock = NSLock()
    private var foreground = false
    private var activeTransactionID: String?
    private var recoveryLease: MutationRecoveryLeaseV1?
    private var recoveryCompletion: MutationRecoveryCompletionV1?
    private var recoveryLeaseError: Error?

    public convenience init(
        locations: AppGroupLocations,
        configuration: Result<HelperConfigurationV1, Error>,
        coreMIDI: CoreMIDIPresentation
    ) {
        self.init(
            locations: DCXHelperStorageLocations(locations),
            configuration: configuration,
            coreMIDI: coreMIDI,
            runner: DCXCTLProcessRunner()
        )
    }

    init(
        locations: DCXHelperStorageLocations,
        configuration: Result<HelperConfigurationV1, Error>,
        coreMIDI: CoreMIDIPresentation,
        runner: any DCXCTLCommandExecuting
    ) {
        self.locations = locations
        self.runner = runner
        snapshotStore = .init(root: locations.snapshotRootURL)
        planStore = .init(root: locations.planRootURL)
        notchPlanStore = .init(planRoot: locations.planRootURL)
        let recoveryLeaseStore = MutationRecoveryLeaseStore(root: locations.planRootURL)
        self.recoveryLeaseStore = recoveryLeaseStore
        switch configuration {
        case let .success(value):
            self.configuration = value
            configurationError = nil
        case let .failure(error):
            self.configuration = nil
            configurationError = error
        }
        self.coreMIDI = coreMIDI
        recoveryLease = nil
        recoveryCompletion = nil
        recoveryLeaseError = nil
        do {
            let processLock = try recoveryLeaseStore.acquireExclusiveLock()
            defer { processLock.release() }
            try reloadRecoveryStateFromStore()
        } catch {
            recoveryLease = nil
            recoveryCompletion = nil
            recoveryLeaseError = error
        }
    }

    public func setForeground(_ value: Bool) {
        stateLock.lock()
        foreground = value
        stateLock.unlock()
    }

    @discardableResult
    func replaceConfiguration(
        load: () -> Result<HelperConfigurationV1, Error>
    ) throws -> HelperConfigurationV1 {
        let processLock = try recoveryLeaseStore.acquireExclusiveLock()
        defer { processLock.release() }
        stateLock.lock()
        defer { stateLock.unlock() }
        try reloadRecoveryStateFromStore()
        let result = load()
        try requireConfigurationReplacementAllowed(result)
        switch result {
        case let .success(value):
            configuration = value
            configurationError = nil
        case let .failure(error):
            configuration = nil
            configurationError = error
        }
        return try result.get()
    }

    /// Serialize persistent configuration replacement with Apply admission.
    /// The closure runs while the coordinator lock prevents a lease from being
    /// created against a configuration that is concurrently being replaced.
    func installConfiguration(
        _ candidate: HelperConfigurationV1,
        persistAndReload: () throws -> HelperConfigurationV1
    ) throws -> HelperConfigurationV1 {
        let processLock = try recoveryLeaseStore.acquireExclusiveLock()
        defer { processLock.release() }
        stateLock.lock()
        defer { stateLock.unlock() }
        try reloadRecoveryStateFromStore()
        try requireConfigurationReplacementAllowed(.success(candidate))
        let loaded = try persistAndReload()
        guard loaded == candidate else {
            throw HelperConfigurationError.invalidConfigurationFile
        }
        configuration = loaded
        configurationError = nil
        return loaded
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
            return preAdmissionFailure(
                request,
                .invalidRequest,
                "request failed bridge validation",
                false
            )
        }
        if case .helperStatus = request.body {
            return BridgeResponse(requestID: request.requestID, body: .helperStatus(status()))
        }
        guard isForeground else {
            return preAdmissionFailure(
                request,
                .helperNotForeground,
                "helper must be open and foreground",
                true
            )
        }

        let target: DCXTargetReference
        switch request.body {
        case .helperStatus:
            return preAdmissionFailure(
                request,
                .invalidRequest,
                "status request reached command dispatch",
                false
            )
        case let .identitySearch(value): target = value.target
        case let .snapshotCapture(value): target = value.target
        case let .diffPreview(value): target = value.target
        case let .apply(value): target = value.target
        case let .readback(value): target = value.target
        case let .rollback(value): target = value.target
        case let .feedbackPlan(value): target = value.target
        }

        var processLock: MutationRecoveryProcessLock?
        do {
            processLock = try recoveryLeaseStore.acquireExclusiveLock()
            stateLock.lock()
            do {
                try reloadRecoveryStateFromStore()
                if recoveryLease == nil {
                    reloadCurrentConfigurationFromStore()
                }
                stateLock.unlock()
            } catch {
                stateLock.unlock()
                throw error
            }
        } catch MutationRecoveryLeaseError.lockBusy {
            return preAdmissionFailure(
                request,
                .operationInFlight,
                "another helper or child owns the device operation lock",
                true
            )
        } catch {
            return preAdmissionFailure(
                request,
                .helperNotConfigured,
                "durable mutation recovery state is unavailable",
                false
            )
        }
        defer { processLock?.release() }

        let configuration: HelperConfigurationV1
        do {
            configuration = try executionConfiguration(for: request)
            try configuration.require(request.operation, target: target)
        } catch MutationRecoveryStateError.applyAlreadyActive {
            return preAdmissionFailure(
                request,
                .operationUnavailable,
                "the durable mutation recovery must complete before another Apply",
                false
            )
        } catch MutationRecoveryStateError.recoveryUnavailable {
            return preAdmissionFailure(
                request,
                .operationUnavailable,
                "durable mutation recovery authority is unavailable",
                false
            )
        } catch MutationRecoveryStateError.recoveryInProgress {
            return preAdmissionFailure(
                request,
                .operationUnavailable,
                "only recovery-bound Readback or Rollback is available",
                false
            )
        } catch HelperConfigurationError.targetNotAllowlisted {
            return preAdmissionFailure(
                request,
                .targetNotAllowlisted,
                "requested target is not configured",
                false
            )
        } catch HelperConfigurationError.operationUnavailable {
            return preAdmissionFailure(
                request,
                .operationUnavailable,
                "operation is not enabled",
                false
            )
        } catch {
            return preAdmissionFailure(
                request,
                .helperNotConfigured,
                "helper configuration is unavailable",
                false
            )
        }
        switch request.body {
        case .diffPreview, .feedbackPlan:
            // Offline children never hold the device operation lock.
            processLock?.release()
            processLock = nil
        default:
            break
        }

        let activeID = UUID().uuidString
        stateLock.lock()
        guard activeTransactionID == nil else {
            stateLock.unlock()
            return preAdmissionFailure(
                request,
                .operationInFlight,
                "one helper transaction is already active",
                true
            )
        }
        activeTransactionID = activeID
        stateLock.unlock()
        defer {
            stateLock.lock()
            activeTransactionID = nil
            stateLock.unlock()
        }

        var applyAdmitted = false
        do {
            let workspace = try TransactionWorkspace(root: locations.transactionRootURL)
            defer { workspace.remove() }
            if case let .feedbackPlan(value) = request.body {
                return try planFeedbackNotches(
                    request,
                    value,
                    configuration: configuration,
                    workspace: workspace
                )
            }
            let invocation = try buildInvocation(
                request: request,
                configuration: configuration,
                workspace: workspace
            )
            try admitMutation(
                request,
                configuration: configuration,
                applyAdmitted: &applyAdmitted
            )
            let result = try runner.run(
                invocation,
                timeoutSeconds: configuration.childTimeoutSeconds,
                mutationLock: processLock
            )
            if let failure = result.failurePayload() {
                return BridgeResponse(
                    requestID: request.requestID,
                    operation: request.operation,
                    error: failure
                )
            }
            let response = try decode(request: request, result: result)
            try finishMutation(request, response: response)
            return response
        } catch DCXCTLRunnerError.timedOut {
            return failure(request, .childTimedOut, "dcxctl exceeded its configured deadline", true)
        } catch DCXCTLRunnerError.terminationUnconfirmed {
            return commandFailure(
                request,
                applyAdmitted: applyAdmitted,
                .childTimedOut,
                "dcxctl exceeded its deadline; child exit is unconfirmed and recovery authority is retained",
                false
            )
        } catch DCXCTLRunnerError.launchFailed {
            return commandFailure(
                request,
                applyAdmitted: applyAdmitted,
                .childFailed,
                "bundled dcxctl could not be launched",
                false
            )
        } catch DCXCTLRunnerError.outputTooLarge {
            return commandFailure(
                request,
                applyAdmitted: applyAdmitted,
                .childFailed,
                "dcxctl exceeded its bounded output limit; device state is not verified",
                false
            )
        } catch MutationRecoveryStateError.applyAlreadyActive {
            return commandFailure(
                request,
                applyAdmitted: applyAdmitted,
                .operationUnavailable,
                "the durable mutation recovery must complete before another Apply",
                false
            )
        } catch MutationRecoveryStateError.bindingMismatch {
            return commandFailure(
                request,
                applyAdmitted: applyAdmitted,
                .invalidRequest,
                "request does not match the durable mutation recovery binding",
                false
            )
        } catch MutationRecoveryStateError.configurationChanged {
            return commandFailure(
                request,
                applyAdmitted: applyAdmitted,
                .operationUnavailable,
                "helper configuration changed before mutation admission; retry",
                true
            )
        } catch is MutationRecoveryLeaseError {
            return commandFailure(
                request,
                applyAdmitted: applyAdmitted,
                .helperNotConfigured,
                "durable mutation recovery state is unavailable",
                false
            )
        } catch FeedbackRequestError.baselineUnavailable {
            return failure(
                request,
                .invalidRequest,
                "the baseline snapshot is not in the helper store; capture a snapshot and replan",
                false
            )
        } catch FeedbackRequestError.invalidMeasurement {
            return failure(
                request,
                .invalidRequest,
                "the measurement document is not a valid O4 dcx.feedback-measurement/v1",
                false
            )
        } catch NotchPlanStoreError.missingPlan {
            return failure(
                request,
                .invalidRequest,
                "the named prior notch plan is not in the helper store",
                false
            )
        } catch ChildResponseError.requestPlanBinding {
            return commandFailure(
                request,
                applyAdmitted: applyAdmitted,
                .invalidRequest,
                "stored plan does not match the request bindings",
                false
            )
        } catch HelperConfigurationError.executableUnavailable {
            return commandFailure(
                request,
                applyAdmitted: applyAdmitted,
                .helperNotConfigured,
                "bundled dcxctl is unavailable",
                false
            )
        } catch {
            return commandFailure(
                request,
                applyAdmitted: applyAdmitted,
                .malformedChildResponse,
                "bounded command did not return its expected schema",
                false
            )
        }
    }

    private var isForeground: Bool {
        stateLock.lock()
        defer { stateLock.unlock() }
        return foreground
    }

    /// Reload only while the caller owns the process-shared operation lock.
    /// A valid active lease suppresses terminal history, including a stale or
    /// partially replaced completion file from an earlier transaction.
    private func reloadRecoveryStateFromStore() throws {
        do {
            let durableLease = try recoveryLeaseStore.load()
            recoveryLease = durableLease
            if durableLease != nil {
                recoveryCompletion = nil
            } else {
                recoveryCompletion = try recoveryLeaseStore.loadCompletion()
            }
            recoveryLeaseError = nil
        } catch {
            recoveryLease = nil
            recoveryCompletion = nil
            recoveryLeaseError = error
            throw error
        }
    }

    /// With no active lease, the fixed App Group file is the cross-process
    /// configuration authority. Recovery execution never calls this path and
    /// remains pinned to the lease rather than overwriting mutable config.
    private func reloadCurrentConfigurationFromStore() {
        do {
            configuration = try HelperConfigurationV1.load(
                from: locations.helperConfigurationURL
            )
            configurationError = nil
        } catch {
            configuration = nil
            configurationError = error
        }
    }

    private func requireConfigurationReplacementAllowed(
        _ result: Result<HelperConfigurationV1, Error>
    ) throws {
        guard recoveryLeaseError == nil else {
            throw HelperConfigurationError.recoveryConfigurationPinned
        }
        guard let recoveryLease else { return }
        guard case let .success(candidate) = result,
              candidate == (try recoveryLease.pinnedConfiguration()) else {
            throw HelperConfigurationError.recoveryConfigurationPinned
        }
    }

    private func executionConfiguration(
        for request: BridgeRequest
    ) throws -> HelperConfigurationV1 {
        stateLock.lock()
        defer { stateLock.unlock() }
        if recoveryLeaseError != nil {
            throw MutationRecoveryStateError.recoveryUnavailable
        }
        switch request.body {
        case .helperStatus:
            throw HelperConfigurationError.operationUnavailable
        case .apply:
            guard recoveryLease == nil else {
                throw MutationRecoveryStateError.applyAlreadyActive
            }
            guard let configuration else {
                throw HelperConfigurationError.invalidConfigurationFile
            }
            return configuration
        case .readback, .rollback:
            guard let recoveryLease else {
                throw MutationRecoveryStateError.recoveryUnavailable
            }
            return try recoveryLease.pinnedConfiguration()
        case .identitySearch, .snapshotCapture, .diffPreview, .feedbackPlan:
            guard recoveryLease == nil else {
                throw MutationRecoveryStateError.recoveryInProgress
            }
            guard let configuration else {
                throw HelperConfigurationError.invalidConfigurationFile
            }
            return configuration
        }
    }

    /// Persist mutation authority under the same lock used by configuration
    /// replacement, immediately before the child process can start.
    private func admitMutation(
        _ request: BridgeRequest,
        configuration capturedConfiguration: HelperConfigurationV1,
        applyAdmitted: inout Bool
    ) throws {
        guard BridgeOperation.mutationCapabilities.contains(request.operation) else { return }
        stateLock.lock()
        defer { stateLock.unlock() }
        guard recoveryLeaseError == nil else {
            throw MutationRecoveryStateError.recoveryUnavailable
        }
        switch request.body {
        case let .apply(value):
            guard recoveryLease == nil else {
                throw MutationRecoveryStateError.applyAlreadyActive
            }
            guard configuration == capturedConfiguration else {
                throw MutationRecoveryStateError.configurationChanged
            }
            let lease = try MutationRecoveryLeaseV1(
                configuration: capturedConfiguration,
                apply: value
            )
            do {
                try recoveryLeaseStore.persistNew(lease)
            } catch MutationRecoveryLeaseError.activeLease {
                do {
                    guard let durable = try recoveryLeaseStore.load() else {
                        throw MutationRecoveryLeaseError.invalidLease
                    }
                    recoveryLease = durable
                    recoveryCompletion = nil
                    recoveryLeaseError = nil
                } catch {
                    recoveryLease = nil
                    recoveryCompletion = nil
                    recoveryLeaseError = error
                }
                throw MutationRecoveryStateError.applyAlreadyActive
            } catch {
                let publicationError = error
                do {
                    if let durable = try recoveryLeaseStore.load() {
                        recoveryLease = durable
                        recoveryCompletion = nil
                        recoveryLeaseError = nil
                        applyAdmitted = durable == lease
                    } else {
                        recoveryLease = nil
                        applyAdmitted = false
                    }
                } catch {
                    // Once exclusive publication may have happened, an
                    // unreadable pathname cannot prove non-admission.
                    recoveryLease = nil
                    recoveryCompletion = nil
                    recoveryLeaseError = error
                    applyAdmitted = true
                }
                throw publicationError
            }
            recoveryLease = lease
            recoveryCompletion = nil
            recoveryLeaseError = nil
            applyAdmitted = true
        case let .readback(value):
            guard let recoveryLease,
                  recoveryLease.matches(readback: value),
                  capturedConfiguration == (try recoveryLease.pinnedConfiguration()) else {
                throw MutationRecoveryStateError.bindingMismatch
            }
        case let .rollback(value):
            guard let recoveryLease,
                  recoveryLease.matches(rollback: value),
                  capturedConfiguration == (try recoveryLease.pinnedConfiguration()) else {
                throw MutationRecoveryStateError.bindingMismatch
            }
        case .helperStatus, .identitySearch, .snapshotCapture, .diffPreview, .feedbackPlan:
            return
        }
    }

    /// A transaction-bound readback or rollback is terminal only when its
    /// decoded snapshot equals the immutable baseline. All other outcomes keep
    /// the durable lease.
    private func finishMutation(
        _ request: BridgeRequest,
        response: BridgeResponse
    ) throws {
        guard let body = response.body else { return }
        stateLock.lock()
        defer { stateLock.unlock() }
        guard let recoveryLease else { return }
        let verifiedBaseline: SnapshotV1
        switch (request.body, body) {
        case let (.readback(requestValue), .readback(result)):
            guard recoveryLease.matches(readback: requestValue),
                  result.transactionID == recoveryLease.transactionID else {
                throw MutationRecoveryStateError.bindingMismatch
            }
            guard result.snapshot.digest == recoveryLease.baselineDigest else { return }
            verifiedBaseline = result.snapshot
        case let (.rollback(requestValue), .rollback(result)):
            guard recoveryLease.matches(rollback: requestValue),
                  result.transactionID == recoveryLease.transactionID,
                  result.baselineDigest == recoveryLease.baselineDigest else {
                throw MutationRecoveryStateError.bindingMismatch
            }
            guard result.equalsBaseline,
                  let restored = result.restored,
                  restored.digest == recoveryLease.baselineDigest else { return }
            verifiedBaseline = restored
        default:
            return
        }
        let completion = try recoveryLeaseStore.complete(
            recoveryLease,
            verifiedBaseline: verifiedBaseline
        )
        self.recoveryLease = nil
        recoveryCompletion = completion
        recoveryLeaseError = nil
    }

    private func status() -> HelperStatusResponse {
        var recoveryUnavailable = false
        do {
            let processLock = try recoveryLeaseStore.acquireExclusiveLock()
            defer { processLock.release() }
            stateLock.lock()
            do {
                try reloadRecoveryStateFromStore()
                if recoveryLease == nil {
                    reloadCurrentConfigurationFromStore()
                }
                stateLock.unlock()
            } catch {
                stateLock.unlock()
                throw error
            }
        } catch MutationRecoveryLeaseError.lockBusy {
            recoveryUnavailable = true
        } catch {
            stateLock.lock()
            recoveryLease = nil
            recoveryCompletion = nil
            recoveryLeaseError = error
            stateLock.unlock()
            recoveryUnavailable = true
        }

        stateLock.lock()
        let foreground = foreground
        let active = activeTransactionID
        let configuration = configuration
        let unavailable = recoveryUnavailable || recoveryLeaseError != nil
        let recovery = unavailable ? nil : recoveryLease.map {
            HelperRecoveryStatusV1(
                transactionID: $0.transactionID,
                target: $0.target,
                capabilities: [.readback, .rollback],
                baseline: $0.baseline,
                desiredSnapshotDigest: $0.desiredSnapshotDigest,
                rollbackPlanDigest: $0.rollbackPlanDigest
            )
        }
        let completion = unavailable || recoveryLease != nil ? nil : recoveryCompletion.map {
            HelperRecoveryCompletionStatusV1(
                transactionID: $0.transactionID,
                target: $0.target,
                baseline: $0.baseline,
                verifiedBaseline: $0.verifiedBaseline,
                desiredSnapshotDigest: $0.desiredSnapshotDigest,
                rollbackPlanDigest: $0.rollbackPlanDigest
            )
        }
        let recoveryBlocksOperations = recoveryLease != nil || unavailable
        let enabledOperations = recoveryBlocksOperations
            ? []
            : configuration?.enabledOperations ?? []
        _ = configurationError
        _ = recoveryLeaseError
        stateLock.unlock()
        return .init(
            foreground: foreground,
            configured: configuration != nil,
            target: configuration?.target,
            capabilities: [.helperStatus] + enabledOperations,
            coreMIDI: coreMIDI.bridgeStatus(),
            activeTransactionID: active,
            recovery: recovery,
            completion: completion,
            recoveryUnavailable: unavailable
        )
    }

    private func buildInvocation(
        request: BridgeRequest,
        configuration: HelperConfigurationV1,
        workspace: TransactionWorkspace
    ) throws -> DCXCTLInvocation {
        let executable = try runner.resolveExecutable(for: configuration)
        let targetArguments = [
            "--tty", configuration.ttyPath,
            "--expected-device", String(configuration.target.expectedDeviceAddress),
        ]
        let arguments: [String]
        switch request.body {
        case .helperStatus, .feedbackPlan:
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
            // dcxctl revalidates the versioned envelope: exact O1/PEQ9 v1 or
            // the cut-only reviewed v2 bank. The document is passed unchanged.
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
                try planStore.makeDurable(plan)
                let rollback = try planStore.loadRollback(
                    rollbackPlanDigest: value.plan.diff.rollbackPlanDigest,
                    transactionID: applyPlanDigest,
                    applyPlanDigest: applyPlanDigest,
                    expectedDevice: value.target.expectedDeviceAddress,
                    baselineDigest: value.plan.baseline.digest
                )
                try planStore.makeDurable(rollback)
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
                try planStore.makeDurable(plan)
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
        case .helperStatus, .feedbackPlan:
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
            let diff: SemanticDiff
            switch (value.desired, output.changes) {
            case let (.v1, .v1(changes)):
                diff = try SemanticDiff(
                    baselineSnapshotDigest: output.baselineSnapshotDigest,
                    desiredProfileDigest: output.desiredProfileDigest,
                    desiredSnapshotDigest: output.desiredSnapshotDigest,
                    applyPlanDigest: apply.digest,
                    rollbackPlanDigest: rollback.digest,
                    changes: changes
                )
            case let (.v2, .v2(changes)):
                // What the AU shows is exactly what Apply writes: the ordered
                // changed fields equal the raw apply plan's command actions.
                let shown = changes.map {
                    DirectParameterActionV2(channel: $0.channel, parameter: $0.parameter, value: $0.after)
                }
                guard try shown == output.appliedActions() else {
                    throw ChildResponseError.bindingMismatch
                }
                diff = try SemanticDiff(
                    baselineSnapshotDigest: output.baselineSnapshotDigest,
                    desiredProfileDigest: output.desiredProfileDigest,
                    desiredSnapshotDigest: output.desiredSnapshotDigest,
                    applyPlanDigest: apply.digest,
                    rollbackPlanDigest: rollback.digest,
                    fieldChanges: changes
                )
            default:
                throw ChildResponseError.bindingMismatch
            }
            try diff.validate(against: value.desired)
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

    private func preAdmissionFailure(
        _ request: BridgeRequest,
        _ code: BridgeErrorPayload.Code,
        _ message: String,
        _ retryable: Bool
    ) -> BridgeResponse {
        commandFailure(
            request,
            applyAdmitted: false,
            code,
            message,
            retryable
        )
    }

    private func commandFailure(
        _ request: BridgeRequest,
        applyAdmitted: Bool,
        _ code: BridgeErrorPayload.Code,
        _ message: String,
        _ retryable: Bool
    ) -> BridgeResponse {
        failure(
            request,
            request.operation == .apply && !applyAdmitted ? .mutationNotAdmitted : code,
            message,
            retryable
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
    /// Changes typed by the desired profile's generation: one O1/PEQ9 slot
    /// (v1) or per-field PEQ changes (v2).
    enum Changes {
        case v1([SemanticChangeV1])
        case v2([FieldChangeV2])
    }

    let baselineSnapshotDigest: String
    let desiredProfileSchema: String
    let desiredProfileDigest: String
    let desiredSnapshotDigest: String
    let changes: Changes
    let applyPlan: JSONValue
    let rollbackPlan: JSONValue

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        baselineSnapshotDigest = try container.decode(String.self, forKey: .baselineSnapshotDigest)
        desiredProfileSchema = try container.decode(String.self, forKey: .desiredProfileSchema)
        desiredProfileDigest = try container.decode(String.self, forKey: .desiredProfileDigest)
        desiredSnapshotDigest = try container.decode(String.self, forKey: .desiredSnapshotDigest)
        switch desiredProfileSchema {
        case DesiredProfileV1.currentSchemaVersion:
            changes = .v1(try container.decode([SemanticChangeV1].self, forKey: .changes))
        case DesiredProfileV2.currentSchemaVersion:
            changes = .v2(try container.decode([FieldChangeV2].self, forKey: .changes))
        default:
            throw ChildResponseError.bindingMismatch
        }
        applyPlan = try container.decode(JSONValue.self, forKey: .applyPlan)
        rollbackPlan = try container.decode(JSONValue.self, forKey: .rollbackPlan)
    }

    /// Ordered `(channel, parameter, value)` actions the raw apply plan writes;
    /// empty when the plan carries no command.
    func appliedActions() throws -> [DirectParameterActionV2] {
        guard case let .object(plan) = applyPlan else { throw ChildResponseError.bindingMismatch }
        guard let command = plan["command"], command != .null else { return [] }
        guard case let .object(fields) = command, case let .array(actions)? = fields["actions"] else {
            throw ChildResponseError.bindingMismatch
        }
        return try actions.map { action in
            guard case let .object(values) = action,
                  let channel = Self.integer(values["channel"]),
                  let parameter = Self.integer(values["parameter"]),
                  let value = Self.integer(values["value"]),
                  channel <= 0xff, parameter <= 0xff else {
                throw ChildResponseError.bindingMismatch
            }
            return .init(channel: UInt8(channel), parameter: UInt8(parameter), value: value)
        }
    }

    private static func integer(_ value: JSONValue?) -> UInt16? {
        guard case let .number(number)? = value, number.isFinite,
              number.rounded(.towardZero) == number, (0...Double(UInt16.max)).contains(number) else {
            return nil
        }
        return UInt16(number)
    }

    enum CodingKeys: String, CodingKey {
        case baselineSnapshotDigest = "baseline_snapshot_digest"
        case desiredProfileSchema = "desired_profile_schema"
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
        try writeData(BridgeJSONCodec.encoder().encode(value), named: name)
    }

    func writeData(_ data: Data, named name: String) throws -> URL {
        guard !name.contains("/"), !name.contains("..") else {
            throw ChildResponseError.invalidWorkspaceLeaf
        }
        let url = directory.appendingPathComponent(name, isDirectory: false)
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

private enum MutationRecoveryStateError: Error {
    case applyAlreadyActive
    case recoveryUnavailable
    case recoveryInProgress
    case bindingMismatch
    case configurationChanged
}

// MARK: - Offline static feedback-notch planning

extension DCXHelperCoordinator {
    /// Per-child deadline for the offline feedback children. Three steps stay
    /// inside the AU socket's 165-second framing envelope.
    static let offlineStepTimeoutSeconds: UInt8 = 45

    /// `feedback import` (source text only), `feedback plan` against the
    /// helper-stored raw baseline, then `feedback desired-profile`. Each child
    /// is offline; none receives a tty, and none holds the device lock.
    fileprivate func planFeedbackNotches(
        _ request: BridgeRequest,
        _ value: FeedbackPlanRequest,
        configuration: HelperConfigurationV1,
        workspace: TransactionWorkspace
    ) throws -> BridgeResponse {
        let executable = try runner.resolveExecutable(for: configuration)
        let timeout = min(configuration.childTimeoutSeconds, Self.offlineStepTimeoutSeconds)
        let decoder = BridgeJSONCodec.decoder()
        let snapshotURL: URL
        do {
            (snapshotURL, _) = try snapshotStore.load(
                digest: value.baseline.digest,
                expectedDevice: value.target.expectedDeviceAddress
            )
        } catch {
            throw FeedbackRequestError.baselineUnavailable
        }
        let targetOutput = String(FeedbackNotchContract.targetOutput)
        // Resolve every helper-owned input before the first child runs.
        let priorPlanURL = try value.priorPlanDigest.map { try notchPlanStore.load(planDigest: $0) }
        func run(_ arguments: [String]) throws -> OfflineStepOutcome {
            let result = try runner.run(
                .init(operation: .feedbackPlan, executableURL: executable, arguments: arguments),
                timeoutSeconds: timeout,
                mutationLock: nil
            )
            if let failure = result.failurePayload() { return .failure(failure) }
            return .success(result)
        }
        func reply(_ error: BridgeErrorPayload) -> BridgeResponse {
            BridgeResponse(requestID: request.requestID, operation: request.operation, error: error)
        }

        var importReceipt: CommandReceiptV1?
        let measurementData: Data
        switch value.measurement.kind {
        case .frequencyList, .rewGenericEq:
            guard let text = value.measurement.text else { throw ChildResponseError.bindingMismatch }
            let source = try workspace.writeData(Data(text.utf8), named: "measurement-source.txt")
            let flag = value.measurement.kind == .frequencyList ? "--frequency-list" : "--rew"
            switch try run(["feedback", "import", flag, source.path, "--target-output", targetOutput]) {
            case let .failure(error): return reply(error)
            case let .success(result):
                importReceipt = result.receipt(for: .feedbackPlan)
                measurementData = result.stdout
            }
        case .measurement:
            measurementData = try BridgeJSONCodec.encoder().encode(value.measurement.document)
        }
        let summary: FeedbackMeasurementSummaryV1
        do {
            summary = try decoder.decode(MeasurementCommandOutput.self, from: measurementData).summary()
        } catch where value.measurement.kind == .measurement {
            // An AU-supplied document, not child output, failed here.
            throw FeedbackRequestError.invalidMeasurement
        }
        switch value.measurement.kind {
        case .measurement:
            guard summary.digest == value.measurement.documentDigest else {
                throw ChildResponseError.bindingMismatch
            }
        case .frequencyList, .rewGenericEq:
            guard let text = value.measurement.text,
                  summary.source.rawValue == value.measurement.kind.rawValue,
                  summary.sourceDigest == FeedbackNotchContract.sourceDigest(text) else {
                throw ChildResponseError.bindingMismatch
            }
        }
        let measurementURL = try workspace.writeData(measurementData, named: "measurement.json")

        var planArguments = [
            "feedback", "plan",
            "--measurement", measurementURL.path,
            "--snapshot", snapshotURL.path,
        ]
        if let priorPlanURL {
            planArguments += ["--prior-plan", priorPlanURL.path]
        }
        let planResult: DCXCTLProcessResult
        switch try run(planArguments) {
        case let .failure(error): return reply(error)
        case let .success(result): planResult = result
        }
        let planOutput = try decoder.decode(NotchPlanCommandOutput.self, from: planResult.stdout)
        let plan = try planOutput.summary()
        guard plan.baselineSnapshotDigest == value.baseline.digest,
              plan.measurementDigest == summary.digest else {
            throw ChildResponseError.bindingMismatch
        }
        let storedPlan = try notchPlanStore.persist(planResult.stdout, planDigest: plan.planDigest)

        let profileResult: DCXCTLProcessResult
        switch try run([
            "feedback", "desired-profile",
            "--plan", storedPlan.path,
            "--profile-id", value.profileID,
            "--revision", value.revision,
        ]) {
        case let .failure(error): return reply(error)
        case let .success(result): profileResult = result
        }
        let desired = try decoder.decode(DesiredProfileV2.self, from: profileResult.stdout)
        try desired.validate()

        let response = FeedbackPlanResponse(
            measurement: summary,
            plan: plan,
            desired: desired,
            importReceipt: importReceipt,
            planReceipt: planResult.receipt(for: .feedbackPlan),
            profileReceipt: profileResult.receipt(for: .feedbackPlan)
        )
        try response.validate(for: value)
        return BridgeResponse(requestID: request.requestID, body: .feedbackPlan(response))
    }
}

private enum FeedbackRequestError: Error {
    case baselineUnavailable
    case invalidMeasurement
}

private enum OfflineStepOutcome {
    case success(DCXCTLProcessResult)
    case failure(BridgeErrorPayload)
}

private struct MeasurementCommandOutput: Decodable {
    struct Peak: Decodable {
        let frequencyHz: Double
        enum CodingKeys: String, CodingKey { case frequencyHz = "frequency_hz" }
    }

    let schemaVersion: String
    let source: FeedbackMeasurementSummaryV1.Source
    let sourceDigest: String
    let targetOutput: UInt8
    let peaks: [Peak]
    let digest: String

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case source
        case sourceDigest = "source_digest"
        case targetOutput = "target_output"
        case peaks, digest
    }

    func summary() throws -> FeedbackMeasurementSummaryV1 {
        guard schemaVersion == FeedbackNotchContract.measurementSchemaVersion else {
            throw ChildResponseError.bindingMismatch
        }
        return try .init(
            digest: digest,
            source: source,
            sourceDigest: sourceDigest,
            targetOutput: targetOutput,
            peakCount: peaks.count
        )
    }
}

private struct NotchPlanCommandOutput: Decodable {
    struct Codes: Decodable {
        let frequencyCode: UInt16
        let qCode: UInt16
        let gainCode: UInt16
        let kindCode: UInt16
        let slopeCode: UInt16
        enum CodingKeys: String, CodingKey {
            case frequencyCode = "frequency_code"
            case qCode = "q_code"
            case gainCode = "gain_code"
            case kindCode = "kind_code"
            case slopeCode = "slope_code"
        }
    }

    struct Notch: Decodable {
        let band: UInt8
        let frequencyHz: Double
        let occurrences: UInt8
        let levelDb: Double?
        let codes: Codes
        enum CodingKeys: String, CodingKey {
            case band
            case frequencyHz = "frequency_hz"
            case occurrences
            case levelDb = "level_db"
            case codes
        }
    }

    struct Dropped: Decodable {
        let frequencyHz: Double
        let reason: DroppedPeakV1.Reason
        enum CodingKeys: String, CodingKey {
            case frequencyHz = "frequency_hz"
            case reason
        }
    }

    struct Action: Decodable {
        let channel: UInt8
        let parameter: UInt8
        let value: UInt16
        let field: String
    }

    let schemaVersion: String
    let targetOutput: UInt8
    let parameterChannel: UInt8
    let baselineSnapshotDigest: String
    let measurementDigest: String
    let eqEnabledBefore: Bool
    let eqCountBefore: UInt8
    let operatorBandCount: UInt8
    let notches: [Notch]
    let dropped: [Dropped]
    let actions: [Action]
    let planDigest: String

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case targetOutput = "target_output"
        case parameterChannel = "parameter_channel"
        case baselineSnapshotDigest = "baseline_snapshot_digest"
        case measurementDigest = "measurement_digest"
        case eqEnabledBefore = "eq_enabled_before"
        case eqCountBefore = "eq_count_before"
        case operatorBandCount = "operator_band_count"
        case notches, dropped, actions
        case planDigest = "plan_digest"
    }

    /// Sanitize the plan and require its own action list to equal the actions
    /// its notches imply, field labels included.
    func summary() throws -> NotchPlanSummaryV1 {
        guard schemaVersion == FeedbackNotchContract.notchPlanSchemaVersion,
              targetOutput == FeedbackNotchContract.targetOutput,
              parameterChannel == FeedbackNotchContract.parameterChannel else {
            throw ChildResponseError.bindingMismatch
        }
        let summary = try NotchPlanSummaryV1(
            planDigest: planDigest,
            baselineSnapshotDigest: baselineSnapshotDigest,
            measurementDigest: measurementDigest,
            eqEnabledBefore: eqEnabledBefore,
            eqCountBefore: eqCountBefore,
            operatorBandCount: operatorBandCount,
            notches: notches.map {
                .init(
                    band: $0.band,
                    frequencyHz: $0.frequencyHz,
                    occurrences: $0.occurrences,
                    levelDb: $0.levelDb,
                    frequencyCode: $0.codes.frequencyCode,
                    qCode: $0.codes.qCode,
                    gainCode: $0.codes.gainCode,
                    kindCode: $0.codes.kindCode,
                    slopeCode: $0.codes.slopeCode
                )
            },
            dropped: dropped.map { .init(frequencyHz: $0.frequencyHz, reason: $0.reason) }
        )
        let carried = actions.map {
            DirectParameterActionV2(channel: $0.channel, parameter: $0.parameter, value: $0.value)
        }
        guard carried == summary.expectedActions,
              actions.allSatisfy({
                  PeqAddressV2(channel: $0.channel, parameter: $0.parameter)?.label == $0.field
              }) else {
            throw ChildResponseError.bindingMismatch
        }
        return summary
    }
}
