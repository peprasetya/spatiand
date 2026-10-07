//  ClickTest.swift — `Spatiand --selftest-click`, through `tools/selftest-click.sh`
//
//  Clicks in a page of Chromium's, in the several ways a click might be made on a window of this Mac, and says which of
//  them the page took as clicks. The page logs what it hears (pointerdown, mousedown, pointerup, mouseup, click, with
//  whether each was trusted and what the buttons were), and a menu that opens on pointerdown, the way the model and
//  effort pickers of the Claude app do, tells whether a press was one.
//
//  It uses the same posts the room does (`MacInput`), at points worked out from where the page says its elements are.
//  It takes the pointer and the front application for a few seconds and gives them back.

import AppKit

enum ClickTest {
    static func run() {
        let env = ProcessInfo.processInfo.environment
        guard let logPath = env["CLICKTEST_LOG"], let layoutPath = env["CLICKTEST_LAYOUT"],
              let layoutData = FileManager.default.contents(atPath: layoutPath),
              let layout = try? JSONSerialization.jsonObject(with: layoutData) as? [String: Any],
              let boxes = layout["boxes"] as? [String: [Double]] else {
            print("run this through tools/selftest-click.sh")
            exit(2)
        }
        let wanted = env["CLICKTEST_VARIANT"] ?? "all"
        func number(_ key: String) -> Double { (layout[key] as? NSNumber)?.doubleValue ?? 0 }
        // Points on the Mac's screen: the page's own coordinates (points, for Chrome) under its title bar.
        func screenPoint(_ id: String) -> CGPoint {
            let b = boxes[id] ?? [0, 0]
            return CGPoint(x: number("sx") + b[0], y: number("sy") + (number("oh") - number("ih")) + b[1])
        }
        func log() -> [String] { ((try? String(contentsOfFile: logPath, encoding: .utf8)) ?? "").split(separator: "\n").map(String.init) }
        func pause(_ seconds: Double) { RunLoop.current.run(until: Date().addingTimeInterval(seconds)) }

        NSApplication.shared.setActivationPolicy(.accessory)
        DispatchQueue.main.async {
            Task {
                let list = await MacWindows.list()
                guard let info = list.first(where: { $0.title.contains("Spatiand click test") }) else {
                    print("the test page's window was not found"); exit(1)
                }
                await MainActor.run { go(info) }
            }
        }

        func go(_ info: MacWindowInfo) {
            let before = NSWorkspace.shared.frontmostApplication
            let home = CGEvent(source: nil)?.location ?? .zero
            var failures = 0
            func front() {
                NSRunningApplication(processIdentifier: info.pid)?.activate()
                for _ in 0..<40 { if NSWorkspace.shared.frontmostApplication?.processIdentifier == info.pid { break }; pause(0.025) }
                MacWindows.raise(info)
                pause(0.15)
            }
            front()
            print("front: \(NSWorkspace.shared.frontmostApplication?.localizedName ?? "?"), window \(info.windowID)")

            func hover(_ p: CGPoint) {
                MacInput.mouse(.mouseMoved, button: .left, at: p, clicks: 1, window: info.windowID, pid: info.pid)
                pause(0.06)
            }
            enum Variant: String, CaseIterable { case room, nowarp, realPointer, held }
            func press(_ p: CGPoint, _ v: Variant) {
                switch v {
                case .room:
                    // As the room does: hover by post, then down and up through the system's stream, the pointer put back
                    // after each.
                    hover(p)
                    MacInput.mouseThrough(.leftMouseDown, button: .left, at: p, clicks: 1)
                    CGWarpMouseCursorPosition(home)
                    pause(0.04)
                    MacInput.mouseThrough(.leftMouseUp, button: .left, at: p, clicks: 1)
                    CGWarpMouseCursorPosition(home)
                case .nowarp:
                    hover(p)
                    MacInput.mouseThrough(.leftMouseDown, button: .left, at: p, clicks: 1)
                    pause(0.04)
                    MacInput.mouseThrough(.leftMouseUp, button: .left, at: p, clicks: 1)
                    CGWarpMouseCursorPosition(home)
                case .realPointer:
                    // The real pointer taken there first, as a hand would.
                    CGWarpMouseCursorPosition(p)
                    MacInput.mouseThrough(.mouseMoved, button: .left, at: p, clicks: 1)
                    pause(0.12)
                    MacInput.mouseThrough(.leftMouseDown, button: .left, at: p, clicks: 1)
                    pause(0.06)
                    MacInput.mouseThrough(.leftMouseUp, button: .left, at: p, clicks: 1)
                    CGWarpMouseCursorPosition(home)
                case .held:
                    hover(p)
                    CGWarpMouseCursorPosition(p)
                    MacInput.mouseThrough(.leftMouseDown, button: .left, at: p, clicks: 1)
                    pause(0.15)
                    MacInput.mouseThrough(.leftMouseUp, button: .left, at: p, clicks: 1)
                    CGWarpMouseCursorPosition(home)
                }
                pause(0.35)
            }

            let variants = Variant.allCases.filter { wanted == "all" || wanted == $0.rawValue }
            for v in variants {
                for (target, expect) in [("a", "a:click"), ("b", "b:menu"), ("c", "c:click")] {
                    let mark = log().count
                    press(screenPoint(target), v)
                    let seen = Array(log().dropFirst(mark))
                    let ok = seen.contains { $0.contains(expect) }
                    print((ok ? "ok    " : "FAIL  ") + "\(v.rawValue) on \(target): " + (ok ? "" : "no \(expect)"))
                    if !ok { failures += 1 }
                    for line in seen where !line.contains("mouseleave") { print("        \(line)") }
                    // Put away what a press opened: the menu is shut by another press on its trigger, a popup by Escape.
                    if target == "b", seen.contains(where: { $0.contains("b:menu block") }) { press(screenPoint("b"), .nowarp) }
                    if target == "c" { MacInput.key(code: 53, flags: [], down: true, pid: info.pid); MacInput.key(code: 53, flags: [], down: false, pid: info.pid); pause(0.2) }
                }
            }
            CGWarpMouseCursorPosition(home)
            before?.activate()
            print(failures == 0 ? "all passed" : "\(failures) failed")
            exit(failures == 0 ? 0 : 1)
        }
        NSApplication.shared.run()
    }
}
