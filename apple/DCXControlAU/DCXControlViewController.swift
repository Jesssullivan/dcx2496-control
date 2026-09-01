import AppKit
import AudioToolbox
import CoreAudioKit
import DCXControlAUCore
import DCXLogicBridge
import UniformTypeIdentifiers

public final class DCXControlViewController: AUViewController, AUAudioUnitFactory {
    private var dcxAudioUnit: DCXControlAudioUnit?
    private var stateObservation: NSKeyValueObservation?
    private let statusLabel = NSTextField(labelWithString: "No desired profile is staged; helper contact requires an explicit action.")
    private let identityLabel = NSTextField(labelWithString: "Device: not identified")
    private let currentLabel = NSTextField(labelWithString: "Current: not captured")
    private let desiredLabel = NSTextField(labelWithString: "Desired: not staged")
    private let diffTextView = NSTextView()
    private let workQueue = DispatchQueue(label: "io.tinyland.dcx2496.logic.au-ui", qos: .userInitiated)
    private var configuredTarget: DCXTargetReference?
    private var helperCapabilities: Set<BridgeOperation> = []
    private var helperForeground = false
    private var helperRecovery: HelperRecoveryStatusV1?
    private var helperRecoveryUnavailable = false
    private var cachedMutationTarget: DCXTargetReference?
    private var bridgeRequestInFlight = false
    private var actionButtons: [NSButton] = []
    private var capabilityButtons: [BridgeOperation: NSButton] = [:]
    private var localActionButtons: [NSButton] = []

