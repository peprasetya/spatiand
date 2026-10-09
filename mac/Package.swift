// swift-tools-version:5.9
import Foundation
import PackageDescription

// `debug` while developing, `release` for the app bundle (see make-app.sh).
let core = ProcessInfo.processInfo.environment["SPATIAND_CORE"] ?? "debug"

// Spatiand on the Mac: a menu-bar app. See ../docs/mac-plan.md.
//
// Everything that is not AppKit's is Rust: ../crates/spatiand-mac, which is the Deck's compositor
// (the room in the glasses) and, through ../crates/spatiand-mac-core, the link to a host for
// windows on this Mac and this Mac as a host. It is built first by `./build.sh` and linked here as
// one static library, with the libxkbcommon it needs (xkbcommon/build.sh).
let package = Package(
    name: "Spatiand",
    platforms: [.macOS(.v14)],
    targets: [
        .target(name: "CSpatiand", path: "Sources/CSpatiand"),
        .executableTarget(
            name: "Spatiand",
            dependencies: ["CSpatiand"],
            path: "Sources/Spatiand",
            linkerSettings: [
                .unsafeFlags(["-L../crates/spatiand-mac/target/\(core)", "-lspatiand_mac", "-Lxkbcommon/out", "-lxkbcommon"]),
                .linkedFramework("Security"),
                .linkedFramework("CoreFoundation"),
                .linkedFramework("SystemConfiguration"),
                .linkedFramework("AudioToolbox"),
                .linkedFramework("CoreAudio"),
                .linkedFramework("CoreMedia"),
                .linkedFramework("CoreVideo"),
                .linkedFramework("VideoToolbox"),
                .linkedFramework("IOKit"),
                .linkedFramework("IOSurface"),
                .linkedFramework("QuartzCore"),
                .linkedFramework("Metal"),
            ]
        ),
    ]
)
