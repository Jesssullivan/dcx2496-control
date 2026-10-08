import AudioToolbox
import AVFoundation
import DCXLogicBridge
import Foundation

public final class DCXControlAudioUnit: AUAudioUnit {
    private static let controlStateKey = DCXControlPersistedStateV1.schemaVersion
    private static let legacyProjectStateKey = StagedProjectStateV1.schemaVersion
    @objc dynamic private var controlStateRevision: UInt64 = 0

    public enum ParameterAddress {
        public static let desiredStateStaged: AUParameterAddress = 0
        public static let pendingChangeCount: AUParameterAddress = 1
        public static let rollbackAvailable: AUParameterAddress = 2
    }

    public override class func keyPathsForValuesAffectingValue(
        forKey key: String
    ) -> Set<String> {
        var paths = super.keyPathsForValuesAffectingValue(forKey: key)
        if key == #keyPath(AUAudioUnit.fullState)
            || key == #keyPath(AUAudioUnit.fullStateForDocument)
            || key == #keyPath(AUAudioUnit.allParameterValues) {
            paths.insert("controlStateRevision")
        }
        return paths
    }

    public let controlState = DCXControlState()
    private lazy var emptyInputBusses = AUAudioUnitBusArray(
        audioUnit: self,
        busType: .input,
        busses: []
    )
    private var midiOutputBus: AUAudioUnitBus!
    private var midiOutputBusses: AUAudioUnitBusArray!

    public override init(
        componentDescription: AudioComponentDescription,
        options: AudioComponentInstantiationOptions = []
    ) throws {
        try super.init(componentDescription: componentDescription, options: options)

        guard let format = AVAudioFormat(
            standardFormatWithSampleRate: 44_100,
            channels: 2
        ) else {
            throw NSError(
                domain: NSOSStatusErrorDomain,
                code: Int(kAudioUnitErr_FailedInitialization),
                userInfo: nil
            )
        }
        midiOutputBus = try AUAudioUnitBus(format: format)
        midiOutputBus.maximumChannelCount = 2
        midiOutputBusses = AUAudioUnitBusArray(
            audioUnit: self,
            busType: .output,
            busses: [midiOutputBus]
        )

        let staged = AUParameterTree.createParameter(
            withIdentifier: "desiredStateStaged",
            name: "Desired State Staged",
            address: ParameterAddress.desiredStateStaged,
            min: 0,
            max: 1,
            unit: .boolean,
            unitName: nil,
            flags: [.flag_IsReadable],
            valueStrings: ["No", "Yes"],
            dependentParameters: nil
        )
        let pending = AUParameterTree.createParameter(
            withIdentifier: "pendingChangeCount",
            name: "Pending Changes",
            address: ParameterAddress.pendingChangeCount,
            min: 0,
            max: 1,
            unit: .indexed,
            unitName: nil,
            flags: [.flag_IsReadable],
            valueStrings: nil,
            dependentParameters: nil
        )
        let rollback = AUParameterTree.createParameter(
            withIdentifier: "rollbackAvailable",
            name: "Rollback Available",
            address: ParameterAddress.rollbackAvailable,
            min: 0,
            max: 1,
            unit: .boolean,
            unitName: nil,
            flags: [.flag_IsReadable],
            valueStrings: ["No", "Yes"],
            dependentParameters: nil
        )
        let tree = AUParameterTree.createTree(withChildren: [staged, pending, rollback])
        let state = controlState
        tree.implementorValueProvider = { parameter in
            let view = state.view()
            switch parameter.address {
            case ParameterAddress.desiredStateStaged:
                return view.projectState == nil ? 0 : 1
            case ParameterAddress.pendingChangeCount:
                return AUValue(view.diff?.changes.count ?? 0)
            case ParameterAddress.rollbackAvailable:
                return view.rollbackBaseline == nil ? 0 : 1
            default:
                return 0
            }
        }
        // Host restoration/automation of presentation parameters cannot cause
        // helper IPC or a device action. Values are derived from typed state.
        tree.implementorValueObserver = { _, _ in }
        parameterTree = tree
    }

    public override var inputBusses: AUAudioUnitBusArray { emptyInputBusses }
    public override var outputBusses: AUAudioUnitBusArray { midiOutputBusses }
    public override var midiOutputNames: [String] { ["MIDI Thru"] }

    public override var fullState: [String: Any]? {
        get {
            var state = super.fullState ?? [:]
            state.removeValue(forKey: Self.controlStateKey)
            state.removeValue(forKey: Self.legacyProjectStateKey)
            if let persisted = try? controlState.persistedState(),
               let data = try? BridgeJSONCodec.encoder().encode(persisted) {
                state[Self.controlStateKey] = data.base64EncodedString()
            }
            return state
        }
        set {
            super.fullState = newValue
            if let encoded = newValue?[Self.controlStateKey] as? String {
                guard let data = Data(base64Encoded: encoded),
                      let persisted = try? BridgeJSONCodec.decoder().decode(
                          DCXControlPersistedStateV1.self,
                          from: data
                      ),
                      (try? controlState.restore(persisted)) != nil else {
                    restoreDesiredOnlyState(nil)
                    return
                }
                return
            }

            // Restore the former desired-only carrier for existing Logic
            // projects, then emit the complete v1 carrier on the next save.
            if let encoded = newValue?[Self.legacyProjectStateKey] as? String,
               let data = Data(base64Encoded: encoded),
               let staged = try? BridgeJSONCodec.decoder().decode(StagedProjectStateV1.self, from: data),
               restoreDesiredOnlyState(staged) {
                return
            }
            restoreDesiredOnlyState(nil)
        }
    }

    @discardableResult
    private func restoreDesiredOnlyState(_ staged: StagedProjectStateV1?) -> Bool {
        (try? controlState.restore(.init(
            projectState: staged, currentSnapshot: nil, diff: nil,
            transactionID: nil, rollbackBaseline: nil, deviceStateUncertain: false
        ))) != nil
    }

    /// Apply one typed state transition and tell the host that the custom AU
    /// document state changed. The notification never performs helper IPC or
    /// device I/O; it only invalidates the read-only presentation parameters.
    @discardableResult
    public func performControlStateMutation<Result>(
        _ update: (DCXControlState) throws -> Result
    ) rethrows -> Result {
        let result = try update(controlState)
        publishControlStateChange()
        return result
    }

    private func publishControlStateChange() {
        let state = controlState.view()
        parameterTree?.parameter(withAddress: ParameterAddress.desiredStateStaged)?.setValue(
            state.projectState == nil ? 0 : 1,
            originator: nil
        )
        parameterTree?.parameter(withAddress: ParameterAddress.pendingChangeCount)?.setValue(
            AUValue(state.diff?.changes.count ?? 0),
            originator: nil
        )
        parameterTree?.parameter(withAddress: ParameterAddress.rollbackAvailable)?.setValue(
            state.rollbackBaseline == nil ? 0 : 1,
            originator: nil
        )
        controlStateRevision &+= 1
    }

    public override var internalRenderBlock: AUInternalRenderBlock {
        let midiOutput = midiOutputEventBlock
        let midiEventListOutput = midiOutputEventListBlock
        return { flags, _, _, _, outputData, realtimeEventListHead, _ in
            // The stereo bus exists for the host's MIDI-effect lifecycle, not
            // audio production. Never leave host-provided samples untouched.
            for buffer in UnsafeMutableAudioBufferListPointer(outputData) {
                if let data = buffer.mData { memset(data, 0, Int(buffer.mDataByteSize)) }
            }
            flags.pointee.insert(.unitRenderAction_OutputIsSilence)
            var event = realtimeEventListHead
            while let current = event {
                let header = current.pointee.head
                switch header.eventType {
                case .MIDI, .midiSysEx:
                    if let midiOutput {
                        let midi = current.pointee.MIDI
                        withUnsafePointer(to: current.pointee.MIDI.data) { tuplePointer in
                            let bytes = UnsafeRawPointer(tuplePointer).assumingMemoryBound(to: UInt8.self)
                            _ = midiOutput(midi.eventSampleTime, midi.cable, Int(midi.length), bytes)
                        }
                    }
                case .midiEventList:
                    if #available(macOS 12.0, *), let midiEventListOutput {
                        let midi = current.pointee.MIDIEventsList
                        withUnsafePointer(to: current.pointee.MIDIEventsList.eventList) { list in
                            _ = midiEventListOutput(midi.eventSampleTime, midi.cable, list)
                        }
                    }
                default:
                    break
                }
                event = header.next.map { UnsafePointer($0) }
            }
            return noErr
        }
    }
}
