//  GlassesOutput.swift — the room, on the glasses.
//
//  Three jobs, all about one pair of glasses on one cable: listen to their sensors and give the
//  head to the room; put the glasses into their side-by-side mode so each eye has its own half
//  of the picture; and keep a window drawn on their display, a frame for every refresh.
//
//  Much of this is HoloFrame's, which found out how macOS treats this display the hard way: it is
//  matched by its EDID, a window placed on it before AppKit's screen list has caught up lands on
//  the wrong screen, a window that was on it when it vanished leaves a ghost in the window server
//  until the process ends (so unplugging restarts the app), and nothing is composited over a
//  display that has not been captured. See HoloFrame's README.

import AppKit
import CoreVideo
import Metal
import QuartzCore

/// A window AppKit is not allowed to move: it constrains new frames to the screens it knows of,
/// which right after a display appears is a stale list.
final class GlassesWindow: NSWindow {
    override func constrainFrameRect(_ frameRect: NSRect, to screen: NSScreen?) -> NSRect { frameRect }
}

final class GlassesOutput: NSObject {
    private let room: RoomController
    private var device: XRealDevice?
    private var window: GlassesWindow?
    private let layer = CAMetalLayer()
    private var link: CADisplayLink?
    private var watchdog: Timer?
    private var starting = false
    private var captured: CGDirectDisplayID?
    private(set) var displayID: CGDirectDisplayID = 0
    /// Two eyes across the display, or one eye over it.
    private(set) var sideBySide = false
    private var lastAim: (UInt16?, Int, Int) = (nil, 0, 0)

    /// Said, on the main thread, once the display has gone away for good.
    var onLost: (() -> Void)?
    /// Said when the state worth showing in the menu changes.
    var onChange: (() -> Void)?
    private(set) var status = "Waiting for the glasses"

    init(room: RoomController) {
        self.room = room
        super.init()
    }

    var isRunning: Bool { window != nil }
    private(set) var hasHadAWindow = false

    // MARK: finding the glasses

    /// XREAL Air identifies itself with EDID vendor 0x3647, product 0x3132.
    static func findDisplay() -> CGDirectDisplayID? {
        var count: UInt32 = 0
        var ids = [CGDirectDisplayID](repeating: 0, count: 16)
        guard CGGetActiveDisplayList(16, &ids, &count) == .success else { return nil }
        return ids.prefix(Int(count)).first { CGDisplayVendorNumber($0) == 0x3647 && CGDisplayModelNumber($0) == 0x3132 }
    }

    private func setStatus(_ text: String) {
        status = text
        onChange?()
    }

    // MARK: starting

    /// Bring the glasses up: sensors first, then the mode, then the window. Safe to call again
    /// while it is already going.
    func start() {
        guard !starting, window == nil else { return }
        starting = true
        modeAsked = false
        stereoWanted = Settings.glassesStereo
        connectDevice()
        beginMode(attempt: 0)
    }

    private var imuTried = 0
    /// Whether the glasses have been asked for their 3D mode this time; asked once, not each time round.
    private var modeAsked = false

