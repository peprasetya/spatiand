//  LauncherTest.swift — `Spatiand --selftest-launcher`
//
//  The launcher's search, driven the way a keyboard drives it: the launcher is opened in a room with no
//  glasses, the keys go through the menu's own key handling, and what is left on show is read back. Nothing
//  of the Mac is touched but the list of installed applications, which is read. Pictures of the launcher
//  as the glasses would show it (real glass, both eyes) go to /tmp/spatiand-launcher-*.png.

import AppKit
import CSpatiand

enum LauncherTest {
    static func run() {
        let room = Model.shared.room
        var failures = 0
        func check(_ name: String, _ ok: Bool) {
            print((ok ? "ok    " : "FAIL  ") + name)
            if !ok { failures += 1 }
        }
        func state() -> [String: Any] {
            guard let json = sp_shell_launcher(room.core.handle) else { return [:] }
            defer { sp_free_string(json) }
            return (try? JSONSerialization.jsonObject(with: Data(String(cString: json).utf8))) as? [String: Any] ?? [:]
        }
        func key(_ code: UInt16, _ text: String) {
            let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil, characters: text, charactersIgnoringModifiers: text, isARepeat: false, keyCode: code)!
            _ = room.menu.key(event)
        }
        func type(_ text: String) {
            let codes: [Character: UInt16] = ["a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "o": 31, "u": 32, "i": 34, "p": 35, "l": 37, "j": 38, "k": 40, "n": 45, "m": 46, " ": 49]
            for c in text { key(codes[c] ?? 0, String(c)) }
        }
        func count() -> Int { state()["count"] as? Int ?? -1 }
        func names() -> [String] { state()["shown"] as? [String] ?? [] }
        func write(_ name: String) {
            room.menu.update(); room.tick()
            guard let image = room.renderer?.snapshot(width: 3840, height: 1080, sideBySide: true) else { return }
            let path = "/tmp/spatiand-launcher-\(name).png"
            try? NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
            print("wrote \(path)")
        }

        func after(_ seconds: Double, _ body: @escaping () -> Void) { DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: body) }
        after(0.5) {
            room.core.holdHead(yaw: 0, pitch: 0)
            room.core.perEye(1920, 1080)
            room.setActive(true)
            room.toggleMenu()   // the launcher
        }
        // The scan of the installed applications runs off the main thread; give it its moment.
        after(4) {
            let all = state()
            check("the launcher is open", (all["mode"] as? String) == "Launcher")
            let total = all["searched"] as? Int ?? 0
            print("note  \(total) applications searched, \(all["pages"] ?? 0) pages")
            check("this Mac's applications are there to search (\(total))", total > 10)
            write("empty")
            type("e")
            check("one letter finds many, on pages (\(count()))", count() > 12 && (state()["pages"] as? Int ?? 0) > 1)
            write("e")
            key(51, "")

            // Typing from the top, without choosing a computer first.
            type("saf")
            check("typing narrows: \"saf\" leaves few (\(count()))", count() >= 1 && count() < total / 4)
            check("Safari is first, if installed", names().first == "Safari" || !(RoomController.installedApplications().contains { $0.name == "Safari" }))
            write("saf")

            // The letters of the name, in order, are enough; a capital is no different.
            key(51, ""); key(51, ""); key(51, "")
            check("backspace to nothing shows the launcher again", (state()["query"] as? String) == "" && count() < 10)
            key(0, "T"); type("erm")
            check("Terminal found by \"Term\" (capitals fold)", names().contains("Terminal") || !(RoomController.installedApplications().contains { $0.name == "Terminal" }))

            // A space is a letter, and the arrows move among what is left rather than being typed.
            key(51, ""); key(51, ""); key(51, ""); key(51, "")
            type("go")
            let before = state()["cursor"] as? Int ?? -1
            key(124, String(UnicodeScalar(0xF703)!))   // right arrow arrives as a private-use character
            let after = state()
            check("an arrow key is not text", (after["query"] as? String) == "go")
            check("and it moves the cursor when there is somewhere to go", (after["cursor"] as? Int ?? -1) >= before)

            // Nothing found: a note, not a blank view, and Return launches nothing.
            type("zzqx")
            check("nothing is found for nonsense", count() == 0)
            write("none")
            key(36, "\r")
            check("Return on nothing leaves the launcher open", room.menu.isOpen)

            // Escape: the search first, then the launcher.
            key(53, "\u{1B}")
            check("Escape clears the search and stays", (state()["query"] as? String) == "" && room.menu.isOpen)
            key(53, "\u{1B}")
            check("Escape again closes the launcher", !room.menu.isOpen)

            // Open again: blank.
            room.toggleMenu()
            check("it opens blank", (state()["query"] as? String) == "")
            room.toggleMenu()
            print(failures == 0 ? "all passed" : "\(failures) failed")
            exit(failures == 0 ? 0 : 1)
        }
        NSApplication.shared.setActivationPolicy(.accessory)
        NSApplication.shared.run()
    }
}
