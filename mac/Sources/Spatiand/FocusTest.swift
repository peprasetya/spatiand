//  FocusTest.swift — `Spatiand --selftest-focus <app> <title>`
//
//  A window of this Mac that is in the room is not the one the Mac thinks has the keyboard, so its text
//  shows no caret and its menus do not stay open. This asks what makes it behave as the front window
//  without taking the keyboard away from Spatiand: it clicks in the window, then counts the pictures the
//  window makes in three seconds. A caret that blinks makes some; one that does not leaves it still.

import AppKit
import ApplicationServices
import CoreImage
import Metal
import QuartzCore
import CoreVideo

enum FocusTest {
    static func run(app: String, title: String) {
        let variant = ProcessInfo.processInfo.environment["SPATIAND_TEST_VARIANT"] ?? "none"
        Task {
            guard let info = await MacWindows.list().first(where: { $0.app == app && $0.title.contains(title) }) else { print("no window; have: " + (await MacWindows.list()).map { "\($0.app):\($0.title)" }.joined(separator: " | ")); exit(1) }
            let capture = MacCapture(info)
            var frames = 0
            var seen = Set<UInt64>()
            let lock = NSLock()
            capture.onFrame = { buffer, _ in
                CVPixelBufferLockBaseAddress(buffer, .readOnly)
                defer { CVPixelBufferUnlockBaseAddress(buffer, .readOnly) }
                guard let base = CVPixelBufferGetBaseAddress(buffer) else { return }
                let bytes = base.assumingMemoryBound(to: UInt8.self)
                let count = CVPixelBufferGetBytesPerRow(buffer) * CVPixelBufferGetHeight(buffer)
                var h: UInt64 = 1469598103934665603
                var i = 0
                while i < count { h = (h ^ UInt64(bytes[i])) &* 1099511628211; i += 7 }
                lock.lock(); frames += 1; seen.insert(h); lock.unlock()
            }
            try? await capture.start()
            try? await Task.sleep(nanoseconds: 1_500_000_000)
            if variant == "activate-wait" {
                await MainActor.run { NSRunningApplication(processIdentifier: info.pid)?.activate() }
                try? await Task.sleep(nanoseconds: 1_000_000_000)
            }
            print("focus[\(variant)]: caret before \(MacWindows.caret(info).map(String.init) ?? "?")")
            await MainActor.run {
                switch variant {
                case "activate": NSRunningApplication(processIdentifier: info.pid)?.activate()
                case "ax":
                    if let w = MacWindows.axWindow(info) {
                        AXUIElementSetAttributeValue(w, kAXMainAttribute as CFString, kCFBooleanTrue)
                        AXUIElementSetAttributeValue(w, kAXFocusedAttribute as CFString, kCFBooleanTrue)
                    }
                case "slps": MacInput.makeKey(window: info.windowID, pid: info.pid)
                default: break
                }
                let point = capture.screenPoint(x: capture.frameSizePixels.width * 0.5, y: capture.frameSizePixels.height * 0.4)
                MacInput.mouse(.mouseMoved, button: .left, at: point, clicks: 1, window: info.windowID, pid: info.pid)
                MacInput.mouse(.leftMouseDown, button: .left, at: point, clicks: 1, window: info.windowID, pid: info.pid)
                MacInput.mouse(.leftMouseUp, button: .left, at: point, clicks: 1, window: info.windowID, pid: info.pid)
            }
            try? await Task.sleep(nanoseconds: 500_000_000)
            if ProcessInfo.processInfo.environment["SPATIAND_TEST_DRAG"] != nil {
                // Press, pull across the line, let go: what a person does to select text.
                let (fx, fy) = ProcessInfo.processInfo.environment["SPATIAND_TEST_DRAG"]!.split(separator: ",").compactMap { Double($0) }.reduce(into: [Double]()) { $0.append($1) }.enumerated().reduce(into: (0.4, 0.4)) { r, e in if e.offset == 0 { r.0 = e.element } else { r.1 = e.element } }
                await MainActor.run {
                    let size = capture.frameSizePixels
                    let a = capture.screenPoint(x: size.width * fx, y: size.height * fy)
                    MacInput.mouse(.leftMouseDown, button: .left, at: a, clicks: 1, window: info.windowID, pid: info.pid)
                    for step in 1...12 {
                        let p = CGPoint(x: a.x + CGFloat(step) * 20, y: a.y)
                        MacInput.mouse(.leftMouseDragged, button: .left, at: p, clicks: 1, window: info.windowID, pid: info.pid)
                        Thread.sleep(forTimeInterval: 0.02)
                    }
                    MacInput.mouse(.leftMouseUp, button: .left, at: CGPoint(x: a.x + 240, y: a.y), clicks: 1, window: info.windowID, pid: info.pid)
                }
            }
            print("focus[\(variant)]: caret after \(MacWindows.caret(info).map(String.init) ?? "?")")
            lock.lock(); seen.removeAll(); lock.unlock()
            try? await Task.sleep(nanoseconds: 3_000_000_000)
            lock.lock(); print("focus[\(variant)]: \(seen.count) different pictures in three seconds after the click"); lock.unlock()
            if let w = MacWindows.axWindow(info) {
                func find(_ e: AXUIElement, _ d: Int) -> String? {
                    var v: CFTypeRef?
                    if AXUIElementCopyAttributeValue(e, kAXRoleAttribute as CFString, &v) == .success, (v as? String) == "AXTextArea" {
                        var sel: CFTypeRef?
                        AXUIElementCopyAttributeValue(e, kAXSelectedTextAttribute as CFString, &sel)
                        return "selected: \((sel as? String) ?? "(nothing)")"
                    }
                    guard d < 6, AXUIElementCopyAttributeValue(e, kAXChildrenAttribute as CFString, &v) == .success, let kids = v as? [AXUIElement] else { return nil }
                    for k in kids { if let t = find(k, d + 1) { return t } }
                    return nil
                }
                print("focus[\(variant)]: \(find(w, 0) ?? "no text area")")
            }
            if let (buffer, _) = capture.current() {
                let image = CIImage(cvPixelBuffer: buffer)
                let rep = NSBitmapImageRep(ciImage: image)
                try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: "/tmp/focus-\(variant).png"))
            }
            exit(0)
        }
        RunLoop.main.run()
    }
}

