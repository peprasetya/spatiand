//  GlassesScreen.swift — a window on the glasses' display, for the compositor to draw into.
//
//  The room is not drawn here. It is the Deck's compositor (crates/spatiand-mac), which talks to
//  the glasses' sensors itself and switches them to their side-by-side mode itself; what it needs
//  from the app is somewhere to draw, and that is this: a borderless window covering the glasses'
//  display, and the layer in it, handed over with `sp_glasses`. When the display changes width --
//  which it does when the glasses go into their 3D mode -- the window is made again and handed
//  over again, as a new surface is on the Beam Pro.
//
//  What macOS does with this display was found out the hard way in HoloFrame: it is matched by its
//  EDID; a window placed on it before AppKit's screen list has caught up lands on the wrong screen;
//  a window that was on it when it vanished leaves a ghost in the window server until the process
//  ends (so unplugging restarts the app); macOS puts it back to its remembered one-eye mode when
//  it feels like it; and nothing is composited over a display that has not been captured.

import AppKit
import CSpatiand
import QuartzCore

/// A window AppKit is not allowed to move: it constrains new frames to the screens it knows of,
/// which right after a display appears is a stale list.
final class GlassesWindow: NSWindow {
    override func constrainFrameRect(_ frameRect: NSRect, to screen: NSScreen?) -> NSRect { frameRect }
}

final class GlassesScreen {
    private var window: GlassesWindow?
    private var watchdog: Timer?
    private var captured: CGDirectDisplayID?
    private var displayID: CGDirectDisplayID = 0
    private var opening = false
    private var lastModeTry = Date.distantPast
    private(set) var hasHadAWindow = false
    private(set) var status = "Waiting for the glasses"
    /// Said, on the main thread, once the display has gone away.
    var onLost: (() -> Void)?
    /// How wide the window on the glasses is, in the display's own pixels.
    private var pixelWidth: CGFloat = 0
    var onChange: (() -> Void)?

    var isShowing: Bool { window != nil }

    /// XREAL Air identifies itself with EDID vendor 0x3647, product 0x3132.
    static func findDisplay() -> CGDirectDisplayID? {
        var count: UInt32 = 0
        var ids = [CGDirectDisplayID](repeating: 0, count: 16)
        guard CGGetActiveDisplayList(16, &ids, &count) == .success else { return nil }
        return ids.prefix(Int(count)).first { CGDisplayVendorNumber($0) == 0x3647 && CGDisplayModelNumber($0) == 0x3132 }
    }

    private func say(_ text: String) {
        status = text
        onChange?()
    }

    /// Put a window on the glasses' display as soon as there is one. Safe to call again.
    func start() {
        guard window == nil, !opening else { return }
        opening = true
        wait(tries: 0)
    }

    /// With no glasses: the room in an ordinary window on this Mac, both eyes side by side. For
    /// looking at it without wearing anything, and for the tests.
    func startPreview() {
        guard window == nil else { return }
        let frame = NSRect(x: 80, y: 120, width: 1280, height: 360)
        let w = GlassesWindow(contentRect: frame, styleMask: [.titled, .closable], backing: .buffered, defer: false)
        w.title = "Spatiand (what the glasses would show)"
        w.isReleasedWhenClosed = false
        let layer = CALayer()
        layer.frame = CGRect(origin: .zero, size: frame.size)
        layer.contentsScale = 2
        layer.backgroundColor = NSColor.black.cgColor
        let view = NSView(frame: NSRect(origin: .zero, size: frame.size))
        view.layer = layer
        view.wantsLayer = true
        w.contentView = view
        w.orderFrontRegardless()
        window = w
        sp_glasses(Unmanaged.passUnretained(layer).toOpaque(), Int32(frame.width * 2), Int32(frame.height * 2), 0)
        say("In a window on this Mac")
        if ProcessInfo.processInfo.environment["SPATIAND_LAYER_DEBUG"] != nil {
            DispatchQueue.main.asyncAfter(deadline: .now() + 3) {
                print("layer: \(layer) frame \(layer.frame) sublayers \(layer.sublayers?.count ?? 0)")
                for sub in layer.sublayers ?? [] {
                    let metal = sub as? CAMetalLayer
                    print("  sub: \(type(of: sub)) frame \(sub.frame) hidden \(sub.isHidden) opacity \(sub.opacity) drawable \(String(describing: metal?.drawableSize)) device \(String(describing: metal?.device?.name)) sync \(String(describing: metal?.displaySyncEnabled))")
                }
            }
        }
    }

