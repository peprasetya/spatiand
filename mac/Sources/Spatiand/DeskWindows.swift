//  DeskWindows.swift — 2D mode: Spatiand's own applications as windows on the Mac's desktop.
//
//  SpatiMesh, SpatiReel and the rest are clients of the room's compositor on every platform.
//  Without glasses the app runs that compositor with nothing to draw into, and it says each of
//  its windows here (SP_DESK_*): a Mac window is made for it, showing the very picture the
//  application drew -- an IOSurface, by number, put straight into the view's layer -- and what
//  the mouse, the trackpad and the keyboard do to it goes back (sp_desk_*), in the window's own
//  pixels. One point of the Mac window is one pixel of the application's, so its text is the
//  size it was made to be. See crates/spatiand-mac/src/mac/desk.rs.
//
//  While any is open, Spatiand is an ordinary application with a Dock icon, so its windows can
//  be gone to with Command-Tab like any browser's.

import AppKit
import IOSurface
import CSpatiand

final class DeskView: NSView {
    let id: UInt32
    private var shown: IOSurfaceRef?
    /// The one shown before, let go of a picture later: the window server may still be
    /// putting it on screen when the next one comes.
    private var leaving: IOSurfaceRef?

    init(id: UInt32, size: NSSize) {
        self.id = id
        super.init(frame: NSRect(origin: .zero, size: size))
        wantsLayer = true
        layerContentsRedrawPolicy = .never
        layer?.backgroundColor = NSColor.black.cgColor
        // Fitted to the window, as the room fits a picture to its quad: a window's picture can be
        // another size than the window (a film's two eyes side by side, a film in SpatiReel's
        // window), and the pointer is measured in the window's own size, not the picture's.
        // While the application catches up with a resize, its last picture is stretched.
        layer?.contentsGravity = .resize
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseMoved, .mouseEnteredAndExited, .activeInActiveApp, .inVisibleRect], owner: self, userInfo: nil))
    }
    required init?(coder: NSCoder) { fatalError() }

    deinit {
        if let shown { IOSurfaceDecrementUseCount(shown) }
        if let leaving { IOSurfaceDecrementUseCount(leaving) }
    }

    override var acceptsFirstResponder: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    /// The application's newest picture. Held in use (the count the application reads to know
    /// which of its pictures it may draw into) until the one after it is on screen.
    func show(surface number: UInt32) {
        // Handed over in private by the application (not a global surface anyone could open).
        guard let surface = spatiand_surfaces_open(number) else { return }
        if let shown, IOSurfaceGetID(shown) == number {
            layer?.setNeedsDisplay()
            layer?.contents = nil
        }
        IOSurfaceIncrementUseCount(surface)
        if let leaving { IOSurfaceDecrementUseCount(leaving) }
        leaving = shown
        shown = surface
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        layer?.contents = surface
        CATransaction.commit()
    }

    /// Where an event is, from the top left, as the application measures. The view is not
    /// flipped, so that the layer's "top left" is the top left on screen.
    private func at(_ event: NSEvent) -> NSPoint {
        let p = convert(event.locationInWindow, from: nil)
        return NSPoint(x: p.x, y: bounds.height - p.y)
    }

    private func moved(_ event: NSEvent) {
        let p = at(event)
        sp_desk_motion(id, Double(p.x), Double(p.y))
    }

    override func mouseMoved(with event: NSEvent) { moved(event) }
    override func mouseDragged(with event: NSEvent) { moved(event) }
    override func rightMouseDragged(with event: NSEvent) { moved(event) }
    override func otherMouseDragged(with event: NSEvent) { moved(event) }
    override func mouseExited(with event: NSEvent) { sp_desk_leave(id) }

    private func button(_ event: NSEvent, _ code: UInt32, _ down: Bool) {
        if down { window?.makeFirstResponder(self) }
        moved(event)
        sp_desk_button(id, code, down)
    }

    override func mouseDown(with event: NSEvent) { button(event, 0x110, true) }
    override func mouseUp(with event: NSEvent) { button(event, 0x110, false) }
    override func rightMouseDown(with event: NSEvent) { button(event, 0x111, true) }
    override func rightMouseUp(with event: NSEvent) { button(event, 0x111, false) }
    override func otherMouseDown(with event: NSEvent) { button(event, event.buttonNumber == 2 ? 0x112 : 0x113, true) }
    override func otherMouseUp(with event: NSEvent) { button(event, event.buttonNumber == 2 ? 0x112 : 0x113, false) }

    override func scrollWheel(with event: NSEvent) {
        moved(event)
        // Wayland's down is positive; the Mac's deltas say which way the content goes, with the
        // owner's choice of "natural" already in them.
        if event.hasPreciseScrollingDeltas {
            sp_desk_scroll(id, Double(-event.scrollingDeltaX), Double(-event.scrollingDeltaY), false)
        } else {
            sp_desk_scroll(id, Double(-event.scrollingDeltaX) * 15, Double(-event.scrollingDeltaY) * 15, true)
        }
    }

    override func magnify(with event: NSEvent) {
        sp_desk_pinch(id, Float(1 + event.magnification))
    }

    /// Keys come through `DeskWindows`' own watch on the application's events, not here:
    /// AppKit never sends a view the release of a key pressed with Command.
    override func keyDown(with event: NSEvent) {}
    override func keyUp(with event: NSEvent) {}
}

final class DeskWindows: NSObject, NSWindowDelegate {
    static let shared = DeskWindows()

    private var windows: [UInt32: NSWindow] = [:]
    private var keyWatch: Any?
    /// Closed by the session, not by the owner: not to be asked about again.
    private var closing: Set<UInt32> = []

