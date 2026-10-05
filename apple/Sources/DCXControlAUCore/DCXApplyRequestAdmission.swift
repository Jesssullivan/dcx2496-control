import DCXLogicBridge

/// Authorize the first explicit Apply while its preview is still staged, then
/// retain recovery before the caller dispatches that exact request. This pure
/// boundary performs no IPC or device work and provides no retry bypass.
public enum DCXApplyRequestAdmission {
    public static func prepare(
        _ request: ApplyRequest,
        state: DCXControlState,
        isAuthorized: () -> Bool
    ) throws {
        try BridgeRequestBody.apply(request).validate()
        guard isAuthorized() else {
            throw DCXApplyRequestAdmissionError.operationUnavailable
        }
        let staged = state.view()
        guard let project = staged.projectState,
              !staged.recoveryActive,
              request.target == project.target,
              request.plan.desired == project.desired,
              request.plan.baseline == staged.currentSnapshot,
              request.plan.diff == staged.diff else {
            throw DCXControlStateError.invalidTransactionBinding
        }
        try state.beginApplyAttempt(
            transactionID: request.plan.diff.applyPlanDigest,
            baseline: request.plan.baseline
        )
    }
}

public enum DCXApplyRequestAdmissionError: Error, Equatable, Sendable {
    case operationUnavailable
}