    private func wait(tries: Int) {
        guard opening else { return }
        guard let id = Self.findDisplay() else {
            if tries > 60 { opening = false; say("The glasses' display did not appear"); return }
            say("Waiting for the glasses' display")
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in self?.wait(tries: tries + 1) }
            return
        }
        open(on: id)
    }

    private static func appKitFrame(of id: CGDirectDisplayID) -> NSRect {
        let bounds = CGDisplayBounds(id)
        let primary = CGDisplayBounds(CGMainDisplayID()).height
        return NSRect(x: bounds.origin.x, y: primary - bounds.maxY, width: bounds.width, height: bounds.height)
    }

    private static func screenID(_ screen: NSScreen) -> CGDirectDisplayID {
        (screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value ?? 0
    }

    /// The display's double-width mode, when it offers one: 72 Hz as the Deck runs it, else any.
    private static func wideMode(of id: CGDirectDisplayID) -> CGDisplayMode? {
        let options = [kCGDisplayShowDuplicateLowResolutionModes: true] as CFDictionary
        let modes = (CGDisplayCopyAllDisplayModes(id, options) as? [CGDisplayMode]) ?? []
        let wide = modes.filter { $0.pixelWidth == 3840 && $0.pixelHeight == 1080 }
        return wide.first { abs($0.refreshRate - 72) < 1 } ?? wide.max { $0.refreshRate < $1.refreshRate }
    }

    private func open(on id: CGDirectDisplayID) {
        guard opening else { return }
        // AppKit's screen list lags a display that has just changed; wait until it agrees.
        let expected = Self.appKitFrame(of: id)
        let deadline = Date().addingTimeInterval(6)
        var screen: NSScreen?
        repeat {
            screen = NSScreen.screens.first { Self.screenID($0) == id }
            if let screen, screen.frame == expected { break }
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        } while Date() < deadline
        let frame = screen?.frame ?? expected
        guard frame.width > 1, frame.height > 1 else { opening = false; say("The glasses' display is not usable"); return }
        displayID = id

        // Take the display from the desktop: nothing else may appear on it, and nothing is
        // composited over a display that has not been captured.
        // Only once it is showing two eyes: a captured display is not looked at again by macOS
        // when the glasses change what they offer, and the wide picture is never found.
        let wide = (CGDisplayCopyDisplayMode(id)?.pixelWidth ?? 0) >= 3000
        // And only when asked: while any display is captured macOS keeps this application in
        // front, and with the mouse given back to the Mac no other application could be chosen.
        // A window above everything covers the display as well.
        if wide, ProcessInfo.processInfo.environment["SPATIAND_CAPTURE"] != nil {
            if CGDisplayCapture(id) == .success { captured = id } else { print("glasses: could not take the display over") }
        }

        let w = GlassesWindow(contentRect: frame, styleMask: .borderless, backing: .buffered, defer: false)
        w.isReleasedWhenClosed = false
        w.level = captured != nil ? NSWindow.Level(rawValue: Int(CGShieldingWindowLevel())) : .screenSaver
        w.backgroundColor = .black
        w.isOpaque = true
        w.ignoresMouseEvents = true
        w.collectionBehavior = [.canJoinAllSpaces, .stationary, .fullScreenNone]

        // A layer of the app's own, for the compositor's driver to put its Metal layer in.
        let scale = screen?.backingScaleFactor ?? 1
        let layer = CALayer()
        layer.frame = CGRect(origin: .zero, size: frame.size)
        layer.contentsScale = scale
        layer.backgroundColor = NSColor.black.cgColor
        layer.isOpaque = true
        let view = NSView(frame: NSRect(origin: .zero, size: frame.size))
        view.layer = layer
        view.wantsLayer = true
        w.contentView = view
        w.orderFrontRegardless()
        w.setFrame(frame, display: true)
        window = w
        hasHadAWindow = true
        opening = false

        let pixels = (Int32(frame.width * scale), Int32(frame.height * scale))
        pixelWidth = CGFloat(pixels.0)
        sp_glasses(Unmanaged.passUnretained(layer).toOpaque(), pixels.0, pixels.1, id)
        let twoEyes = pixels.0 >= pixels.1 * 3
        say(twoEyes ? "In the glasses, in 3D" : "In the glasses, one eye")
        print("glasses: a window of \(pixels.0)x\(pixels.1) on display \(id), \(twoEyes ? "two eyes" : "one eye")")

        let dog = Timer(timeInterval: 0.2, repeats: true) { [weak self] _ in self?.check() }
        RunLoop.main.add(dog, forMode: .common)
        watchdog = dog
    }

    private func check() {
        guard let window else { return }
        let here = Self.findDisplay() != nil && CGDisplayIsActive(displayID) != 0
        if !here {
            hide()
            onLost?()
            return
        }
        // The compositor has asked the glasses for two eyes; macOS may still be showing the
        // display in its remembered one-eye mode, and keeps it there until asked.
        // In pixels: macOS may show the two-eye mode as a doubled 1920 by 540, whose size in
        // points is as wide as one eye's was.
        let width = CGFloat(CGDisplayCopyDisplayMode(displayID)?.pixelWidth ?? Int(CGDisplayBounds(displayID).width))
        if width < 3000, Date().timeIntervalSince(lastModeTry) > 1.0, let wide = Self.wideMode(of: displayID) {
            lastModeTry = Date()
            var config: CGDisplayConfigRef?
            CGBeginDisplayConfiguration(&config)
            CGConfigureDisplayWithDisplayMode(config, displayID, wide, nil)
            CGCompleteDisplayConfiguration(config, .forSession)
        }
        if abs(pixelWidth - width) > 1 || abs(window.frame.width - CGDisplayBounds(displayID).width) > 1 {
            print("glasses: the display is now \(Int(width)) wide; a new window")
            let id = displayID
            hide()
            if let captured { CGDisplayRelease(captured) }
            captured = nil
            opening = true
            // Let it hold still before building on it.
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { [weak self] in
                guard let self, self.opening else { return }
                self.open(on: Self.findDisplay() ?? id)
            }
        }
    }

    /// Off the glasses this instant: before AppKit moves a window whose display has gone to the
    /// Mac's own screen. The compositor is told first, and has stopped drawing when this returns.
    private func hide() {
        watchdog?.invalidate()
        watchdog = nil
        guard window != nil else { return }
        sp_glasses_gone()
        window?.contentView = nil
        window?.orderOut(nil)
        window?.close()
        window = nil
        for case let w as GlassesWindow in NSApp.windows { w.orderOut(nil) }
    }

    func stop() {
        opening = false
        hide()
        if let captured { CGDisplayRelease(captured) }
        captured = nil
        say("Waiting for the glasses")
    }
}
