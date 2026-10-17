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

    // MARK: the real pointer, for a Mac that is being used from somewhere else

    /// What is done to a window from another device is done in order, off the thread it arrives on:
    /// an application is waited for, briefly, when it is brought forward.
    static let real = DispatchQueue(label: "spatiand.macinput.real", qos: .userInteractive)

    /// The window on top at a point of the Mac's screen, and whose it is: among ordinary windows
    /// and whatever `pid` has up. (The system's own see-through sheets are over everything.)
    static func top(at p: CGPoint, for pid: pid_t) -> (pid: pid_t, id: CGWindowID)? {
        let mine = ProcessInfo.processInfo.processIdentifier
        for window in (CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]]) ?? [] {
            guard let owner = window[kCGWindowOwnerPID as String] as? pid_t, owner != mine,
                  let number = window[kCGWindowNumber as String] as? CGWindowID,
                  owner == pid || (window[kCGWindowLayer as String] as? Int ?? 0) == 0,
                  (window[kCGWindowAlpha as String] as? Double ?? 1) > 0.01,
                  let bounds = window[kCGWindowBounds as String] as? NSDictionary, let frame = CGRect(dictionaryRepresentation: bounds),
                  frame.contains(p) else { continue }
            return (owner, number)
        }
        return nil
    }

    /// Have the window be what is under `p` on the Mac's screen, and its application the active
    /// one -- unless what is there already is something the application has put up itself, a
    /// list or a bubble, which bringing the window forward would close. `others` are the
    /// application's windows in their own right.
    static func bringForward(_ info: MacWindowInfo, at p: CGPoint, others: Set<CGWindowID>) {
        let app = NSRunningApplication(processIdentifier: info.pid)
        if let top = top(at: p, for: info.pid), top.pid == info.pid, top.id == info.windowID || !others.contains(top.id) {
            // Nothing to bring.
        } else if let ax = MacWindows.axWindow(info) {
            AXUIElementPerformAction(ax, kAXRaiseAction as CFString)
            AXUIElementSetAttributeValue(ax, kAXMainAttribute as CFString, kCFBooleanTrue)
        }
        if app?.isActive != true {
            app?.activate(options: [])
            let deadline = Date().addingTimeInterval(0.25)
            while app?.isActive != true, Date() < deadline { usleep(5_000) }
        }
    }

    /// **A press as the Mac's own mouse makes it**: the real pointer put there, and the event
    /// given to the system and not to the application. Addressed to an application in the
    /// background, a press is taken by some and ignored by others -- anything drawn by a
    /// browser's engine among them, which is a great many applications.
    static func realMouse(_ type: CGEventType, button: CGMouseButton, at point: CGPoint, clicks: Int) {
        CGWarpMouseCursorPosition(point)
        guard let event = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point, mouseButton: button) else { return }
        event.setIntegerValueField(.mouseEventClickState, value: Int64(max(1, clicks)))
        event.setIntegerValueField(.eventSourceUserData, value: spatiandEventMark)
        event.post(tap: .cghidEventTap)
    }

    /// The wheel, where the real pointer is put: which is where the system scrolls.
    static func realScroll(dx: Int32, dy: Int32, at point: CGPoint) {
        CGWarpMouseCursorPosition(point)
        guard let event = CGEvent(scrollWheelEvent2Source: source, units: .pixel, wheelCount: 2, wheel1: dy, wheel2: dx, wheel3: 0) else { return }
        event.location = point
        event.setIntegerValueField(.eventSourceUserData, value: spatiandEventMark)
        event.post(tap: .cghidEventTap)
    }

    /// A pad's scroll as a trackpad's, where the real pointer is: continuous, exact to the point,
    /// in phases (began, changed, ended) so an application scrolls it as it scrolls a trackpad
    /// rather than animating a wheel notch for each, and carried on by momentum when the
    /// finger leaves. `dx`/`dy` in points, positive moving the content up and left.
    static let pad = PadScroll()

    /// The wheel, where the real pointer is put, as one event of a gesture.
    static func realScroll(dx: Double, dy: Double, at point: CGPoint, phase: Int64, momentum: Int64) {
        CGWarpMouseCursorPosition(point)
        guard let event = CGEvent(scrollWheelEvent2Source: source, units: .pixel, wheelCount: 2, wheel1: Int32(dy.rounded()), wheel2: Int32(dx.rounded()), wheel3: 0) else { return }
        event.location = point
        event.setIntegerValueField(.scrollWheelEventIsContinuous, value: 1)
        event.setIntegerValueField(.scrollWheelEventPointDeltaAxis1, value: Int64(dy.rounded()))
        event.setIntegerValueField(.scrollWheelEventPointDeltaAxis2, value: Int64(dx.rounded()))
        event.setDoubleValueField(.scrollWheelEventFixedPtDeltaAxis1, value: dy)
        event.setDoubleValueField(.scrollWheelEventFixedPtDeltaAxis2, value: dx)
        event.setIntegerValueField(.scrollWheelEventScrollPhase, value: phase)
        event.setIntegerValueField(.scrollWheelEventMomentumPhase, value: momentum)
        event.setIntegerValueField(.eventSourceUserData, value: spatiandEventMark)
        event.post(tap: .cghidEventTap)
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

    /// To an application, or with no application named to the system, which gives it to whichever has the keys.
    static func key(code: UInt16, flags: NSEvent.ModifierFlags, down: Bool, pid: pid_t?) {
        guard let cg = CGEvent(keyboardEventSource: source, virtualKey: CGKeyCode(code), keyDown: down) else { return }
        cg.flags = CGEventFlags(rawValue: UInt64(flags.rawValue) & 0xFFFF_0000)
        if let pid { cg.postToPid(pid) } else {
            cg.setIntegerValueField(.eventSourceUserData, value: spatiandEventMark)
            cg.post(tap: .cghidEventTap)
        }
    }

    /// A modifier key going down or up on its own.
    static func modifier(_ event: NSEvent, pid: pid_t) {
        guard let cg = CGEvent(keyboardEventSource: source, virtualKey: CGKeyCode(event.keyCode), keyDown: true) else { return }
        cg.type = .flagsChanged
        cg.flags = CGEventFlags(rawValue: UInt64(event.modifierFlags.rawValue) & 0xFFFF_0000)
        cg.postToPid(pid)
    }
}


