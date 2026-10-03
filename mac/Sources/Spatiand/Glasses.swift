//  Glasses.swift — is a pair of XREAL glasses plugged in?
//
//  Spatiand on the Mac does not need glasses. With none, everything it shows is an ordinary
//  window on this Mac; with a pair, the same things go into the 3D world. This is the one place
//  that knows which it is, and it only says *whether*: how the glasses are driven is HoloFrame's
//  business today (HeadTracker, the virtual display) and will be this app's later.
//
//  Detected the way HoloFrame does it: the glasses' own USB HID interface, vendor 0x3318.

import Foundation
import IOKit.hid

final class GlassesWatcher {
    private let manager = IOHIDManagerCreate(kCFAllocatorDefault, IOOptionBits(kIOHIDOptionsTypeNone))
    private(set) var count = 0
    /// Called on the main thread whenever the number of connected pairs changes.
    var onChange: ((Bool) -> Void)?

    var isPluggedIn: Bool { count > 0 }

    func start() {
        let match: [String: Any] = [kIOHIDVendorIDKey: 0x3318]
        IOHIDManagerSetDeviceMatching(manager, match as CFDictionary)
        let this = Unmanaged.passUnretained(self).toOpaque()
        IOHIDManagerRegisterDeviceMatchingCallback(manager, { context, _, _, _ in
            guard let context else { return }
            Unmanaged<GlassesWatcher>.fromOpaque(context).takeUnretainedValue().changed(+1)
        }, this)
        IOHIDManagerRegisterDeviceRemovalCallback(manager, { context, _, _, _ in
            guard let context else { return }
            Unmanaged<GlassesWatcher>.fromOpaque(context).takeUnretainedValue().changed(-1)
        }, this)
        IOHIDManagerScheduleWithRunLoop(manager, CFRunLoopGetMain(), CFRunLoopMode.defaultMode.rawValue)
        // Not opened: matching and removal callbacks need no access to the device, and opening a
        // HID device would ask for Input Monitoring for nothing.
    }

    private func changed(_ delta: Int) {
        let before = isPluggedIn
        count = max(0, count + delta)
        if isPluggedIn != before { onChange?(isPluggedIn) }
    }
}
