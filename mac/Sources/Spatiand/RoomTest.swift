//  RoomTest.swift — `Spatiand --selftest-room <address> <fingerprint> <app>`
//
//  Puts a host's windows into the room with no glasses on: the head is held where it is told, the
//  room is drawn for two eyes into an offscreen image, and the pointer is driven by hand. What
//  it writes is what the glasses would be shown, so it can be looked at.
//
//  `SPATIAND_TEST_HEAD=yaw,pitch` holds the head turned; `SPATIAND_TEST_CLICK=1` clicks what the
//  cursor is on, to see the host answer.

import AppKit

enum RoomTest {
    static func run(address: String, fingerprint: String, app: String) {
        let model = Model.shared
        let room = model.room
        model.onProblem = { print("problem: \($0)") }
        // The hint test is of an empty room, so it has no computer to take windows from.
        if ProcessInfo.processInfo.environment["SPATIAND_TEST_HINT"] == nil, ProcessInfo.processInfo.environment["SPATIAND_TEST_MAC"] == nil {
            model.connect(PairedHost(name: "test", address: address, fingerprint: fingerprint))
        }

        func after(_ seconds: Double, _ body: @escaping () -> Void) {
            DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: body)
        }
        let env = ProcessInfo.processInfo.environment
        let head = (env["SPATIAND_TEST_HEAD"] ?? "0,0").split(separator: ",").compactMap { Double($0) }

