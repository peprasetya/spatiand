//  MacInput.swift — clicks, wheel and keys, posted at a Mac window that is in the room.
//
//  The wearer's pointer is virtual, so a click on a Mac window has to be made for it: a mouse event
//  at the right place in the window, posted at the application that owns it. Posting to the
//  application's process, and naming the window the event is for, means it needs neither the
//  window to be in front nor the real pointer to be anywhere near -- which matters, because the
//  real pointer is frozen under Spatiand's own input window.
//
//  Posting events needs the Accessibility permission, which macOS asks for the first time.

import ApplicationServices
import AppKit

enum MacInput {
    /// Whether this app may post events, asking if it may not.
    static func allowed(ask: Bool) -> Bool {
        let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue(): ask] as CFDictionary
        return AXIsProcessTrustedWithOptions(options)
    }

    private static let source = CGEventSource(stateID: .privateState)
    // The fields that say which window a mouse event is for. Not in the public header; the same
    // numbers every tool that clicks in a background window uses.
    private static let windowUnderPointer = CGEventField(rawValue: 91)!
    private static let windowThatCanHandle = CGEventField(rawValue: 92)!

    private static func address(_ event: CGEvent, window: CGWindowID, pid: pid_t) {
        event.setIntegerValueField(windowUnderPointer, value: Int64(window))
        event.setIntegerValueField(windowThatCanHandle, value: Int64(window))
        event.postToPid(pid)
    }

    /// A mouse event at `point` (the global space, points) for a window.
    static func mouse(_ type: CGEventType, button: CGMouseButton, at point: CGPoint, clicks: Int, window: CGWindowID, pid: pid_t) {
        guard let event = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point, mouseButton: button) else { return }
        event.setIntegerValueField(.mouseEventClickState, value: Int64(max(1, clicks)))
        address(event, window: window, pid: pid)
    }

    private static let hid = CGEventSource(stateID: .hidSystemState)

    /// A mouse event through the system's own stream, at a place on the screen; see `RoomInput.sendThrough`.
    static func mouseThrough(_ type: CGEventType, button: CGMouseButton, at point: CGPoint, clicks: Int) {
        guard let event = CGEvent(mouseEventSource: hid, mouseType: type, mouseCursorPosition: point, mouseButton: button) else { return }
        event.setIntegerValueField(.mouseEventClickState, value: Int64(max(1, clicks)))
        if type == .leftMouseDown || type == .rightMouseDown { event.setDoubleValueField(.mouseEventPressure, value: 1) }
        event.post(tap: .cghidEventTap)
    }

    /// The wheel through the system's own stream, at a place on the screen.
    static func scrollThrough(dx: Int32, dy: Int32, at point: CGPoint) {
        guard let event = CGEvent(scrollWheelEvent2Source: hid, units: .pixel, wheelCount: 2, wheel1: dy, wheel2: dx, wheel3: 0) else { return }
        event.location = point
        event.post(tap: .cghidEventTap)
    }

    /// The wheel, in pixels: vertical then horizontal.
    static func scroll(dx: Int32, dy: Int32, at point: CGPoint, window: CGWindowID, pid: pid_t) {
        guard let event = CGEvent(scrollWheelEvent2Source: source, units: .pixel, wheelCount: 2, wheel1: dy, wheel2: dx, wheel3: 0) else { return }
        event.location = point
        address(event, window: window, pid: pid)
    }

    /// A key, as the Mac itself reported it: its code and the modifiers held.
    static func key(_ event: NSEvent, down: Bool, pid: pid_t) {
        key(code: event.keyCode, flags: event.modifierFlags, down: down, pid: pid)
    }

    static func key(code: UInt16, flags: NSEvent.ModifierFlags, down: Bool, pid: pid_t) {
        guard let cg = CGEvent(keyboardEventSource: source, virtualKey: CGKeyCode(code), keyDown: down) else { return }
        cg.flags = CGEventFlags(rawValue: UInt64(flags.rawValue) & 0xFFFF_0000)
        cg.postToPid(pid)
    }

    /// A modifier key going down or up on its own.
    static func modifier(_ event: NSEvent, pid: pid_t) {
        guard let cg = CGEvent(keyboardEventSource: source, virtualKey: CGKeyCode(event.keyCode), keyDown: true) else { return }
        cg.type = .flagsChanged
        cg.flags = CGEventFlags(rawValue: UInt64(event.modifierFlags.rawValue) & 0xFFFF_0000)
        cg.postToPid(pid)
    }

    // MARK: making a window the key one

    private typealias PostRecord = @convention(c) (UnsafeRawPointer, UnsafeMutablePointer<UInt8>) -> Int32
    private typealias ProcessForPID = @convention(c) (pid_t, UnsafeMutableRawPointer) -> Int32
    private static let skyLight = dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight", RTLD_LAZY)
    private static let postRecord: PostRecord? = skyLight.flatMap { dlsym($0, "SLPSPostEventRecordTo") }.map { unsafeBitCast($0, to: PostRecord.self) }
    private static let processForPID: ProcessForPID? = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "GetProcessForPID").map { unsafeBitCast($0, to: ProcessForPID.self) }

    /// Tell an application its window is the key one, without bringing it to the front: two event records, as
    /// the window servers sends when a window is clicked. Without them a window that has been posted clicks
    /// still looks inactive -- no caret, menus that close at once -- because as far as it knows, it is.
    static func makeKey(window: CGWindowID, pid: pid_t) {
        guard let postRecord, let processForPID else { return }
        var psn = [UInt32](repeating: 0, count: 2)
        guard processForPID(pid, &psn) == 0 else { return }
        for kind: UInt8 in [0x01, 0x02] {
            var bytes = [UInt8](repeating: 0, count: 0xF8)
            bytes[0x04] = 0xF8
            bytes[0x08] = kind
            bytes[0x3A] = 0x10
            var id = window
            withUnsafeBytes(of: &id) { for i in 0..<4 { bytes[0x3C + i] = $0[i] } }
            for i in 0..<16 { bytes[0x20 + i] = 0xFF }
            _ = psn.withUnsafeBytes { postRecord($0.baseAddress!, &bytes) }
        }
    }
}