/// `Spatiand --selftest-panel TextEdit spatiand-note`: whether a panel that does not activate its app still
/// hears the pointer and the wheel while another application is the active one with the keyboard.
enum PanelTest {
    final class View: NSView {
        var moves = 0, wheels = 0, downs = 0, keys = 0
        var deltas = 0.0
        override var acceptsFirstResponder: Bool { true }
        override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
        override func updateTrackingAreas() {
            super.updateTrackingAreas()
            trackingAreas.forEach(removeTrackingArea)
            addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseMoved, .activeAlways, .inVisibleRect], owner: self, userInfo: nil))
        }
        override func mouseMoved(with event: NSEvent) { moves += 1; deltas += Double(event.deltaX) }
        override func scrollWheel(with event: NSEvent) { wheels += 1 }
        override func mouseDown(with event: NSEvent) { downs += 1 }
        override func keyDown(with event: NSEvent) { keys += 1 }
    }
    final class Panel: NSPanel {
        override var canBecomeKey: Bool { false }
        override var canBecomeMain: Bool { false }
    }

    static func run(app: String, title: String) {
        let ns = NSApplication.shared
        ns.setActivationPolicy(.accessory)
        guard let screen = NSScreen.main else { exit(1) }
        let panel = Panel(contentRect: screen.frame, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.level = .screenSaver
        panel.isFloatingPanel = true
        panel.hidesOnDeactivate = false
        panel.backgroundColor = NSColor(white: 0, alpha: 0.004)
        panel.isOpaque = false
        panel.acceptsMouseMovedEvents = true
        let view = View(frame: NSRect(origin: .zero, size: screen.frame.size))
        panel.contentView = view
        panel.orderFrontRegardless()
        Task {
            guard let info = await MacWindows.list().first(where: { $0.app == app && $0.title.contains(title) }) else { print("no window"); exit(1) }
            let main = CGDisplayBounds(CGMainDisplayID())
            await MainActor.run {
                NSRunningApplication(processIdentifier: info.pid)?.activate()
                CGWarpMouseCursorPosition(CGPoint(x: main.midX, y: main.midY))
                CGAssociateMouseAndMouseCursorPosition(0)
            }
            try? await Task.sleep(nanoseconds: 1_000_000_000)
            let front = NSWorkspace.shared.frontmostApplication?.localizedName ?? "?"
            for _ in 0..<20 {
                let e = CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: CGPoint(x: main.midX, y: main.midY), mouseButton: .left)
                e?.setIntegerValueField(.mouseEventDeltaX, value: 3)
                e?.setIntegerValueField(.mouseEventDeltaY, value: 1)
                e?.post(tap: .cghidEventTap)
                try? await Task.sleep(nanoseconds: 15_000_000)
            }
            let w = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 2, wheel1: 5, wheel2: 0, wheel3: 0)
            w?.post(tap: .cghidEventTap)
            try? await Task.sleep(nanoseconds: 500_000_000)
            await MainActor.run {
                CGAssociateMouseAndMouseCursorPosition(1)
                print("panel: front is \(front); the panel heard \(view.moves) moves (total delta \(view.deltas)), \(view.wheels) wheel events")
                exit(0)
            }
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 12) { CGAssociateMouseAndMouseCursorPosition(1); exit(2) }
        ns.run()
    }
}

