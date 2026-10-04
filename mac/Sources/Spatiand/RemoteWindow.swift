//  RemoteWindow.swift — one window of an application running on another computer.
//
//  The picture is decoded and shown by an AVSampleBufferDisplayLayer, which takes the host's
//  access units as they are. The mouse and keyboard go back as the units the host thinks in:
//  pixels of that window, and evdev codes.

import AppKit
import AVFoundation

final class VideoView: NSView {
    let display = AVSampleBufferDisplayLayer()
    /// Said when something happened that the host should hear: an event in the host's own units.
    var send: (([String: Any]) -> Void)?
    var windowID: UInt16 = 0
    /// The decoder lost its place; ask the host for a fresh start.
    var needKeyframe: (() -> Void)?
    /// The size of the host's picture, in its pixels; what pointer positions are measured in.
    var hostSize = CGSize(width: 1280, height: 800) { didSet { needsLayout = true } }
    /// Whether the application in this window is a terminal, where copy and paste are Control-Shift.
    var isTerminal = false
    /// Keys whose release is ours to swallow because their press was sent as something else.
    private var swallowed: Set<UInt16> = []
    /// What the pointer looks like over this window: the host's own, when it has said.
    var cursor: NSCursor? { didSet { window?.invalidateCursorRects(for: self) } }
    override func resetCursorRects() {
        if let cursor { addCursorRect(bounds, cursor: cursor) }
    }
    /// Popups are never the key window, and still have to see the pointer.
    var trackAlways = false {
        didSet {
            trackingAreas.forEach(removeTrackingArea)
            addTrackingArea(NSTrackingArea(
                rect: .zero,
                options: [.mouseMoved, .mouseEnteredAndExited, trackAlways ? .activeAlways : .activeInKeyWindow, .inVisibleRect],
                owner: self, userInfo: nil))
        }
    }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        display.videoGravity = .resize
        display.backgroundColor = NSColor.black.cgColor
        layer?.addSublayer(display)
        addTrackingArea(NSTrackingArea(
            rect: .zero,
            options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
            owner: self, userInfo: nil))
    }
    required init?(coder: NSCoder) { fatalError() }

    override var acceptsFirstResponder: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    private var scale: CGFloat { window?.backingScaleFactor ?? NSScreen.main?.backingScaleFactor ?? 2 }

    /// The picture is shown at its own size, against the top left, as the application's pixels
    /// are: while the host catches up with a resize it is neither stretched nor squeezed, and the
    /// pointer needs no letterbox arithmetic.
    override func layout() {
        super.layout()
        let w = hostSize.width / scale, h = hostSize.height / scale
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        display.frame = CGRect(x: 0, y: bounds.height - h, width: w, height: h)
        CATransaction.commit()
    }

    // MARK: pictures

    /// How many pictures have been shown: none means the window is still black and should not
    /// be left that way.
    private(set) var pictures = 0

    func show(_ sample: CMSampleBuffer) {
        pictures += 1
        if display.status == .failed || display.requiresFlushToResumeDecoding {
            display.flush()
            needKeyframe?()
        }
        // The picture's own size is the truth about the picture; a message about it can come
        // late, or not at all for a restart, and the picture would be stretched meanwhile.
        if let format = CMSampleBufferGetFormatDescription(sample) {
            let d = CMVideoFormatDescriptionGetDimensions(format)
            let size = CGSize(width: Int(d.width), height: Int(d.height))
            if size.width > 0, size != hostSize {
                if ProcessInfo.processInfo.environment["SPATIAND_DEBUG"] != nil { print("picture \(size) (was \(hostSize)), layer \(display.frame), bounds \(bounds)") }
                hostSize = size
            }
        }
        display.enqueue(sample)
    }

    // MARK: the pointer

    private func now() -> UInt32 { UInt32(truncatingIfNeeded: Int(ProcessInfo.processInfo.systemUptime * 1000)) }

    private func input(_ input: Any) {
        send?(["InputAt": ["window": Int(windowID), "input": input, "time_ms": Int(now())]])
    }

    /// Where in the host's picture a point of this view is: the picture sits at the top left, one
    /// of its pixels to a pixel of the screen.
    private func hostPoint(_ event: NSEvent) -> CGPoint? {
        let p = convert(event.locationInWindow, from: nil)
        let k = scale
        let x = p.x * k
        let y = (bounds.height - p.y) * k    // the host's y runs down
        return CGPoint(x: max(0, min(hostSize.width, x)), y: max(0, min(hostSize.height, y)))
    }

    private func motion(_ event: NSEvent) {
        if let p = hostPoint(event) { input(["Motion": ["x": p.x, "y": p.y]]) }
    }

    override func mouseMoved(with event: NSEvent) { motion(event) }
    override func mouseDragged(with event: NSEvent) { motion(event) }
    override func rightMouseDragged(with event: NSEvent) { motion(event) }
    override func otherMouseDragged(with event: NSEvent) { motion(event) }
    override func mouseExited(with event: NSEvent) { input("Leave") }

    private func button(_ event: NSEvent, code: Int, pressed: Bool) {
        motion(event)
        input(["Button": ["button": code, "pressed": pressed]])
    }
    override func mouseDown(with event: NSEvent) { window?.makeFirstResponder(self); button(event, code: 0x110, pressed: true) }
    override func mouseUp(with event: NSEvent) { button(event, code: 0x110, pressed: false) }
    override func rightMouseDown(with event: NSEvent) { button(event, code: 0x111, pressed: true) }
    override func rightMouseUp(with event: NSEvent) { button(event, code: 0x111, pressed: false) }
    override func otherMouseDown(with event: NSEvent) { button(event, code: 0x112, pressed: true) }
    override func otherMouseUp(with event: NSEvent) { button(event, code: 0x112, pressed: false) }

    override func scrollWheel(with event: NSEvent) {
        // The host's wheel runs the other way round from the Mac's, which already folds the
        // owner's "natural scrolling" choice into the sign. A mouse wheel gives lines, a
        // trackpad gives points; a line is about ten.
        let unit: CGFloat = event.hasPreciseScrollingDeltas ? 1 : 10
        input(["Scroll": ["horizontal": -event.scrollingDeltaX * unit, "vertical": -event.scrollingDeltaY * unit]])
    }

    // MARK: the keyboard

    private func send(code: UInt32, pressed: Bool) {
        if ProcessInfo.processInfo.environment["SPATIAND_DEBUG"] != nil { print("key \(code) \(pressed ? "down" : "up")") }
        input(["Key": ["code": Int(code), "pressed": pressed]])
    }

    /// Keys sent down and not yet let go, so a release is sent once and only for a press.
    private var down: Set<UInt16> = []

    private func release(_ keyCode: UInt16) {
        guard down.remove(keyCode) != nil else { return }
        if swallowed.remove(keyCode) != nil {
            send(code: KeyMap.evdev[keyCode] ?? 0, pressed: false)
            send(code: 42, pressed: false)
        } else if let code = KeyMap.code(for: keyCode, commandIsControl: Settings.commandIsControl) {
            send(code: code, pressed: false)
        }
    }

    private func key(_ event: NSEvent, pressed: Bool) {
        let keyCode = event.keyCode
        guard pressed else { release(keyCode); return }
        guard down.insert(keyCode).inserted else { return }
        if Settings.commandIsControl, isTerminal, event.modifierFlags.contains(.command),
           keyCode == 8 || keyCode == 9 {
            // A terminal copies and pastes with Control-Shift-C and -V, because Control-C already
            // means something there. Command stands in for Control, so add the Shift.
            swallowed.insert(keyCode)
            send(code: 42, pressed: true)
            send(code: KeyMap.evdev[keyCode] ?? 0, pressed: true)
        } else if let code = KeyMap.code(for: keyCode, commandIsControl: Settings.commandIsControl) {
            send(code: code, pressed: true)
        }
        // macOS never delivers the release of a key pressed while Command is held, so the host
        // would hold it down and repeat it for ever. Let it go here, a moment later.
        if event.modifierFlags.contains(.command), KeyMap.modifierMask(keyCode) == nil {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.04) { [weak self] in self?.release(keyCode) }
        }
    }

    /// A paste is the host's own, so what is on this Mac's pasteboard must be there first.
    private func paste(_ event: NSEvent, pressed: Bool) {
        if pressed, event.modifierFlags.contains(.command), event.keyCode == 9, Model.shared.clipboard.flush() {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { [weak self] in self?.key(event, pressed: true) }
        } else {
            key(event, pressed: pressed)
        }
    }

    override func keyDown(with event: NSEvent) { if !event.isARepeat { paste(event, pressed: true) } }
    override func keyUp(with event: NSEvent) { key(event, pressed: false) }
    override func flagsChanged(with event: NSEvent) {
        guard let mask = KeyMap.modifierMask(event.keyCode) else { return }
        key(event, pressed: event.modifierFlags.rawValue & mask != 0)
    }
    // Command-key shortcuts would otherwise be taken by the menu before the application sees them.
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard event.modifierFlags.contains(.command) else { return false }
        if event.type == .keyDown, event.isARepeat { return true }
        paste(event, pressed: event.type == .keyDown)
        return true
    }
}

