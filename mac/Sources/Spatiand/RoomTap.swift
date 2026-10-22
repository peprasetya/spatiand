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

// How an application that is not in front hides the pointer: the window server lets a connection
// that has said so set the cursor from the background. Not in any header; what every tool that
// hides the pointer for a presentation or a game streamed elsewhere does.
@_silgen_name("_CGSDefaultConnection")
private func _CGSDefaultConnection() -> Int32
@_silgen_name("CGSSetConnectionProperty")
private func CGSSetConnectionProperty(_ connection: Int32, _ target: Int32, _ key: CFString, _ value: CFTypeRef) -> Int32

final class RoomTap {
    static let shared = RoomTap()
    private var cursorHidden = false
    private var lastToggle = Date.distantPast

    /// The Mac's own pointer, out of sight while the room has the mouse: it still moves under
    /// the hand, over the Mac's screen and onto the glasses' picture, pointing at nothing.
    private func hideCursor(_ hide: Bool) {
        guard hide != cursorHidden else { return }
        cursorHidden = hide
        if hide {
            _ = CGSSetConnectionProperty(_CGSDefaultConnection(), _CGSDefaultConnection(), "SetsCursorInBackground" as CFString, kCFBooleanTrue)
            CGDisplayHideCursor(CGMainDisplayID())
        } else {
            CGDisplayShowCursor(CGMainDisplayID())
        }
    }

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
    /// Whether the room's pointer is inside one of this Mac's own windows, whose real window the
    /// Mac's real pointer is then over: scrolling and pinching there are left to macOS, which
    /// delivers them to that window as it would have -- momentum, phases, smart zoom and all.
    var overMacWindow: () -> Bool = { false }

    // Three fingers and more.
    private var recognizer = GestureRecognizer()
    private var threeDown: (since: Double, x: Double, y: Double)?
    private var dragging = false
    /// Whether macOS is doing three-finger drag itself (Accessibility's setting): then it sends
    /// a held button and nothing needs doing here.
    private lazy var systemDrags = SystemGestures.enabled().contains("three-finger drag")

    // **Putting the Mac's real cursor somewhere, and the mouse's travel afterwards.** A movement
    // heard after the cursor has been put carries the distance it was put, as though the hand
    // had made it. Which movement is not certain: with the Mac busy, several puts were on their
    // way at once, and one event's time said it came before the put whose distance it carried
    // -- a thousand points taken for the hand's, the room's pointer sent across the screen, the
    // cursor put there, and back (pointer.log, 2026-10-20). So every put of the last second is
    // kept with the distance it moved the cursor, and an event whose travel is that distance
    // plus a hand's worth has it taken off, and that put forgotten.
    private let warpLock = NSLock()
    private var warps: [(by: CGVector, at: UInt64)] = []
    /// The last put, until a movement is heard from where it put the cursor.
    private var putUnseen: (to: CGPoint, at: UInt64)?
    /// The last put, wherever it was to.
    private var lastPut: (to: CGPoint, at: UInt64)?

    /// Whether a movement says where the cursor was before the last put: one already on its way
    /// when the cursor was put, which still has it where it was. Sent to that place, the pointer
    /// went back for a moment and forward again (pointer.log, 2026-10-20, after the cursor was
    /// made the pointer's place).
    func fromBeforePut(_ event: CGEvent, dx: Double, dy: Double) -> Bool {
        warpLock.lock(); defer { warpLock.unlock() }
        guard let put = lastPut, clock_gettime_nsec_np(CLOCK_UPTIME_RAW) - put.at < 150_000_000 else { return false }
        let off = hypot(event.location.x - put.to.x, event.location.y - put.to.y)
        if off <= hypot(dx, dy) * 1.5 + 6 {
            lastPut = nil // the cursor is seen where it was put: later ones are after it
            return false
        }
        return true
    }

