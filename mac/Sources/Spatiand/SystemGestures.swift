//  SystemGestures.swift — which of the trackpad's own multi-finger gestures macOS has turned on.
//
//  Three, four and five fingers do things in the glasses; they also do things on the Mac (Mission
//  Control, Spaces, Launchpad), and macOS does not let an app take them from it. So the settings say
//  which of them are on, for the owner to turn off if they get in the way of the glasses.

import Foundation

enum SystemGestures {
    private static let domains = ["com.apple.AppleMultitouchTrackpad", "com.apple.driver.AppleBluetoothMultitouch.trackpad"]
    private static let gestures: [(key: String, name: String)] = [
        ("TrackpadThreeFingerHorizSwipeGesture", "swipe between pages with three fingers"),
        ("TrackpadThreeFingerVertSwipeGesture", "Mission Control and App Exposé with three fingers"),
        ("TrackpadThreeFingerDrag", "three-finger drag"),
        ("TrackpadFourFingerHorizSwipeGesture", "swipe between full-screen apps with four fingers"),
        ("TrackpadFourFingerVertSwipeGesture", "Mission Control and App Exposé with four fingers"),
        ("TrackpadFourFingerPinchGesture", "Launchpad and Show Desktop"),
        ("TrackpadFiveFingerPinchGesture", "Launchpad and Show Desktop"),
    ]

    /// The gestures that are on, in words.
    static func enabled() -> [String] {
        var on: [String] = []
        for gesture in gestures {
            let isOn = domains.contains { domain in
                guard let value = CFPreferencesCopyAppValue(gesture.key as CFString, domain as CFString) else { return false }
                if let number = value as? NSNumber { return number.intValue != 0 }
                return false
            }
            if isOn, !on.contains(gesture.name) { on.append(gesture.name) }
        }
        return on
    }

    // MARK: off while the room has the trackpad

    /// **macOS's own swipes are off while the room has the trackpad**, and back as they were when
    /// it gives it back -- the glasses taken off, Spatiand quitting -- or, after a crash, when
    /// Spatiand next starts. The trackpad is the room's then (see `RoomTap`), but these few are
    /// done by macOS before any application hears of them: four fingers up for Mission Control
    /// was the room's launcher too, and both happened. What they were is kept in Spatiand's own
    /// settings until it is put back.
    private static let swipes = ["TrackpadThreeFingerHorizSwipeGesture", "TrackpadThreeFingerVertSwipeGesture",
                                 "TrackpadFourFingerHorizSwipeGesture", "TrackpadFourFingerVertSwipeGesture",
                                 "TrackpadFourFingerPinchGesture", "TrackpadFiveFingerPinchGesture",
                                 "TrackpadTwoFingerFromRightEdgeSwipeGesture"]
    private static let savedKey = "systemSwipesWhileRoomHeld"

    static func swipesOff(_ off: Bool) {
        let defaults = UserDefaults.standard
        if off {
            guard defaults.dictionary(forKey: savedKey) == nil else { return }
            var saved: [String: Int] = [:]
            for domain in domains {
                for key in swipes {
                    let value = (CFPreferencesCopyAppValue(key as CFString, domain as CFString) as? NSNumber)?.intValue
                    if let value { saved[domain + "|" + key] = value }
                    if value != 0 { CFPreferencesSetAppValue(key as CFString, 0 as CFNumber, domain as CFString) }
                }
                CFPreferencesAppSynchronize(domain as CFString)
            }
            defaults.set(saved, forKey: savedKey)
            print("input: macOS's own trackpad swipes are off while the room has the trackpad")
        } else {
            guard let saved = defaults.dictionary(forKey: savedKey) as? [String: Int] else { return }
            for domain in domains {
                for key in swipes {
                    let value = saved[domain + "|" + key].map { $0 as CFNumber }
                    CFPreferencesSetAppValue(key as CFString, value, domain as CFString)
                }
                CFPreferencesAppSynchronize(domain as CFString)
            }
            defaults.removeObject(forKey: savedKey)
            print("input: macOS's own trackpad swipes are back as they were")
        }
        apply()
    }

    /// The trackpad's driver reads its settings again (what System Settings does after a change).
    private static func apply() {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/System/Library/PrivateFrameworks/SystemAdministration.framework/Resources/activateSettings")
        process.arguments = ["-u"]
        try? process.run()
    }
}
