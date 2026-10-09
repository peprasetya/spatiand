//  RoomTap.swift — this Mac's mouse, trackpad and keyboard, taken for the room.
//
//  While the wearer is in the room the Mac's own screen is out of sight, so its pointer and keys
//  are the room's: one event tap takes every mouse, wheel and key event before any application
//  sees it and hands it to the compositor (`sp_pointer`, `sp_key`), which decides what it was
//  for -- a menu, a window's title bar, a host's application, or a window of this Mac's own, in
//  which case it comes back and `RoomWindows` posts it at that window.
//
//  One chord always gets through: Control-Option-G switches between the two worlds, so the mouse
//  can be given back to the Mac's own screen and taken again without reaching for a menu that
//  cannot be seen. Needs the Accessibility permission, which posting to a window needs anyway.

import AppKit
import CSpatiand

/// What marks an event as posted by Spatiand itself: the tap lets these through.
let spatiandEventMark: Int64 = 0x5350_4154

final class RoomTap {
    static let shared = RoomTap()

    private var tap: CFMachPort?
    private var source: CFRunLoopSource?
    /// Whether the room has the pointer and keys now.
    private(set) var holding = false
    private var buttons: Int32 = 0
    /// The modifier keys that are down, as the last real event said.
    private(set) var flags: CGEventFlags = []
    /// Said, on the main thread, when the room takes or gives back the pointer.
    var onChange: (() -> Void)?
    /// A key that is being held down and repeating, for a window of this Mac's that has the keys.
    var onRepeat: ((UInt16) -> Void)?

    var isInstalled: Bool { tap != nil }