/// `Spatiand --selftest-perf <app> <title>`: the room's frame loop at the display's rate with a window of this Mac in
/// it, offscreen, and where the time of each frame goes.
enum PerfTest {
    static func run(app: String, title: String) {
        let room = Model.shared.room
        let ns = NSApplication.shared
        ns.setActivationPolicy(.accessory)
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) {
            room.core.holdHead(yaw: 0, pitch: 0)
            room.core.perEye(1920, 1080)
            room.setActive(true)
            Task {
                guard let info = await MacWindows.list().first(where: { $0.app == app && $0.title.contains(title) }) else { print("no window"); exit(1) }
                await MainActor.run { room.bringMacWindow(info) }
            }
        }
        guard let renderer = room.renderer else { print("no renderer"); exit(1) }
        let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .bgra8Unorm, width: 3840, height: 1080, mipmapped: false)
        d.usage = [.renderTarget, .shaderRead]
        d.storageMode = .private
        guard let target = renderer.device.makeTexture(descriptor: d) else { exit(1) }
        var t = (aim: 0.0, tick: 0.0, render: 0.0, frames: 0, worst: 0.0)
        var started = Date()
        var last = CACurrentMediaTime()
        var gaps: [Double] = []
        let timer = Timer(timeInterval: 1.0 / 60.0, repeats: true) { _ in
            let a = CACurrentMediaTime()
            _ = room.core.aim()
            let b = CACurrentMediaTime()
            room.tick()
            let c = CACurrentMediaTime()
            renderer.render(into: target, sideBySide: true)
            let e = CACurrentMediaTime()
            gaps.append(a - last); last = a
            t.aim += b - a; t.tick += c - b; t.render += e - c; t.frames += 1; t.worst = max(t.worst, e - a)
            if Date().timeIntervalSince(started) > 5 {
                let n = Double(max(1, t.frames))
                let late = gaps.filter { $0 > 0.025 }.count
                print(String(format: "perf: %d frames in 5 s (%d late); per frame: aim %.2f ms, tick %.2f ms, render %.2f ms; worst %.1f ms",
                             t.frames, late, t.aim / n * 1000, t.tick / n * 1000, t.render / n * 1000, t.worst * 1000))
                t = (0, 0, 0, 0, 0); gaps = []; started = Date()
            }
        }
        RunLoop.main.add(timer, forMode: .common)
        DispatchQueue.main.asyncAfter(deadline: .now() + 22) { exit(0) }
        ns.run()
    }
}

