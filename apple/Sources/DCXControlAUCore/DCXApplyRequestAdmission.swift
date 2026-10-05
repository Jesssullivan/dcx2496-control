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
        try state.beginApplyAttempt(request)
    }
}

public enum DCXApplyRequestAdmissionError: Error, Equatable, Sendable {
    case operationUnavailable
}
