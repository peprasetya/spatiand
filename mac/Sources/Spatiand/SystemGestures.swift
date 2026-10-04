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
}
