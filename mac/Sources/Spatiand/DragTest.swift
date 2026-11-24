//  DragTest.swift — a drag that ends in a window of this Mac's, tested end to end.
//
//  `--selftest-drag`: two windows of this program's own are opened (it again, as `--drag-source`
//  and `--drop-target`), one over the other on the Mac's screen, and brought into the room. Then
//    1. files said to be dragged in the room over the target (as the room says of SpatiShelf's)
//       are let go of there: the target must be given them (`MacDragOut`);
//    2. the same, taken away again and not let go of: the target must be given nothing;
//    3. a file dragged out of the source by the room's pointer, moved as fingers move it, across
//       the room onto the target and let go of: the target must be given it (`RoomWindows.dragOver`);
//    4. the same with the mouse's own travel.
//  **Moves the Mac's real pointer and drags with it: not for while the Mac is in use.**

import AppKit
import CSpatiand

private func note(_ line: String, to file: String) {
    let line = line + "\n"
    if let handle = FileHandle(forWritingAtPath: file) {
        handle.seekToEndOfFile()
        handle.write(line.data(using: .utf8)!)
        handle.closeFile()
    } else {
        try? line.write(toFile: file, atomically: true, encoding: .utf8)
    }
}

/// A window's inside from which a file is dragged, by a press and a movement, as from Finder.
private final class DragSourceView: NSView, NSDraggingSource {
    var file = ""
    private var began = false
    override func draw(_ dirtyRect: NSRect) {
        NSColor(red: 0.16, green: 0.3, blue: 0.5, alpha: 1).setFill()
        bounds.fill()
    }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func mouseDown(with event: NSEvent) { began = false }
    override func mouseDragged(with event: NSEvent) {
        guard !began else { return }
        began = true
        let item = NSDraggingItem(pasteboardWriter: URL(fileURLWithPath: file) as NSURL)
        let icon = NSWorkspace.shared.icon(forFile: file)
        item.setDraggingFrame(NSRect(x: convert(event.locationInWindow, from: nil).x - 16, y: convert(event.locationInWindow, from: nil).y - 16, width: 32, height: 32), contents: icon)
        beginDraggingSession(with: [item], event: event, source: self)
    }
    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation { .copy }
}

/// A window's inside that takes files, and writes down what it was given.
private final class DropTargetView: NSView {
    var report = ""
    override func draw(_ dirtyRect: NSRect) {
        NSColor(red: 0.45, green: 0.25, blue: 0.12, alpha: 1).setFill()
        bounds.fill()
    }
    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation { note("E", to: report); return .copy }
    override func draggingUpdated(_ sender: NSDraggingInfo) -> NSDragOperation { .copy }
    override func draggingExited(_ sender: NSDraggingInfo?) { note("X", to: report) }
    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        let urls = sender.draggingPasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
        for url in urls { note("P \(url.path)", to: report) }
        return !urls.isEmpty
    }
}

enum DragTest {
    static let sourceTitle = "Spatiand drag source", targetTitle = "Spatiand drop target"

    private static func window(_ title: String, at origin: NSPoint, width: CGFloat = 520, _ view: NSView) -> Never {
        let application = NSApplication.shared
        application.setActivationPolicy(.regular)
        let window = NSWindow(contentRect: NSRect(x: origin.x, y: origin.y, width: width, height: 360), styleMask: [.titled, .resizable, .closable], backing: .buffered, defer: false)
        window.title = title
        view.frame = NSRect(x: 0, y: 0, width: width, height: 360)
        view.autoresizingMask = [.width, .height]
        window.contentView = view
        window.makeKeyAndOrderFront(nil)
        application.activate(ignoringOtherApps: true)
        application.run()
        exit(0)
    }

    static func source(file: String) -> Never {
        let view = DragSourceView()
        view.file = file
        // The room opens both straight ahead, one before the other: the source is the wider, so
        // that its ends show either side of the target.
        window(sourceTitle, at: NSPoint(x: 140, y: 500), width: 1000, view)
    }

    /// Nearly where the source is: on the Mac's screen one lies over the other.
    static func target(report: String) -> Never {
        let view = DropTargetView()
        view.report = report
        view.registerForDraggedTypes([.fileURL])
        window(targetTitle, at: NSPoint(x: 300, y: 470), width: 400, view)
    }

