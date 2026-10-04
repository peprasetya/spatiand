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
//  and, with Option held, dragging a window moves it and the wheel (or a pinch) resizes it.

import AppKit
import Carbon.HIToolbox

final class InputWindow: NSWindow {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { true }
}

final class InputView: NSView {
    weak var controller: RoomController?
    var onRelease: (() -> Void)?

    override var acceptsFirstResponder: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    // MARK: the mouse

    private func moved(_ event: NSEvent) { controller?.pointerMoved(dx: Double(event.deltaX), dy: Double(event.deltaY)) }
    override func mouseMoved(with event: NSEvent) { moved(event) }
    override func mouseDragged(with event: NSEvent) { moved(event) }
    override func rightMouseDragged(with event: NSEvent) { moved(event) }
    override func otherMouseDragged(with event: NSEvent) { moved(event) }

    override func mouseDown(with event: NSEvent) { controller?.buttonDown(0x110, grab: event.modifierFlags.contains(.option)) }
    override func mouseUp(with event: NSEvent) { controller?.buttonUp(0x110) }
    override func rightMouseDown(with event: NSEvent) { controller?.buttonDown(0x111, grab: false) }
    override func rightMouseUp(with event: NSEvent) { controller?.buttonUp(0x111) }
    override func otherMouseDown(with event: NSEvent) { controller?.buttonDown(0x112, grab: false) }
    override func otherMouseUp(with event: NSEvent) { controller?.buttonUp(0x112) }

    override func scrollWheel(with event: NSEvent) {
        if event.modifierFlags.contains(.option) {
            controller?.zoom(by: exp(Double(event.scrollingDeltaY) * (event.hasPreciseScrollingDeltas ? 0.004 : 0.04)))
        } else {
            controller?.scrolled(dx: Double(event.scrollingDeltaX), dy: Double(event.scrollingDeltaY), precise: event.hasPreciseScrollingDeltas)
        }
    }

    /// A pinch on the trackpad resizes the window being pointed at.
    override func magnify(with event: NSEvent) { controller?.zoom(by: 1 + Double(event.magnification)) }

    // MARK: the keyboard

    /// Spatiand's own chords, which are never sent on.
    private func chord(_ event: NSEvent) -> Bool {
        guard event.type == .keyDown, event.modifierFlags.intersection([.control, .option, .command, .shift]) == [.control, .option] else { return false }
        switch Int(event.keyCode) {
        case kVK_ANSI_G: onRelease?()
        case kVK_ANSI_R: controller?.recentre()
        case kVK_ANSI_P: controller?.togglePin()
        case kVK_ANSI_B: controller?.bringAimedHere()
        case kVK_ANSI_C: controller?.core.nextCorner()
        case kVK_ANSI_S: controller?.core.toggleSize()
        default: return false
        }
        return true
    }

    override func keyDown(with event: NSEvent) {
        if chord(event) { return }
        controller?.keyTarget?.keyDown(with: event)
    }
    override func keyUp(with event: NSEvent) { controller?.keyTarget?.keyUp(with: event) }
    override func flagsChanged(with event: NSEvent) { controller?.keyTarget?.flagsChanged(with: event) }
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if chord(event) { return true }
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
        let w = InputWindow(contentRect: screen.frame, styleMask: .borderless, backing: .buffered, defer: false)
        w.isReleasedWhenClosed = false
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
        view.onRelease = { [weak self] in self?.stop() }
        w.contentView = view
        window = w
        NSApp.activate(ignoringOtherApps: true)
        w.makeKeyAndOrderFront(nil)
        w.makeFirstResponder(view)
        CGAssociateMouseAndMouseCursorPosition(0)
        CGDisplayHideCursor(CGMainDisplayID())
        capturing = true
        onChange?()
    }

    func stop() {
        guard capturing else { return }
        capturing = false
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
        if capturing { stop() }
    }
}
