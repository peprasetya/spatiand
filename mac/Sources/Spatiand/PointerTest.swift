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
    override func mouseDown(with event: NSEvent) { say("C", event) }
    override func mouseDragged(with event: NSEvent) { say("D", event) }
    override func mouseUp(with event: NSEvent) { say("U", event) }

    private func say(_ what: String, _ event: NSEvent) {
        guard let window else { return }
        // From the window's top left, title bar and all: what a picture of the window counts in.
        let line = String(format: "%@ %.1f %.1f %.0f %.0f\n", what, event.locationInWindow.x, window.frame.height - event.locationInWindow.y, window.frame.width, window.frame.height)
        if let handle = FileHandle(forWritingAtPath: file) {
            handle.seekToEndOfFile()
            handle.write(line.data(using: .utf8)!)
            handle.closeFile()
        } else {
            try? line.write(toFile: file, atomically: true, encoding: .utf8)
        }
    }
}

/// A window with no title bar that may still have the keys, as a bubble or a popover has.
final class KeyableWindow: NSWindow {
    override var canBecomeKey: Bool { true }
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
        // With --with-child, something hung beside the window after a while, sticking out past
        // its right and bottom edges, as a browser's profile menu does: clicks in it are written too.
        if CommandLine.arguments.contains("--with-child") {
            DispatchQueue.main.asyncAfter(deadline: .now() + 9) {
                let child = KeyableWindow(contentRect: NSRect(x: window.frame.maxX - 120, y: window.frame.minY - 160, width: 320, height: 300), styleMask: [.borderless], backing: .buffered, defer: false)
                let inside = ClickTargetView(frame: NSRect(x: 0, y: 0, width: 320, height: 300))
                inside.file = file + ".child"
                child.contentView = inside
                window.addChildWindow(child, ordered: .above)
                // With --child-leaves it is as a browser's bubble is: it has the keys, and goes
                // the moment it loses them -- as it does when the window it hangs from is
                // brought forward. A click in it that is never heard is that having happened.
                if CommandLine.arguments.contains("--child-leaves") {
                    child.makeKeyAndOrderFront(nil)
                    NotificationCenter.default.addObserver(forName: NSWindow.didResignKeyNotification, object: child, queue: .main) { _ in
                        try? "gone at \(Date().timeIntervalSince1970), the application \(NSApp.isActive ? "active" : "not active"), the key window \(NSApp.keyWindow?.title ?? "none")\n".write(toFile: file + ".left", atomically: true, encoding: .utf8)
                        child.orderOut(nil)
                    }
                    return
                }
                child.orderFront(nil)
                // With --child-goes it is taken away again, from a window that is otherwise still.
                if CommandLine.arguments.contains("--child-goes") {
                    DispatchQueue.main.asyncAfter(deadline: .now() + 4) { window.removeChildWindow(child); child.orderOut(nil) }
                }
            }
        }
        // With --with-tab, a second tab after a while, chosen: to the Mac another window, in
        // the same place, with the first gone from the screen.
        if CommandLine.arguments.contains("--with-tab") {
            DispatchQueue.main.asyncAfter(deadline: .now() + 9) {
                let second = NSWindow(contentRect: window.frame, styleMask: [.titled, .resizable, .closable], backing: .buffered, defer: false)
                second.title = title + " two"
                second.contentView = ClickTargetView(frame: NSRect(x: 0, y: 0, width: 640, height: 420))
                window.addTabbedWindow(second, ordered: .above)
                second.makeKeyAndOrderFront(nil)
                DispatchQueue.main.asyncAfter(deadline: .now() + 5) { window.makeKeyAndOrderFront(nil) }
            }
        }
        application.run()
        exit(0)
    }

    private static func after(_ seconds: Double, _ body: @escaping () -> Void) { DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: body) }

    /// Travel of the mouse, as the mouse's own: not marked, so the tap takes it for a hand's.
    /// How far the mouse has travelled since the room's pointer was last known to be in the window.
    static var strayed = (0.0, 0.0)

    private static func travel(_ dx: Double, _ dy: Double) {
        strayed = (strayed.0 + dx.rounded(), strayed.1 + dy.rounded())
        let here = CGEvent(source: nil)?.location ?? .zero
        // Where a hand's would say the cursor now is: moved by as much.
        let there = CGPoint(x: here.x + CGFloat(dx.rounded()), y: here.y + CGFloat(dy.rounded()))
        guard let event = CGEvent(mouseEventSource: nil, mouseType: held ? .leftMouseDragged : .mouseMoved, mouseCursorPosition: there, mouseButton: .left) else { return }
        event.setIntegerValueField(.mouseEventDeltaX, value: Int64(dx.rounded()))
        event.setIntegerValueField(.mouseEventDeltaY, value: Int64(dy.rounded()))
        event.post(tap: .cghidEventTap)
    }

    private static var held = false

    private static func button(_ down: Bool) {
        held = down
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
        // --popup: the target hangs a bubble beside itself, and the bubble is what is clicked.
        let popup = CommandLine.arguments.contains("--popup")
        for extra in [".child", ".left"] { try? FileManager.default.removeItem(atPath: file + extra) }
        helper.arguments = ["--click-target", file] + (popup ? ["--with-child", "--child-leaves"] : [])
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
                            if popup {
                                // The bubble comes nine seconds after the target does; it hangs 200
                                // points past the right edge and 160 below the bottom one.
                                after(6) {
                                    let size = MacWindows.currentFrame(info)?.size ?? info.frame.size
                                    let scale = Double(NSScreen.main?.backingScaleFactor ?? 2)
                                    let corner = ((Double(size.width) - 120) * scale, (Double(size.height) - 140) * scale)
                                    let places = [(150.0, 250.0), (260.0, 120.0), (60.0, 60.0)].map { CGPoint(x: corner.0 + $0.0 * scale, y: corner.1 + $0.1 * scale) }
                                    round(info, "in what hangs beside the window", [], pixels: places, into: file + ".child", from: corner) {
                                        if let left = try? String(contentsOfFile: file + ".left", encoding: .utf8) { print("pointer: FAILED, what hung beside the window went away: \(left)"); failures += 1 }
                                        finish()
                                    }
                                }
                                return
                            }
                            round(info, "as it opened", grid) {
                              round(info, "before the sweep", [(0.15, 0.5)]) {
                               sweep(info) {
                               round(info, "before the drag", [(0.2, 0.6)]) {
                               drag(info) {
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
                    }
                }
            }
        }

        /// The mouse moved steadily across the window, two points a hundredth of a second: the
        /// room's pointer should cross it as steadily, neither sticking nor leaping.
        func sweep(_ info: MacWindowInfo, then: @escaping () -> Void) {
            var samples: [(Double, Double)] = []
            let started = Date()
            var sent = 0
            func tick() {
                if let at = RoomWindows.shared.pointerIn(info.windowID) { samples.append((Date().timeIntervalSince(started), Double(at.x))); PointerTest.strayed = (0, 0) }
                guard sent < 100 else {
                    after(0.3) {
                        var stall = 0.0, leap = 0.0, lastChange = 0.0, last = samples.first?.1 ?? 0
                        for (t, x) in samples where x != last {
                            stall = max(stall, t - lastChange)
                            leap = max(leap, abs(x - last))
                            lastChange = t
                            last = x
                        }
                        let crossed = (samples.last?.1 ?? 0) - (samples.first?.1 ?? 0)
                        let good = stall < 0.1 && leap < 22 && crossed > 100
                        if !good { failures += 1 }
                        print(String(format: "pointer: %@ a steady sweep, 200 points of travel in 1 s crossed %.0f px of the window; longest it stood still %.0f ms, biggest single move %.0f px", good ? "PASSED" : "FAILED", crossed, stall * 1000, leap))
                        then()
                    }
                    return
                }
                sent += 1
                travel(2, 0)
                after(0.01) { tick() }
            }
            tick()
        }

        /// The button held and the mouse carried across the window, as text is selected: the
        /// window should hear a drag that ends where the room's pointer does, and the pointer
        /// should go as steadily as it does with nothing held.
        func drag(_ info: MacWindowInfo, then: @escaping () -> Void) {
            func lines(_ kind: String) -> [[Double]] {
                ((try? String(contentsOfFile: file, encoding: .utf8)) ?? "").split(separator: "\n").filter { $0.hasPrefix(kind + " ") }
                    .map { $0.split(separator: " ").dropFirst().compactMap { Double($0) } }
            }
            let dragsBefore = lines("D").count
            let scale = Double(NSScreen.main?.backingScaleFactor ?? 2)
            let from = RoomWindows.shared.pointerIn(info.windowID)
            var xs: [Double] = []
            button(true)
            var sent = 0
            func tick() {
                if let at = RoomWindows.shared.pointerIn(info.windowID) { xs.append(Double(at.x)) }
                guard sent < 60 else {
                    after(0.3) {
                        let end = RoomWindows.shared.pointerIn(info.windowID)
                        button(false)
                        after(0.5) {
                            let drags = lines("D").dropFirst(dragsBefore)
                            var leap = 0.0
                            for i in 1..<max(1, xs.count) { leap = max(leap, abs(xs[i] - xs[i - 1])) }
                            let crossed = Double((end?.x ?? 0) - (from?.x ?? 0))
                            let lastDrag = drags.last.map { $0[0] * scale } ?? -1
                            let good = drags.count >= 10 && leap < 30 && crossed > 60 && abs(lastDrag - Double(end?.x ?? 0)) <= 6 && !lines("U").isEmpty
                            if !good { failures += 1 }
                            print(String(format: "pointer: %@ a drag, 120 points of travel with the button held crossed %.0f px (biggest single move %.0f px); the window heard %d drags, the last at %.0f where the pointer was at %.0f, and %@", good ? "PASSED" : "FAILED", crossed, leap, drags.count, lastDrag, Double(end?.x ?? 0), lines("U").isEmpty ? "never heard the button come up" : "the button come up"))
                            then()
                        }
                    }
                    return
                }
                sent += 1
                travel(2, 0)
                after(0.02) { tick() }
            }
            after(0.3) { tick() }
        }

        /// Point at each place in turn and click it; `then` when all are done.
        /// Or, with `pixels`, at those places of the picture, the clicks written to `into` by
        /// something whose top left is at `from` in the picture.
        func round(_ info: MacWindowInfo, _ what: String, _ places: [(Double, Double)], pixels: [CGPoint]? = nil, into: String? = nil, from: (Double, Double) = (0, 0), then: @escaping () -> Void) {
            let size = MacWindows.currentFrame(info)?.size ?? info.frame.size
            let scale = Double(NSScreen.main?.backingScaleFactor ?? 2)
            let file = into ?? file
            var left = pixels ?? places.map { CGPoint(x: $0.0 * Double(size.width) * scale, y: $0.1 * Double(size.height) * scale) }
            func next() {
                guard !left.isEmpty else { then(); return }
                let wanted = left.removeFirst()
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
                    if at != nil { PointerTest.strayed = (0, 0) }
                    guard let at else {
                        // Not in the window: stirred, which is also what brings the laser back.
                        // Back the way it came, if it was in; else stirred, which shows the laser.
                        // Or back toward where it last was in the window, by the travel since.
                        if let last = steps.popLast() { travel(-last.0, -last.1) }
                        else if hypot(strayed.0, strayed.1) > 4 { let n = hypot(strayed.0, strayed.1); travel(-strayed.0 / n * 6, -strayed.1 / n * 6) }
                        else { travel(tries % 2 == 0 ? 3 : -3, 0) }
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
                        func clicks() -> [Substring] { ((try? String(contentsOfFile: file, encoding: .utf8)) ?? "").split(separator: "\n").filter { $0.hasPrefix("C ") } }
                        let before = clicks().count
                        if into != nil { print("pointer: pressing at \(Date().timeIntervalSince1970)") }
                        button(true)
                        after(0.08) { button(false) }
                        after(0.6) {
                            let lines = clicks()
                            guard lines.count > before, let last = lines.last else {
                                print("pointer: FAILED \(what), the click at \(Int(wanted.x)),\(Int(wanted.y)) never reached the window")
                                failures += 1
                                next()
                                return
                            }
                            let got = last.split(separator: " ").dropFirst().compactMap { Double($0) }
                            let (gx, gy) = (got[0] * scale + from.0, got[1] * scale + from.1)
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
