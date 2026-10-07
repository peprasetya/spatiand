//  EnvironmentTest.swift — `Spatiand --selftest-environment`
//
//  The environment picker, end to end, as the Deck's is used: the settings list, Environment, the generated and
//  the black ones, images found in the folder, an image added from the file browser, what is remembered, and what
//  the glasses are then shown. Nothing of the wearer's own is touched: the folders are made here, and the run
//  must be started with XDG_DATA_HOME, SPATIAND_ENVIRONMENTS and HOME pointing into a scratch folder, which the
//  `tools/selftest-environment.sh` script does.
//
//  Pictures of what the glasses would show go to /tmp/spatiand-env-*.png.

import AppKit
import CSpatiand

enum EnvironmentTest {
    /// A panorama to look at: a sky that is lighter towards the horizon, a white bar straight ahead, a red one fifteen
    /// degrees to the right and a blue one fifteen degrees to the left. Mirrored or turned, it shows.
    static func panorama(_ path: String, width: Int, height: Int, tint: (UInt8, UInt8, UInt8), bars: Bool = true) {
        let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: width, pixelsHigh: height, bitsPerSample: 8, samplesPerPixel: 4,
                                   hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: width * 4, bitsPerPixel: 32)!
        let data = rep.bitmapData!
        for y in 0..<height {
            let elevation = 0.5 - (Double(y) + 0.5) / Double(height)            // +0.5 up, -0.5 down
            let light = 1.0 - min(1.0, abs(elevation) * 2.4)
            for x in 0..<width {
                let u = (Double(x) + 0.5) / Double(width)
                let degrees = (u - 0.5) * 360
                var r = Double(tint.0) * (0.25 + 0.75 * light), g = Double(tint.1) * (0.25 + 0.75 * light), b = Double(tint.2) * (0.25 + 0.75 * light)
                if bars && abs(elevation) < 0.2 {
                    if abs(degrees) < 1.5 { (r, g, b) = (255, 255, 255) }
                    if abs(degrees - 15) < 1.5 { (r, g, b) = (255, 40, 40) }
                    if abs(degrees + 15) < 1.5 { (r, g, b) = (40, 80, 255) }
                }
                let i = (y * width + x) * 4
                data[i] = UInt8(min(255, r)); data[i + 1] = UInt8(min(255, g)); data[i + 2] = UInt8(min(255, b)); data[i + 3] = 255
            }
        }
        try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
    }

    static func run() {
        let env = ProcessInfo.processInfo.environment
        guard let folder = env["SPATIAND_ENVIRONMENTS"], let home = env["HOME"], env["XDG_DATA_HOME"] != nil, folder.contains("selftest") else {
            print("run this through tools/selftest-environment.sh: it must not touch the real folders")
            exit(2)
        }
        let fm = FileManager.default
        try? fm.createDirectory(atPath: folder, withIntermediateDirectories: true)
        try? fm.createDirectory(atPath: home + "/Pictures", withIntermediateDirectories: true)
        panorama(folder + "/sunset-360.png", width: 2048, height: 1024, tint: (255, 150, 90))
        panorama(folder + "/stereo_ou.png", width: 1024, height: 1024, tint: (80, 200, 120), bars: false)
        panorama(folder + "/front-180.png", width: 2048, height: 1024, tint: (120, 140, 255))
        panorama(home + "/Pictures/mine.png", width: 2048, height: 1024, tint: (190, 120, 255))
        fm.createFile(atPath: home + "/Pictures/notes.txt", contents: Data("not an image".utf8))

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
        func press(_ i: RoomShell.Intent) { room.menu.intent(i) }
        func goTo(_ label: String) -> Bool {
            guard let target = rows().firstIndex(of: label) else { return false }
            var guardCount = 0
            while cursor() != target, guardCount < 60 {
                press(cursor() < target ? .down : .up)
                guardCount += 1
            }
            return cursor() == target
        }
        func snapshot(_ name: String) {
            for _ in 0..<6 { room.tick(); RunLoop.main.run(until: Date().addingTimeInterval(0.15)) }
            room.menu.update()
            if let image = room.renderer?.snapshot(width: 3840, height: 1080, sideBySide: true) {
                try? NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: "/tmp/spatiand-env-\(name).png"))
                print("wrote /tmp/spatiand-env-\(name).png")
            }
        }
        /// Waits for the next environment the room is given, running the room's frames meanwhile.
        func nextSky() -> sp_sky_info? {
            for _ in 0..<80 {
                if let renderer = room.renderer, let (info, pixels) = room.core.newEnvironment() { renderer.setSky(info, pixels: pixels); return info }
                RunLoop.main.run(until: Date().addingTimeInterval(0.1))
            }
            return nil
        }

        func after(_ seconds: Double, _ body: @escaping () -> Void) { DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: body) }
        after(0.5) {
            room.core.holdHead(yaw: 0, pitch: 0)
            room.core.perEye(1920, 1080)
            room.setActive(true)
            room.core.useDiskEnvironments()
            room.core.setStatus(StatusLine.text())
        }
        after(1.5) {
            // --- it starts in the studio, which is generated
            let first = nextSky()
            check("it starts on the generated studio", first != nil && first!.width == 2048 && first!.height == 1024)
            snapshot("studio")

            // --- the picker
            room.toggleSettings()
            check("the settings list has an Environment row", goTo("Environment"))
            press(.accept)
            let list = rows()
            print("note  environment rows: \(list)")
            check("Blank and Studio come first", list.prefix(2).elementsEqual(["Blank (black)", "Studio (generated)"]))
            check("the images in the folder are listed by name", list.contains("sunset-360") && list.contains("stereo_ou") && list.contains("front-180"))
            check("and the way to add another comes last", list.last == "Add an image...")
            check("the cursor starts on what is in use", rows()[cursor()] == "Studio (generated)")

            // --- the status line, held to the head in the upper left
            snapshot("status")
            var ids = [UInt32](repeating: 0, count: 64)
            let count = sp_shell_panel_ids(room.core.handle, &ids, ids.count)
            check("the status line is a panel of its own", ids.prefix(count).contains(0xFFF6))

            // --- a 360 panorama
            check("can move to the sunset", goTo("sunset-360"))
            press(.accept)
            let sunset = nextSky()
            check("choosing it loads it: 2048x1024, all the way round, one picture", sunset != nil && sunset!.width == 2048 && sunset!.projection == 0 && sunset!.stereo == 0)
            check("and closes the menu", !room.menu.isOpen)
            snapshot("sunset")

            // --- stereo and 180
            room.toggleSettings(); _ = goTo("Environment"); press(.accept)
            check("the cursor is now on the sunset", rows()[cursor()] == "sunset-360")
            _ = goTo("stereo_ou"); press(.accept)
            let stereo = nextSky()
            check("a square picture is read as a stacked stereo pair", stereo != nil && stereo!.stereo == 1)
            snapshot("stereo")
            room.toggleSettings(); _ = goTo("Environment"); press(.accept)
            _ = goTo("front-180"); press(.accept)
            let half = nextSky()
            check("a name with 180 in it covers the front only", half != nil && half!.projection == 1)
            snapshot("front180")

            // --- adding an image from the file browser
            room.toggleSettings(); _ = goTo("Environment"); press(.accept)
            check("can reach Add an image", goTo("Add an image..."))
            press(.accept)
            let browser = rows()
            print("note  browser rows: \(browser)")
            check("the browser lists the pictures folder and shows only images", browser.contains("mine.png") && !browser.contains("notes.txt"))
            check("and a way up", browser.contains("Go up"))
            _ = goTo("mine.png"); press(.accept)
            let mine = nextSky()
            check("the added image is loaded", mine != nil && mine!.width == 2048)
            let listFile = (try? String(contentsOfFile: env["XDG_DATA_HOME"]! + "/spatiand/environments.list", encoding: .utf8)) ?? ""
            check("and remembered for next time", listFile.contains("mine.png"))
            snapshot("mine")

            // --- black
            room.toggleSettings(); _ = goTo("Environment"); press(.accept)
            check("the added image is in the list now", rows().contains("mine"))
            _ = goTo("Blank (black)"); press(.accept)
            let blank = nextSky()
            check("Blank is black", blank != nil && blank!.width <= 4)
            let state = (try? String(contentsOfFile: env["XDG_DATA_HOME"]! + "/spatiand/environment", encoding: .utf8)) ?? ""
            check("and what was chosen last is written down", state.contains("blank"))
            snapshot("blank")

            print(failures == 0 ? "all passed" : "\(failures) failed")
            exit(failures == 0 ? 0 : 1)
        }
        NSApplication.shared.setActivationPolicy(.accessory)
        NSApplication.shared.run()
    }
}
