//  RoomInput.swift — this Mac's mouse, trackpad and keyboard, taken to steer the room.
//
//  While the glasses are on, the Mac's own screen is not what the wearer is looking at, so the
//  pointer must not wander over it. A transparent window covers the Mac's screen and becomes the
//  key window: the system pointer is frozen and hidden, and every movement the mouse makes comes
//  here as a distance instead, which moves the room's pointer. Clicks, the wheel and the keys go
//  where that pointer is and where the keyboard is.
//
//  No Accessibility permission is asked for: a window the app owns hears its own events. The cost
//  is that the Mac's own apps cannot be used while it holds the input, so letting go has to be
//  easy and certain -- Ctrl-Option-G (and the menu, and switching away from the app) hands the
//  mouse and keyboard back to the Mac.
//
//  The keys that stay Spatiand's own, even while the keyboard is the room's:
//    Ctrl-Option-G   hand the mouse and keyboard back to this Mac, or take them again
//    Ctrl-Option-R   recentre: where you are looking is the middle
//    Ctrl-Option-P   pin the window you are pointing at to the glass, or let it go
//    Ctrl-Option-B   bring the window you are pointing at to where you are looking
//    Ctrl-Option-C   which corner a pinned window sits in
//    Ctrl-Option-S   how big a pinned window is
//    Ctrl-Option-W   close the window you are pointing at
//    Ctrl-Space      the menu, in the glasses
//  and, with Option held, dragging a window moves it and the wheel (or a pinch) resizes it. On the
//  trackpad, three fingers swipe sideways to bring the next window here, up for the menu and down
//  to put it away, and tap to recentre; four fingers swipe to move the window pointed at round the
//  room, and pinch to resize it; five fingers pinch to gather every window in front of you and
//  spread to give them room.

import AppKit
import Carbon.HIToolbox

/// The window that covers the Mac's screen to take its mouse. A panel that does not activate Spatiand when it is
/// clicked, so that a window of this Mac can be the active application, with the keyboard, while the pointer is
/// still Spatiand's: `keyable` is whether Spatiand itself has the keyboard.
final class InputWindow: NSPanel {
    var keyable = true
    override var canBecomeKey: Bool { keyable }
    override var canBecomeMain: Bool { keyable }
}

final class InputView: NSView {
    weak var controller: RoomController?
    var onRelease: (() -> Void)?

    override var acceptsFirstResponder: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    // MARK: the trackpad's fingers

    private var recognizer = GestureRecognizer()

    override init(frame: NSRect) {
        super.init(frame: frame)
        // The raw touches of a trackpad, not of a screen, and not the ones merely resting on it.
        allowedTouchTypes = [.indirect]
        wantsRestingTouches = false
    }
    required init?(coder: NSCoder) { fatalError() }