        if let appName = env["SPATIAND_TEST_MAC"] {
            // A window of this Mac, in the room: the first one of the named application.
            after(1) {
                room.core.holdHead(yaw: 0, pitch: 0)
                room.core.perEye(1920, 1080)
                room.setActive(true)
            }
            after(2) {
                Task {
                    let list = await MacWindows.list()
                    print("mac windows: \(list.count) listed")
                    guard let info = list.first(where: { $0.app == appName && $0.title.contains(env["SPATIAND_TEST_TITLE"] ?? "") }) else { print("no window of \(appName)"); exit(1) }
                    await MainActor.run { room.bringMacWindow(info) }
                }
            }
            after(4) {
                print("accessibility trusted: \(AXIsProcessTrusted())")
                guard env["SPATIAND_TEST_MACINPUT"] != nil, let id = room.macWindowIDs.first, let info = room.macInfo(id) else { return }
                // Click the window, type into it, and make it bigger: all through the room's own paths.
                room.core.centrePointer()
                room.buttonDown(0x110, grab: false); room.buttonUp(0x110)
                for code: CGKeyCode in [4, 34] { MacInput.key(code: code, flags: [], down: true, pid: info.pid); MacInput.key(code: code, flags: [], down: false, pid: info.pid) }
                let before = MacWindows.currentFrame(info)
                MacWindows.resize(info, toPoints: CGSize(width: 820, height: 500))
                after(1) { print("mac window size: \(before.map { "\(Int($0.width))x\(Int($0.height))" } ?? "?") -> \(MacWindows.currentFrame(info).map { "\(Int($0.width))x\(Int($0.height))" } ?? "?")") }
                after(1.5) { print("text now: \(MacWindows.text(info) ?? "?")") }
            }
            if env["SPATIAND_TEST_KEYMODE"] != nil {
                // The room takes the mouse and keyboard with a window of this Mac in front: that window's
                // application should then be the active one, and Spatiand's panel should still be there.
                after(4.5) { room.input.start() }
                after(5) { room.tick() }
                after(7) {
                    let front = NSWorkspace.shared.frontmostApplication?.localizedName ?? "?"
                    print("keymode: capturing \(room.input.capturing), the front application is \(front)")
                    room.input.stop()
                }
            }
            after(8) {
                let a = room.core.aim()
                print("aim: window \(a.window.map { String($0, radix: 16) } ?? "none") at \(Int(a.x)),\(Int(a.y))")
                room.tick(); if let image = room.renderer?.snapshot(width: 3840, height: 1080, sideBySide: true) { write(image, "/tmp/spatiand-room-mac.png") }
                exit(0)
            }
            return
        }
        if env["SPATIAND_TEST_HINT"] != nil {
            after(4) {
                room.core.holdHead(yaw: 0, pitch: 0)
                room.core.perEye(1920, 1080)
                room.setActive(true)
            }
            after(6) {
                room.tick(); if let image = room.renderer?.snapshot(width: 3840, height: 1080, sideBySide: true) { write(image, "/tmp/spatiand-room-hint.png") }
                exit(0)
            }
            return
        }
        after(2) { if let a = model.apps.first(where: { $0.id == app }) { model.launch(a) } else { print("no such app: \(model.apps.map(\.id))") } }
        after(5) {
            room.core.holdHead(yaw: head.first ?? 0, pitch: head.count > 1 ? head[1] : 0)
            room.core.perEye(1920, 1080)
            room.setActive(true)
            print("room: \(room.known.count) windows known, renderer \(room.renderer != nil)")
        }
        after(8) {
            let aim = room.core.aim()
            print("aim: window \(aim.window.map(String.init) ?? "none") at \(Int(aim.x)),\(Int(aim.y))")
            room.tick(); if let image = room.renderer?.snapshot(width: 3840, height: 1080, sideBySide: true) {
                write(image, "/tmp/spatiand-room.png")
            }
            room.tick(); if let image = room.renderer?.snapshot(width: 1920, height: 1080, sideBySide: false) {
                write(image, "/tmp/spatiand-room-mono.png")
            }
            if env["SPATIAND_TEST_INPUT"] != nil {
                // Real NSEvents through the room's own input window, as the OS would hand them.
                room.drivesGlasses = false
                let input = room.input
                input.start()
                func post(_ cg: CGEvent?) { if let cg, let e = NSEvent(cgEvent: cg) { NSApp.sendEvent(e) } }
                let src = CGEventSource(stateID: .privateState)
                room.core.centrePointer()
                // The page's address bar is near the top of the window: steer there by mouse deltas.
                for _ in 0..<8 {
                    let a = room.core.aim()
                    guard a.window != nil else { break }
                    let e = CGEvent(mouseEventSource: src, mouseType: .mouseMoved, mouseCursorPosition: .zero, mouseButton: .left)
                    e?.setIntegerValueField(.mouseEventDeltaX, value: Int64((400 - a.x) * 0.49))
                    e?.setIntegerValueField(.mouseEventDeltaY, value: Int64((86 - a.y) * 0.49))
                    post(e)
                }
                let a = room.core.aim()
                print("input test: aiming at \(Int(a.x)),\(Int(a.y))")
                post(CGEvent(mouseEventSource: src, mouseType: .leftMouseDown, mouseCursorPosition: .zero, mouseButton: .left))
                post(CGEvent(mouseEventSource: src, mouseType: .leftMouseUp, mouseCursorPosition: .zero, mouseButton: .left))
                for code: CGKeyCode in [4, 34] {   // h, i
                    post(CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: true))
                    post(CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: false))
                }
                input.stop()
            }
            if env["SPATIAND_TEST_GESTURE"] != nil {
                room.gesture(.swipe(.up, fingers: 3))
                print("three fingers up: menu open \(room.menu.isOpen)")
                room.gesture(.swipe(.down, fingers: 3))
                print("three fingers down: menu open \(room.menu.isOpen)")
                let before = room.core.aim().window
                room.gesture(.swipe(.left, fingers: 4))
                room.gesture(.swipe(.left, fingers: 4))
                let after = room.core.aim()
                print("four fingers left twice: aimed window \(before.map(String.init) ?? "none") -> \(after.window.map(String.init) ?? "none")")
                room.gesture(.pinch(fingers: 5, spreading: false))
                let gathered = room.core.aim()
                print("five fingers together: aimed window \(gathered.window.map(String.init) ?? "none") at \(Int(gathered.x)),\(Int(gathered.y))")
                exit(0)
            }
            if env["SPATIAND_TEST_RESIZE"] != nil {
                // Take the right edge of the window's frame and pull it out; the application is asked for
                // more pixels at the same density, and the window grows with them.
                guard let id = room.known.keys.min() else { print("no window"); exit(1) }
                room.core.centrePointer()
                var a = room.core.aim()
                var steps = 0
                while a.zone != .right && steps < 200 {
                    room.pointerMoved(dx: 6, dy: 0)
                    a = room.core.aim()
                    steps += 1
                }
                print("edge aim: window \(a.window.map(String.init) ?? "none") zone \(a.zone) after \(steps) steps; window \(room.known[id].map { "\(Int($0.width))x\(Int($0.height))" } ?? "?")")
                room.buttonDown(0x110, grab: false)
                for _ in 0..<8 { room.pointerMoved(dx: 10, dy: 0); Thread.sleep(forTimeInterval: 0.15) }
                room.buttonUp(0x110)
                after(2) {
                    print("after the pull: \(room.known[id].map { "\(Int($0.width))x\(Int($0.height))" } ?? "?")")
                    room.tick(); if let image = room.renderer?.snapshot(width: 3840, height: 1080, sideBySide: true) { write(image, "/tmp/spatiand-room-resize.png") }
                    exit(0)
                }
                return
            }
            if env["SPATIAND_TEST_BAR"] != nil {
                // Onto the title bar's close button, to see it lit; then the middle of the bar, to take
                // the window and carry it to the right.
                room.core.centrePointer()
                room.pointerMoved(dx: 318, dy: -226)
                let a = room.core.aim()
                print("bar aim: window \(a.window.map(String.init) ?? "none") zone \(a.zone) at \(Int(a.x)),\(Int(a.y))")
                room.tick()
                if let image = room.renderer?.snapshot(width: 3840, height: 1080, sideBySide: true) { write(image, "/tmp/spatiand-room-bar1.png") }
                room.pointerMoved(dx: -250, dy: 0)
                room.buttonDown(0x110, grab: false)
                print("grabbing: \(room.core.isGrabbing)")
                room.pointerMoved(dx: 200, dy: 0)
                room.core.centrePointer()
                room.buttonUp(0x110)
                room.tick()
                if let image = room.renderer?.snapshot(width: 3840, height: 1080, sideBySide: true) { write(image, "/tmp/spatiand-room-bar2.png") }
                exit(0)
            }
            if env["SPATIAND_TEST_MENU"] != nil {
                if env["SPATIAND_TEST_MENU"] == "launcher" { room.toggleMenu() } else { room.toggleSettings() }
                room.tick()
                // Up and to the left a little, onto the first rows.
                room.core.centrePointer()
                room.pointerMoved(dx: 150, dy: -50)
                let a = room.core.aim()
                print("menu aim: window \(a.window.map { String($0, radix: 16) } ?? "none") at \(Int(a.x)),\(Int(a.y))")
                room.tick(); if let image = room.renderer?.snapshot(width: 3840, height: 1080, sideBySide: true) { write(image, "/tmp/spatiand-room-menu.png") }
                exit(0)
            }
            // Move the cursor right and down and look again.
            room.pointerMoved(dx: -300, dy: 120)
            let moved = room.core.aim()
            print("aim after moving: window \(moved.window.map(String.init) ?? "none") at \(Int(moved.x)),\(Int(moved.y))")
            if let spec = env["SPATIAND_TEST_CLICK"] {
                // "x,y" in the window's pixels: steer the pointer there, as a mouse would, and click.
                let target = spec.split(separator: ",").compactMap { Double($0) }
                if target.count == 2 {
                    room.core.centrePointer()
                    for _ in 0..<6 {
                        let a = room.core.aim()
                        guard a.window != nil else { break }
                        room.pointerMoved(dx: (target[0] - a.x) * 0.49, dy: (target[1] - a.y) * 0.49)
                    }
                    let a = room.core.aim()
                    print("click at \(Int(a.x)),\(Int(a.y)) in window \(a.window.map(String.init) ?? "none")")
                }
                room.buttonDown(0x110, grab: false)
                room.buttonUp(0x110)
            }
        }
        after(env["SPATIAND_TEST_RESIZE"] != nil ? 40 : 11) {
            room.tick(); if let image = room.renderer?.snapshot(width: 3840, height: 1080, sideBySide: true) {
                write(image, "/tmp/spatiand-room-2.png")
            }
            exit(0)
        }
    }

    private static func write(_ image: CGImage, _ path: String) {
        let rep = NSBitmapImageRep(cgImage: image)
        try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
        print("wrote \(path): \(image.width)x\(image.height)")
    }
}
