//  RoomController.swift — Spatiand's windows, in the glasses' world instead of on this Mac.
//
//  While there are glasses and the owner wants them used, a host's windows are not NSWindows on
//  the desktop: each is a picture decoded here and drawn by the room onto a curved surface in
//  front of the wearer, and the mouse and keyboard of this Mac steer a pointer in that room.
//  This object is the join between the parts: the session (Model) says what windows there are
//  and brings their pictures; the room (RoomCore, in Rust) says where they are and what the
//  pointer is over; the renderer draws; the glasses' output shows it.
//
//  When the glasses go, the windows come back to the desktop exactly as they were.

import AppKit
import CSpatiand
import CoreMedia

final class RoomController {
    let core = RoomCore()
    private(set) var renderer: RoomRenderer?
    /// Whether windows are in the room rather than on this Mac.
    private(set) var active = false
    /// Windows the host has told us of, so a room started late can take them in.
    private(set) var known: [UInt16: CGSize] = [:]
    private var apps: [UInt16: String] = [:]
    private var shown: Set<UInt16> = []

    /// Said when the world changes in a way the rest of the app should show: a window arriving, a
    /// pointer captured or let go.
    var onChange: (() -> Void)?

    private var model: Model { Model.shared }

    /// The glasses, and this Mac's mouse and keyboard while they are in use. Left out of the
    /// offscreen tests, which have no glasses to drive.
    var drivesGlasses = false
    private(set) lazy var output = GlassesOutput(room: self)
    private(set) lazy var input = RoomInput(controller: self)

    private(set) lazy var menu = RoomShell(controller: self)
    private(set) lazy var hint = RoomHint(controller: self)
    private(set) lazy var titles = RoomTitles(controller: self)
    /// Whether any application window is in the room.
    var hasWindows: Bool { !known.isEmpty || !macCaptures.isEmpty }
    /// Where pinned windows sit and how big, as the wearer left them: kept here because the room
    /// only knows how to step on to the next corner.
    var cornerIndex = Settings.pinnedCorner % 4 { didSet { Settings.pinnedCorner = cornerIndex } }
    var pinnedLarge = Settings.pinnedLarge { didSet { Settings.pinnedLarge = pinnedLarge } }

    init() {
        renderer = RoomRenderer(core: core)
        if renderer == nil { print("room: no Metal device; the glasses cannot be used") }
        for _ in 0..<cornerIndex { core.nextCorner() }
        if pinnedLarge { core.toggleSize() }
    }

    func nextCorner() {
        cornerIndex = (cornerIndex + 1) % 4
        core.nextCorner()
    }

    func toggleSize() {
        pinnedLarge.toggle()
        core.toggleSize()
    }

    // MARK: what the menus ask of the app