    /// Put the real cursor at `point`. From any thread.
    func warp(to point: CGPoint) {
        warpLock.lock()
        let from = CGEvent(source: nil)?.location ?? point
        CGWarpMouseCursorPosition(point)
        let now = clock_gettime_nsec_np(CLOCK_UPTIME_RAW)
        lastPut = (point, now)
        warps.removeAll { now - $0.at > 1_000_000_000 }
        let by = CGVector(dx: point.x - from.x, dy: point.y - from.y)
        putUnseen = (point, now)
        PointerLog.shared.note(String(format: "warp %.1f %.1f from %.1f %.1f", point.x, point.y, from.x, from.y))
        // Small ones are not kept: a hand's movement is easily taken for one of them.
        if hypot(by.dx, by.dy) >= 8 { warps.append((by, now)) }
        warpLock.unlock()
    }

    private func travel(of event: CGEvent) -> (Double, Double) {
        let dx = event.getDoubleValueField(.mouseEventDeltaX), dy = event.getDoubleValueField(.mouseEventDeltaY)
        warpLock.lock()
        defer { warpLock.unlock() }
        // **The first movement heard from where the cursor was put** carries the put, however
        // far: a click in a window of this Mac's puts the cursor there, and its 1193 points
        // came back as the hand's and sent the room's pointer to the corner of the view
        // (pointer.log, 2026-10-22) -- what the put was said to be from had not matched. The
        // cursor is exactly where it was put, so the hand's own part is too small to matter:
        // that one movement counts for nothing.
        if let put = putUnseen, clock_gettime_nsec_np(CLOCK_UPTIME_RAW) - put.at < 1_000_000_000,
           hypot(event.location.x - put.to.x, event.location.y - put.to.y) < 2.5 {
            putUnseen = nil
            if hypot(dx, dy) > 20 {
                warps.removeAll()
                return (0, 0)
            }
        }
        // The put whose distance best accounts for this travel, if taking it off leaves less.
        var best: (index: Int, left: Double)?
        for (i, w) in warps.enumerated() {
            let left = hypot(dx - w.by.dx, dy - w.by.dy)
            if left < hypot(dx, dy), left < 60, left < (best?.left ?? .infinity) { best = (i, left) }
        }
        guard let best else { return (dx, dy) }
        let w = warps.remove(at: best.index)
        return (dx - w.by.dx, dy - w.by.dy)
    }

    private var lastFollow = Date.distantPast
    static func displayBounds(at point: CGPoint) -> CGRect? {
        var display: CGDirectDisplayID = 0
        var count: UInt32 = 0
        guard CGGetDisplaysWithPoint(point, 1, &display, &count) == .success, count > 0 else { return nil }
        return CGDisplayBounds(display)
    }
    /// When the hand last moved during a drag in a window of this Mac's (uptime, ns).
    private var handDragged: UInt64 = 0
    /// When the hand last moved at all (uptime, ns).
    private var handMoved: UInt64 = 0
    /// Whether the hand has moved just now: a pointer moving otherwise moved with the head.
    var handIsMoving: Bool {
        warpLock.lock(); defer { warpLock.unlock() }
        return clock_gettime_nsec_np(CLOCK_UPTIME_RAW) - handMoved < 80_000_000
    }
    /// Whether the hand has moved in the drag just now: a pointer moving otherwise moved with the head.
    var handIsDragging: Bool {
        warpLock.lock(); defer { warpLock.unlock() }
        return clock_gettime_nsec_np(CLOCK_UPTIME_RAW) - handDragged < 80_000_000
    }

