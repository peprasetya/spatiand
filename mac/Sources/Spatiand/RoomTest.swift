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
        model.connect(PairedHost(name: "test", address: address, fingerprint: fingerprint))

        func after(_ seconds: Double, _ body: @escaping () -> Void) {
            DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: body)
        }
        let env = ProcessInfo.processInfo.environment
        let head = (env["SPATIAND_TEST_HEAD"] ?? "0,0").split(separator: ",").compactMap { Double($0) }

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
            if env["SPATIAND_TEST_CLICK"] != nil {
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
