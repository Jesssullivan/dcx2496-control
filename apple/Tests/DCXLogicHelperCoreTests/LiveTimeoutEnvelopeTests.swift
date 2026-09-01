import DCXLogicBridge
@testable import DCXLogicHelperCore
import Foundation
import XCTest

final class LiveTimeoutEnvelopeTests: XCTestCase {
    func testMutationEnvelopeOutlivesRustApplyBudgetAndFitsInsideSocket() throws {
        let configuration = try HelperConfigurationV1(
            bindingID: "studio-dcx2496",
            expectedDeviceAddress: 0,
            ttyPath: "/dev/cu.usbserial-test",
            enabledFeatures: [.apply, .readback, .rollback]
        )

        XCTAssertEqual(configuration.childTimeoutSeconds, 155)
        XCTAssertGreaterThan(
            AppGroupSocketClient.defaultTimeoutSeconds,
            Int(configuration.childTimeoutSeconds)
        )
    }

    func testPriorMutationConfigurationMigratesFrom125Seconds() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false)
        defer { try? FileManager.default.removeItem(at: directory) }

        let url = directory.appendingPathComponent("helper-config-v1.json")
        let priorConfiguration = """
        {
          "schemaVersion": "dcx.helper-configuration/v1",
          "target": {
            "bindingID": "studio-dcx2496",
            "expectedDeviceAddress": 0
          },
          "ttyPath": "/dev/cu.usbserial-test",
          "enabledOperations": [
            "device.apply",
            "device.readback",
            "device.rollback"
          ],
          "childTimeoutSeconds": 125
        }
        """
        try Data(priorConfiguration.utf8).write(to: url)
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o600],
            ofItemAtPath: url.path
        )

        let migrated = try HelperConfigurationV1.load(from: url)

        XCTAssertEqual(migrated.childTimeoutSeconds, 155)
        XCTAssertEqual(
            Set(migrated.enabledFeatures),
            Set([.apply, .readback, .rollback])
        )
    }
}
