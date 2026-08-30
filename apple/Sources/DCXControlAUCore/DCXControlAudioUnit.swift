import AudioToolbox
import DCXLogicBridge
import Foundation

public final class DCXControlAudioUnit: AUAudioUnit {
    private static let controlStateKey = DCXControlPersistedStateV1.schemaVersion
    private static let legacyProjectStateKey = StagedProjectStateV1.schemaVersion

    public enum ParameterAddress {
        public static let desiredStateStaged: AUParameterAddress = 0
        public static let pendingChangeCount: AUParameterAddress = 1
        public static let rollbackAvailable: AUParameterAddress = 2
    }

    public let controlState = DCXControlState()
    private lazy var emptyInputBusses = AUAudioUnitBusArray(
        audioUnit: self,
        busType: .input,
        busses: []
    )
    private lazy var emptyOutputBusses = AUAudioUnitBusArray(
        audioUnit: self,
        busType: .output,
        busses: []
    )

    public override init(
        componentDescription: AudioComponentDescription,
        options: AudioComponentInstantiationOptions = []
    ) throws {
        try super.init(componentDescription: componentDescription, options: options)

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
    public override var outputBusses: AUAudioUnitBusArray { emptyOutputBusses }
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
                    try? controlState.reset()
                    return
                }
                return
            }

            // Restore the former desired-only carrier for existing Logic
            // projects, then emit the complete v1 carrier on the next save.
            if let encoded = newValue?[Self.legacyProjectStateKey] as? String,
               let data = Data(base64Encoded: encoded),
               let staged = try? BridgeJSONCodec.decoder().decode(StagedProjectStateV1.self, from: data),
               (try? controlState.stage(staged)) != nil {
                return
            }
            try? controlState.reset()
        }
    }

    public override var internalRenderBlock: AUInternalRenderBlock {
        let midiOutput = midiOutputEventBlock
        let midiEventListOutput = midiOutputEventListBlock
        return { _, _, _, _, _, realtimeEventListHead, _ in
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