    // The pointer moves are heard even when another application is the active one.
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseMoved, .activeAlways, .inVisibleRect], owner: self, userInfo: nil))
    }

    /// Whether three or more fingers are down, in which case the pointer is the gesture's and does not move.
    var gesturing: Bool { recognizer.active }

    private func touched(_ event: NSEvent) {
        let touches = event.touches(matching: .touching, in: self).map {
            GestureRecognizer.Touch(id: $0.identity.hash, x: Double($0.normalizedPosition.x), y: Double($0.normalizedPosition.y))
        }
        if let gesture = recognizer.update(touches, at: event.timestamp) { controller?.gesture(gesture) }
    }
    override func touchesBegan(with event: NSEvent) { touched(event) }
    override func touchesMoved(with event: NSEvent) { touched(event) }
    override func touchesEnded(with event: NSEvent) { touched(event) }
    override func touchesCancelled(with event: NSEvent) { touched(event) }

    // MARK: the mouse

    private static let trace = ProcessInfo.processInfo.environment["SPATIAND_DEBUG_INPUT"] != nil

    private func moved(_ event: NSEvent) {
        if gesturing { return }
        if Self.trace { print("input: move \(event.deltaX), \(event.deltaY)") }
        controller?.pointerMoved(dx: Double(event.deltaX), dy: Double(event.deltaY))
    }
    override func mouseMoved(with event: NSEvent) { moved(event) }
    override func mouseDragged(with event: NSEvent) { moved(event) }
    override func rightMouseDragged(with event: NSEvent) { moved(event) }
    override func otherMouseDragged(with event: NSEvent) { moved(event) }

    // With three fingers down the trackpad is making a gesture, and macOS's own three-finger drag turns
    // that into a mouse press as well: the press is the gesture's, and not a click in a window.
    override func mouseDown(with event: NSEvent) {
        if gesturing { return }
        if Self.trace { print("input: left down") }
        controller?.buttonDown(0x110, grab: event.modifierFlags.contains(.option))
    }
    override func mouseUp(with event: NSEvent) {
        // A release is always let through, or a button pressed just before the fingers came down would stay down.
        controller?.buttonUp(0x110)
    }
    override func rightMouseDown(with event: NSEvent) { if !gesturing { controller?.buttonDown(0x111, grab: false) } }
    override func rightMouseUp(with event: NSEvent) { controller?.buttonUp(0x111) }
    override func otherMouseDown(with event: NSEvent) { if !gesturing { controller?.buttonDown(0x112, grab: false) } }
    override func otherMouseUp(with event: NSEvent) { controller?.buttonUp(0x112) }

    override func scrollWheel(with event: NSEvent) {
        if event.modifierFlags.contains(.option) {
            if let id = controller?.core.aim().window ?? controller?.core.focused { controller?.push(id, metres: -Double(event.scrollingDeltaY) * (event.hasPreciseScrollingDeltas ? 0.01 : 0.1)) }
        } else {
            controller?.scrolled(dx: Double(event.scrollingDeltaX), dy: Double(event.scrollingDeltaY), precise: event.hasPreciseScrollingDeltas)
        }
    }

    /// A pinch on the trackpad: nearer or further on a window's frame, the application's own zoom over its contents.
    override func magnify(with event: NSEvent) { controller?.pointingAgain(force: true); controller?.pinched(by: Double(event.magnification)) }

    // MARK: the keyboard

    /// Spatiand's own chords, which are never sent on.
    private func chord(_ event: NSEvent) -> Bool {
        guard event.type == .keyDown, event.modifierFlags.intersection([.control, .option, .command, .shift]) == [.control, .option] else { return false }
        switch Int(event.keyCode) {
        case kVK_ANSI_G: onRelease?()
        case kVK_ANSI_R: controller?.recentre()
        case kVK_ANSI_P: controller?.togglePin()
        case kVK_ANSI_B: controller?.bringAimedHere()
        case kVK_ANSI_C: controller?.nextCorner()
        case kVK_ANSI_S: controller?.toggleSize()
        case kVK_ANSI_W: controller?.closeAimed()
        default: return false
        }
        return true
    }

    override func keyDown(with event: NSEvent) {
        if Self.trace { print("input: key \(event.keyCode)") }
        if chord(event) { return }
        // A menu takes the keyboard: the arrows and Return work it, as a D-pad and A do.
        if controller?.menu.key(event) == true { return }
        if controller?.macKey(event) == true { return }
        controller?.keyTarget?.keyDown(with: event)
    }
    override func keyUp(with event: NSEvent) {
        if controller?.macKey(event) == true { return }
        controller?.keyTarget?.keyUp(with: event)
    }
    override func flagsChanged(with event: NSEvent) {
        if controller?.macKey(event) == true { return }
        controller?.keyTarget?.flagsChanged(with: event)
    }
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if chord(event) { return true }
        // A Command-key shortcut reaches here first; it is the window's, not the menu's.
        if controller?.macKey(event) == true { return true }
        guard let target = controller?.keyTarget else { return false }
        return target.performKeyEquivalent(with: event)
    }
}

final class RoomInput: NSObject, NSWindowDelegate {
    private let controller: RoomController
    private var window: InputWindow?
    private(set) var capturing = false
    /// Said when the Mac has the mouse and keyboard back, however that came about.
    var onChange: (() -> Void)?

    init(controller: RoomController) { self.controller = controller }

