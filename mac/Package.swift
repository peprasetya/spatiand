// swift-tools-version:5.9
import PackageDescription

// Spatiand on the Mac: a menu-bar app. See ../docs/mac-plan.md.
let package = Package(
    name: "Spatiand",
    platforms: [.macOS(.v13)],
    targets: [
        .executableTarget(name: "Spatiand", path: "Sources/Spatiand"),
    ]
)
