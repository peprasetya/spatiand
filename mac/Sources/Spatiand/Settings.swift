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

enum Settings {
    private static let defaults = UserDefaults.standard

    static var presentation: Presentation {
        get { Presentation(rawValue: defaults.string(forKey: "presentation") ?? "") ?? .automatic }
        set { defaults.set(newValue.rawValue, forKey: "presentation") }
    }

    /// Whether Command stands in for Control in a remote window: a Mac user's copy and paste are
    /// Command, and on the other end they are Control.
    static var commandIsControl: Bool {
        defaults.object(forKey: "commandIsControl") as? Bool ?? true
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
