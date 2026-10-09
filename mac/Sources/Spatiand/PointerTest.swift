//  PointerTest.swift — the room's pointer and a real window of this Mac's, tested end to end.
//
//  `--selftest-pointer`: a window that says where it was clicked is opened (this program again,
//  as `--click-target`), brought into the room, and pointed at and clicked through the same path
//  a hand takes: events posted as the mouse's own, taken by the event tap, turned into the room's
//  pointer by the compositor, and come back as a click on the real window. What the window says
//  it got is compared with where the room's pointer was. Then the window is resized and it is
//  done again. **Moves the Mac's real pointer and clicks with it: not for while the Mac is in use.**

import AppKit
import CSpatiand

final class ClickTargetView: NSView {
    var file = ""
    override func draw(_ dirtyRect: NSRect) {
        NSColor(white: 0.14, alpha: 1).setFill()
        bounds.fill()
        NSColor(white: 0.4, alpha: 1).setStroke()
        for x in stride(from: 0.0, to: Double(bounds.width), by: 50) { NSBezierPath.strokeLine(from: NSPoint(x: x, y: 0), to: NSPoint(x: x, y: Double(bounds.height))) }
        for y in stride(from: 0.0, to: Double(bounds.height), by: 50) { NSBezierPath.strokeLine(from: NSPoint(x: 0, y: y), to: NSPoint(x: Double(bounds.width), y: y)) }
    }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func mouseDown(with event: NSEvent) {
        guard let window else { return }
        // From the window's top left, title bar and all: what a picture of the window counts in.
        let line = String(format: "%.1f %.1f %.0f %.0f\n", event.locationInWindow.x, window.frame.height - event.locationInWindow.y, window.frame.width, window.frame.height)
        if let handle = FileHandle(forWritingAtPath: file) {
            handle.seekToEndOfFile()
            handle.write(line.data(using: .utf8)!)
            handle.closeFile()
        } else {
            try? line.write(toFile: file, atomically: true, encoding: .utf8)
        }
    }
}

enum PointerTest {
    static let title = "Spatiand click target"

    /// The window that is clicked. Runs until it is terminated.
    static func target(file: String) -> Never {
        let application = NSApplication.shared
        application.setActivationPolicy(.regular)
        let window = NSWindow(contentRect: NSRect(x: 240, y: 900, width: 640, height: 420), styleMask: [.titled, .resizable, .closable], backing: .buffered, defer: false)
        window.title = title
        let view = ClickTargetView(frame: NSRect(x: 0, y: 0, width: 640, height: 420))
        view.file = file
        view.autoresizingMask = [.width, .height]
        window.contentView = view
        window.makeKeyAndOrderFront(nil)
        application.activate(ignoringOtherApps: true)
        application.run()
        exit(0)
    }