    var isEmpty: Bool { windows.isEmpty }
    /// The windows on show, for the self-test.
    var all: [NSWindow] { Array(windows.values) }

    func asked(_ what: Int32, id: UInt32, a: Double, b: Double, text: String) {
        switch Int(what) {
        case SP_DESK_OPEN:
            let lines = text.split(separator: "\n", maxSplits: 1, omittingEmptySubsequences: false)
            open(id, size: NSSize(width: max(a, 64), height: max(b, 64)), title: lines.first.map(String.init) ?? "", app: lines.count > 1 ? String(lines[1]) : "")
        case SP_DESK_PICTURE:
            (windows[id]?.contentView as? DeskView)?.show(surface: UInt32(a))
        case SP_DESK_TITLE:
            windows[id]?.title = text.isEmpty ? "Spatiand" : text
        case SP_DESK_SIZE:
            guard let window = windows[id], !window.inLiveResize else { return }
            let size = NSSize(width: a, height: b)
            if window.contentRect(forFrameRect: window.frame).size != size { window.setContentSize(size) }
        case SP_DESK_CLOSE:
            guard let window = windows.removeValue(forKey: id) else { return }
            closing.insert(id)
            window.close()
            closing.remove(id)
            settle()
        default:
            break
        }
    }

    private func open(_ id: UInt32, size: NSSize, title: String, app: String) {
        if windows[id] != nil { return }
        let window = NSWindow(contentRect: NSRect(origin: .zero, size: size), styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.title = title.isEmpty ? "Spatiand" : title
        window.contentView = DeskView(id: id, size: size)
        window.delegate = self
        window.acceptsMouseMovedEvents = true
        window.tabbingMode = .disallowed
        window.contentMinSize = NSSize(width: 320, height: 240)
        // Each application's windows remember where they were, as a Mac's do.
        let plain = app.split(separator: "@").first.map(String.init) ?? app
        if !plain.isEmpty { window.setFrameAutosaveName("Spatiand " + plain) }
        if windows.isEmpty, !window.setFrameUsingName("Spatiand " + plain) { window.center() }
        if let last = windows.values.first(where: { $0.isVisible }), windows.count > 0 {
            window.setFrameTopLeftPoint(window.cascadeTopLeft(from: NSPoint(x: last.frame.minX, y: last.frame.maxY)))
        }
        windows[id] = window
        settle()
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(window.contentView)
        NSApp.activate(ignoringOtherApps: true)
    }

    /// A Dock icon and the keys watched while there are windows; neither once there are none.
    private func settle() {
        if windows.isEmpty {
            if let keyWatch { NSEvent.removeMonitor(keyWatch) }
            keyWatch = nil
            if NSApp.activationPolicy() != .accessory { NSApp.setActivationPolicy(.accessory) }
            return
        }
        if NSApp.activationPolicy() != .regular { NSApp.setActivationPolicy(.regular) }
        if keyWatch == nil {
            keyWatch = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .keyUp, .flagsChanged]) { [weak self] event in
                self?.key(event) == true ? nil : event
            }
        }
    }

    /// A key, if a window of these has the keyboard: every key, Command's included, is the
    /// application's -- as a browser's are its own.
    private func key(_ event: NSEvent) -> Bool {
        guard let view = NSApp.keyWindow?.contentView as? DeskView, windows[view.id] != nil else { return false }
        switch event.type {
        case .keyDown:
            // The application repeats a held key itself, at the Mac's own rate.
            if !event.isARepeat { sp_desk_key(view.id, event.keyCode, true) }
        case .keyUp:
            sp_desk_key(view.id, event.keyCode, false)
        case .flagsChanged:
            let flags = event.modifierFlags
            let down: Bool
            switch event.keyCode {
            case 0x38, 0x3C: down = flags.contains(.shift)
            case 0x3B, 0x3E: down = flags.contains(.control)
            case 0x3A, 0x3D: down = flags.contains(.option)
            case 0x37, 0x36: down = flags.contains(.command)
            case 0x39:
                // Caps Lock says only that it is now on or off: a press either way.
                sp_desk_key(view.id, event.keyCode, true)
                sp_desk_key(view.id, event.keyCode, false)
                return true
            default: return false
            }
            sp_desk_key(view.id, event.keyCode, down)
        default:
            return false
        }
        return true
    }

    private func id(of window: NSWindow) -> UInt32? {
        (window.contentView as? DeskView)?.id
    }

    // MARK: the window's own buttons and edges

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        guard let id = id(of: sender) else { return true }
        if closing.contains(id) { return true }
        // Asked, as the room's close button asks: the application decides, and says so by
        // closing (SP_DESK_CLOSE).
        sp_desk_close(id)
        return false
    }

    func windowDidResize(_ notification: Notification) {
        guard let window = notification.object as? NSWindow, let id = id(of: window) else { return }
        let size = window.contentRect(forFrameRect: window.frame).size
        sp_desk_resize(id, Int32(size.width.rounded()), Int32(size.height.rounded()))
    }

    func windowDidBecomeKey(_ notification: Notification) {
        guard let window = notification.object as? NSWindow, let id = id(of: window) else { return }
        sp_desk_focus(id, true)
    }

    func windowDidResignKey(_ notification: Notification) {
        guard let window = notification.object as? NSWindow, let id = id(of: window) else { return }
        sp_desk_focus(id, false)
    }

    /// The glasses have the windows, or the session is ending: every Mac window goes.
    func closeAll() {
        for (id, window) in windows {
            closing.insert(id)
            window.close()
        }
        windows.removeAll()
        closing.removeAll()
        settle()
    }
}