/// Anything the host draws that has a picture and takes input: a window, or a menu hanging off one.
protocol RemoteSurface: AnyObject {
    var view: VideoView { get }
    func setSize(_ size: CGSize)
    func closeForReal()
    func show()
}

extension RemoteSurface {
    func video(_ data: Data, codec: Int32, captured: UInt64) {
        let samples = (self as? RemoteWindow)?.samples ?? (self as? RemotePopup)?.samples
        if let sample = samples?.sample(data, codec: codec, capturedMicros: captured) { view.show(sample) }
    }
}

/// A menu, a dropdown or a tooltip: a panel with no frame, hung off its window where the host
/// says. It never takes the keyboard -- the window it belongs to keeps that, as in any desktop.
final class RemotePopup: NSObject, RemoteSurface {
    let id: UInt16
    let view = VideoView(frame: .zero)
    let samples = VideoSamples()
    private let panel: NSPanel
    private weak var parent: RemoteWindow?
    /// Where on the parent's picture it hangs, in the host's pixels.
    private let offset: CGPoint

    init(id: UInt16, parent: RemoteWindow, offset: CGPoint, size: CGSize) {
        self.id = id
        self.parent = parent
        self.offset = offset
        let scale = parent.window.backingScaleFactor
        panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: size.width / scale, height: size.height / scale),
            styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        super.init()
        view.windowID = id
        view.hostSize = size
        view.trackAlways = true
        view.send = { [weak parent] in parent?.view.send?($0) }
        panel.contentView = view
        panel.isOpaque = true
        panel.hasShadow = true
        panel.level = .popUpMenu
        panel.isReleasedWhenClosed = false
        panel.hidesOnDeactivate = false
        place(size: size)
    }

    private func place(size: CGSize) {
        guard let parent else { return }
        let scale = parent.window.backingScaleFactor
        let content = parent.window.contentLayoutRect
        let origin = parent.window.convertToScreen(content).origin
        // The host's y runs down; the screen's runs up.
        let x = origin.x + offset.x / scale
        let y = origin.y + content.height - (offset.y + size.height) / scale
        panel.setFrame(NSRect(x: x, y: y, width: size.width / scale, height: size.height / scale), display: false)
    }

    func setSize(_ size: CGSize) {
        view.hostSize = size
        place(size: size)
    }

    func show() {
        guard let parent else { return }
        if panel.parent == nil { parent.window.addChildWindow(panel, ordered: .above) }
        panel.orderFront(nil)
    }

    func closeForReal() {
        parent?.window.removeChildWindow(panel)
        panel.close()
    }
}

