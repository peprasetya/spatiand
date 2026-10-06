//  KeyTap.swift — the menu's keys, whichever application has the keyboard.
//
//  A menu in the room is worked with the arrow keys and Return. They reach Spatiand when it is the active
//  application, and the system decides whether it is: after a window of this Mac has had the keyboard it may
//  take a moment to give it back, or decline. A menu that cannot hear its keys is worse than one that is
//  late, so while one is open the keys are taken as they pass, before the application that has them sees
//  them. Needs the Accessibility permission, which the app already asks for to click and type in windows;
//  without it there is no tap, and the keys go where the system sends them.

import AppKit
import ApplicationServices

final class KeyTap {
    /// Whether a key down is the menu's. Called on the main thread.
    var handler: ((NSEvent) -> Bool)?
    private var tap: CFMachPort?
    private var source: CFRunLoopSource?
    /// Keys whose press was taken: their release is taken too, or the application would see a key come up
    /// that never went down.
    private var taken = Set<Int64>()

    private static var asked = false
    var running: Bool { tap != nil }

    @discardableResult
    func start() -> Bool {
        if tap != nil { return true }
        guard AXIsProcessTrusted() else { return false }
        // Watching the keyboard is the Input Monitoring permission, asked for once; until it is given the menu
        // depends on Spatiand being the active application.
        guard CGPreflightListenEventAccess() else {
            if !Self.asked { Self.asked = true; CGRequestListenEventAccess() }
            return false
        }
        let mask = CGEventMask(1 << CGEventType.keyDown.rawValue) | CGEventMask(1 << CGEventType.keyUp.rawValue)
        let me = Unmanaged.passUnretained(self).toOpaque()
        guard let port = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap, options: .defaultTap,
                                           eventsOfInterest: mask, callback: { _, type, event, user in
            let pass = Unmanaged.passUnretained(event)
            guard let user else { return pass }
            let keys = Unmanaged<KeyTap>.fromOpaque(user).takeUnretainedValue()
            switch type {
            case .tapDisabledByTimeout, .tapDisabledByUserInput:
                if let port = keys.tap { CGEvent.tapEnable(tap: port, enable: true) }
                return pass
            case .keyDown:
                // A chord with Control, Option or Command is not the menu's: Ctrl-Space closes it.
                let flags = event.flags
                if flags.contains(.maskControl) || flags.contains(.maskAlternate) || flags.contains(.maskCommand) { return pass }
                guard let ns = NSEvent(cgEvent: event), keys.handler?(ns) == true else { return pass }
                keys.taken.insert(event.getIntegerValueField(.keyboardEventKeycode))
                return nil
            case .keyUp:
                return keys.taken.remove(event.getIntegerValueField(.keyboardEventKeycode)) != nil ? nil : pass
            default:
                return pass
            }
        }, userInfo: me) else { return false }
        tap = port
        source = CFMachPortCreateRunLoopSource(nil, port, 0)
        CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)
        CGEvent.tapEnable(tap: port, enable: true)
        return true
    }

    func stop() {
        if let source { CFRunLoopRemoveSource(CFRunLoopGetMain(), source, .commonModes) }
        if let tap { CGEvent.tapEnable(tap: tap, enable: false) }
        source = nil
        tap = nil
        taken = []
    }
}
