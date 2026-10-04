//  Settings.swift — what the owner has chosen, kept between runs.

import Foundation

/// Where Spatiand's windows go.
enum Presentation: String, CaseIterable {
    /// In the glasses when they are plugged in, as windows on this Mac when they are not.
    case automatic
    /// Always as windows on this Mac, glasses or no glasses.
    case onThisMac

    var title: String {
        switch self {
        case .automatic: return "In the glasses when plugged in"
        case .onThisMac: return "Always on this Mac"
        }
    }
}

/// One place for the app's saved choices, under its own name rather than the executable's, so the
/// copy run from the build folder and the one in Spatiand.app see the same ones.
enum Defaults {
    static let store = UserDefaults(suiteName: "com.peprasetya.spatiand") ?? .standard
}

enum Settings {
    private static let defaults = Defaults.store

    static var presentation: Presentation {
        get { Presentation(rawValue: defaults.string(forKey: "presentation") ?? "") ?? .automatic }
        set { defaults.set(newValue.rawValue, forKey: "presentation") }
    }

    /// Whether Command stands in for Control in a remote window: a Mac user's copy and paste are
    /// Command, and on the other end they are Control.
    static var commandIsControl: Bool {
        defaults.object(forKey: "commandIsControl") as? Bool ?? true
    }

    /// The most video the host may send, in kbit/s: what this Mac's link to it can carry. A host
    /// that sends more than the link takes makes the picture stutter and drops the sound.
    static var maxKbit: Int {
        get { defaults.object(forKey: "maxKbit") as? Int ?? 10_000 }
        set { defaults.set(newValue, forKey: "maxKbit") }
    }

    /// Whether the glasses are put into their 3D mode, each eye with a half of the picture.
    static var glassesStereo: Bool {
        get { defaults.object(forKey: "glassesStereo") as? Bool ?? true }
        set { defaults.set(newValue, forKey: "glassesStereo") }
    }

    /// Whether this Mac's mouse, trackpad and keyboard steer the room while the glasses are on.
    static var captureInput: Bool {
        get { defaults.object(forKey: "captureInput") as? Bool ?? true }
        set { defaults.set(newValue, forKey: "captureInput") }
    }

    /// Where a window pinned to the glass sits (0 bottom right, then round) and how big.
    static var pinnedCorner: Int {
        get { defaults.integer(forKey: "pinnedCorner") }
        set { defaults.set(newValue, forKey: "pinnedCorner") }
    }
    static var pinnedLarge: Bool {
        get { defaults.bool(forKey: "pinnedLarge") }
        set { defaults.set(newValue, forKey: "pinnedLarge") }
    }

    /// Whether other devices may use this Mac's windows: it listens on a port, so it is off until asked for.
    static var hostEnabled: Bool {
        get { defaults.bool(forKey: "hostEnabled") }
        set { defaults.set(newValue, forKey: "hostEnabled") }
    }

    static var hotkeys: Bool {
        defaults.object(forKey: "hotkeys") as? Bool ?? true
    }

    /// The UID of the device Spatiand's sound plays into; `nil` follows the Mac's own output.
    static var audioOutputUID: String? {
        get { defaults.string(forKey: "audioOutputUID") }
        set { defaults.set(newValue, forKey: "audioOutputUID") }
    }
}
