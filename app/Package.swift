// swift-tools-version:6.0
import PackageDescription

let package = Package(
    name: "ReadAloud",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(
            name: "ReadAloud",
            path: "Sources/ReadAloud",
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .testTarget(
            name: "ReadAloudTests",
            dependencies: ["ReadAloud"],
            path: "Tests/ReadAloudTests",
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
    ]
)
