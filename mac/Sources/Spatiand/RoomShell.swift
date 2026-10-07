//  RoomShell.swift — the Deck's menus in the glasses: settings, the launcher, the window list, the
//  environment picker and the computers page.
//
//  What the menus say and do is the Deck's own state machine (`spatiand-shell`, run by the Rust side); what
//  they look like is the Deck's drawing, done on the CPU in `shell_ui.rs`. This is the join: intents in
//  (the keyboard's arrows and Return, a pad's D-pad and buttons, the pointer), pictures out to the
//  renderer, and the things the shell asks for -- launch this, recentre, take a screenshot -- done.
//
//  Ctrl-Space opens the launcher and Ctrl-Tab the settings, as the Deck's ⋯ and STEAM buttons do.

import AppKit
import CSpatiand
import Metal

final class RoomShell {
    /// The shell's intents, as the Rust side numbers them.
    enum Intent: Int32 { case up = 0, down, left, right, accept, back, settings, launcher, windows, close, hide }

    private unowned let controller: RoomController
    private var version: UInt64 = 0
    private var textures: [UInt32: MTLTexture] = [:]
    private static let buffer = UnsafeMutablePointer<UInt8>.allocate(capacity: 2048 * 2048 * 4)

    init(controller: RoomController) { self.controller = controller }

    private var state: Int32 { sp_shell_open(controller.core.handle) }
    var isOpen: Bool { state & 1 != 0 }
    var wantsText: Bool { state & 2 != 0 }
    /// The launcher is open: what is typed narrows its bubbles down.
    var isSearching: Bool { state & 4 != 0 }
    /// The controller layout editor is the menu on show.
    var isEditing: Bool { state & 8 != 0 }

    // MARK: opening and closing

    func toggleSettings() {
        if !isOpen { prepare() }
        intent(.settings)
    }
    func toggleLauncher() {
        if !isOpen { prepare() }
        intent(.launcher)
    }
    func close() {
        // Back until it is closed: backing out of a page lands on the one it came from.
        var guardCount = 0
        while isOpen, guardCount < 6 { intent(.back); guardCount += 1 }
    }

    /// What the menus are told before they open: the computers, the pinned windows' corner, the environment.
    private func prepare() {
        controller.syncHosts()
        sp_shell_set_pip(controller.core.handle, Int32(controller.cornerIndex), controller.pinnedLarge ? 1 : 0)
        controller.refreshMacList()
    }

    // MARK: intents

    @discardableResult
    func intent(_ intent: Intent) -> Bool {
        guard let json = sp_shell_intent(controller.core.handle, intent.rawValue) else { update(); return false }
        defer { sp_free_string(json) }
        handle(String(cString: json))
        // The editor was left some other way than its own Back: what was changed is kept.
        if !isEditing { PadInput.shared.closeEditor() }
        update()
        return true
    }

    /// A game controller's D-pad, A, B and Y (see `PadInput`): the same intents the Deck gives them.
    func padPress(_ bits: UInt32) {
        guard isOpen else { return }
        if bits & 1 != 0 { intent(.up) }
        if bits & 2 != 0 { intent(.down) }
        if bits & 4 != 0 { intent(.left) }
        if bits & 8 != 0 { intent(.right) }
        if bits & 0x10 != 0 { intent(.accept) }
        if bits & 0x20 != 0 { intent(.back) }
        if bits & 0x40 != 0 { intent(.close) }
    }

    /// A key of this Mac's keyboard, while a menu is open. Returns whether the menu took it.
    func key(_ event: NSEvent) -> Bool {
        guard isOpen else { return false }
        if wantsText {
            switch Int(event.keyCode) {
            case 36, 76: typed(text: "", backspace: false, enter: true); return true
            case 51: typed(text: "", backspace: true, enter: false); return true
            case 53: intent(.back); return true
            default:
                if let text = event.characters, !text.isEmpty, event.modifierFlags.intersection([.command, .control]).isEmpty {
                    typed(text: text, backspace: false, enter: false)
                    return true
                }
                return false
            }
        }
        if isSearching { return searchKey(event) }
        switch Int(event.keyCode) {
        case 126: intent(.up)
        case 125: intent(.down)
        case 123: intent(.left)
        case 124: intent(.right)
        case 36, 76, 49: intent(.accept)
        case 53, 51: intent(.back)
        case 7: intent(.hide)       // X
        case 6: intent(.close)      // Z, standing in for Y
        default: return true
        }
        return true
    }

    /// A key in the launcher: letters narrow the bubbles down, the arrows move among what is left, Return opens
    /// the one the cursor is on, and Escape takes back what was typed before it takes back the launcher.
    private func searchKey(_ event: NSEvent) -> Bool {
        switch Int(event.keyCode) {
        case 126: intent(.up)
        case 125: intent(.down)
        case 123: intent(.left)
        case 124: intent(.right)
        case 36, 76: intent(.accept)
        case 53: intent(.back)
        case 51: typed(text: "", backspace: true, enter: false)
        case 48: break
        default:
            guard event.modifierFlags.intersection([.command, .control]).isEmpty, let text = event.characters else { return true }
            // The arrows, Home and the like arrive as characters in the private-use area; they are not text.
            let printable = text.unicodeScalars.filter { $0.value >= 0x20 && $0.value != 0x7F && !(0xF700...0xF8FF).contains($0.value) }
            if !printable.isEmpty { typed(text: String(String.UnicodeScalarView(printable)), backspace: false, enter: false) }
        }
        return true
    }