    private func connectDevice() {
        guard device == nil else { return }
        do {
            let d = try XRealDevice()
            let core = room.core
            try d.startIMU { sample in
                core.imu(timestamp: sample.timestamp, gyro: sample.gyro, accel: sample.accel, mag: sample.mag)
            }
            // The button on the temple: where you are looking becomes the middle.
            d.startButtons { [weak self] msgid, _ in
                guard msgid == 0x6C05 || msgid == 0x6C04 else { return }
                self?.room.recentre()
            }
            device = d
            print("glasses: head tracking on")
        } catch {
            imuTried += 1
            if imuTried == 1 { print("glasses: no head tracking yet (\(error))") }
            // The sensor's USB interfaces can turn up a moment after the display does.
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak self] in
                guard let self, self.starting || self.window != nil else { return }
                self.connectDevice()
            }
        }
    }

    /// Ask for side-by-side, and wait for the display to offer it. Falls back to one eye.
    private func beginMode(attempt: Int) {
        guard starting else { return }
        guard let id = Self.findDisplay() else {
            if attempt > 40 { starting = false; setStatus("The glasses' display did not appear"); return }
            setStatus("Waiting for the glasses' display")
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in self?.beginMode(attempt: attempt + 1) }
            return
        }
        guard Settings.glassesStereo, let device else {
            if device == nil, Settings.glassesStereo, attempt < 40 {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in self?.beginMode(attempt: attempt + 1) }
                return
            }
            print("glasses: staying in one eye (\(Settings.glassesStereo ? "no sensor link" : "3D is off"))")
            open(on: id, stereo: false)
            return
        }
        if !modeAsked {
            modeAsked = true
            setStatus("Switching the glasses to 3D")
            do { try device.setDisplayMode(.sbs3D60) } catch { print("glasses: could not change the mode: \(error)") }
        }
        if let wide = Self.wideMode(of: id) {
            // macOS goes back to the 1920 wide mode by itself when the display returns; ask for the
            // double-width one for this session.
            var config: CGDisplayConfigRef?
            CGBeginDisplayConfiguration(&config)
            CGConfigureDisplayWithDisplayMode(config, id, wide, nil)
            CGCompleteDisplayConfiguration(config, .forSession)
            waitForWidth(id, tries: 0)
            return
        }
        if attempt > 24 {
            print("glasses: no 3D mode was offered; drawing one eye")
            open(on: id, stereo: false)
            return
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in self?.beginMode(attempt: attempt + 1) }
    }

    private static func wideMode(of id: CGDirectDisplayID) -> CGDisplayMode? {
        let options = [kCGDisplayShowDuplicateLowResolutionModes: true] as CFDictionary
        let modes = (CGDisplayCopyAllDisplayModes(id, options) as? [CGDisplayMode]) ?? []
        return modes.first { $0.pixelWidth == 3840 && $0.pixelHeight == 1080 && abs($0.refreshRate - 60) < 1 }
            ?? modes.first { $0.pixelWidth == 3840 && $0.pixelHeight == 1080 }
    }

    private func waitForWidth(_ id: CGDirectDisplayID, tries: Int) {
        guard starting else { return }
        let width = Self.findDisplay().map { CGDisplayBounds($0).width } ?? 0
        if width >= 3800, let now = Self.findDisplay() {
            // Let it hold still before building on it.
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { [weak self] in
                guard let self, self.starting else { return }
                self.open(on: Self.findDisplay() ?? now, stereo: true)
            }
            return
        }
        if tries > 20 {
            print("glasses: the display stayed one eye wide")
            open(on: Self.findDisplay() ?? id, stereo: false)
            return
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self] in self?.waitForWidth(id, tries: tries + 1) }
    }

    // MARK: the window

    private static func appKitFrame(of id: CGDirectDisplayID) -> NSRect {
        let bounds = CGDisplayBounds(id)
        let primary = CGDisplayBounds(CGMainDisplayID()).height
        return NSRect(x: bounds.origin.x, y: primary - bounds.maxY, width: bounds.width, height: bounds.height)
    }

    private static func screenID(_ screen: NSScreen) -> CGDirectDisplayID {
        (screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value ?? 0
    }

    private func open(on id: CGDirectDisplayID, stereo: Bool) {
        guard starting, let renderer = room.renderer else { return }
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
        guard frame.width > 1, frame.height > 1 else { starting = false; setStatus("The glasses' display is not usable"); return }

        displayID = id
        sideBySide = stereo && frame.width >= 3000
        room.core.perEye(sideBySide ? Int(frame.width / 2) : Int(frame.width), Int(frame.height))

        // Take the display from the desktop: nothing else may appear on it, and nothing is
        // composited over a display that has not been captured.
        if CGDisplayCapture(id) == .success { captured = id } else { print("glasses: could not take the display over") }

        let w = GlassesWindow(contentRect: frame, styleMask: .borderless, backing: .buffered, defer: false)
        w.isReleasedWhenClosed = false
        w.level = captured != nil ? NSWindow.Level(rawValue: Int(CGShieldingWindowLevel())) : .screenSaver
        w.backgroundColor = .black
        w.isOpaque = true
        w.ignoresMouseEvents = true      // the mouse and keyboard are taken by RoomInput, not this
        w.collectionBehavior = [.canJoinAllSpaces, .stationary, .fullScreenNone]

        layer.device = renderer.device
        layer.pixelFormat = .bgra8Unorm
        layer.framebufferOnly = true
        layer.maximumDrawableCount = 3
        layer.displaySyncEnabled = true
        layer.frame = CGRect(origin: .zero, size: frame.size)
        layer.drawableSize = frame.size
        let view = NSView(frame: NSRect(origin: .zero, size: frame.size))
        view.wantsLayer = true
        view.layer?.backgroundColor = NSColor.black.cgColor
        view.layer?.addSublayer(layer)
        w.contentView = view
        w.orderFrontRegardless()
        w.setFrame(frame, display: true)
        window = w
        hasHadAWindow = true

        let tick = view.displayLink(target: self, selector: #selector(draw))
        tick.add(to: .main, forMode: .common)
        link = tick

        let dog = Timer(timeInterval: 0.2, repeats: true) { [weak self] _ in self?.check() }
        RunLoop.main.add(dog, forMode: .common)
        watchdog = dog

        starting = false
        if let path = ProcessInfo.processInfo.environment["SPATIAND_DEBUG_SNAPSHOT"] { watchSnapshots(path) }
        setStatus(sideBySide ? "In the glasses, in 3D" : "In the glasses, one eye")
        print("glasses: drawing \(Int(frame.width))x\(Int(frame.height)) on display \(id), \(sideBySide ? "two eyes" : "one eye")")
    }

    @objc private func draw() {
        guard let renderer = room.renderer, window != nil, let drawable = layer.nextDrawable() else { return }
        // Where the cursor is changes when the head turns, with the mouse still.
        let aim = room.core.aim()
        let now = (aim.window, Int(aim.x), Int(aim.y))
        if now != lastAim { lastAim = now; room.pointerChanged() }
        room.tick()
        renderer.render(into: drawable.texture, sideBySide: sideBySide, present: drawable)
    }

    /// `SPATIAND_DEBUG_SNAPSHOT=/tmp/x.png`: every few seconds, what is being drawn, written out, and
    /// where the head is. For checking the glasses path with nobody wearing them.
    private func watchSnapshots(_ path: String) {
        var n = 0
        let t = Timer(timeInterval: 4, repeats: true) { [weak self] _ in
            guard let self, self.window != nil else { return }
            n += 1
            let h = self.room.core.headDegrees
            print(String(format: "glasses: head yaw %.1f pitch %.1f roll %.1f, %d frames drawn, input captured: %@",
                         h.yaw, h.pitch, h.roll, self.room.renderer?.framesDrawn ?? 0, self.room.input.capturing ? "yes" : "no"))
            if let image = self.room.renderer?.snapshot(width: Int(CGDisplayBounds(self.displayID).width), height: 1080, sideBySide: self.sideBySide) {
                let rep = NSBitmapImageRep(cgImage: image)
                try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path.replacingOccurrences(of: ".png", with: "-\(n % 3).png")))
            }
        }
        RunLoop.main.add(t, forMode: .common)
    }

    // MARK: the display going away

    private var lastModeTry = Date.distantPast
    private var stereoWanted = false

    private func check() {
        guard window != nil else { return }
        let here = Self.findDisplay() != nil && CGDisplayIsActive(displayID) != 0
        if !here {
            hideNow()
            onLost?()
            return
        }
        // macOS puts the display back to its remembered one-eye mode when it feels like it, and
        // keeps it there until asked again; so a display that is not the width we are drawn for is
        // put right, and a window drawn for another width is drawn again.
        let width = CGDisplayBounds(displayID).width
        if stereoWanted, width < 3000, Date().timeIntervalSince(lastModeTry) > 1.0, let wide = Self.wideMode(of: displayID) {
            lastModeTry = Date()
            var config: CGDisplayConfigRef?
            CGBeginDisplayConfiguration(&config)
            CGConfigureDisplayWithDisplayMode(config, displayID, wide, nil)
            CGCompleteDisplayConfiguration(config, .forSession)
        }
        if let frame = window?.frame, abs(frame.width - width) > 1 {
            print("glasses: the display is now \(Int(width)) wide; drawing again")
            let id = displayID
            hideNow()
            if let captured { CGDisplayRelease(captured) }
            captured = nil
            starting = true
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { [weak self] in
                guard let self, self.starting else { return }
                self.open(on: Self.findDisplay() ?? id, stereo: self.stereoWanted)
            }
        }
    }

    /// Off the glasses this instant: before AppKit moves a window whose display has gone to the
    /// Mac's own screen.
    func hideNow() {
        link?.invalidate()
        link = nil
        watchdog?.invalidate()
        watchdog = nil
        layer.removeFromSuperlayer()
        window?.contentView = nil
        window?.orderOut(nil)
        window?.close()
        window = nil
        for case let w as GlassesWindow in NSApp.windows { w.orderOut(nil) }
    }

    func stop(restoreMode: Bool = true) {
        starting = false
        hideNow()
        if let captured { CGDisplayRelease(captured) }
        captured = nil
        if let device {
            if restoreMode, Self.findDisplay() != nil { try? device.setDisplayMode(.mono1080p60) }
            device.stopButtons()
            device.stopIMU()
        }
        device = nil
        imuTried = 0
        setStatus("Waiting for the glasses")
    }
}