    /// While set, told of every movement of the mouse the room takes: for tracing.
    var trace: ((String) -> Void)?

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
        hideCursor(on)
        sp_pointer_held(on)
        sp_pointer(0, 0, 0, 0, 0)
        print("input: the mouse and keyboard are \(on ? "the room's" : "this Mac's")")
        DispatchQueue.main.async { [weak self] in self?.onChange?() }
    }

    /// Heard twice for one press where both the tap and the system's hot key hear it: once is meant.
    func toggle() {
        guard Date().timeIntervalSince(lastToggle) > 0.3 else { return }
        lastToggle = Date()
        hold(!holding)
    }

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
        // Control-Option-W: close the window that has the keyboard, as the cross on its bar does.
        if control, option, plain, code == 0x0D { sp_close_window(); return true }
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

        // **A drag in a window of this Mac's is not taken.** The application gets the hand's own
        // events -- the only ones that turn a 3D object, select text or pull a slider as it
        // expects -- and the room's pointer is steered after the real cursor instead: a little
        // of the way each time, by how far apart they are in the window.
        // (Only with the button really down: the room hears of its coming up a frame late.)
        if buttons & 1 != 0, type == .mouseMoved || type == .leftMouseDragged, let lag = RoomWindows.shared.dragLag(cursor: event.location) {
            warpLock.lock(); handDragged = clock_gettime_nsec_np(CLOCK_UPTIME_RAW); warpLock.unlock()
            let now = Date()
            if now.timeIntervalSince(lastFollow) > 0.03 {
                lastFollow = now
                func step(_ px: CGFloat) -> Float { Float(max(-30, min(30, px * 0.25))) }
                if abs(lag.x) > 1.5 || abs(lag.y) > 1.5 { sp_pointer(step(lag.x), step(lag.y), buttons, 0, 0) }
            }
            return pass
        }

        switch type {
        case .mouseMoved, .leftMouseDragged, .rightMouseDragged, .otherMouseDragged:
            let dx = event.getDoubleValueField(.mouseEventDeltaX), dy = event.getDoubleValueField(.mouseEventDeltaY)
            warpLock.lock(); handMoved = clock_gettime_nsec_np(CLOCK_UPTIME_RAW); warpLock.unlock()
            trace?("\(type.rawValue),\(dx),\(dy),\(event.location.x),\(event.location.y)")
            var (tx, ty) = travel(of: event)
            // **In a screen of this Mac's the cursor is where the pointer is**, and the hand
            // moves it there itself: so the room's pointer is sent to where the cursor now is,
            // exactly, and not by a travel that the cursor being put by the head's movements
            // keeps spoiling. (Keeping the hand from the cursor does not last there: macOS
            // gives it back whenever another application is in front, which in a screen is
            // always.) At an edge of the display the cursor stops, and the hand's own travel
            // beyond it is added, so the pointer can still leave the screen.
            if let at = RoomWindows.shared.screenPointer(), fromBeforePut(event, dx: dx, dy: dy) {
                // Its own travel, then: it was made before the put, so the put is not in it.
                (tx, ty) = (dx, dy)
                RoomWindows.shared.noteScreenPointer(CGPoint(x: at.x + tx, y: at.y + ty))
            } else if let at = RoomWindows.shared.screenPointer() {
                let here = event.location
                tx = here.x - at.x
                ty = here.y - at.y
                if let bounds = Self.displayBounds(at: here) {
                    if (here.x <= bounds.minX + 0.5 && dx < 0) || (here.x >= bounds.maxX - 1.5 && dx > 0) { tx += dx }
                    if (here.y <= bounds.minY + 0.5 && dy < 0) || (here.y >= bounds.maxY - 1.5 && dy > 0) { ty += dy }
                }
                RoomWindows.shared.noteScreenPointer(CGPoint(x: at.x + tx, y: at.y + ty))
            }
            PointerLog.shared.note({
                // Where the room's pointer was at the last frame, across and up from -1 to 1 of the view.
                var vx: Float = 0, vy: Float = 0
                sp_pointer_where(&vx, &vy)
                return String(format: "hand %ld %.1f %.1f at %.1f %.1f -> %.1f %.1f%@ | room view %.3f %.3f", Int(type.rawValue), dx, dy, event.location.x, event.location.y, tx, ty, (tx != dx || ty != dy) ? " (to the cursor)" : " (its own)", vx, vy)
            }())
            sp_pointer(Float(tx), Float(ty), buttons, 0, 0)
        case .leftMouseDown: buttons |= 1; sp_pointer(0, 0, buttons, 0, 0)
        case .leftMouseUp: buttons &= ~1; sp_pointer(0, 0, buttons, 0, 0)
        case .rightMouseDown: buttons |= 2; sp_pointer(0, 0, buttons, 0, 0)
        case .rightMouseUp: buttons &= ~2; sp_pointer(0, 0, buttons, 0, 0)
        case .otherMouseDown: buttons |= 4; sp_pointer(0, 0, buttons, 0, 0)
        case .otherMouseUp: buttons &= ~4; sp_pointer(0, 0, buttons, 0, 0)
        case .scrollWheel:
            // Over a window of this Mac's own, macOS scrolls it: the real pointer is over it.
            if overMacWindow() { RoomWindows.shared.readyToScroll(); return pass }
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
            guard type.rawValue == 29, let gesture = NSEvent(cgEvent: event) else { break }
            // **The Touch Bar's touches come the same way as the trackpad's**, and are not the
            // room's: taken with the rest, the bar went dead for as long as the room had the
            // mouse. A finger on a screen is a direct touch; one on a trackpad is not.
            if gesture.type == .directTouch { return pass }
            if gesture.type == .gesture || gesture.type == .beginGesture || gesture.type == .endGesture {
                let touches = gesture.allTouches()
                if !touches.isEmpty, touches.allSatisfy({ $0.type == .direct }) { return pass }
            }
            if gesture.type == .magnify {
                // Over a window of this Mac's own, the pinch is that application's.
                if overMacWindow() { return pass }
                sp_pinch(Float(1 + gesture.magnification))
            } else if gesture.type == .gesture {
                fingers(gesture)
            }
        }
        return nil
    }

    // MARK: three, four and five fingers

    /// Where the fingers are now.
    ///
    /// **Three fingers drag**, as macOS's own three-finger drag does: they are the button held
    /// down, and moving them moves the pointer -- so a window is carried by its title bar, an
    /// edge is pulled, text is selected, with nothing pressed. **Four fingers** are the room's:
    /// up for the launcher, down to back out, a tap to recentre. Five are not used yet.
    private func fingers(_ event: NSEvent) {
        let all = event.allTouches().filter { $0.phase != .ended && $0.phase != .cancelled }
        let touches = all.map { GestureRecognizer.Touch(id: $0.identity.hash, x: Double($0.normalizedPosition.x), y: Double($0.normalizedPosition.y)) }
        let now = event.timestamp
        if touches.count == 3, !systemDrags {
            let x = touches.reduce(0) { $0 + $1.x } / 3, y = touches.reduce(0) { $0 + $1.y } / 3
            let size = all.first?.deviceSize ?? NSSize(width: 440, height: 300)
            if let held = threeDown {
                if !dragging, now - held.since >= 0.06 {
                    dragging = true
                    buttons |= 1
                    sp_pointer(0, 0, buttons, 0, 0)
                } else if dragging {
                    // The pad's own size in points, so three fingers carry as far as one does.
                    sp_pointer(Float((x - held.x) * size.width * 1.6), Float(-(y - held.y) * size.height * 1.6), buttons, 0, 0)
                }
                threeDown = (held.since, x, y)
            } else {
                threeDown = (now, x, y)
            }
        } else if threeDown != nil {
            threeDown = nil
            if dragging {
                dragging = false
                buttons &= ~1
                sp_pointer(0, 0, buttons, 0, 0)
            }
        }
        guard let done = recognizer.update(touches, at: now) else { return }
        switch done {
        case .swipe(.up, fingers: 4): press(1)
        case .swipe(.down, fingers: 4): press(3)
        case .tap(fingers: 4): sp_recentre()
        default: break
        }
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