    private func typed(text: String, backspace: Bool, enter: Bool) {
        if let json = sp_shell_type(controller.core.handle, text, backspace ? 1 : 0, enter ? 1 : 0) {
            handle(String(cString: json))
            sp_free_string(json)
        }
        update()
    }

    // MARK: the pointer

    func hover() {
        guard isOpen else { return }
        sp_shell_hover(controller.core.handle)
        update()
    }

    func click() {
        guard isOpen else { return }
        if let json = sp_shell_click(controller.core.handle) {
            handle(String(cString: json))
            sp_free_string(json)
        }
        update()
    }

    // MARK: pictures

    /// Draw again what changed, and hand the renderer the pictures. Once a frame.
    func update() {
        guard let renderer = controller.renderer else { return }
        let now = sp_shell_sync(controller.core.handle)
        guard now != version else { return }
        version = now
        var ids = [UInt32](repeating: 0, count: 64)
        let count = sp_shell_panel_ids(controller.core.handle, &ids, ids.count)
        let present = Set(ids.prefix(count))
        for id in Array(textures.keys) where !present.contains(id) {
            textures[id] = nil
            renderer.panels[id] = nil
        }
        for id in present {
            var w: UInt32 = 0, h: UInt32 = 0
            guard sp_shell_panel_image(controller.core.handle, id, &w, &h, Self.buffer, 2048 * 2048 * 4) != 0 else { continue }
            var texture = textures[id]
            if texture == nil || texture!.width != Int(w) || texture!.height != Int(h) {
                let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .bgra8Unorm, width: Int(w), height: Int(h), mipmapped: false)
                texture = renderer.device.makeTexture(descriptor: d)
                textures[id] = texture
            }
            texture?.replace(region: MTLRegionMake2D(0, 0, Int(w), Int(h)), mipmapLevel: 0, withBytes: Self.buffer, bytesPerRow: Int(w) * 4)
            renderer.panels[id] = texture
        }
        controller.hint.update()
    }

    // MARK: what the shell asks for

    private func handle(_ json: String) {
        guard let data = json.data(using: .utf8), let event = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
        if let hud = event["hud"] as? String {
            if hud.hasPrefix("OpenSystemSettings") {
                controller.openSystemSettings(panel: hud.contains("bluetooth") ? "bluetooth" : "wifi")
                return
            }
            switch hud {
            case "Recentre": controller.recentre()
            case "Screenshot": controller.screenshot()
            case "PipCorner": controller.nextCorner(); sp_shell_set_pip(controller.core.handle, Int32(controller.cornerIndex), controller.pinnedLarge ? 1 : 0)
            case "PipSize": controller.toggleSize(); sp_shell_set_pip(controller.core.handle, Int32(controller.cornerIndex), controller.pinnedLarge ? 1 : 0)
            case "OpenEnvironments":
                if controller.core.environmentImageCount == 0 {
                    controller.hint.say("No panoramas yet. Add an image here, or get the default ones in Spatiand's settings on this Mac.")
                }
            case "OpenHosts": controller.syncHosts()
            case "ReturnToDesktop": controller.leave()
            case "ToggleKeyboard": controller.hint.say("Type on this Mac's keyboard: it goes to the window in front of you.")
            case "ControllerLayout": PadInput.shared.openEditor(for: controller.core.aim().window ?? controller.core.focused)
            case "Record": controller.toggleRecording()
            case "Calibrate": controller.hint.say("The Mac learns the glasses' sensors as they are worn, and remembers them.")
            default: break
            }
        } else if let name = event["controller"] as? String {
            PadInput.shared.editorInput(name)
        } else if let row = event["controller_click"] as? Int {
            PadInput.shared.editorClick(row)
        } else if let focus = event["focus"] as? Int {
            if focus >= 0xE000 && focus < 0xF000 {
                controller.bringCandidate(UInt32(focus))
            } else {
                controller.focusFromList(UInt16(truncatingIfNeeded: focus))
            }
        } else if let close = event["close"] as? Int {
            controller.closeWindow(UInt16(truncatingIfNeeded: close))
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { [weak self] in self?.controller.core.markShellDirty() }
        } else if let remote = event["launch_remote"] as? [String: Any], let host = remote["host"] as? String, let app = remote["app"] as? String {
            controller.launch(app: app, on: host)
        } else if let address = event["pair"] as? String {
            NotificationCenter.default.post(name: .spatiandPair, object: address)
        } else if let address = event["forget"] as? String {
            controller.forgetHost(address: address)
        }
    }
}

extension Notification.Name {
    static let spatiandPair = Notification.Name("spatiand.pair")
    static let spatiandLeave = Notification.Name("spatiand.leave")
}
