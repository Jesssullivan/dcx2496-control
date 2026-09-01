import AudioToolbox
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
}
