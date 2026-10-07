//  ControllerTest.swift — `Spatiand --selftest-controller`
//
//  The controller layout editor in the room, as on the Deck: the settings list's Controller layout row opens the
//  Deck's own editor on the layout in force, drawn as a card with the controller picture above it; the D-pad walks
//  it, A opens a control, B goes back, and a layout that was changed is saved where the Deck keeps its own.
//  Run through `tools/selftest-scratch.sh`, which keeps the wearer's layouts out of it.
//
//  Pictures of what the glasses would show go to /tmp/spatiand-controller-*.png.

import AppKit
import CSpatiand

enum ControllerTest {
    static func run() {
        let env = ProcessInfo.processInfo.environment
        guard let layouts = env["SPATIAND_LAYOUTS"], layouts.contains("selftest") else {
            print("run this through tools/selftest-scratch.sh: it must not touch the real layouts")
            exit(2)
        }
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
        func title() -> String { card()["title"] as? String ?? "" }
        func press(_ i: RoomShell.Intent) { room.menu.intent(i) }
        func goTo(_ label: String) -> Bool {
            guard let target = rows().firstIndex(of: label) else { return false }
            var n = 0
            while cursor() != target, n < 60 { press(cursor() < target ? .down : .up); n += 1 }
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
                try? NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: "/tmp/spatiand-controller-\(name).png"))
                print("wrote /tmp/spatiand-controller-\(name).png")
            }
        }

        func after(_ seconds: Double, _ body: @escaping () -> Void) { DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: body) }
        after(0.5) {
            room.core.holdHead(yaw: 0, pitch: 0)
            room.core.perEye(1920, 1080)
            room.setActive(true)
            PadInput.shared.start()
        }
        after(1.5) {
            room.toggleSettings()
            check("the settings list has Controller layout", goTo("Controller layout"))
            press(.accept)
            check("A opens the editor, as a menu of its own", room.menu.isEditing && room.menu.isOpen)
            print("note  \(title()): \(rows())")
            check("it has rows to walk", rows().count >= 3)
            check("and the controller picture is above them", panels().contains(0xFFF7))
            snapshot("editor")

            // Walk down, in, and back out: nothing is changed, so nothing is saved.
            press(.down)
            check("the D-pad walks the rows", cursor() == 1)
            let first = title()
            press(.accept)
            let inner = title()
            press(.back)
            check("A goes in and B comes back (\(first) / \(inner))", title() == first)
            press(.up)
            check("up goes back up", cursor() == 0)
            press(.back)
            check("B at the top closes the editor and the menu", !room.menu.isEditing)
            check("the menu is gone, and with it the picture", !room.menu.isOpen && !panels().contains(0xFFF7))
            let saved = (try? FileManager.default.contentsOfDirectory(atPath: layouts)) ?? []
            check("nothing was changed, so nothing was written", saved.isEmpty)
            print(failures == 0 ? "all passed" : "\(failures) failed")
            exit(failures == 0 ? 0 : 1)
        }
        NSApplication.shared.setActivationPolicy(.accessory)
        NSApplication.shared.run()
    }
}
