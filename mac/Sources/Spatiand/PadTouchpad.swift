//  PadTouchpad.swift — a DualShock 4's touchpad (and gyro), read from the pad's own reports.
//
//  GameController gives a PlayStation pad's buttons and sticks, but on many Macs and connections it
//  says nothing about the touchpad, which on the Beam Pro is the pad's pointer. The pad sends it
//  all the same: in every input report, after the sticks, there is where up to two fingers are.
//  This reads those reports through IOHID, alongside GameController and without taking the pad
//  from it, and keeps the newest touch for `PadInput`.
//
//  Over Bluetooth a DualShock 4 starts out sending a short report with no touchpad in it; asking it
//  for its calibration (a feature report every driver reads) makes it send the long one.

import Foundation
import IOKit.hid

final class PadTouchpad {
    static let shared = PadTouchpad()

    struct Touch { var x: Float; var y: Float; var touched: Bool; var clicked: Bool; var ps = false }

    private var manager: IOHIDManager?
    private let lock = NSLock()
    private var newest = Touch(x: 0, y: 0, touched: false, clicked: false, ps: false)
    private var buffers: [UnsafeMutablePointer<UInt8>] = []
    private(set) var reports = 0
    private(set) var devices = 0

    /// What the touchpad says now, in -1..1 with +y up, or nil when no pad that has one is attached.
    func current() -> Touch? {
        lock.lock()
        defer { lock.unlock() }
        return devices > 0 ? newest : nil
    }

    func start() {
        guard manager == nil else { return }
        let m = IOHIDManagerCreate(kCFAllocatorDefault, IOOptionBits(kIOHIDOptionsTypeNone))
        let matching: [[String: Any]] = [0x05C4, 0x09CC, 0x081F].map { [kIOHIDVendorIDKey: 0x054C, kIOHIDProductIDKey: $0] }
        IOHIDManagerSetDeviceMatchingMultiple(m, matching as CFArray)
        let context = Unmanaged.passUnretained(self).toOpaque()
        IOHIDManagerRegisterDeviceMatchingCallback(m, { context, _, _, device in
            Unmanaged<PadTouchpad>.fromOpaque(context!).takeUnretainedValue().attached(device)
        }, context)
        IOHIDManagerRegisterDeviceRemovalCallback(m, { context, _, _, device in
            Unmanaged<PadTouchpad>.fromOpaque(context!).takeUnretainedValue().detached(device)
        }, context)
        IOHIDManagerScheduleWithRunLoop(m, CFRunLoopGetMain(), CFRunLoopMode.commonModes.rawValue)
        IOHIDManagerOpen(m, IOOptionBits(kIOHIDOptionsTypeNone))
        manager = m
    }

    private func attached(_ device: IOHIDDevice) {
        lock.lock(); devices += 1; lock.unlock()
        print("pad: touchpad reader attached to a PlayStation controller")
        // Over Bluetooth: reading the calibration is what switches it to the long report.
        var calibration = [UInt8](repeating: 0, count: 64)
        var length = calibration.count
        _ = IOHIDDeviceGetReport(device, kIOHIDReportTypeFeature, 0x02, &calibration, &length)
        let buffer = UnsafeMutablePointer<UInt8>.allocate(capacity: 128)
        buffers.append(buffer)
        let context = Unmanaged.passUnretained(self).toOpaque()
        IOHIDDeviceRegisterInputReportCallback(device, buffer, 128, { context, _, _, _, id, report, length in
            Unmanaged<PadTouchpad>.fromOpaque(context!).takeUnretainedValue().input(id: id, report: report, length: length)
        }, context)
    }

    private func detached(_ device: IOHIDDevice) {
        lock.lock()
        devices = max(0, devices - 1)
        newest = Touch(x: 0, y: 0, touched: false, clicked: false, ps: false)
        lock.unlock()
        print("pad: PlayStation controller detached")
    }

    /// A report. USB sends 0x01 (the sticks and buttons from byte 1) and Bluetooth, once switched,
    /// 0x11 (the same, two bytes later). Whether the report id is in the buffer or only beside it
    /// depends on the driver, so the data's start is found from the length and the id.
    private func input(id: UInt32, report: UnsafeMutablePointer<UInt8>, length: CFIndex) {
        let n = Int(length)
        let withID = n > 0 && report[0] == UInt8(truncatingIfNeeded: id)
        // The offset of "byte 1" of a USB report: where the sticks start.
        let base: Int
        switch id {
        case 0x01 where n >= 64 || (withID && n >= 64): base = withID ? 1 : 0
        case 0x11 where n >= 70: base = withID ? 3 : 2
        default: return
        }
        // Within the data: buttons at 4..5, 6 has the PS button (bit 0) and the touchpad's press (bit 1);
        // the touch packet is at 33: count, timestamp, then the first finger's id (bit 7 set = lifted) and 12-bit x and y.
        let touch = base + 34
        guard touch + 3 < n else { return }
        let click = report[base + 6] & 0x02 != 0
        let ps = report[base + 6] & 0x01 != 0
        // The finger's id, with bit 7 set when it is not touching; then its position as two 12-bit numbers.
        let down = report[touch] & 0x80 == 0
        let x = Int(report[touch + 1]) | (Int(report[touch + 2] & 0x0F) << 8)
        let y = Int(report[touch + 2] >> 4) | (Int(report[touch + 3]) << 4)
        lock.lock()
        reports += 1
        newest = Touch(x: Float(x) / 1919 * 2 - 1, y: 1 - Float(y) / 941 * 2, touched: down, clicked: click, ps: ps)
        lock.unlock()
    }
}
