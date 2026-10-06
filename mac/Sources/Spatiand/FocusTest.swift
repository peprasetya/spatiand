//  FocusTest.swift — `Spatiand --selftest-focus <app> <title>`
//
//  A window of this Mac that is in the room is not the one the Mac thinks has the keyboard, so its text
//  shows no caret and its menus do not stay open. This asks what makes it behave as the front window
//  without taking the keyboard away from Spatiand: it clicks in the window, then counts the pictures the
//  window makes in three seconds. A caret that blinks makes some; one that does not leaves it still.

import AppKit
import ApplicationServices
import CoreImage
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