    /// An image's pixels, straight RGBA, drawn into a square.
    static func pixels(of image: NSImage, side: Int) -> [UInt8]? {
        var pixels = [UInt8](repeating: 0, count: side * side * 4)
        let drawn = pixels.withUnsafeMutableBytes { raw -> Bool in
            guard let context = CGContext(data: raw.baseAddress, width: side, height: side, bitsPerComponent: 8, bytesPerRow: side * 4,
                                          space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return false }
            NSGraphicsContext.saveGraphicsState()
            NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: false)
            image.draw(in: NSRect(x: 0, y: 0, width: side, height: side))
            NSGraphicsContext.restoreGraphicsState()
            return true
        }
        guard drawn else { return nil }
        // Straight alpha is what the room takes.
        for i in stride(from: 0, to: pixels.count, by: 4) {
            let a = Int(pixels[i + 3])
            if a > 0 && a < 255 { for c in 0..<3 { pixels[i + c] = UInt8(min(255, Int(pixels[i + c]) * 255 / a)) } }
        }
        return pixels
    }

    /// The computers, and what each offers, for the launcher and the computers page: every one this Mac has
    /// paired with, the one in use online with its applications, and this Mac itself.
    /// Applications whose pictures the menus already have.
    private var iconsSent: Set<String> = []

    /// An application of this Mac that is installed, for the launcher.
    struct InstalledApp { let id: String; let name: String; let url: URL }
    private var installed: [InstalledApp] = []
    private var installedIcons: [String: [UInt8]] = [:]
    private var installedScannedAt = Date.distantPast
    private var scanning = false

    private func refreshInstalled() {
        guard !scanning, Date().timeIntervalSince(installedScannedAt) > 60 else { return }
        scanning = true
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let apps = Self.installedApplications()
            var icons: [String: [UInt8]] = [:]
            for app in apps {
                if let pixels = Self.pixels(of: NSWorkspace.shared.icon(forFile: app.url.path), side: 128) { icons[app.id] = pixels }
            }
            DispatchQueue.main.async {
                guard let self else { return }
                self.installed = apps
                self.installedIcons = icons
                self.installedScannedAt = Date()
                self.scanning = false
                // The list is already open; show what has been found.
                self.syncHosts()
            }
        }
    }

    /// The applications in the usual folders, one level of subfolders deep, that have a window to show.
    static func installedApplications() -> [InstalledApp] {
        let fm = FileManager.default
        let roots = ["/Applications", "/Applications/Utilities", "/System/Applications", "/System/Applications/Utilities", NSHomeDirectory() + "/Applications"]
        var found: [String: InstalledApp] = [:]
        func visit(_ dir: String, depth: Int) {
            guard let names = try? fm.contentsOfDirectory(atPath: dir) else { return }
            for name in names where !name.hasPrefix(".") {
                let path = dir + "/" + name
                if name.hasSuffix(".app") {
                    guard let bundle = Bundle(path: path), let id = bundle.bundleIdentifier, id != Bundle.main.bundleIdentifier else { continue }
                    let info = bundle.infoDictionary ?? [:]
                    func flag(_ key: String) -> Bool {
                        if let n = info[key] as? NSNumber { return n.boolValue }
                        return (info[key] as? String).map { $0 == "1" || $0.lowercased() == "true" } ?? false
                    }
                    if flag("LSUIElement") || flag("LSBackgroundOnly") { continue }
                    var display = fm.displayName(atPath: path)
                    if display.hasSuffix(".app") { display.removeLast(4) }
                    found[id] = InstalledApp(id: id, name: display, url: URL(fileURLWithPath: path))
                } else if depth < 1 {
                    var isDirectory: ObjCBool = false
                    if fm.fileExists(atPath: path, isDirectory: &isDirectory), isDirectory.boolValue { visit(path, depth: depth + 1) }
                }
            }
        }
        for root in roots { visit(root, depth: 0) }
        return Array(found.values)
    }

    func syncHosts() {
        var rows: [[String: Any]] = []
        var tabs: [[String: Any]] = []
        for host in Hosts.all {
            let current = model.connected && model.host == host
            rows.append(["label": host.name, "address": host.address, "status": current ? "online" : "offline"])
            var apps: [[String: Any]] = []
            if current {
                for app in model.apps {
                    apps.append(["id": app.id, "name": app.name])
                    if iconsSent.insert(host.address + app.id).inserted, let icon = app.icon, let pixels = Self.pixels(of: icon, side: 128) { sp_shell_set_icon(core.handle, app.name, 128, 128, pixels) }
                }
            }
            tabs.append(["label": host.name, "address": host.address, "online": current, "apps": apps])
        }
        // Everything installed, running or not: a bubble for an application that is not open starts it, and its
        // window comes into the room when it has one. The scan is of a few folders, once a minute at most, off
        // the main thread; what is running is added from the system's own list, which is current.
        refreshInstalled()
        var found: [String: String] = [:]
        for app in installed { found[app.id] = app.name }
        for app in NSWorkspace.shared.runningApplications where app.activationPolicy == .regular && app.bundleIdentifier != Bundle.main.bundleIdentifier {
            guard let id = app.bundleIdentifier, let name = app.localizedName else { continue }
            found[id] = name
            if iconsSent.insert(id).inserted, let icon = app.icon, let pixels = Self.pixels(of: icon, side: 128) { sp_shell_set_icon(core.handle, name, 128, 128, pixels) }
        }
        for app in installed where !iconsSent.contains(app.id) {
            guard let pixels = installedIcons[app.id] else { continue }
            iconsSent.insert(app.id)
            sp_shell_set_icon(core.handle, app.name, 128, 128, pixels)
        }
        let mac: [[String: Any]] = found.map { ["id": $0.key, "name": $0.value] }
            .sorted { ($0["name"] as? String ?? "").localizedCaseInsensitiveCompare($1["name"] as? String ?? "") == .orderedAscending }
        tabs.append(["label": "This Mac", "address": "mac", "online": true, "apps": mac])
        if let data = try? JSONSerialization.data(withJSONObject: ["rows": rows, "tabs": tabs]), let text = String(data: data, encoding: .utf8) {
            sp_shell_set_hosts(core.handle, text)
        }
    }

    /// Start an application on a computer: a host's, or one of this Mac's, whose window comes into the room.
    func launch(app id: String, on host: String) {
        if host == "mac" {
            Task { [weak self] in
                let list = await MacWindows.list()
                await MainActor.run {
                    if let info = list.first(where: { $0.bundle == id }) {
                        self?.bringMacWindow(info)
                    } else {
                        self?.openMacApplication(id)
                    }
                }
            }
            return
        }
        guard let paired = Hosts.all.first(where: { $0.address == host }) else { return }
        if model.connected, model.host == paired {
            if let app = model.apps.first(where: { $0.id == id }) { model.launch(app) }
        } else {
            model.connect(paired)
            DispatchQueue.main.asyncAfter(deadline: .now() + 2.5) { [weak self] in
                if let app = self?.model.apps.first(where: { $0.id == id }) { self?.model.launch(app) }
            }
        }
    }

    /// An application of this Mac that is not showing a window: opened, and its window brought into the room as
    /// soon as it has one. An application that is running but has none (all minimised, or closed) is asked to
    /// open one, as clicking its icon in the Dock does.
    private func openMacApplication(_ id: String) {
        guard let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: id) else {
            hint.say("That application could not be found.")
            return
        }
        let name = FileManager.default.displayName(atPath: url.path).replacingOccurrences(of: ".app", with: "")
        hint.say("Opening \(name)\u{2026}")
        NSWorkspace.shared.openApplication(at: url, configuration: NSWorkspace.OpenConfiguration()) { _, error in
            if let error { print("room: could not open \(name): \(error)") }
        }
        waitForWindow(of: id, tries: 40)
    }

    /// Look twice a second, for twenty seconds, for the window an application has been asked to open.
    private func waitForWindow(of id: String, tries: Int) {
        guard tries > 0 else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in
            Task { [weak self] in
                let list = await MacWindows.list()
                await MainActor.run {
                    guard let self else { return }
                    if let info = list.first(where: { $0.bundle == id }) {
                        self.bringMacWindow(info)
                    } else {
                        self.waitForWindow(of: id, tries: tries - 1)
                    }
                }
            }
        }
    }

    func forgetHost(address: String) {
        guard let paired = Hosts.all.first(where: { $0.address == address }) else { return }
        model.forget(paired)
        syncHosts()
    }

    /// The window chosen in the list has been brought here and has the keyboard.
    func focusFromList(_ id: UInt16) {
        sendFocus(MacWindows.isMac(id) ? nil : id)
        hint.update()
    }

    /// What the wearer is looking at, to the Pictures folder.
    func screenshot() {
        guard let renderer, let image = renderer.snapshot(width: 3840, height: 1080, sideBySide: true) else { return }
        let stamp = ISO8601DateFormatter().string(from: Date()).replacingOccurrences(of: ":", with: "-")
        let url = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Pictures/Spatiand \(stamp).png")
        let rep = NSBitmapImageRep(cgImage: image)
        try? rep.representation(using: .png, properties: [:])?.write(to: url)
        hint.say("Saved to your Pictures folder.")
    }

    /// "Leave Spatiand": the windows go back to this Mac's screen.
    func leave() {
        NotificationCenter.default.post(name: .spatiandLeave, object: nil)
    }

    /// An application's icon, drawn small in its window's title bar.
    func setIcon(_ id: UInt16, _ image: NSImage) {
        guard let pixels = Self.pixels(of: image, side: 96) else { return }
        core.setIcon(id, width: 96, height: 96, rgba: pixels)
    }

    /// The keyboard has moved to this window.
    func focusChanged(_ id: UInt16?) { sendFocus(id) }

    /// Ctrl-Space: the menu, in the glasses when they are what is being looked at.
    func toggleMenu() { menu.toggleLauncher() }
    func toggleSettings() { menu.toggleSettings() }

    var available: Bool { renderer != nil }

    // MARK: in and out of the room

    /// Take the windows into the room, or give them back to the desktop.
    func setActive(_ wanted: Bool) {
        guard wanted != active else { return }
        guard !wanted || available else { return }
        active = wanted
        if wanted {
            for (id, size) in known.sorted(by: { $0.key < $1.key }) {
                core.setWindow(id, width: Int(size.width), height: Int(size.height))
                if let app = apps[id] { core.setApp(id, app) }
            }
            // Each window needs a picture to start from, and one that is not changing sends none.
            for id in known.keys { model.link.say(["WantKeyframe": ["window": Int(id)]]) }
            for case let window as RemoteWindow in model.windows.values { window.hideForRoom() }
            startWatchingMacWindows()
            if drivesGlasses {
                output.onChange = { [weak self] in self?.onChange?() }
                input.onChange = { [weak self] in self?.onChange?() }
                output.start()
                if Settings.captureInput { input.start() }
            }
        } else {
            menu.close()
            stopWatchingMacWindows()
            if drivesGlasses {
                input.stop()
                output.stop()
            }
            core.clear()
            shown = []
            renderer?.dropAll()
            for case let window as RemoteWindow in model.windows.values { window.showAgain() }
            for id in known.keys { model.link.say(["WantKeyframe": ["window": Int(id)]]) }
        }
        hint.update()
        onChange?()
    }

    // MARK: windows, as the session tells of them

    func windowOpened(_ id: UInt16, size: CGSize, app: String, title: String) {
        known[id] = size
        titles.set(id, title: title)
        if let icon = model.apps.first(where: { $0.id == app })?.icon { setIcon(id, icon) }
        defer { hint.update() }
        apps[id] = app
        core.setApp(id, app)
        if active {
            core.setWindow(id, width: Int(size.width), height: Int(size.height))
            core.setApp(id, app)
            sendFocus(id)
        }
    }

    /// An application's sound, placed where its window is in the room. Nil when the room is not
    /// in use, and the sound is plain stereo.
    func place(app: String, channels: Int, pcm: Data) -> [Float]? {
        guard active else { return nil }
        return core.audio(app: app, channels: channels, pcm: pcm)
    }

    func windowSized(_ id: UInt16, size: CGSize) {
        known[id] = size
        if active { core.setWindow(id, width: Int(size.width), height: Int(size.height)) }
    }

    func windowClosed(_ id: UInt16) {
        defer { hint.update() }
        titles.drop(id)
        known[id] = nil
        apps[id] = nil
        shown.remove(id)
        core.remove(id)
        renderer?.drop(id)
        if active, let now = core.focused, !MacWindows.isMac(now) { sendFocus(now) }
    }

    /// A window has a new name.
    func retitled(_ id: UInt16, _ title: String) {
        if titles.titles[id] != nil { titles.set(id, title: title) }
    }

    /// Once a frame: what the bars say, if that has changed.
    private var captureFocus: UInt16?
    private var keyOwner: UInt16?
    /// Whether the wearer is typing into a window of this Mac. Only then is that window's application the active
    /// one: trackpad gestures are heard by Spatiand's input window only while Spatiand is active, so the Mac
    /// application is handed the keyboard when a key is pressed and Spatiand takes it back when the pointer or
    /// the fingers are used again.
    private(set) var typing = false
    private var menuKeysWired = false
    private var lastKey = Date.distantPast

    func keyNoted() { lastKey = Date() }

    /// The pointer, the wheel or the fingers are in use: typing is over, once the last key has had a moment.
    func pointingAgain(force: Bool = false) {
        guard typing else { return }
        if force || Date().timeIntervalSince(lastKey) > 0.6 { typing = false }
    }

    /// Who has this Mac's keyboard: the window in front of the wearer if it is one of this Mac's, which is
    /// then made the active window; otherwise Spatiand, which sends the keys to a host's window or a menu.
    private func syncKeyboard() {
        guard input.capturing else { keyOwner = nil; return }
        // A menu is Spatiand's, and a menu that was opened ends the typing that was going on.
        if menu.isOpen { typing = false }
        // And while it is open its keys are taken from whoever has them, if Spatiand is not yet the one.
        if input.capturing, menu.isOpen, !input.hasKeyboard {
            if !menuKeysWired {
                menuKeysWired = true
                input.menuKeys.handler = { [weak self] event in
                    guard let self, self.menu.isOpen, !self.input.hasKeyboard else { return false }
                    return self.menu.key(event)
                }
            }
            input.menuKeys.start()
        } else if input.menuKeys.running {
            input.menuKeys.stop()
        }
        var want: UInt16?
        if Settings.activateMacWindows, typing, !menu.isOpen, let id = core.focused, MacWindows.isMac(id), macCaptures[id] != nil { want = id }
        guard want != keyOwner else { return }
        keyOwner = want
        if let id = want, let capture = macCaptures[id] { input.giveKeyboard(to: capture.info) } else { input.takeKeyboard() }
    }

    private var damping = -1

    func tick() {
        // Steady while typing: for two seconds after a key the view is held firmly, because reading what has just
        // been typed is when a head that is never quite still is most in the way.
        let level = !Settings.steadyView ? 0 : (Date().timeIntervalSince(lastKey) < 2.0 ? 2 : 1)
        if level != damping { damping = level; core.setDamping(level) }
        syncKeyboard()
        // Only the window being used is captured at the display's rate; the rest at half of it, which is
        // what keeps this Mac's fan quiet with several windows in the room.
        let focusedNow = core.focused
        if focusedNow != captureFocus {
            captureFocus = focusedNow
            for (id, capture) in macCaptures { capture.setFast(id == focusedNow) }
        }
        updateSound()
        titles.update()
        menu.update()
    }

    /// The host's windows are gone with the session; this Mac's own stay.
    func sessionEnded() {
        defer { hint.update() }
        for id in known.keys {
            core.remove(id)
            renderer?.drop(id)
            titles.drop(id)
        }
        known = [:]
        apps = [:]
        shown = []
        pressed = nil
        hovered = nil
    }

    /// One picture of a window, as the session sent it. Decoded here when the room is in use.
    func picture(_ id: UInt16, _ sample: CMSampleBuffer) {
        guard active, let renderer else { return }
        let decoder = renderer.decoder(for: id) { [weak self] in
            DispatchQueue.main.async { self?.model.link.say(["WantKeyframe": ["window": Int(id)]]) }
        }
        decoder.onPicture = { [weak self] _ in
            DispatchQueue.main.async {
                guard let self, self.shown.insert(id).inserted else { return }
                self.core.show(id)
            }
        }
        decoder.decode(sample)
    }

    // MARK: the pointer and keyboard

    private func send(_ window: UInt16, _ input: Any) {
        let now = Int(ProcessInfo.processInfo.systemUptime * 1000)
        model.link.say(["InputAt": ["window": Int(window), "input": input, "time_ms": now]])
    }

    private func sendFocus(_ id: UInt16?) {
        let target: Any = id.map { Int($0) } ?? NSNull()
        model.link.say(["Focus": ["window": target]])
    }

    /// The window a button went down on, so the matching release and any drag go to it too.
    private var pressed: UInt16?
    /// When the application was last asked for a size while an edge is being dragged.
    private var lastAsk = Date.distantPast
    private var hovered: UInt16?
    /// A pinch's worth of zoom not yet sent on to the application.
    private var pinchSum = 0.0

    func pointerMoved(dx: Double, dy: Double) {
        pointingAgain()
        core.movePointer(dx: dx, dy: dy)
        pointerChanged()
    }

    /// Where the pointer is may change without the mouse moving: the head turns.
    func pointerChanged() {
        if core.isGrabbing { core.drag(); return }
        if core.isSizing {
            if let drag = core.dragResize(), Date().timeIntervalSince(lastAsk) > 0.12 { askSize(drag.id, drag.width, drag.height) }
            return
        }
        if menu.isOpen { menu.hover(); return }
        if let down = pressed {
            if let at = core.aim(at: down) {
                if MacWindows.isMac(down) { macMouse(down, held: true, at.x, at.y) } else { send(down, ["Motion": ["x": at.x, "y": at.y]]) }
            }
            return
        }
        let aim = core.aim()
        // The frame is Spatiand's, not the application's: the application is told the pointer left it, and the
        // button under the pointer lights.
        if let id = aim.window, id < RoomCore.panelFirst, aim.zone != .content, aim.zone != .none {
            core.setHover(id, aim.zone)
        } else {
            core.setHover(0, .none)
        }
        let over = aim.zone == .content ? aim.window : nil
        if over != hovered {
            if let old = hovered, !MacWindows.isMac(old), old < RoomCore.panelFirst { send(old, "Leave") }
            hovered = over
        }
        if let id = over, id < RoomCore.panelFirst {
            if MacWindows.isMac(id) { macMouse(id, held: false, aim.x, aim.y) } else { send(id, ["Motion": ["x": aim.x, "y": aim.y]]) }
        }
    }

    func buttonDown(_ button: Int, grab: Bool) {
        pointingAgain(force: true)
        let aim = core.aim()
        if menu.isOpen { menu.click(); return }
        guard let id = aim.window, id < RoomCore.panelFirst else { return }
        focus(id)
        switch aim.zone {
        case .close: closeWindow(id); return
        case .hide: hideWindow(id); return
        case .pin: core.setPinned(id, !core.isPinned(id)); return
        case .mute: toggleMute(id); return
        case .title: core.beginGrab(id); return
        case .left, .right, .bottom, .bottomLeft, .bottomRight:
            if button == 0x110 {
                core.beginResize(id, aim.zone)
                lastAsk = .distantPast
            }
            return
        default: break
        }
        if grab {
            core.beginGrab(id)
            return
        }
        pressed = id
        pressedButton = button
        if MacWindows.isMac(id) {
            macClick(id, button: button, down: true, aim.x, aim.y)
            return
        }
        send(id, ["Motion": ["x": aim.x, "y": aim.y]])
        send(id, ["Button": ["button": button, "pressed": true]])
    }

    func buttonUp(_ button: Int) {
        if core.isSizing {
            var sized: UInt16?
            if let drag = core.dragResize() { askSize(drag.id, drag.width, drag.height); sized = drag.id }
            core.endResize()
            // Now the window is what the application has made of it, and again a moment later: a resize that is
            // asked for is not done until the application has redrawn.
            if let id = sized {
                reconcileSize(id)
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { [weak self] in self?.reconcileSize(id) }
            }
            return
        }
        if core.isGrabbing {
            core.endGrab()
            return
        }
        guard let id = pressed else { return }
        pressed = nil
        let at = core.aim(at: id)
        if MacWindows.isMac(id) {
            macClick(id, button: pressedButton, down: false, at?.x ?? 0, at?.y ?? 0)
            return
        }
        if let at { send(id, ["Motion": ["x": at.x, "y": at.y]]) }
        send(id, ["Button": ["button": button, "pressed": false]])
    }

    func scrolled(dx: Double, dy: Double, precise: Bool) {
        pointingAgain(force: true)
        guard !menu.isOpen, let id = core.aim().window ?? core.focused, id < RoomCore.panelFirst else { return }
        let unit: Double = precise ? 1 : 10
        if MacWindows.isMac(id), let capture = macCaptures[id], let at = core.aim(at: id) {
            MacInput.scroll(dx: Int32(dx * unit), dy: Int32(dy * unit), at: capture.screenPoint(x: at.x, y: at.y),
                            window: capture.info.windowID, pid: capture.info.pid)
            return
        }
        send(id, ["Scroll": ["horizontal": -dx * unit, "vertical": -dy * unit]])
    }

    // MARK: this Mac's own windows

    private var macCaptures: [UInt16: MacCapture] = [:]
    private var macShown: Set<UInt16> = []
    var macWindowIDs: [UInt16] { macCaptures.keys.sorted() }
    func macInfo(_ id: UInt16) -> MacWindowInfo? { macCaptures[id]?.info }
    private var nextMac = MacWindows.firstID
    private(set) var macList: [MacWindowInfo] = []
    private var macTimer: Timer?
    private var pressedButton = 0x110
    private var lastClick: (at: Date, window: UInt16, point: CGPoint, count: Int)?

    /// What could be brought in, looked up again every few seconds while the room is in use.
    func startWatchingMacWindows() {
        macTimer?.invalidate()
        let t = Timer(timeInterval: 3, repeats: true) { [weak self] _ in self?.refreshMacList() }
        RunLoop.main.add(t, forMode: .common)
        macTimer = t
        refreshMacList()
    }

    func stopWatchingMacWindows() {
        macTimer?.invalidate()
        macTimer = nil
        for id in Array(macCaptures.keys) { removeMacWindow(id) }
    }

    func refreshMacList() {
        guard active, MacWindows.allowed(ask: false) else { return }
        Task { [weak self] in
            let list = await MacWindows.list()
            await MainActor.run {
                self?.macList = list
                // Pictures of every running application are drawn only for a menu that is open: doing it every few
                // seconds for nothing was a visible hitch in the view.
                if self?.menu.isOpen == true { self?.syncHosts() }
            }
        }
    }

    /// A window of this Mac, into the room beside the host's.
    func bringMacWindow(_ info: MacWindowInfo) {
        guard active, let renderer else { return }
        if let have = macCaptures.first(where: { $0.value.info.windowID == info.windowID })?.key {
            core.bringHere(have)
            core.focused = have
            return
        }
        guard MacWindows.allowed(ask: true) else {
            model.onProblem?("Spatiand needs Screen Recording to show a Mac window in the glasses. Allow it in System Settings \u{2192} Privacy & Security \u{2192} Screen Recording, then choose the window again.")
            return
        }
        if !MacInput.allowed(ask: true) {
            print("room: no Accessibility yet; the window will show but cannot be clicked or typed into")
        }
        let id = nextMac
        nextMac = nextMac &+ 1 < RoomCore.panelFirst ? nextMac + 1 : MacWindows.firstID
        let capture = MacCapture(info)
        macCaptures[id] = capture
        titles.set(id, title: info.title.isEmpty ? info.app : info.app + " \u{2014} " + info.title)
        if let icon = NSRunningApplication(processIdentifier: info.pid)?.icon { setIcon(id, icon) }
        renderer.decoders[id] = capture
        capture.onSize = { [weak self] size in DispatchQueue.main.async { self?.macSized(id, size) } }
        capture.onEnded = { [weak self] in self?.removeMacWindow(id) }
        Task { [weak self] in
            do { try await capture.start() } catch {
                await MainActor.run {
                    self?.removeMacWindow(id)
                    print("room: could not capture \(info.app): \(error)")
                }
            }
        }
    }

    private func macSized(_ id: UInt16, _ size: CGSize) {
        guard macCaptures[id] != nil else { return }
        core.setWindow(id, width: Int(size.width), height: Int(size.height))
        if macShown.insert(id).inserted {
            core.show(id)
            core.focused = id
            sendFocus(nil)
            hint.update()
        }
    }

    func removeMacWindow(_ id: UInt16) {
        guard let capture = macCaptures.removeValue(forKey: id) else { return }
        capture.invalidate()
        macShown.remove(id)
        renderer?.drop(id)
        titles.drop(id)
        core.remove(id)
        hint.update()
    }

    private var lastMacMove = (at: Date.distantPast, point: CGPoint.zero)
    private var trailingMove: DispatchWorkItem?

    /// The pointer over, or dragged across, a window of this Mac. The head turns the pointer sixty times a
    /// second, and an application that is told each time spends its own time on hover states and redraws
    /// that nobody sees: so it is told at most every few milliseconds, and the last place always arrives.
    private func macMouse(_ id: UInt16, held: Bool, _ x: Double, _ y: Double) {
        guard let capture = macCaptures[id] else { return }
        let point = capture.screenPoint(x: x, y: y)
        let right = pressedButton == 0x111
        let type: CGEventType = held ? (right ? .rightMouseDragged : .leftMouseDragged) : .mouseMoved
        let send = { [weak self] in
            self?.lastMacMove = (Date(), point)
            MacInput.mouse(type, button: right ? .right : .left, at: point, clicks: 1, window: capture.info.windowID, pid: capture.info.pid)
        }
        trailingMove?.cancel()
        let since = Date().timeIntervalSince(lastMacMove.at)
        let moved = hypot(point.x - lastMacMove.point.x, point.y - lastMacMove.point.y)
        if moved < 0.5 { return }
        if since >= (held ? 0.008 : 0.025) {
            send()
        } else {
            let item = DispatchWorkItem(block: send)
            trailingMove = item
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.03, execute: item)
        }
    }

    private func macClick(_ id: UInt16, button: Int, down: Bool, _ x: Double, _ y: Double) {
        guard let capture = macCaptures[id] else { return }
        trailingMove?.cancel()
        let there = capture.screenPoint(x: x, y: y)
        if down, hypot(there.x - lastMacMove.point.x, there.y - lastMacMove.point.y) >= 0.5 {
            MacInput.mouse(.mouseMoved, button: .left, at: there, clicks: 1, window: capture.info.windowID, pid: capture.info.pid)
        }
        lastMacMove = (Date(), there)
        let point = capture.screenPoint(x: x, y: y)
        var clicks = 1
        if down {
            if let last = lastClick, last.window == id, Date().timeIntervalSince(last.at) < 0.4,
               abs(last.point.x - point.x) < 6, abs(last.point.y - point.y) < 6 {
                clicks = last.count + 1
            }
            lastClick = (Date(), id, point, clicks)
        } else {
            clicks = lastClick?.count ?? 1
        }
        let right = button == 0x111
        let type: CGEventType = down ? (right ? .rightMouseDown : .leftMouseDown) : (right ? .rightMouseUp : .leftMouseUp)
        MacInput.mouse(type, button: right ? .right : .left, at: point, clicks: clicks,
                       window: capture.info.windowID, pid: capture.info.pid)
    }

    /// A key meant for a Mac window, if that is what has the keyboard: posted at its application.
    func macKey(_ event: NSEvent) -> Bool {
        guard let id = core.focused, MacWindows.isMac(id), let capture = macCaptures[id] else { return false }
        lastKey = Date()
        // The first key hands the application the keyboard, as a window in front has it, so it shows its caret; this
        // key is still posted at it, and the rest arrive of their own accord.
        if !typing, Settings.activateMacWindows, !menu.isOpen { typing = true }
        switch event.type {
        case .keyDown:
            MacInput.key(event, down: true, pid: capture.info.pid)
            // The Mac never says a key chorded with Command was let go of; so it is let go of here.
            if event.modifierFlags.contains(.command) {
                let (code, flags, pid) = (event.keyCode, event.modifierFlags, capture.info.pid)
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.04) { MacInput.key(code: code, flags: flags, down: false, pid: pid) }
            }
        case .keyUp: MacInput.key(event, down: false, pid: capture.info.pid)
        case .flagsChanged: MacInput.modifier(event, pid: capture.info.pid)
        default: return false
        }
        return true
    }

    /// The window a key goes to: the one with the keyboard.
    var keyTarget: VideoView? {
        guard let id = core.focused else { return nil }
        return (model.windows[id] as? RemoteWindow)?.view
    }

    /// Nearer or further, by this many metres: the window is moved, not resized.
    func push(_ id: UInt16, metres: Double) { core.pushPull(id, metres: metres) }

    /// A pinch on the trackpad. On a window's frame or title bar it brings the window nearer or puts it
    /// further away, in front of the others or behind them; over what the application draws it is the
    /// application's, which zooms its own contents.
    func pinched(by magnification: Double) {
        let aim = core.aim()
        guard let id = aim.window, id < RoomCore.panelFirst, !menu.isOpen else { return }
        if aim.zone == .content {
            pinchSum += magnification
            while abs(pinchSum) >= 0.12 {
                zoomContents(id, in: pinchSum > 0)
                pinchSum -= pinchSum > 0 ? 0.12 : -0.12
            }
        } else if aim.zone != .none {
            pinchSum = 0
            push(id, metres: -magnification * 3.0)
        }
    }

    /// One step of zoom in an application: Control and the wheel, as a browser takes it, or Command and
    /// plus or minus for a window of this Mac.
    private func zoomContents(_ id: UInt16, in zoomIn: Bool) {
        if MacWindows.isMac(id) {
            guard let capture = macCaptures[id] else { return }
            // = and - (ANSI), with Command.
            let code: UInt16 = zoomIn ? 24 : 27
            MacInput.key(code: code, flags: .command, down: true, pid: capture.info.pid)
            MacInput.key(code: code, flags: .command, down: false, pid: capture.info.pid)
            return
        }
        let at = Int(ProcessInfo.processInfo.systemUptime * 1000)
        func say(_ input: Any) { send(id, input) }
        _ = at
        model.link.say(["InputAt": ["window": Int(id), "input": ["Key": ["code": 29, "pressed": true]], "time_ms": at]])
        model.link.say(["InputAt": ["window": Int(id), "input": ["Scroll": ["horizontal": 0.0, "vertical": zoomIn ? -10.0 : 10.0]], "time_ms": at + 1]])
        model.link.say(["InputAt": ["window": Int(id), "input": ["Key": ["code": 29, "pressed": false]], "time_ms": at + 2]])
    }

    /// The window's size in the room is its application's own.
    private func reconcileSize(_ id: UInt16) {
        if MacWindows.isMac(id) {
            guard let capture = macCaptures[id] else { return }
            let size = capture.frameSizePixels
            if size.width > 0, size.height > 0 { core.setWindow(id, width: Int(size.width), height: Int(size.height)) }
        } else if let size = known[id] {
            core.setWindow(id, width: Int(size.width), height: Int(size.height))
        }
    }

    /// Ask the application to make its window about this many pixels.
    private func askSize(_ id: UInt16, _ width: Int, _ height: Int) {
        lastAsk = Date()
        if MacWindows.isMac(id) {
            guard let capture = macCaptures[id] else { return }
            MacWindows.resize(capture.info, toPoints: CGSize(width: Double(width) / capture.scale, height: Double(height) / capture.scale))
        } else {
            model.link.say(["Configure": ["window": Int(id), "width": width, "height": height]])
        }
    }

    /// Put a window away; the menu brings it back.
    func hideWindow(_ id: UInt16) {
        core.setHidden(id, true)
        if core.focused == id { core.focused = nil; sendFocus(nil) }
        hint.update()
    }

    func showWindow(_ id: UInt16) {
        core.setHidden(id, false)
        core.bringHere(id)
        focus(id)
        hint.update()
    }

    /// The windows put away, for the menu.
    var hiddenWindows: [(id: UInt16, title: String)] {
        (Array(known.keys) + macWindowIDs).filter { core.isHidden($0) }.sorted().map { ($0, titles.titles[$0] ?? "Window") }
    }

    func toggleMute(_ id: UInt16) {
        guard let app = apps[id] ?? macInfo(id)?.bundle else { return }
        AudioOut.shared.toggleMute(app)
    }

    /// What each window's frame says about sound, once a frame.
    func updateSound() {
        for id in Array(known.keys) + macWindowIDs {
            guard let app = apps[id] ?? macInfo(id)?.bundle else { continue }
            core.setSound(id, sounding: AudioOut.shared.isSounding(app), muted: AudioOut.shared.isMuted(app))
        }
    }

    /// Pin the window being pointed at to the glass, or let it go.
    func togglePin() {
        guard let id = core.aim().window ?? core.focused else { return }
        core.setPinned(id, !core.isPinned(id))
    }

    func closeAimed() {
        guard let id = core.aim().window ?? core.focused, id < RoomCore.panelFirst else { return }
        closeWindow(id)
    }

    /// A host's window is asked to close; one of this Mac's simply leaves the room.
    func closeWindow(_ id: UInt16) {
        if MacWindows.isMac(id) { removeMacWindow(id); return }
        model.link.say(["Close": ["window": Int(id)]])
    }

    /// The keyboard goes to this window: a Mac window's, and then the host has none.
    private func focus(_ id: UInt16) {
        guard core.focused != id else { return }
        core.focused = id
        sendFocus(MacWindows.isMac(id) ? nil : id)
    }

    func bringAimedHere() {
        guard let id = core.aim().window ?? core.focused else { return }
        core.bringHere(id)
    }

    /// How the host drew the pointer: the room's pointer takes the same shape.
    func hostCursor(pixels: Data, width: Int, height: Int, hotX: Double, hotY: Double) {
        renderer?.setCursorPicture(pixels, width: width, height: height)
        core.cursorShape(hotX: hotX, hotY: hotY, width: Double(width), height: Double(height))
    }

    // MARK: gestures

    /// Three, four and five fingers on the trackpad. Three work the room as a whole, four the
    /// window being pointed at, and five all the windows at once.
    func gesture(_ event: GestureRecognizer.Event) {
        pointingAgain(force: true)
        let target = core.aim().window.flatMap { $0 < RoomCore.panelFirst ? $0 : nil } ?? core.focused
        switch event {
        case .swipe(.left, 3), .swipe(.right, 3):
            // The next window round, brought to where you are looking.
            if case .swipe(let direction, _) = event, let id = core.focusStep(direction == .left ? 1 : -1) {
                core.bringHere(id)
                focus(id)
            }
        case .swipe(.up, 3):
            if !menu.isOpen { menu.toggleLauncher() }
        case .swipe(.down, 3):
            menu.close()
        case .tap(3):
            recentre()
        case .swipe(let direction, 4):
            guard let id = target else { return }
            switch direction {
            case .left: core.nudge(id, yaw: 14, pitch: 0)
            case .right: core.nudge(id, yaw: -14, pitch: 0)
            case .up: core.nudge(id, yaw: 0, pitch: 8)
            case .down: core.nudge(id, yaw: 0, pitch: -8)
            }
        case .pinch(4, let spreading):
            if let id = target { push(id, metres: spreading ? -0.5 : 0.5) }
        case .pinch(5, let spreading):
            core.arrange(spread: spreading ? 2.4 : 1.0)
        default:
            break
        }
    }

    func recentre() {
        core.recentre()
        core.centrePointer()
    }
}