    private static func after(_ seconds: Double, _ body: @escaping () -> Void) { DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: body) }

    /// Travel of the mouse, as the mouse's own: not marked, so the tap takes it for a hand's.
    private static func travel(_ dx: Double, _ dy: Double) {
        let here = CGEvent(source: nil)?.location ?? .zero
        guard let event = CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: here, mouseButton: .left) else { return }
        event.setIntegerValueField(.mouseEventDeltaX, value: Int64(dx.rounded()))
        event.setIntegerValueField(.mouseEventDeltaY, value: Int64(dy.rounded()))
        event.post(tap: .cghidEventTap)
    }

    private static func button(_ down: Bool) {
        let here = CGEvent(source: nil)?.location ?? .zero
        CGEvent(mouseEventSource: nil, mouseType: down ? .leftMouseDown : .leftMouseUp, mouseCursorPosition: here, mouseButton: .left)?.post(tap: .cghidEventTap)
    }

    static func run() -> Never {
        setenv("SPATIAND_HMD", "null", 1)
        setenv("XDG_CONFIG_HOME", NSTemporaryDirectory() + "spatiand-preview", 0)
        let file = NSTemporaryDirectory() + "spatiand-clicks.txt"
        try? FileManager.default.removeItem(atPath: file)
        let helper = Process()
        helper.executableURL = Bundle.main.executableURL
        helper.arguments = ["--click-target", file]
        try? helper.run()
        let application = NSApplication.shared
        application.setActivationPolicy(.accessory)
        var failures = 0
        func finish() {
            RoomTap.shared.hold(false)
            helper.terminate()
            print(failures == 0 ? "pointer: ALL PASSED" : "pointer: \(failures) FAILED")
            after(0.5) { Room.shared.stop(); exit(failures == 0 ? 0 : 1) }
        }
        DispatchQueue.main.async {
            Room.shared.start(preview: true)
            after(1.5) { sp_control(3, true); after(0.1) { sp_control(3, false) } }
            after(3) {
                Task {
                    guard let info = await MacWindows.list().first(where: { $0.title == title }) else {
                        print("pointer: FAILED, the target window is not on this Mac's list")
                        failures += 1
                        DispatchQueue.main.async { finish() }
                        return
                    }
                    RoomWindows.shared.bring(info)
                    DispatchQueue.main.async {
                        after(3) {
                            guard RoomTap.shared.install() else { print("pointer: FAILED, no event tap"); failures += 1; finish(); return }
                            RoomTap.shared.hold(true)
                            let grid: [(Double, Double)] = [(0.5, 0.5), (0.2, 0.3), (0.8, 0.3), (0.2, 0.85), (0.8, 0.85), (0.5, 0.2)]
                            round(info, "as it opened", grid) {
                                MacWindows.resize(info, toPoints: CGSize(width: 860, height: 340))
                                after(2.5) {
                                    round(info, "after a resize to 860x340", grid) {
                                        MacWindows.resize(info, toPoints: CGSize(width: 420, height: 520))
                                        after(2.5) { round(info, "after a resize to 420x520", grid) { finish() } }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        /// Point at each place in turn and click it; `then` when all are done.
        func round(_ info: MacWindowInfo, _ what: String, _ places: [(Double, Double)], then: @escaping () -> Void) {
            let size = MacWindows.currentFrame(info)?.size ?? info.frame.size
            let scale = Double(NSScreen.main?.backingScaleFactor ?? 2)
            var left = places
            func next() {
                guard !left.isEmpty else { then(); return }
                let place = left.removeFirst()
                let wanted = CGPoint(x: place.0 * Double(size.width) * scale, y: place.1 * Double(size.height) * scale)
                var tries = 0
                var settled = 0
                var steps: [(Double, Double)] = []
                func aim() {
                    tries += 1
                    let at = RoomWindows.shared.pointerIn(info.windowID)
                    guard tries < 900 else {
                        print("pointer: FAILED \(what), could not bring the pointer to \(Int(wanted.x)),\(Int(wanted.y)); it is at \(at.map { "\(Int($0.x)),\(Int($0.y))" } ?? "no place in the window")")
                        failures += 1
                        next()
                        return
                    }
                    guard let at else {
                        // Not in the window: stirred, which is also what brings the laser back.
                        // Back the way it came, if it was in; else stirred, which shows the laser.
                        if let last = steps.popLast() { travel(-last.0, -last.1) } else { travel(tries % 2 == 0 ? 3 : -3, 0) }
                        after(0.06) { aim() }
                        return
                    }
                    let (ex, ey) = (Double(wanted.x - at.x), Double(wanted.y - at.y))
                    if abs(ex) <= 2, abs(ey) <= 2, settled < 3 {
                        // Held still for a moment first: where it is said to be is a little old.
                        settled += 1
                        after(0.08) { aim() }
                        return
                    }
                    if abs(ex) <= 2, abs(ey) <= 2 {
                        let before = (try? String(contentsOfFile: file, encoding: .utf8))?.split(separator: "\n").count ?? 0
                        button(true)
                        after(0.08) { button(false) }
                        after(0.6) {
                            let lines = (try? String(contentsOfFile: file, encoding: .utf8))?.split(separator: "\n") ?? []
                            guard lines.count > before, let last = lines.last else {
                                print("pointer: FAILED \(what), the click at \(Int(wanted.x)),\(Int(wanted.y)) never reached the window")
                                failures += 1
                                next()
                                return
                            }
                            let got = last.split(separator: " ").compactMap { Double($0) }
                            let (gx, gy) = (got[0] * scale, got[1] * scale)
                            let error = hypot(gx - Double(at.x), gy - Double(at.y))
                            let good = error <= 4
                            if !good { failures += 1 }
                            print(String(format: "pointer: %@ %@, pointed at %.0f,%.0f and the window was clicked at %.0f,%.0f (%.1f px out; the window is %.0fx%.0f)", good ? "PASSED" : "FAILED", what, at.x, at.y, gx, gy, error, got[2], got[3]))
                            next()
                        }
                        return
                    }
                    settled = 0
                    func step(_ e: Double) -> Double { abs(e) <= 2 ? 0 : (e > 0 ? 1 : -1) * min(12, max(1, abs(e) * 0.15)).rounded() }
                    let s = (step(ex), step(ey))
                    steps.append(s)
                    if steps.count > 12 { steps.removeFirst() }
                    travel(s.0, s.1)
                    after(0.05) { aim() }
                }
                aim()
            }
            next()
        }
        application.run()
        exit(0)
    }
}
