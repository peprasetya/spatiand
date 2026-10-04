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
}