    /// Whether events may be tapped and posted, asking if they may not.
    static func allowed(ask: Bool) -> Bool {
        let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue(): ask] as CFDictionary
        return AXIsProcessTrustedWithOptions(options)
    }

    /// Install the tap. It passes everything through until `hold(true)`.
    @discardableResult
    func install() -> Bool {
        if tap != nil { return true }
        var mask: CGEventMask = 0
        let types: [CGEventType] = [.mouseMoved, .leftMouseDown, .leftMouseUp, .leftMouseDragged, .rightMouseDown, .rightMouseUp,
                                    .rightMouseDragged, .otherMouseDown, .otherMouseUp, .otherMouseDragged, .scrollWheel,
                                    .keyDown, .keyUp, .flagsChanged]
        for type in types { mask |= 1 << CGEventMask(type.rawValue) }
        // Trackpad gestures: not in the public list, and what a pinch arrives as.
        mask |= 1 << 29
        let me = Unmanaged.passUnretained(self).toOpaque()
        guard let made = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap, options: .defaultTap,
                                           eventsOfInterest: mask, callback: { _, type, event, user in
            guard let user else { return Unmanaged.passUnretained(event) }
            return Unmanaged<RoomTap>.fromOpaque(user).takeUnretainedValue().handle(type, event)
        }, userInfo: me) else {
            print("input: no event tap (Accessibility has not been allowed)")
            return false
        }
        tap = made
        source = CFMachPortCreateRunLoopSource(nil, made, 0)
        CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)
        CGEvent.tapEnable(tap: made, enable: true)
        return true
    }

    func remove() {
        hold(false)
        if let tap { CGEvent.tapEnable(tap: tap, enable: false) }
        if let source { CFRunLoopRemoveSource(CFRunLoopGetMain(), source, .commonModes) }
        tap = nil
        source = nil
    }

    /// Take the Mac's pointer and keys for the room, or give them back.
    func hold(_ on: Bool) {
        guard on != holding else { return }
        if on, tap == nil, !install() { return }
        holding = on
        buttons = 0
        // The Mac's own pointer stays where it is while the room has the mouse: what moves is the
        // room's. Its travel still arrives in every event.
        CGAssociateMouseAndMouseCursorPosition(on ? 0 : 1)
        if on {
            // And is not hidden by the system for being still, nor frozen for a quarter second
            // after being put somewhere, which is what posting at a window does.
            CGEventSource(stateID: .combinedSessionState)?.localEventsSuppressionInterval = 0
        }
        sp_pointer_held(on)
        sp_pointer(0, 0, 0, 0, 0)
        print("input: the mouse and keyboard are \(on ? "the room's" : "this Mac's")")
        DispatchQueue.main.async { [weak self] in self?.onChange?() }
    }

    func toggle() { hold(!holding) }

    private func chord(_ event: CGEvent) -> Bool {
        let code = event.getIntegerValueField(.keyboardEventKeycode)
        let f = event.flags
        let control = f.contains(.maskControl), option = f.contains(.maskAlternate)
        let plain = !f.contains(.maskCommand) && !f.contains(.maskShift)
        // Control-Option-G: between the room and the Mac's own screen, either way.
        if control, option, plain, code == 0x05 { toggle(); return true }
        guard holding else { return false }
        // Control-Option-R: where the wearer is looking becomes the middle.
        if control, option, plain, code == 0x0F { sp_recentre(); return true }
        if control, !option, plain {
            // Control-Space: the launcher. Control-Tab: the settings.
            if code == 0x31 { press(1); return true }
            if code == 0x30 { press(0); return true }
        }
        return false
    }

    private func press(_ control: Int32) {
        sp_control(control, true)
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { sp_control(control, false) }
    }

    private func handle(_ type: CGEventType, _ event: CGEvent) -> Unmanaged<CGEvent>? {
        let pass = Unmanaged.passUnretained(event)
        // macOS turns a tap off when it thinks it slow, or the user is typing a password.
        if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
            if let tap { CGEvent.tapEnable(tap: tap, enable: true) }
            return pass
        }
        // What Spatiand posted itself is on its way to a window: not the room's.
        if event.getIntegerValueField(.eventSourceUserData) == spatiandEventMark { return pass }
        if type == .keyDown, event.getIntegerValueField(.keyboardEventAutorepeat) == 0, chord(event) { return nil }
        guard holding else { return pass }

        switch type {
        case .mouseMoved, .leftMouseDragged, .rightMouseDragged, .otherMouseDragged:
            sp_pointer(Float(event.getDoubleValueField(.mouseEventDeltaX)), Float(event.getDoubleValueField(.mouseEventDeltaY)), buttons, 0, 0)
        case .leftMouseDown: buttons |= 1; sp_pointer(0, 0, buttons, 0, 0)
        case .leftMouseUp: buttons &= ~1; sp_pointer(0, 0, buttons, 0, 0)
        case .rightMouseDown: buttons |= 2; sp_pointer(0, 0, buttons, 0, 0)
        case .rightMouseUp: buttons &= ~2; sp_pointer(0, 0, buttons, 0, 0)
        case .otherMouseDown: buttons |= 4; sp_pointer(0, 0, buttons, 0, 0)
        case .otherMouseUp: buttons &= ~4; sp_pointer(0, 0, buttons, 0, 0)
        case .scrollWheel:
            // In pixels where the device says them (a trackpad, a Magic Mouse), else in lines. A
            // notch of a wheel is fifteen of the compositor's units, and so about fifteen pixels.
            let continuous = event.getIntegerValueField(.scrollWheelEventIsContinuous) != 0
            let dy = continuous ? event.getDoubleValueField(.scrollWheelEventPointDeltaAxis1) / 15 : event.getDoubleValueField(.scrollWheelEventFixedPtDeltaAxis1)
            let dx = continuous ? event.getDoubleValueField(.scrollWheelEventPointDeltaAxis2) / 15 : event.getDoubleValueField(.scrollWheelEventFixedPtDeltaAxis2)
            sp_pointer(0, 0, buttons, Float(-dx), Float(dy))
        case .keyDown:
            flags = event.flags
            let code = UInt16(event.getIntegerValueField(.keyboardEventKeycode))
            if event.getIntegerValueField(.keyboardEventAutorepeat) != 0 { onRepeat?(code) } else { sp_key(code, true) }
        case .keyUp:
            flags = event.flags
            sp_key(UInt16(event.getIntegerValueField(.keyboardEventKeycode)), false)
        case .flagsChanged:
            flags = event.flags
            let code = UInt16(event.getIntegerValueField(.keyboardEventKeycode))
            if let bit = Self.modifierBit(code) { sp_key(code, event.flags.rawValue & bit != 0) }
        default:
            // A gesture: two fingers spreading or closing is all that is taken from them here.
            if type.rawValue == 29, let gesture = NSEvent(cgEvent: event), gesture.type == .magnify {
                sp_pinch(Float(1 + gesture.magnification))
            }
        }
        return nil
    }

    /// The device-dependent bit of the flags that says one particular modifier key is down.
    private static func modifierBit(_ code: UInt16) -> UInt64? {
        switch code {
        case 0x38: return 0x02       // left shift
        case 0x3C: return 0x04       // right shift
        case 0x3B: return 0x01       // left control
        case 0x3E: return 0x2000     // right control
        case 0x3A: return 0x20       // left option
        case 0x3D: return 0x40       // right option
        case 0x37: return 0x08       // left command
        case 0x36: return 0x10       // right command
        case 0x39: return 0x10000    // caps lock
        default: return nil
        }
    }
}