    public override func loadView() {
        let root = NSView()
        root.translatesAutoresizingMaskIntoConstraints = false

        let title = NSTextField(labelWithString: "DCX2496 Control")
        title.font = .systemFont(ofSize: 20, weight: .semibold)
        statusLabel.maximumNumberOfLines = 2
        statusLabel.textColor = .secondaryLabelColor
        identityLabel.maximumNumberOfLines = 2
        currentLabel.maximumNumberOfLines = 2
        desiredLabel.maximumNumberOfLines = 2

        diffTextView.isEditable = false
        diffTextView.isSelectable = true
        diffTextView.isRichText = false
        diffTextView.drawsBackground = false
        diffTextView.font = .monospacedSystemFont(ofSize: NSFont.smallSystemFontSize, weight: .regular)
        diffTextView.textContainerInset = NSSize(width: 6, height: 6)
        diffTextView.isHorizontallyResizable = false
        diffTextView.isVerticallyResizable = true
        diffTextView.textContainer?.widthTracksTextView = true
        diffTextView.string = "Diff: not previewed"
        let diffScrollView = NSScrollView()
        diffScrollView.documentView = diffTextView
        diffScrollView.hasVerticalScroller = true
        diffScrollView.hasHorizontalScroller = false
        diffScrollView.autohidesScrollers = true
        diffScrollView.borderType = .bezelBorder

        let refresh = button("Helper Status", action: #selector(helperStatus))
        let identify = button("Identify", action: #selector(identifyDevice))
        let stage = button("Stage Desired Profile…", action: #selector(stageDesiredProfile))
        let snapshot = button("Snapshot", action: #selector(captureSnapshot))
        let preview = button("Preview Diff", action: #selector(previewDiff))
        let apply = button("Apply", action: #selector(applyDesired))
        let readback = button("Readback", action: #selector(readback))
        let rollback = button("Rollback", action: #selector(rollback))
        let preparation = NSStackView(views: [refresh, identify, stage, snapshot, preview])
        preparation.orientation = .horizontal
        preparation.spacing = 8
        let mutation = NSStackView(views: [apply, readback, rollback])
        mutation.orientation = .horizontal
        mutation.spacing = 8
        actionButtons = [refresh, identify, stage, snapshot, preview, apply, readback, rollback]
        capabilityButtons = [
            .helperStatus: refresh,
            .identitySearch: identify,
            .snapshotCapture: snapshot,
            .diffPreview: preview,
            .apply: apply,
            .readback: readback,
            .rollback: rollback,
        ]
        localActionButtons = [stage]

        let stack = NSStackView(views: [
            title,
            statusLabel,
            identityLabel,
            currentLabel,
            desiredLabel,
            diffScrollView,
            preparation,
            mutation,
        ])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 12
        stack.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(stack)
        NSLayoutConstraint.activate([
            diffScrollView.widthAnchor.constraint(equalTo: stack.widthAnchor),
            diffScrollView.heightAnchor.constraint(equalToConstant: 132),
            stack.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 20),
            stack.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -20),
            stack.topAnchor.constraint(equalTo: root.topAnchor, constant: 20),
            stack.bottomAnchor.constraint(lessThanOrEqualTo: root.bottomAnchor, constant: -20),
        ])
        view = root
        refreshLabels()
        refreshActionAvailability()
    }

    public func createAudioUnit(
        with componentDescription: AudioComponentDescription
    ) throws -> AUAudioUnit {
        let audioUnit = try DCXControlAudioUnit(componentDescription: componentDescription)
        dcxAudioUnit = audioUnit
        stateObservation?.invalidate()
        stateObservation = audioUnit.observe(\.allParameterValues, options: [.initial, .new]) {
            [weak self, weak audioUnit] observed, _ in
            DispatchQueue.main.async {
                guard let self, let audioUnit,
                      self.dcxAudioUnit === audioUnit,
                      observed === audioUnit else { return }
                self.refreshLabels()
            }
        }
        return audioUnit
    }

    private func button(_ title: String, action: Selector) -> NSButton {
        let button = NSButton(title: title, target: self, action: action)
        button.bezelStyle = .rounded
        return button
    }

    @objc private func helperStatus() {
        send(.helperStatus(.init())) { [weak self] body in
            guard let self, case let .helperStatus(status) = body else { return }
            helperForeground = status.foreground
            helperCapabilities = Set(status.capabilities)
            helperRecoveryUnavailable = status.recoveryUnavailable
            if BridgeOperation.mutationCapabilities.isSubset(of: helperCapabilities),
               let target = status.target {
                cachedMutationTarget = target
            }
            if !status.recoveryUnavailable {
                helperRecovery = status.recovery
            }
            let targetChanged = configuredTarget != status.target
            configuredTarget = status.target
            if targetChanged {
                identityLabel.stringValue = "Device: not identified"
            }
            var acceptedCompletion = false
            var rejectedCompletion = false
            if !status.recoveryUnavailable,
               status.recovery == nil,
               let completion = status.completion,
               dcxAudioUnit?.controlState.view().recoveryActive == true {
                if let audioUnit = dcxAudioUnit {
                    do {
                        try audioUnit.performControlStateMutation {
                            try $0.acceptRecoveryCompletion(completion)
                        }
                        acceptedCompletion = true
                    } catch {
                        rejectedCompletion = true
                    }
                } else {
                    rejectedCompletion = true
                }
            }
            refreshLabels()
            if status.recoveryUnavailable {
                statusLabel.stringValue = "Helper recovery authority is temporarily unavailable; no device operation is authorized"
            } else if let recovery = status.recovery {
                statusLabel.stringValue = status.foreground
                    ? "Helper reachable; mutation recovery \(recovery.transactionID) is pinned"
                    : "Helper is not foreground; mutation recovery remains pinned"
            } else if acceptedCompletion {
                statusLabel.stringValue = "Helper terminal proof matched the local transaction; exact baseline recovery is complete"
            } else if rejectedCompletion {
                statusLabel.stringValue = "Helper terminal proof does not match the local recovery transaction"
            } else if let completion = status.completion {
                statusLabel.stringValue = "Helper retained exact-baseline completion \(completion.transactionID)"
            } else {
                statusLabel.stringValue = status.foreground
                    ? "Helper reachable; \(status.capabilities.count) capability entries"
                    : "Helper is not foreground"
            }
        }
    }

    @objc private func identifyDevice() {
        guard let target = configuredTarget else {
            report("Request Helper Status before identifying the configured DCX target")
            return
        }
        send(.identitySearch(.init(target: target))) { [weak self] body in
            guard let self, case let .identitySearch(result) = body else { return }
            guard configuredTarget == target else {
                report("Helper target changed while identity search was in progress")
                return
            }
            let identity = result.identity
            identityLabel.stringValue = "Device: \(identity.manufacturer) \(identity.model) · address \(identity.deviceAddress) · \(identity.selectedBaud) baud · \(identity.validSearchResponses) responses"
            report("Named DCX identity confirmed")
        }
    }

    @objc private func stageDesiredProfile() {
        guard dcxAudioUnit?.controlState.view().recoveryActive != true,
              helperRecovery == nil,
              !helperRecoveryUnavailable else {
            report("Staging is blocked until the active mutation recovery reaches its baseline")
            return
        }
        guard let target = configuredTarget else {
            report("Request Helper Status before staging the configured DCX target")
            return
        }
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.json]
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        panel.canChooseFiles = true
        panel.begin { [weak self] response in
            guard response == .OK, let url = panel.url else { return }
            guard self?.helperRecovery == nil,
                  self?.helperRecoveryUnavailable == false else {
                self?.report("Staging is blocked until the helper's mutation recovery completes")
                return
            }
            do {
                let values = try url.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey])
                guard values.isRegularFile == true,
                      let size = values.fileSize,
                      size <= 1_048_576 else {
                    self?.report("Desired profile must be one regular JSON file no larger than 1 MiB")
                    return
                }
                let profile = try BridgeJSONCodec.decoder().decode(
                    DesiredProfileV1.self,
                    from: Data(contentsOf: url)
                )
                try profile.validate()
                let staged = StagedProjectStateV1(target: target, desired: profile)
                try staged.validate()
                try self?.dcxAudioUnit?.performControlStateMutation {
                    try $0.stage(staged)
                }
                self?.report("Desired profile staged in Logic project state; no device call occurred")
                self?.refreshLabels()
            } catch DCXControlStateError.recoveryInProgress {
                self?.report("Staging is blocked until mutation recovery reaches its baseline")
                self?.refreshLabels()
            } catch {
                self?.report("Selected file is not a valid bounded desired profile")
            }
        }
    }

    @objc private func captureSnapshot() {
        let state = dcxAudioUnit?.controlState.view()
        guard let target = state?.projectState?.target ?? configuredTarget else {
            report("Request Helper Status before capturing the configured DCX target")
            return
        }
        send(.snapshotCapture(.init(target: target))) { [weak self] body in
            guard let self, case let .snapshotCapture(result) = body else { return }
            do {
                if let project = dcxAudioUnit?.controlState.view().projectState {
                    guard project.target == target else {
                        report("Staged target changed while snapshot capture was in progress")
                        return
                    }
                    try dcxAudioUnit?.performControlStateMutation {
                        try $0.accept(
                            snapshot: result.snapshot,
                            validSearchResponses: 10
                        )
                    }
                    report("Complete snapshot captured and bound to the staged profile")
                    refreshLabels()
                } else {
                    guard configuredTarget == target else {
                        report("Helper target changed while snapshot capture was in progress")
                        return
                    }
                    currentLabel.stringValue = "Current: \(result.snapshot.digest) (read-only; not stored in project state)"
                    report("Complete read-only snapshot captured; stage a profile before previewing a diff")
                }
                let identity = result.snapshot.identity
                identityLabel.stringValue = "Device: \(identity.manufacturer) \(identity.model) · address \(identity.deviceAddress) · \(identity.selectedBaud) baud · \(identity.validSearchResponses) responses"
            } catch {
                report("Snapshot no longer matches the staged project target")
            }
        }
    }

    @objc private func previewDiff() {
        guard let state = dcxAudioUnit?.controlState.view(),
              let project = state.projectState,
              let baseline = state.currentSnapshot else {
            report("A staged desired state and complete snapshot are required")
            return
        }
        send(.diffPreview(.init(target: project.target, baseline: baseline, desired: project.desired))) {
            [weak self] body in
            guard case let .diffPreview(result) = body else { return }
            do {
                try self?.dcxAudioUnit?.performControlStateMutation {
                    try $0.accept(diff: result.diff)
                }
                self?.report("Semantic diff previewed")
                self?.refreshLabels()
            } catch {
                self?.report("Diff no longer matches the staged profile and snapshot")
            }
        }
    }

    @objc private func applyDesired() {
        guard helperRecovery == nil else {
            report("The helper already has a pinned mutation recovery transaction")
            return
        }
        guard let state = dcxAudioUnit?.controlState.view(),
              let project = state.projectState,
              let baseline = state.currentSnapshot,
              let diff = state.diff else {
            report("Snapshot and preview the exact diff before Apply")
            return
        }
        do {
            let plan = try ApplyPlanV1(
                baseline: baseline,
                desired: project.desired,
                diff: diff
            )
            try dcxAudioUnit?.performControlStateMutation {
                try $0.beginApplyAttempt(
                    transactionID: diff.applyPlanDigest,
                    baseline: baseline
                )
            }
            refreshLabels()
            send(
                .apply(.init(target: project.target, plan: plan)),
                onError: { [weak self] error in
                    guard let self else { return }
                    guard error.code == .mutationNotAdmitted else {
                        report(error.message)
                        return
                    }
                    do {
                        try dcxAudioUnit?.performControlStateMutation {
                            try $0.rejectApplyBeforeAdmission(
                                transactionID: diff.applyPlanDigest,
                                baseline: baseline
                            )
                        }
                        report("Apply was rejected before mutation admission; the reviewed diff remains staged")
                        refreshLabels()
                    } catch {
                        report("Apply admission rejection no longer matches the local transaction")
                    }
                }
            ) { [weak self] body in
                guard case let .apply(result) = body else { return }
                do {
                    try self?.dcxAudioUnit?.performControlStateMutation {
                        try $0.acceptApply(
                            transactionID: result.transactionID,
                            baseline: baseline,
                            desiredSnapshotDigest: result.desiredSnapshotDigest,
                            readback: result.readback,
                            validSearchResponses: 1
                        )
                    }
                    self?.report(result.readback == nil
                        ? "Apply may have written; device state is unknown and rollback is required"
                        : result.rollbackRequired
                            ? "Apply readback mismatch; rollback is required"
                            : "Apply readback matched the desired digest")
                    self?.refreshLabels()
                } catch {
                    self?.report("Apply result did not match the previewed transaction")
                }
            }
        } catch {
            report("An exact snapshot-bound diff is required")
        }
    }

    @objc private func readback() {
        do {
            let state = dcxAudioUnit?.controlState.view()
            let request: ReadbackRequest
            let baselineDigest: String
            let updatesLocalState: Bool
            if let project = state?.projectState,
               let transactionID = state?.transactionID,
               let diff = state?.diff,
               let baseline = state?.rollbackBaseline {
                request = try ReadbackRequest(
                    target: project.target,
                    transactionID: transactionID,
                    expectedDesiredDigest: diff.desiredSnapshotDigest
                )
                baselineDigest = baseline.digest
                updatesLocalState = true
            } else if let recovery = helperRecovery {
                request = try ReadbackRequest(
                    target: recovery.target,
                    transactionID: recovery.transactionID,
                    expectedDesiredDigest: recovery.desiredSnapshotDigest
                )
                baselineDigest = recovery.baseline.digest
                updatesLocalState = false
            } else {
                report("No bounded apply transaction is available for readback")
                return
            }
            send(.readback(request)) { [weak self] body in
                guard case let .readback(result) = body else { return }
                do {
                    if updatesLocalState {
                        try self?.dcxAudioUnit?.performControlStateMutation {
                            try $0.acceptReadback(
                                transactionID: result.transactionID,
                                snapshot: result.snapshot,
                                validSearchResponses: 10
                            )
                        }
                    } else {
                        guard result.transactionID == request.transactionID,
                              result.snapshot.target == request.target else {
                            throw DCXControlStateError.invalidTransactionBinding
                        }
                    }
                    if result.snapshot.digest == baselineDigest {
                        self?.helperRecovery = nil
                        self?.report("Readback equals the immutable baseline; recovery is complete")
                    } else {
                        self?.report(result.matchesDesired ? "Readback matches desired" : "Readback mismatch")
                    }
                    self?.refreshLabels()
                } catch {
                    self?.report("Readback no longer matches the active transaction")
                }
            }
        } catch {
            report("Readback binding is invalid")
        }
    }

    @objc private func rollback() {
        do {
            let state = dcxAudioUnit?.controlState.view()
            let target: DCXTargetReference
            let transactionID: String
            let baseline: SnapshotV1
            let rollbackPlanDigest: String
            let updatesLocalState: Bool
            if let project = state?.projectState,
               let localTransactionID = state?.transactionID,
               let localBaseline = state?.rollbackBaseline,
               let diff = state?.diff {
                target = project.target
                transactionID = localTransactionID
                baseline = localBaseline
                rollbackPlanDigest = diff.rollbackPlanDigest
                updatesLocalState = true
            } else if let recovery = helperRecovery {
                target = recovery.target
                transactionID = recovery.transactionID
                baseline = recovery.baseline
                rollbackPlanDigest = recovery.rollbackPlanDigest
                updatesLocalState = false
            } else {
                report("No immutable rollback baseline is available")
                return
            }
            let plan = try RollbackPlanV1(
                transactionID: transactionID,
                baseline: baseline,
                rollbackPlanDigest: rollbackPlanDigest
            )
            if updatesLocalState {
                try dcxAudioUnit?.performControlStateMutation {
                    try $0.beginRollbackAttempt(
                        transactionID: transactionID,
                        baseline: baseline
                    )
                }
            }
            refreshLabels()
            send(.rollback(.init(target: target, plan: plan))) { [weak self] body in
                guard case let .rollback(result) = body else { return }
                do {
                    if updatesLocalState {
                        try self?.dcxAudioUnit?.performControlStateMutation {
                            try $0.acceptRollback(
                                transactionID: result.transactionID,
                                baselineDigest: result.baselineDigest,
                                restored: result.restored,
                                equalsBaseline: result.equalsBaseline,
                                validSearchResponses: 1
                            )
                        }
                    } else {
                        guard result.transactionID == transactionID,
                              result.baselineDigest == baseline.digest,
                              result.equalsBaseline == (result.restored?.digest == baseline.digest) else {
                            throw DCXControlStateError.invalidRollbackBinding
                        }
                    }
                    if result.equalsBaseline {
                        self?.helperRecovery = nil
                    }
                    self?.report(result.restored == nil
                        ? "Rollback may have written; device state remains unknown"
                        : result.equalsBaseline
                            ? "Rollback readback equals the immutable baseline"
                            : "Rollback readback mismatch; device state remains unresolved")
                    self?.refreshLabels()
                } catch {
                    self?.report("Rollback result did not match the active transaction")
                }
            }
        } catch {
            report("The immutable rollback binding is invalid")
        }
    }

    private func send(
        _ body: BridgeRequestBody,
        onError: (@MainActor (BridgeErrorPayload) -> Void)? = nil,
        accept: @escaping @MainActor (BridgeResponseBody) -> Void
    ) {
        guard isOperationAvailable(body.operation) else {
            let error = BridgeErrorPayload(
                code: body.operation == .apply ? .mutationNotAdmitted : .operationUnavailable,
                message: "\(body.operation.rawValue) is not available from the foreground helper",
                retryable: false
            )
            if let onError { onError(error) } else { report(error.message) }
            return
        }
        guard !bridgeRequestInFlight else {
            let error = BridgeErrorPayload(
                code: body.operation == .apply ? .mutationNotAdmitted : .operationInFlight,
                message: "One bounded helper request is already in progress",
                retryable: true
            )
            if let onError { onError(error) } else { report(error.message) }
            return
        }
        let request: BridgeRequest
        let client: AppGroupSocketClient
        do {
            request = try BridgeRequest(body: body)
            let locations = try AppGroupLocations()
            client = try AppGroupSocketClient(socketURL: locations.socketURL)
        } catch {
            let error = BridgeErrorPayload(
                code: body.operation == .apply ? .mutationNotAdmitted : .ipcUnavailable,
                message: "The bounded helper request could not be prepared",
                retryable: true
            )
            if let onError { onError(error) } else { report(error.message) }
            return
        }
        bridgeRequestInFlight = true
        actionButtons.forEach { $0.isEnabled = false }
        report("Request in progress")
        workQueue.async { [weak self] in
            do {
                let response = try client.exchange(request)
                DispatchQueue.main.async {
                    self?.completeBridgeRequest()
                    if let error = response.error {
                        if error.code == .helperNotForeground
                            || error.code == .mutationNotAdmitted {
                            self?.invalidateVolatileHelperState()
                        }
                        if let onError {
                            onError(error)
                        } else {
                            self?.report(error.message)
                        }
                    } else if let body = response.body {
                        accept(body)
                    } else {
                        self?.report("Helper returned an empty response")
                    }
                }
            } catch {
                DispatchQueue.main.async {
                    self?.completeBridgeRequest()
                    self?.invalidateVolatileHelperState()
                    self?.report("Foreground helper is unavailable")
                }
            }
        }
    }

    @MainActor
    private func completeBridgeRequest() {
        bridgeRequestInFlight = false
        refreshActionAvailability()
    }

    @MainActor
    private func invalidateVolatileHelperState() {
        helperForeground = false
        helperCapabilities = []
        refreshActionAvailability()
    }

    @MainActor
    private func isOperationAvailable(_ operation: BridgeOperation) -> Bool {
        if operation == .helperStatus { return true }
        guard helperForeground else { return false }
        guard !helperRecoveryUnavailable else { return false }
        if BridgeOperation.mutationCapabilities.contains(operation) {
            return hasMutationAuthorization(
                operation,
                for: dcxAudioUnit?.controlState.view()
            )
        }
        return configuredTarget != nil && helperCapabilities.contains(operation)
    }

    @MainActor
    private func hasMutationAuthorization(
        _ operation: BridgeOperation,
        for state: DCXControlStateView?
    ) -> Bool {
        let localRecoveryActive = state?.recoveryActive == true
        if let helperRecovery {
            guard !localRecoveryActive
                    || (helperRecovery.transactionID == state?.transactionID
                        && helperRecovery.target == state?.projectState?.target) else {
                return false
            }
            return Set(helperRecovery.capabilities).contains(operation)
        }
        if localRecoveryActive {
            return operation != .apply
                && cachedMutationTarget == state?.projectState?.target
        }
        let requiredTarget = state?.projectState?.target ?? configuredTarget
        return configuredTarget == requiredTarget
            && requiredTarget != nil
            && BridgeOperation.mutationCapabilities.isSubset(of: helperCapabilities)
    }

    @MainActor
    private func refreshActionAvailability() {
        let state = dcxAudioUnit?.controlState.view()
        let localRecoveryActive = state?.recoveryActive == true
        let advertisedRecoveryActive = helperRecovery != nil
        let recoveryAuthorityMatches = !localRecoveryActive
            || helperRecovery == nil
            || (helperRecovery?.transactionID == state?.transactionID
                && helperRecovery?.target == state?.projectState?.target)
        let recoveryActive = localRecoveryActive
            || advertisedRecoveryActive
            || helperRecoveryUnavailable
        for (operation, button) in capabilityButtons {
            let phaseAllows: Bool
            switch operation {
            case .helperStatus:
                phaseAllows = true
            case .readback, .rollback:
                phaseAllows = recoveryActive && recoveryAuthorityMatches
            case .identitySearch, .snapshotCapture, .diffPreview, .apply:
                phaseAllows = !recoveryActive
            }
            button.isEnabled = !bridgeRequestInFlight
                && phaseAllows
                && isOperationAvailable(operation)
            if BridgeOperation.mutationCapabilities.contains(operation) {
                button.isHidden = !hasMutationAuthorization(operation, for: state)
            }
        }
        localActionButtons.forEach {
            $0.isEnabled = !bridgeRequestInFlight
                && !recoveryActive
                && configuredTarget != nil
        }
    }

    @MainActor
    private func report(_ message: String) {
        statusLabel.stringValue = message
    }

    @MainActor
    private func refreshLabels() {
        guard let state = dcxAudioUnit?.controlState.view() else {
            refreshActionAvailability()
            return
        }
        currentLabel.stringValue = state.deviceStateUncertain
            ? "Current: unresolved after a device write"
            : "Current: " + (state.currentSnapshot?.digest ?? "not captured")
        desiredLabel.stringValue = "Desired: " + (state.projectState?.desired.digest ?? "not staged")
        diffTextView.string = render(diff: state.diff)
        refreshActionAvailability()
    }

    private func render(diff: SemanticDiffV1?) -> String {
        guard let diff else { return "Diff: not previewed" }
        guard !diff.changes.isEmpty else { return "Diff: no semantic changes" }
        let changes = diff.changes.enumerated().map { index, change in
            "\(index + 1). \(change.path)\n   \(render(change.before)) → \(render(change.after))"
        }
        return (["Diff: \(diff.changes.count) semantic change\(diff.changes.count == 1 ? "" : "s")"] + changes)
            .joined(separator: "\n")
    }

    private func render(_ value: JSONValue) -> String {
        guard let data = try? BridgeJSONCodec.encoder().encode(value),
              let text = String(data: data, encoding: .utf8) else {
            return "<unrenderable JSON>"
        }
        return text
    }
}