    private static func after(_ seconds: Double, _ body: @escaping () -> Void) { DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: body) }

    // The pointer moved as fingers on the trackpad move it (the room's own), or as a mouse does
    // (the mouse's own events, which the tap takes for a hand's).
    private static var held = false
    private static var byMouse = false

    private static func travel(_ dx: Double, _ dy: Double) {
        if !byMouse { sp_pointer(Float(dx), Float(dy), held ? 1 : 0, 0, 0); return }
        let here = CGEvent(source: nil)?.location ?? .zero
        let there = CGPoint(x: here.x + CGFloat(dx.rounded()), y: here.y + CGFloat(dy.rounded()))
        guard let event = CGEvent(mouseEventSource: nil, mouseType: held ? .leftMouseDragged : .mouseMoved, mouseCursorPosition: there, mouseButton: .left) else { return }
        event.setIntegerValueField(.mouseEventDeltaX, value: Int64(dx.rounded()))
        event.setIntegerValueField(.mouseEventDeltaY, value: Int64(dy.rounded()))
        event.post(tap: .cghidEventTap)
    }

    private static func button(_ down: Bool) {
        held = down
        if !byMouse { sp_pointer(0, 0, down ? 1 : 0, 0, 0); return }
        let here = CGEvent(source: nil)?.location ?? .zero
        CGEvent(mouseEventSource: nil, mouseType: down ? .leftMouseDown : .leftMouseUp, mouseCursorPosition: here, mouseButton: .left)?.post(tap: .cghidEventTap)
    }

    static func run() -> Never {
        setenv("SPATIAND_HMD", "null", 1)
        setenv("XDG_CONFIG_HOME", NSTemporaryDirectory() + "spatiand-preview", 0)
        let dragged = NSTemporaryDirectory() + "spatiand-dragged.txt"
        let report = NSTemporaryDirectory() + "spatiand-dropped.txt"
        try? "dragged\n".write(toFile: dragged, atomically: true, encoding: .utf8)
        try? FileManager.default.removeItem(atPath: report)
        let helpers = [["--drag-source", dragged], ["--drop-target", report]].map { arguments -> Process in
            let helper = Process()
            helper.executableURL = Bundle.main.executableURL
            helper.arguments = arguments
            try? helper.run()
            return helper
        }
        let application = NSApplication.shared
        application.setActivationPolicy(.accessory)
        var failures = 0
        func finish() {
            button(false)
            RoomTap.shared.hold(false)
            helpers.forEach { $0.terminate() }
            print(failures == 0 ? "drag: ALL PASSED" : "drag: \(failures) FAILED")
            after(0.5) { Room.shared.stop(); exit(failures == 0 ? 0 : 1) }
        }
        func said() -> [Substring] { ((try? String(contentsOfFile: report, encoding: .utf8)) ?? "").split(separator: "\n") }
        func given() -> Bool { said().contains { $0 == "P \(dragged)" || $0 == "P /private\(dragged)" } }
        func check(_ what: String, _ good: Bool) {
            if !good { failures += 1 }
            print("drag: \(good ? "PASSED" : "FAILED") \(what); the target wrote: \(said().joined(separator: " | "))")
            try? FileManager.default.removeItem(atPath: report)
        }
        let scale = Double(NSScreen.main?.backingScaleFactor ?? 2)
        func pixels(_ info: MacWindowInfo) -> CGSize {
            let size = MacWindows.currentFrame(info)?.size ?? info.frame.size
            return CGSize(width: Double(size.width) * scale, height: Double(size.height) * scale)
        }

        /// The pointer taken into the window -- across the room, to and fro, further each time --
        /// and then to `place` of it (a part of its width and height); `then(false)` if it never
        /// got there.
        func bring(to info: MacWindowInfo, _ place: (Double, Double), then: @escaping (Bool) -> Void) {
            let size = pixels(info)
            let wanted = CGPoint(x: place.0 * Double(size.width), y: place.1 * Double(size.height))
            var tries = 0, settled = 0, leg = 0, gone = 0
            func aim() {
                tries += 1
                guard tries < 1500 else { then(false); return }
                if tries % 100 == 0 {
                    var vx: Float = 0, vy: Float = 0
                    sp_pointer_where(&vx, &vy)
                    print("drag: try \(tries) for \(info.title): the pointer is at \(vx), \(vy) of the view, \(RoomWindows.shared.pointerIn(info.windowID).map { "in it at \(Int($0.x)),\(Int($0.y))" } ?? "not in it")")
                }
                guard let at = RoomWindows.shared.pointerIn(info.windowID) else {
                    // Not in it: across, 60 steps one way, 120 back the other, 180 again.
                    let length = 60 * (leg + 1)
                    travel(leg % 2 == 0 ? 10 : -10, 0)
                    gone += 1
                    if gone >= length { gone = 0; leg += 1 }
                    after(0.03) { aim() }
                    return
                }
                let (ex, ey) = (Double(wanted.x - at.x), Double(wanted.y - at.y))
                if abs(ex) <= 6, abs(ey) <= 6 {
                    settled += 1
                    if settled >= 4 { then(true); return }
                    after(0.08) { aim() }
                    return
                }
                settled = 0
                func step(_ e: Double) -> Double { abs(e) <= 3 ? 0 : (e > 0 ? 1 : -1) * min(12, max(1, abs(e) * 0.15)).rounded() }
                travel(step(ex), step(ey))
                after(0.05) { aim() }
            }
            aim()
        }

        /// A file dragged out of the source by the pointer, across the room onto the target,
        /// and let go of there.
        func across(_ source: MacWindowInfo, _ target: MacWindowInfo, _ how: String, then: @escaping () -> Void) {
            sp_screenshot()
            bring(to: source, (0.08, 0.55)) { there in
                guard there else {
                    var vx: Float = 0, vy: Float = 0
                    sp_pointer_where(&vx, &vy)
                    sp_screenshot()
                    check("\(how): the pointer could not be brought into the source (it is at \(vx), \(vy) of the view)", false); then(); return
                }
                button(true)
                var nudges = 0
                func nudge() {
                    nudges += 1
                    travel(4, 0)
                    if nudges < 8 { after(0.04) { nudge() }; return }
                    after(0.3) {
                        bring(to: target, (0.5, 0.55)) { there in
                            after(0.5) {
                                button(false)
                                after(2) {
                                    check("\(how): a file dragged out of one window of this Mac's, across the room, and let go of over another\(there ? "" : " (the pointer never came to the target)")", there && given())
                                    then()
                                }
                            }
                        }
                    }
                }
                after(0.3) { nudge() }
            }
        }

        DispatchQueue.main.async {
            Room.shared.start(preview: true)
            after(1.5) { sp_control(3, true); after(0.1) { sp_control(3, false) } }
            after(3) {
                Task {
                    let all = await MacWindows.list()
                    guard let source = all.first(where: { $0.title == sourceTitle }), let target = all.first(where: { $0.title == targetTitle }) else {
                        print("drag: FAILED, the test's windows are not on this Mac's list")
                        failures += 1
                        DispatchQueue.main.async { finish() }
                        return
                    }
                    RoomWindows.shared.bring(source)
                    RoomWindows.shared.bring(target)
                    DispatchQueue.main.async {
                        after(4) {
                            guard RoomTap.shared.install() else { print("drag: FAILED, no event tap"); failures += 1; finish(); return }
                            RoomTap.shared.hold(true)
                            guard let id = RoomWindows.shared.roomID(of: target.windowID) else { print("drag: FAILED, the target is not in the room"); failures += 1; finish(); return }
                            let size = pixels(target)
                            let files = "file://" + dragged
                            /// The room's files said to be over the target, from its left to its middle.
                            func over(then: @escaping () -> Void) {
                                var step = 0
                                func tick() {
                                    let x = Double(size.width) * (0.2 + 0.03 * Double(step))
                                    RoomWindows.shared.askedFiles(Int32(SP_FILES_OVER), id: id, a: x, b: Double(size.height) * 0.55, text: step == 0 ? files : "")
                                    step += 1
                                    if step <= 10 { after(0.04) { tick() } } else { after(0.3) { then() } }
                                }
                                tick()
                            }
                            over {
                                RoomWindows.shared.askedFiles(Int32(SP_FILES_DROP), id: id, a: 0, b: 0, text: "")
                                after(2) {
                                    check("files of the room's let go of over a window of this Mac's", given())
                                    over {
                                        RoomWindows.shared.askedFiles(Int32(SP_FILES_LEAVE), id: 0, a: 0, b: 0, text: "")
                                        after(2) {
                                            check("files of the room's taken away again are given to nothing", !given())
                                            byMouse = false
                                            across(source, target, "as fingers move the pointer") {
                                                byMouse = true
                                                across(source, target, "as a mouse moves it") { finish() }
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
        application.run()
        exit(0)
    }
}
