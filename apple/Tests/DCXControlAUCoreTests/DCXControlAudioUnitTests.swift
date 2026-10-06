import AudioToolbox
import DCXLogicBridge
import XCTest
@testable import DCXControlAUCore

final class DCXControlAudioUnitTests: XCTestCase {
    func testMIDIAudioUnitAllocatesRenderResourcesWithOneOutputBus() throws {
        let description = AudioComponentDescription(
            componentType: kAudioUnitType_MIDIProcessor,
            componentSubType: 0x4463_7843, // DcxC
            componentManufacturer: 0x546E_4C64, // TnLd
            componentFlags: 0,
            componentFlagsMask: 0
        )
        let audioUnit = try DCXControlAudioUnit(componentDescription: description)

        XCTAssertEqual(audioUnit.inputBusses.count, 0)
        XCTAssertEqual(audioUnit.outputBusses.count, 1)
        XCTAssertEqual(audioUnit.outputBusses[0].format.channelCount, 2)

        try audioUnit.allocateRenderResources()
        XCTAssertTrue(audioUnit.renderResourcesAllocated)
        audioUnit.deallocateRenderResources()
        XCTAssertFalse(audioUnit.renderResourcesAllocated)
    }

    func testRenderSilencesBothAudioBuffersAndPreservesMIDI() throws {
        let audioUnit = try makeAudioUnit()
        let expectedMIDI: [UInt8] = [0x90, 60, 64]
        var receivedMIDI: [UInt8] = []
        var receivedTime: AUEventSampleTime = 0
        var receivedCable: UInt8 = 0
        audioUnit.midiOutputEventBlock = { time, cable, count, bytes in
            receivedMIDI = Array(UnsafeBufferPointer(start: bytes, count: count))
            receivedTime = time
            receivedCable = cable
            return noErr
        }
        var event = AURenderEvent()
        event.MIDI.eventType = .MIDI
        event.MIDI.eventSampleTime = 1234
        event.MIDI.length = 3
        event.MIDI.cable = 2
        event.MIDI.data = (0x90, 60, 64)
        let output = AudioBufferList.allocate(maximumBuffers: 2)
        defer { output.unsafeMutablePointer.deallocate() }
        var left = [Float](repeating: 0.25, count: 8)
        var right = [Float](repeating: -0.5, count: 8)
        var flags: AudioUnitRenderActionFlags = []
        var timestamp = AudioTimeStamp()
        let render = audioUnit.internalRenderBlock
        let result = left.withUnsafeMutableBytes { leftBytes in
            right.withUnsafeMutableBytes { rightBytes in
                output[0] = AudioBuffer(mNumberChannels: 1,
                                        mDataByteSize: UInt32(leftBytes.count),
                                        mData: leftBytes.baseAddress)
                output[1] = AudioBuffer(mNumberChannels: 1,
                                        mDataByteSize: UInt32(rightBytes.count),
                                        mData: rightBytes.baseAddress)
                return render(&flags, &timestamp, 8, 0,
                              output.unsafeMutablePointer, &event, nil)
            }
        }
        XCTAssertEqual(result, noErr)
        XCTAssertTrue(left.allSatisfy { $0 == 0 })
        XCTAssertTrue(right.allSatisfy { $0 == 0 })
        XCTAssertTrue(flags.contains(.unitRenderAction_OutputIsSilence))
        XCTAssertEqual(receivedMIDI, expectedMIDI)
        XCTAssertEqual(receivedTime, 1234)
        XCTAssertEqual(receivedCable, 2)
    }

    func testDocumentStateRestoresExactDesiredDigestBeforeInvalidation() throws {
        let expectedDigest = "sha256/6135022f405de2d172475865d2ecac3479b898eba209c687a0e6e5d92d774ec4"
        let profile = try DesiredProfileV1(
            profileID: "pzm-rew-o1-peq9-qualification",
            revision: "2026-09-01",
            digest: expectedDigest,
            document: .object([
                "target_output": .number(1),
                "parameter_channel": .number(5),
                "slot": .number(9),
                "actions": .array([
                    .object(["channel": .number(5), "parameter": .number(59), "value": .number(53)]),
                    .object(["channel": .number(5), "parameter": .number(60), "value": .number(32)]),
                    .object(["channel": .number(5), "parameter": .number(61), "value": .number(118)]),
                    .object(["channel": .number(5), "parameter": .number(62), "value": .number(1)]),
                ]),
            ])
        )
        let target = try DCXTargetReference(
            bindingID: "studio-dcx2496",
            expectedDeviceAddress: 0
        )
        let source = try makeAudioUnit()
        let sourcePublished = keyValueObservingExpectation(
            for: source,
            keyPath: "fullStateForDocument"
        ) { object, _ in
            guard let audioUnit = object as? DCXControlAudioUnit else { return false }
            return audioUnit.controlState.view().projectState?.desired.digest == expectedDigest
        }
        try source.performControlStateMutation {
            try $0.stage(.init(target: target, desired: profile))
        }
        wait(for: [sourcePublished], timeout: 1)
        XCTAssertEqual(
            source.parameterTree?.parameter(
                withAddress: DCXControlAudioUnit.ParameterAddress.desiredStateStaged
            )?.value,
            1
        )
        let documentState = try XCTUnwrap(source.fullStateForDocument)
        let encodedCarrier = try XCTUnwrap(
            documentState[DCXControlPersistedStateV1.schemaVersion] as? String
        )

        let restored = try makeAudioUnit()
        let invalidatedAfterRestore = expectation(description: "restored state reaches the AU view")
        invalidatedAfterRestore.assertForOverFulfill = false
        let restorationObservation = restored.observe(\.allParameterValues, options: [.new]) {
            [weak restored] _, _ in
            DispatchQueue.main.async {
                if restored?.controlState.view().projectState?.desired.digest == expectedDigest {
                    invalidatedAfterRestore.fulfill()
                }
            }
        }
        restored.fullStateForDocument = documentState
        wait(for: [invalidatedAfterRestore], timeout: 1)
        withExtendedLifetime(restorationObservation) {}

        let view = restored.controlState.view()
        XCTAssertEqual(view.projectState?.target, target)
        XCTAssertEqual(view.projectState?.desired.digest, expectedDigest)
        XCTAssertNil(view.currentSnapshot)
        XCTAssertNil(view.diff)
        XCTAssertFalse(view.recoveryActive)
        var presentation = DCXControlPresentation()
        XCTAssertEqual(
            presentation.statusAfterStateRefresh(
                DCXControlPresentation.unstagedStatus,
                state: view
            ),
            DCXControlPresentation.restoredStatus
        )
        XCTAssertEqual(
            presentation.statusAfterStateRefresh(
                "Semantic diff previewed",
                state: view
            ),
            "Semantic diff previewed"
        )
        XCTAssertEqual(
            restored.fullStateForDocument?[DCXControlPersistedStateV1.schemaVersion] as? String,
            encodedCarrier
        )
    }

    private func makeAudioUnit() throws -> DCXControlAudioUnit {
        try DCXControlAudioUnit(
            componentDescription: .init(
                componentType: kAudioUnitType_MIDIProcessor,
                componentSubType: 0x4463_7843,
                componentManufacturer: 0x546E_4C64,
                componentFlags: 0,
                componentFlagsMask: 0
            )
        )
    }
}