/// One pad's scroll gesture, replayed as a trackpad's (see `MacInput.pad`). Everything on
/// `MacInput.real`, the queue the host's input is posted on.
final class PadScroll {
    // CGScrollPhase and CGMomentumScrollPhase.
    private static let began: Int64 = 1, changed: Int64 = 2, ended: Int64 = 4
    private static let glideBegin: Int64 = 1, glideOn: Int64 = 2, glideEnd: Int64 = 3

    private var going = false
    private var point = CGPoint.zero
    private var last = Date.distantPast
    /// Points a second, smoothed over the last movements: what momentum starts from.
    private var velocity = (x: 0.0, y: 0.0)
    private var glide: DispatchSourceTimer?
    /// No word of the finger leaving for this long, and the gesture is over without momentum.
    private var quiet: DispatchWorkItem?

    func moved(dx: Double, dy: Double, at point: CGPoint) {
        stopGlide()
        let now = Date()
        let dt = now.timeIntervalSince(last)
        if going && dt > 0 && dt < 0.1 {
            let k = min(1, dt / 0.05)
            velocity = (velocity.x + (dx / dt - velocity.x) * k, velocity.y + (dy / dt - velocity.y) * k)
        } else {
            velocity = (0, 0)
        }
        last = now
        self.point = point
        MacInput.realScroll(dx: dx, dy: dy, at: point, phase: going ? Self.changed : Self.began, momentum: 0)
        going = true
        quiet?.cancel()
        let end = DispatchWorkItem { [weak self] in self?.end(carryOn: false) }
        quiet = end
        MacInput.real.asyncAfter(deadline: .now() + 0.25, execute: end)
    }

    /// The finger has left the pad.
    func stopped() {
        end(carryOn: true)
    }

    private func end(carryOn: Bool) {
        quiet?.cancel()
        quiet = nil
        guard going else { return }
        going = false
        MacInput.realScroll(dx: 0, dy: 0, at: point, phase: Self.ended, momentum: 0)
        // A finger that had stopped before it left leaves the page where it is.
        let fresh = Date().timeIntervalSince(last) < 0.08
        guard carryOn, fresh, hypot(velocity.x, velocity.y) > 120 else { return }
        var v = velocity
        var first = true
        let timer = DispatchSource.makeTimerSource(queue: MacInput.real)
        timer.schedule(deadline: .now(), repeating: .milliseconds(16))
        timer.setEventHandler { [weak self] in
            guard let self else { return }
            // As a trackpad's: a little slower each frame, until it is too slow to see.
            v = (v.x * 0.95, v.y * 0.95)
            if hypot(v.x, v.y) < 20 {
                MacInput.realScroll(dx: 0, dy: 0, at: self.point, phase: 0, momentum: Self.glideEnd)
                self.stopGlide()
                return
            }
            MacInput.realScroll(dx: v.x * 0.016, dy: v.y * 0.016, at: self.point, phase: 0, momentum: first ? Self.glideBegin : Self.glideOn)
            first = false
        }
        glide = timer
        timer.resume()
    }

    private func stopGlide() {
        glide?.cancel()
        glide = nil
    }
}
