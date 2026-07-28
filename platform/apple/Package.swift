// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "HaloDiscoveryApple",
    platforms: [
        .iOS(.v16),
        .macOS(.v13),
    ],
    products: [
        .library(name: "HaloDiscoveryApple", targets: ["HaloDiscoveryApple"]),
    ],
    targets: [
        .target(name: "HaloDiscoveryApple"),
        .testTarget(
            name: "HaloDiscoveryAppleTests",
            dependencies: ["HaloDiscoveryApple"]
        ),
    ]
)

