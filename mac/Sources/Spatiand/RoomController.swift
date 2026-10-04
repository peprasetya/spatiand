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

    private(set) lazy var menu = RoomMenu(controller: self)
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

    /// The keyboard has moved to this window.
    func focusChanged(_ id: UInt16?) { sendFocus(id) }

    /// Ctrl-Space: the menu, in the glasses when they are what is being looked at.
    func toggleMenu() { menu.toggle() }

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
            if drivesGlasses {
                output.onChange = { [weak self] in self?.onChange?() }
                input.onChange = { [weak self] in self?.onChange?() }
                output.start()
                if Settings.captureInput { input.start() }
            }
        } else {
            menu.close()
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
        onChange?()
    }

    // MARK: windows, as the session tells of them

    func windowOpened(_ id: UInt16, size: CGSize, app: String) {
        known[id] = size
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
        known[id] = nil
        apps[id] = nil
        shown.remove(id)
        core.remove(id)
        renderer?.drop(id)
        if active, let now = core.focused { sendFocus(now) }
    }

    func sessionEnded() {
        known = [:]
        shown = []
        core.clear()
        renderer?.dropAll()
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
    private var hovered: UInt16?

    func pointerMoved(dx: Double, dy: Double) {
        core.movePointer(dx: dx, dy: dy)
        pointerChanged()
    }

    /// Where the pointer is may change without the mouse moving: the head turns.
    func pointerChanged() {
        if core.isGrabbing { core.drag(); return }
        if menu.isOpen {
            let aim = core.aim()
            if aim.window == RoomMenu.panelID { menu.hover(aim.x, aim.y) } else { menu.hover(-1, -1) }
            if aim.window == RoomMenu.panelID { return }
        }
        if let down = pressed {
            if let at = core.aim(at: down) { send(down, ["Motion": ["x": at.x, "y": at.y]]) }
            return
        }
        let aim = core.aim()
        if aim.window != hovered {
            if let old = hovered { send(old, "Leave") }
            hovered = aim.window
        }
        if let id = aim.window { send(id, ["Motion": ["x": aim.x, "y": aim.y]]) }
    }

    func buttonDown(_ button: Int, grab: Bool) {
        let aim = core.aim()
        if menu.isOpen {
            if aim.window == RoomMenu.panelID { menu.click(aim.x, aim.y) } else { menu.close() }
            return
        }
        guard let id = aim.window else { return }
        if core.focused != id {
            core.focused = id
            sendFocus(id)
        }
        if grab {
            core.beginGrab(id)
            return
        }
        pressed = id
        send(id, ["Motion": ["x": aim.x, "y": aim.y]])
        send(id, ["Button": ["button": button, "pressed": true]])
    }

    func buttonUp(_ button: Int) {
        if core.isGrabbing {
            core.endGrab()
            return
        }
        guard let id = pressed else { return }
        if let at = core.aim(at: id) { send(id, ["Motion": ["x": at.x, "y": at.y]]) }
        send(id, ["Button": ["button": button, "pressed": false]])
        pressed = nil
    }

    func scrolled(dx: Double, dy: Double, precise: Bool) {
        guard !menu.isOpen, let id = core.aim().window ?? core.focused else { return }
        let unit: Double = precise ? 1 : 10
        send(id, ["Scroll": ["horizontal": -dx * unit, "vertical": -dy * unit]])
    }

    /// The window a key goes to: the one with the keyboard.
    var keyTarget: VideoView? {
        guard let id = core.focused else { return nil }
        return (model.windows[id] as? RemoteWindow)?.view
    }

    func zoom(by factor: Double) {
        guard let id = core.aim().window ?? core.focused else { return }
        core.scale(id, by: factor)
    }

    /// Pin the window being pointed at to the glass, or let it go.
    func togglePin() {
        guard let id = core.aim().window ?? core.focused else { return }
        core.setPinned(id, !core.isPinned(id))
    }

    func closeAimed() {
        guard let id = core.aim().window ?? core.focused, id < 0xFFF0 else { return }
        model.link.say(["Close": ["window": Int(id)]])
    }

    func bringAimedHere() {
        guard let id = core.aim().window ?? core.focused else { return }
        core.bringHere(id)
    }

    func recentre() {
        core.recentre()
        core.centrePointer()
    }
}
