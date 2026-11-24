// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "FactoryApp",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(name: "FactoryApp", path: "Sources/FactoryApp"),
        .testTarget(name: "FactoryAppTests", dependencies: ["FactoryApp"], path: "Tests/FactoryAppTests"),
    ]
)
