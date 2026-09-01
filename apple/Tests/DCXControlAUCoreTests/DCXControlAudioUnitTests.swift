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
        XCTAssertEqual(
            DCXControlPresentation.statusAfterStateRefresh(
                DCXControlPresentation.unstagedStatus,
                state: view
            ),
            DCXControlPresentation.restoredStatus
        )
        XCTAssertEqual(
            DCXControlPresentation.statusAfterStateRefresh(
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