final class RemoteWindow: NSObject, NSWindowDelegate, RemoteSurface {
    let id: UInt16
    let window: NSWindow
    let view = VideoView(frame: .zero)
    let samples = VideoSamples()
    private var resizeTimer: Timer?
    /// When the host was last asked for a size: the pictures that come back in the next moment
    /// are answers, and must not resize the window the person is still dragging.
    private var askedAt = Date.distantPast
    var onClose: ((UInt16) -> Void)?
    var onFocus: ((UInt16, Bool) -> Void)?
    var onResize: ((UInt16, Int, Int) -> Void)?

    init(id: UInt16, title: String, size: CGSize) {
        self.id = id
        let scale = NSScreen.main?.backingScaleFactor ?? 2
        // The host's pixels, shown at the Mac's own density.
        let content = NSRect(x: 0, y: 0, width: size.width / scale, height: size.height / scale)
        window = NSWindow(
            contentRect: content,
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered, defer: false)
        super.init()
        view.windowID = id
        view.hostSize = size
        window.contentView = view
        window.title = title
        window.delegate = self
        window.isReleasedWhenClosed = false
        window.acceptsMouseMovedEvents = true
        window.center()
    }

    /// The application changed the size of its own window: the Mac's follows, unless it is
    /// already that size (which is what a resize the owner asked for comes back as).
    func setSize(_ size: CGSize) {
        view.hostSize = size
        let scale = window.backingScaleFactor
        let wanted = NSSize(width: size.width / scale, height: size.height / scale)
        let have = view.bounds.size
        if window.inLiveResize || Date().timeIntervalSince(askedAt) < 1.0 { return }
        if abs(have.width - wanted.width) > 2 || abs(have.height - wanted.height) > 2 {
            window.setContentSize(wanted)
        }
    }

    func show() {
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(view)
    }

    func closeForReal() {
        window.delegate = nil
        window.close()
    }

    // MARK: the window's own events

    /// Closing asks the application to close, and the window goes when the host says it has.
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        onClose?(id)
        return false
    }

    func windowDidBecomeKey(_ note: Notification) { onFocus?(id, true) }
    func windowDidResignKey(_ note: Notification) { onFocus?(id, false) }

    /// Resizing asks the host for that many pixels as the frame is dragged, not once it is let
    /// go: the application lays itself out for the shape it is given, which is what makes this a
    /// window and not a video of one. A request every tenth of a second keeps an encoder restart
    /// from landing on every frame of the drag.
    func windowDidResize(_ note: Notification) {
        guard resizeTimer == nil else { return }
        let timer = Timer(timeInterval: 0.1, repeats: false) { [weak self] _ in
            self?.resizeTimer = nil
            self?.askForSize()
        }
        RunLoop.main.add(timer, forMode: .common)   // a drag runs the loop in its own mode
        resizeTimer = timer
    }

    func windowDidEndLiveResize(_ note: Notification) {
        resizeTimer?.invalidate()
        resizeTimer = nil
        askForSize()
    }

    private func askForSize() {
        let scale = window.backingScaleFactor
        let size = view.bounds.size
        askedAt = Date()
        onResize?(id, Int(size.width * scale), Int(size.height * scale))
    }
}
