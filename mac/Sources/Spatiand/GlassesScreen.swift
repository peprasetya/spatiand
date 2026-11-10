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
//  ends (one more each time, so each display gets a window of its own); macOS puts it back to its remembered one-eye mode when
//  it feels like it; and nothing is composited over a display that has not been captured.

import AppKit
import AVFoundation
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
    /// The room's picture is on the glasses (true), or no longer (false): what the pointer and
    /// keys follow (`Room.start`).
    var onShown: ((Bool) -> Void)?
    /// How wide the window on the glasses is, in the display's own pixels.
    private var pixelWidth: CGFloat = 0
    var onChange: (() -> Void)?

    var isShowing: Bool { window != nil }
    /// The glasses' display while the room is on it, and the window the room is in.
    var display: CGDirectDisplayID? { window != nil && displayID != 0 ? displayID : nil }
    var windowNumber: CGWindowID? { window.map { CGWindowID($0.windowNumber) } }

    /// XREAL Air identifies itself with EDID vendor 0x3647, product 0x3132.
    static func findDisplay() -> CGDirectDisplayID? {
        // For the test of glasses whose display never comes (`wait`): their USB alone.
        if ProcessInfo.processInfo.environment["SPATIAND_TEST_NO_GLASSES_DISPLAY"] != nil { return nil }
        var count: UInt32 = 0
        var ids = [CGDirectDisplayID](repeating: 0, count: 16)
        guard CGGetActiveDisplayList(16, &ids, &count) == .success else { return nil }
        return ids.prefix(Int(count)).first { CGDisplayVendorNumber($0) == 0x3647 && CGDisplayModelNumber($0) == 0x3132 }
    }

    /// ANGLE's Metal layer, put inside ours when the compositor first draws, shows each frame
    /// the moment it is given one -- across the middle of the last, if that is where the
    /// display has got to. Told to wait for the refresh instead, here, since the compositor's
    /// way of asking for that halved the frame rate.
    private var syncTimer: Timer?
    private func keepInStep(_ layer: CALayer) {
        syncTimer?.invalidate()
        var looks = 0
        let t = Timer(timeInterval: 0.25, repeats: true) { [weak layer] timer in
            looks += 1
            for case let metal as CAMetalLayer in layer?.sublayers ?? [] where !metal.displaySyncEnabled {
                metal.displaySyncEnabled = true
                print("glasses: frames are shown on the display's refresh")
            }
            // It is made anew when the size changes; looked at for as long as the window lives.
            if layer == nil { timer.invalidate() }
            _ = looks
        }
        RunLoop.main.add(t, forMode: .common)
        syncTimer = t
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
        keepInStep(layer)
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

    /// **The glasses plugged in and their display never there.** Their USB is, so the room is
    /// wanted, but macOS has no display for them: the picture's link is down, as it was after a
    /// night's display sleep on littleotter (2026-11-10), which only unplugging and plugging them in
    /// again cured. The room used to stop waiting after 30 seconds, saying so only in its menu, which
    /// nobody could see: blank glasses, and the virtual screen the Mac's only other. It goes on
    /// waiting now (the display can still come), and says aloud what to do -- the one way to reach
    /// someone looking at blank glasses -- once the Mac's screens have been awake 30 seconds
    /// without it: asleep, no display is there, and that is no fault.
    private static let missingAfter: TimeInterval = 30
    private var missingSince: Date?
    private var missingSaid = false
    private var screensAsleep = false
    private let voice = AVSpeechSynthesizer()

    init() {
        screensAsleep = CGDisplayIsAsleep(CGMainDisplayID()) != 0
        let centre = NSWorkspace.shared.notificationCenter
        centre.addObserver(forName: NSWorkspace.screensDidSleepNotification, object: nil, queue: .main) { [weak self] _ in
            self?.screensAsleep = true
        }
        centre.addObserver(forName: NSWorkspace.screensDidWakeNotification, object: nil, queue: .main) { [weak self] _ in
            guard let self else { return }
            self.screensAsleep = false
            // Counted from the wake: the display is given a moment to come back as it does.
            if self.missingSince != nil { self.missingSince = Date() }
        }
    }

    private func wait(tries: Int) {
        guard opening else { missingSince = nil; return }
        guard let id = Self.findDisplay() else {
            if screensAsleep {
                missingSince = nil
            } else if missingSince == nil {
                missingSince = Date()
            } else if let since = missingSince, Date().timeIntervalSince(since) > Self.missingAfter, !missingSaid {
                missingSaid = true
                let text = "The glasses are plugged in, but their screen has not come on. Unplug them, and plug them in again."
                print("glasses: plugged in (USB) for \(Int(Self.missingAfter)) s awake with no display: \(text)")
                say("The glasses' display has not come on: unplug them and plug them in again")
                voice.speak(AVSpeechUtterance(string: text))
            }
            if !missingSaid { say("Waiting for the glasses' display") }
            // Often at first, while it is usually a mode change; then gently, for as long as the
            // glasses are plugged in (unplugged, the room stops this: `stop`).
            DispatchQueue.main.asyncAfter(deadline: .now() + (tries < 60 ? 0.5 : 2)) { [weak self] in self?.wait(tries: tries + 1) }
            return
        }
        if missingSaid { print("glasses: their display came on after all") }
        missingSince = nil
        missingSaid = false
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

    private static func modes(of id: CGDirectDisplayID) -> [CGDisplayMode] {
        let options = [kCGDisplayShowDuplicateLowResolutionModes: true] as CFDictionary
        return (CGDisplayCopyAllDisplayModes(id, options) as? [CGDisplayMode]) ?? []
    }

    /// The display's double-width mode, when it offers one: 72 Hz as the Deck runs it, else any.
    /// Only one drawn at its own size: glasses in one eye also get a 3840 by 1080 from macOS, as
    /// a picture at twice the size shrunk onto their 1920 by 1080, which they show squeezed and
    /// the same to both eyes. Once chosen macOS keeps it for them until the Mac logs out, so
    /// every plug-in came up in it and the room, seeing a wide window, never asked for 3D.
    private static func wideMode(of id: CGDirectDisplayID) -> CGDisplayMode? {
        let wide = modes(of: id).filter { $0.pixelWidth == 3840 && $0.pixelHeight == 1080 && $0.width == 3840 }
        return wide.first { abs($0.refreshRate - 72) < 1 } ?? wide.max { $0.refreshRate < $1.refreshRate }
    }

    /// One eye at its own size, for glasses left in that shrunk double width.
    private static func oneEyeMode(of id: CGDirectDisplayID) -> CGDisplayMode? {
        let one = modes(of: id).filter { $0.pixelWidth == 1920 && $0.pixelHeight == 1080 && $0.width == 1920 }
        return one.max { $0.refreshRate < $1.refreshRate }
    }

    /// Wide in pixels, but not on the cable: the shrunk mode `wideMode` refuses.
    private static func isShrunkWide(_ id: CGDirectDisplayID) -> Bool {
        guard let mode = CGDisplayCopyDisplayMode(id), mode.pixelWidth >= 3000 else { return false }
        return mode.width != mode.pixelWidth && wideMode(of: id) == nil
    }

    private static func use(_ mode: CGDisplayMode, on id: CGDirectDisplayID) {
        var config: CGDisplayConfigRef?
        CGBeginDisplayConfiguration(&config)
        CGConfigureDisplayWithDisplayMode(config, id, mode, nil)
        CGCompleteDisplayConfiguration(config, .forSession)
    }

    private func open(on id: CGDirectDisplayID) {
        guard opening else { return }
        if Self.isShrunkWide(id), let one = Self.oneEyeMode(of: id) {
            print("glasses: macOS has them in a shrunk double width; back to one eye first")
            Self.use(one, on: id)
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { [weak self] in
                guard let self, self.opening else { return }
                self.open(on: Self.findDisplay() ?? id)
            }
            return
        }
        // With nothing else to show the Mac's desktop on, a virtual screen takes it (MacScreens):
        // **once the glasses show two eyes**, not before. Rearranged while they were changing to
        // it, the glasses' display never came back (an M6 Mac mini, 2026-11-09), until Spatiand quit.
        if (CGDisplayCopyDisplayMode(id)?.pixelWidth ?? 0) >= 3000, MacScreens.leadIfAlone(besides: id) {
            RunLoop.current.run(until: Date().addingTimeInterval(0.3))
        }
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
        // **Not taken over, unless asked (SPATIAND_CAPTURE=1).** While an application has any
        // display captured, macOS treats the whole Mac as its: the menu bar goes from every
        // screen, the Touch Bar's controls stop answering, and no other application can be
        // brought forward. A window above everything covers the glasses as well. (It was put
        // back once for the frame rate, which turned out to be the swap interval's doing.)
        if wide, ProcessInfo.processInfo.environment["SPATIAND_CAPTURE"] == "1" {
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
        onShown?(true)

        let pixels = (Int32(frame.width * scale), Int32(frame.height * scale))
        pixelWidth = CGFloat(pixels.0)
        sp_glasses(Unmanaged.passUnretained(layer).toOpaque(), pixels.0, pixels.1, id)
        keepInStep(layer)
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
            Self.use(wide, on: displayID)
        }
        // And where the display is: one that has moved leaves the window where it was, which
        // may be the Mac's own screen.
        let place = Self.appKitFrame(of: displayID)
        if abs(pixelWidth - width) <= 1, abs(window.frame.width - place.width) <= 1, window.frame.origin != place.origin {
            window.setFrame(place, display: true)
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
        defer { onShown?(false) }
        sp_glasses_gone()
        // Made nothing before it is put away: the window server has been seen to keep a window
        // whose display changed under it, on the Mac's own screen, black and above everything.
        window?.alphaValue = 0
        window?.setFrame(NSRect(x: -30000, y: -30000, width: 1, height: 1), display: false)
        window?.contentView = nil
        window?.orderOut(nil)
        window?.close()
        window = nil
        for case let w as GlassesWindow in NSApp.windows { w.orderOut(nil) }
    }

    func stop() {
        opening = false
        missingSince = nil
        missingSaid = false
        hide()
        if let captured { CGDisplayRelease(captured) }
        captured = nil
        say("Waiting for the glasses")
    }
}
