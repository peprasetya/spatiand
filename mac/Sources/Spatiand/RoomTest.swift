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
                    guard let info = list.first(where: { $0.app == appName }) else { print("no window of \(appName)"); exit(1) }
                    await MainActor.run { room.bringMacWindow(info) }
                }
            }
            after(6) {
                let a = room.core.aim()
                print("aim: window \(a.window.map { String($0, radix: 16) } ?? "none") at \(Int(a.x)),\(Int(a.y))")
                if let image = room.renderer?.snapshot(width: 3840, height: 1080, sideBySide: true) { write(image, "/tmp/spatiand-room-mac.png") }
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
                if let image = room.renderer?.snapshot(width: 3840, height: 1080, sideBySide: true) { write(image, "/tmp/spatiand-room-hint.png") }
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
            if let image = room.renderer?.snapshot(width: 3840, height: 1080, sideBySide: true) {
                write(image, "/tmp/spatiand-room.png")
            }
            if let image = room.renderer?.snapshot(width: 1920, height: 1080, sideBySide: false) {
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
            if env["SPATIAND_TEST_MENU"] != nil {
                room.menu.open()
                // Up and to the left a little, onto the first rows.
                room.core.centrePointer()
                room.pointerMoved(dx: 150, dy: -50)
                let a = room.core.aim()
                print("menu aim: window \(a.window.map { String($0, radix: 16) } ?? "none") at \(Int(a.x)),\(Int(a.y))")
                if let image = room.renderer?.snapshot(width: 3840, height: 1080, sideBySide: true) { write(image, "/tmp/spatiand-room-menu.png") }
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
        after(11) {
            if let image = room.renderer?.snapshot(width: 3840, height: 1080, sideBySide: true) {
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
