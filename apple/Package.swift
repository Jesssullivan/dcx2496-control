// swift-tools-version: 5.10

import PackageDescription

let package = Package(
    name: "DCXApple",
    platforms: [
        .macOS(.v14),
    ],
    products: [
        .library(name: "DCXLogicBridge", targets: ["DCXLogicBridge"]),
        .library(name: "DCXLogicHelperCore", targets: ["DCXLogicHelperCore"]),
        .library(name: "DCXControlAUCore", targets: ["DCXControlAUCore"]),
    ],
    targets: [
        .target(name: "DCXLogicBridge"),
        .target(
            name: "DCXLogicHelperCore",
            dependencies: ["DCXLogicBridge"]
        ),
        .target(
            name: "DCXControlAUCore",
            dependencies: ["DCXLogicBridge"]
        ),
        .testTarget(
            name: "DCXControlAUCoreTests",
            dependencies: ["DCXControlAUCore"]
        ),
        .testTarget(
            name: "DCXLogicHelperCoreTests",
            dependencies: ["DCXLogicBridge", "DCXLogicHelperCore"]
        ),
    ]
)
