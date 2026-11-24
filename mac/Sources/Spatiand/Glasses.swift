//  Glasses.swift — is a pair of glasses the device table knows plugged in?
//
//  Spatiand on the Mac does not need glasses. With none, everything it shows is an ordinary
//  window on this Mac; with a pair, the same things go into the 3D world. This is the one place
//  that knows which it is, and it only says *whether*: how the glasses are driven is HoloFrame's
//  business today (HeadTracker, the virtual display) and will be this app's later.
//
//  Detected the way HoloFrame does it: the glasses' own USB HID interface, by the vendor and
//  product the device table has for each headset it knows (Headsets.swift).

import Foundation
import IOKit.hid

final class GlassesWatcher {
    private let manager = IOHIDManagerCreate(kCFAllocatorDefault, IOOptionBits(kIOHIDOptionsTypeNone))
    private(set) var count = 0
    /// Called on the main thread whenever the number of connected pairs changes.
    var onChange: ((Bool) -> Void)?

    var isPluggedIn: Bool { count > 0 }

    func start() {
        // Every headset the device table knows (Headsets.swift, generated from devices.toml).
        let match: [[String: Any]] = Headsets.usb.map { [kIOHIDVendorIDKey: $0.vendor, kIOHIDProductIDKey: $0.product] }
        IOHIDManagerSetDeviceMatchingMultiple(manager, match as CFArray)
        let this = Unmanaged.passUnretained(self).toOpaque()
        IOHIDManagerRegisterDeviceMatchingCallback(manager, { context, _, _, _ in
            guard let context else { return }
            Unmanaged<GlassesWatcher>.fromOpaque(context).takeUnretainedValue().changed(+1)
        }, this)
        IOHIDManagerRegisterDeviceRemovalCallback(manager, { context, _, _, _ in
            guard let context else { return }
            Unmanaged<GlassesWatcher>.fromOpaque(context).takeUnretainedValue().changed(-1)
        }, this)
        // In every mode of the run loop: a menu held open, or a window being dragged, is no
        // reason not to hear the glasses go.
        IOHIDManagerScheduleWithRunLoop(manager, CFRunLoopGetMain(), CFRunLoopMode.commonModes.rawValue)
        // Not opened: matching and removal callbacks need no access to the device, and opening a
        // HID device would ask for Input Monitoring for nothing.
    }

    /// The glasses' interfaces came or went. They are **counted anew** rather than added up: a
    /// pair shows as several interfaces, a callback missed left the sum wrong for good, and
    /// then neither their going nor their coming back was heard (2026-11-23: unplugged twice
    /// with the room up, and "the glasses have gone" was never said). Counted a moment later,
    /// when the set has settled.
    private func changed(_ delta: Int) {
        apply(max(0, count + delta))
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self] in
            guard let self else { return }
            let present = (IOHIDManagerCopyDevices(self.manager) as? Set<IOHIDDevice>)?.count ?? 0
            if present != self.count { print("glasses: \(present) of their USB interfaces present, \(self.count) counted; put right") }
            self.apply(present)
        }
    }

    private func apply(_ now: Int) {
        let before = isPluggedIn
        count = now
        if isPluggedIn != before { onChange?(isPluggedIn) }
    }
}