/// `Spatiand --selftest-post <app> <title>`: which way of posting a click at another application's window it takes.
enum PostTest {
    static func run(app: String, title: String) {
        // Spatiand is the active application when the wearer clicks, so the test is too.
        let ns = NSApplication.shared
        ns.setActivationPolicy(.regular)
        let w = NSWindow(contentRect: NSRect(x: 50, y: 50, width: 200, height: 100), styleMask: [.titled], backing: .buffered, defer: false)
        w.makeKeyAndOrderFront(nil)
        ns.activate(ignoringOtherApps: true)
        Task {
            try? await Task.sleep(nanoseconds: 600_000_000)
            print("post: this process is active: \(await MainActor.run { NSApp.isActive })")
            guard let info = await MacWindows.list().first(where: { $0.app == app && $0.title.contains(title) }) else { print("no window"); exit(1) }
            await MainActor.run { NSRunningApplication(processIdentifier: info.pid)?.activate() }
            try? await Task.sleep(nanoseconds: 800_000_000)
            print("post: front application is \(NSWorkspace.shared.frontmostApplication?.localizedName ?? "?")")
            let f = info.frame
            let point = CGPoint(x: f.midX, y: f.midY)
            func click(_ name: String, source: CGEventSource?, fields: Bool, tap: Bool, toPid: Bool) {
                for type in [CGEventType.mouseMoved, .leftMouseDown, .leftMouseUp] {
                    guard let e = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point, mouseButton: .left) else { continue }
                    e.setIntegerValueField(.mouseEventClickState, value: 1)
                    if type != .mouseMoved { e.setDoubleValueField(.mouseEventPressure, value: type == .leftMouseDown ? 1 : 0) }
                    if fields {
                        e.setIntegerValueField(CGEventField(rawValue: 91)!, value: Int64(info.windowID))
                        e.setIntegerValueField(CGEventField(rawValue: 92)!, value: Int64(info.windowID))
                    }
                    if tap { e.post(tap: .cghidEventTap) } else if toPid { e.postToPid(info.pid) }
                    Thread.sleep(forTimeInterval: 0.03)
                }
                print("post: tried \(name)")
                Thread.sleep(forTimeInterval: 0.4)
            }
            // The wheel, as the room posts it.
            MacInput.scrollThrough(dx: 0, dy: -12, at: point)
            Thread.sleep(forTimeInterval: 0.3)
            print("post: tried the wheel")
            // SkyLight's own way of posting at a process.
            typealias SLPost = @convention(c) (pid_t, CGEvent) -> Void
            if let sl = dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight", RTLD_LAZY), let sym = dlsym(sl, "SLEventPostToPid") {
                let post = unsafeBitCast(sym, to: SLPost.self)
                for (name, source) in [("sl/hid", CGEventSource(stateID: .hidSystemState)), ("sl/private", CGEventSource(stateID: .privateState))] {
                    for type in [CGEventType.mouseMoved, .leftMouseDown, .leftMouseUp] {
                        guard let e = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point, mouseButton: .left) else { continue }
                        e.setIntegerValueField(.mouseEventClickState, value: 1)
                        e.setIntegerValueField(CGEventField(rawValue: 91)!, value: Int64(info.windowID))
                        e.setIntegerValueField(CGEventField(rawValue: 92)!, value: Int64(info.windowID))
                        post(info.pid, e)
                        Thread.sleep(forTimeInterval: 0.03)
                    }
                    print("post: tried \(name)")
                    Thread.sleep(forTimeInterval: 0.4)
                }
            }
            click("private/fields/pid", source: CGEventSource(stateID: .privateState), fields: true, tap: false, toPid: true)
            click("hid/fields/pid", source: CGEventSource(stateID: .hidSystemState), fields: true, tap: false, toPid: true)
            click("hid/nofields/pid", source: CGEventSource(stateID: .hidSystemState), fields: false, tap: false, toPid: true)
            click("nil/nofields/pid", source: nil, fields: false, tap: false, toPid: true)
            click("combined/nofields/pid", source: CGEventSource(stateID: .combinedSessionState), fields: false, tap: false, toPid: true)
            click("hid/nofields/hidtap", source: CGEventSource(stateID: .hidSystemState), fields: false, tap: true, toPid: false)
            exit(0)
        }
        ns.run()
    }
}