    func start() {
        guard !capturing, let screen = NSScreen.main ?? NSScreen.screens.first else { return }
        let w = InputWindow(contentRect: screen.frame, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        w.isReleasedWhenClosed = false
        w.isFloatingPanel = true
        w.hidesOnDeactivate = false
        w.becomesKeyOnlyIfNeeded = false
        w.level = .screenSaver
        // Nearly invisible, but not clear: a window that is wholly transparent hears nothing.
        w.backgroundColor = NSColor(white: 0, alpha: 0.004)
        w.isOpaque = false
        w.hasShadow = false
        w.acceptsMouseMovedEvents = true
        w.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
        w.delegate = self
        let view = InputView(frame: NSRect(origin: .zero, size: screen.frame.size))
        view.controller = controller
        view.allowedTouchTypes = [.indirect]
        view.onRelease = { [weak self] in self?.stop() }
        // A word on the Mac's own screen, so a Mac whose pointer has stopped answering says why.
        let note = NSTextField(labelWithString: "Spatiand has this Mac\u{2019}s mouse and keyboard for the glasses.   Ctrl-Option-G gives them back.")
        note.font = .systemFont(ofSize: 13, weight: .medium)
        note.textColor = .white
        note.alignment = .center
        note.wantsLayer = true
        note.layer?.backgroundColor = NSColor(white: 0.1, alpha: 0.88).cgColor
        note.layer?.cornerRadius = 8
        note.sizeToFit()
        note.frame = NSRect(x: (screen.frame.width - note.frame.width - 24) / 2, y: 14, width: note.frame.width + 24, height: note.frame.height + 14)
        view.addSubview(note)
        w.contentView = view
        window = w
        NSApp.activate(ignoringOtherApps: true)
        w.makeKeyAndOrderFront(nil)
        w.makeFirstResponder(view)
        // The pointer is brought onto the screen this window covers first: a mouse that is held still
        // over the glasses' display would be heard by nothing at all.
        let main = CGDisplayBounds(CGMainDisplayID())
        CGWarpMouseCursorPosition(CGPoint(x: main.midX, y: main.midY))
        CGAssociateMouseAndMouseCursorPosition(0)
        CGDisplayHideCursor(CGMainDisplayID())
        capturing = true
        chords.onChord = { [weak self] key in self?.chord(key) }
        chords.enable()
        // While another application has the keyboard Spatiand hears neither its keys nor the fingers that begin a
        // gesture; the system says so to anyone who listens.
        keyMonitor = NSEvent.addGlobalMonitorForEvents(matching: [.keyDown, .flagsChanged]) { [weak self] _ in self?.controller.keyNoted() }
        gestureMonitor = NSEvent.addGlobalMonitorForEvents(matching: [.magnify, .swipe, .gesture, .beginGesture, .scrollWheel]) { [weak self] _ in self?.controller.pointingAgain(force: true) }
        watcher = NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main) { [weak self] note in
            self?.activated(note.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication)
        }
        onChange?()
    }

    private let chords = RoomChords()
    private var watcher: NSObjectProtocol?
    private var keyMonitor: Any?
    private var gestureMonitor: Any?
    /// The application of this Mac that has the keyboard, when it is not Spatiand.
    private var keyboardOwner: pid_t?

    /// A chord asked of the system, for when another application has the keyboard.
    private func chord(_ key: Int) {
        switch key {
        case Int(kVK_ANSI_G): stop()
        case Int(kVK_ANSI_R): controller.recentre()
        case Int(kVK_ANSI_P): controller.togglePin()
        case Int(kVK_ANSI_B): controller.bringAimedHere()
        case Int(kVK_ANSI_C): controller.nextCorner()
        case Int(kVK_ANSI_S): controller.toggleSize()
        case Int(kVK_ANSI_W): controller.closeAimed()
        default: break
        }
    }

    /// Another application has come to the front of its own accord -- the wearer switched with Command-Tab --
    /// and that is a way of asking for the Mac back.
    private func activated(_ app: NSRunningApplication?) {
        guard capturing, let owner = keyboardOwner, let app else { return }
        if app.processIdentifier != owner, app.processIdentifier != ProcessInfo.processInfo.processIdentifier { stop() }
    }

    /// A window of this Mac has the keyboard: its application is made the active one and the window its key
    /// window, so it behaves as a window in front does -- a caret, selection, menus that stay open -- while
    /// the pointer is still Spatiand's.
    func giveKeyboard(to info: MacWindowInfo) {
        guard capturing else { return }
        keyboardOwner = info.pid
        window?.keyable = false
        NSRunningApplication(processIdentifier: info.pid)?.activate()
        if let ax = MacWindows.axWindow(info) {
            AXUIElementSetAttributeValue(ax, kAXMainAttribute as CFString, kCFBooleanTrue)
            AXUIElementPerformAction(ax, kAXRaiseAction as CFString)
        }
    }

    /// Spatiand has the keyboard again: for a host's window, or a menu.
    func takeKeyboard() {
        guard capturing, let w = window else { return }
        keyboardOwner = nil
        w.keyable = true
        NSApp.activate(ignoringOtherApps: true)
        w.makeKeyAndOrderFront(nil)
        w.makeFirstResponder(w.contentView)
    }

    func stop() {
        guard capturing else { return }
        capturing = false
        chords.disable()
        if let watcher { NSWorkspace.shared.notificationCenter.removeObserver(watcher) }
        watcher = nil
        if let keyMonitor { NSEvent.removeMonitor(keyMonitor) }
        if let gestureMonitor { NSEvent.removeMonitor(gestureMonitor) }
        keyMonitor = nil
        gestureMonitor = nil
        keyboardOwner = nil
        CGAssociateMouseAndMouseCursorPosition(1)
        CGDisplayShowCursor(CGMainDisplayID())
        window?.delegate = nil
        window?.orderOut(nil)
        window?.close()
        window = nil
        onChange?()
    }

    /// Switching to another app is a way of asking for the Mac back.
    func windowDidResignKey(_ note: Notification) {
        // Not when a window of this Mac was given the keyboard on purpose.
        if capturing, keyboardOwner == nil { stop() }
    }
}
