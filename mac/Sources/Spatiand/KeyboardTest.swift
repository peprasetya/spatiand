//  KeyboardTest.swift — `Spatiand --selftest-keyboard`
//
//  The on-screen keyboard from the settings list, as on the Deck: the Keyboard row shows it, the same row puts it away,
//  and what it looks like goes to /tmp/spatiand-keyboard-*.png. (What a press on a key does is tested in the room's own
//  tests, where the pointer can be steered exactly.)

import AppKit
import CSpatiand

enum KeyboardTest {
    static func run() {
        let room = Model.shared.room
        var failures = 0
        func check(_ name: String, _ ok: Bool) {
            print((ok ? "ok    " : "FAIL  ") + name)
            if !ok { failures += 1 }
        }
        func card() -> [String: Any] {
            guard let json = sp_shell_card(room.core.handle) else { return [:] }
            defer { sp_free_string(json) }
            return (try? JSONSerialization.jsonObject(with: Data(String(cString: json).utf8))) as? [String: Any] ?? [:]
        }
        func rows() -> [String] { (card()["rows"] as? [[Any]] ?? []).compactMap { $0.first as? String } }
        func cursor() -> Int { card()["cursor"] as? Int ?? -1 }
        func goTo(_ label: String) -> Bool {
            guard let target = rows().firstIndex(of: label) else { return false }
            var n = 0
            while cursor() != target, n < 60 { room.menu.intent(cursor() < target ? .down : .up); n += 1 }
            return cursor() == target
        }
        func panels() -> Set<UInt32> {
            var ids = [UInt32](repeating: 0, count: 64)
            let count = sp_shell_panel_ids(room.core.handle, &ids, ids.count)
            return Set(ids.prefix(count))
        }
        func snapshot(_ name: String) {
            for _ in 0..<4 { room.tick(); RunLoop.main.run(until: Date().addingTimeInterval(0.15)) }
            room.menu.update()
            if let image = room.renderer?.snapshot(width: 3840, height: 1080, sideBySide: true) {
                try? NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: "/tmp/spatiand-keyboard-\(name).png"))
                print("wrote /tmp/spatiand-keyboard-\(name).png")
            }
        }
        func after(_ seconds: Double, _ body: @escaping () -> Void) { DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: body) }
        after(0.5) {
            room.core.holdHead(yaw: 0, pitch: 0)
            room.core.perEye(1920, 1080)
            room.setActive(true)
            room.core.useDiskEnvironments()
        }
        after(1.5) {
            check("no keyboard to begin with", !panels().contains(0xFFF8))
            room.toggleSettings()
            check("the settings list has a Keyboard row", goTo("Keyboard"))
            room.menu.intent(.accept)
            check("choosing it shows the keyboard", panels().contains(0xFFF8) && !room.menu.isOpen)
            snapshot("shown")
            room.toggleSettings()
            _ = goTo("Keyboard")
            room.menu.intent(.accept)
            check("choosing it again puts it away", !panels().contains(0xFFF8))
 
            // A radial menu of a controller layout, round the middle of the view.
            sp_room_set_radial(room.core.handle, "[\"Copy\",\"Paste\",\"Undo\",\"Redo\",\"Select all\",\"Find\"]", 2)
            room.menu.update()
            check("a radial menu is six plates", panels().filter { $0 >= 0xFF50 && $0 < 0xFF60 }.count == 6)
            snapshot("radial")
            sp_room_set_radial(room.core.handle, nil, -1)
            room.menu.update()
            check("and is gone when it closes", panels().filter { $0 >= 0xFF50 && $0 < 0xFF60 }.isEmpty)
            print(failures == 0 ? "all passed" : "\(failures) failed")
            exit(failures == 0 ? 0 : 1)
        }
        NSApplication.shared.setActivationPolicy(.accessory)
        NSApplication.shared.run()
    }
}
