// swift-tools-version:5.9
import Foundation
import PackageDescription

// `debug` while developing, `release` for the app bundle (see make-app.sh).
let core = ProcessInfo.processInfo.environment["SPATIAND_CORE"] ?? "debug"

// Spatiand on the Mac: a menu-bar app. See ../docs/mac-plan.md.
//
// The link to a host is Rust (../crates/spatiand-mac-core), built first by `./build.sh` into
// ../target and linked here as a static library.
let package = Package(
    name: "Spatiand",
    platforms: [.macOS(.v13)],
    targets: [
        .target(name: "CSpatiand", path: "Sources/CSpatiand"),
        .executableTarget(
            name: "Spatiand",
            dependencies: ["CSpatiand"],
            path: "Sources/Spatiand",
            linkerSettings: [
                .unsafeFlags(["-L../target/\(core)", "-lspatiand_mac_core"]),
                .linkedFramework("Security"),
                .linkedFramework("CoreFoundation"),
                .linkedFramework("SystemConfiguration"),
            ]
        ),
    ]
)
