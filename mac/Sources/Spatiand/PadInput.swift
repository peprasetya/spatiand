//  PadInput.swift — a game controller, on this Mac, doing what it does on the Deck and the Beam Pro.
//
//  Whatever GameController finds -- a DualShock 4 on a cable or over Bluetooth, an Xbox pad, a
//  DualSense -- is read a hundred and twenty times a second, merged into one controller, and run
//  through the focused application's layout by the same engine the Deck uses (see `pads.rs`). What
//  comes out goes where it goes there:
//
//  * the pad as a gamepad, to the host that owns the window in front, and whatever that game
//    shakes it with comes back as rumble;
//  * keys, to the window in front: on the desktop layout the D-pad is the arrow keys, A is Enter
//    and B is Escape;
//  * in the room, the pointer: the touchpad slides it, its press and the triggers click, the
//    sticks scroll, and the PS button is the menu (held, the launcher), which the D-pad, A and B
//    then work.
//
//  A layout saved on the Deck (`~/.config/spatiand/layouts`) works here when it is copied to
//  `~/Library/Application Support/Spatiand/layouts`.

import AppKit
import CSpatiand
import CoreHaptics
import GameController

final class PadInput {
    static let shared = PadInput()

    private var core: OpaquePointer?
    private var timer: Timer?
    private var focusedApp = ""
    private var lastPointer: UInt32 = 0
    private var lastReport: [String: Any]?
    private var lastTarget: UInt16?
    private var engines: [ObjectIdentifier: CHHapticEngine] = [:]
    private var player: CHHapticAdvancedPatternPlayer?
    private(set) var names: [String] = []
    /// What the last frame read and made of it, for the test that watches a pad.
    private(set) var debugLine = ""
    private var model: Model { Model.shared }

    private var enabledAt = Date.distantPast
    private var enabledNow = true
    /// Read from the preferences once a second, not a hundred and twenty times.
    var enabled: Bool {
        if Date().timeIntervalSince(enabledAt) > 1 {
            enabledAt = Date()
            enabledNow = Defaults.store.object(forKey: "gamepads") as? Bool ?? true
        }
        return enabledNow
    }

    func start() {
        guard timer == nil else { return }
        let dir = NSString("~/Library/Application Support/Spatiand/layouts").expandingTildeInPath
        core = sp_pads_new(dir)
        PadTouchpad.shared.start()
        // The room is for the glasses, and the Mac is not looking at Spatiand while they are on.
        GCController.shouldMonitorBackgroundEvents = true
        let center = NotificationCenter.default
        center.addObserver(forName: .GCControllerDidConnect, object: nil, queue: .main) { [weak self] n in
            if let c = n.object as? GCController { self?.arrived(c) }
        }
        center.addObserver(forName: .GCControllerDidDisconnect, object: nil, queue: .main) { [weak self] n in
            if let c = n.object as? GCController { self?.left(c) }
        }
        for c in GCController.controllers() { arrived(c) }
        let t = Timer(timeInterval: 1.0 / 120.0, repeats: true) { [weak self] _ in self?.tick() }
        RunLoop.main.add(t, forMode: .common)
        timer = t
    }

    func stop() {
        for c in GCController.controllers() { claimSystemButtons(c, false) }
        timer?.invalidate()
        timer = nil
        if let core { sp_pads_free(core) }
        core = nil
    }

    /// macOS gives a pad's PS or Home button to Game Center (and its Share button to the screen
    /// recorder). While Spatiand is on and using the pad, those buttons are Spatiand's: the PS button
    /// is the menu. Switched back when pads are turned off or Spatiand quits.
    private func claimSystemButtons(_ c: GCController, _ claim: Bool) {
        for button in c.physicalInputProfile.allButtons {
            button.preferredSystemGestureState = claim ? .disabled : .enabled
        }
    }

    private var claimed = false

    private func arrived(_ c: GCController) {
        claimSystemButtons(c, enabled)
        let kind = c.extendedGamepad is GCDualShockGamepad ? "DualShock 4" : (c.extendedGamepad is GCDualSenseGamepad ? "DualSense" : (c.productCategory))
        names.append("\(c.vendorName ?? "controller") (\(kind))")
        print("pad: \(c.vendorName ?? "a controller") arrived, \(kind), motion \(c.motion != nil)")
        c.motion?.sensorsActive = true
    }

    private func left(_ c: GCController) {
        names.removeAll { $0.hasPrefix(c.vendorName ?? "controller") }
        print("pad: \(c.vendorName ?? "a controller") left")
        engines[ObjectIdentifier(c)] = nil
    }

    // MARK: reading

    private func read() -> sp_pad_in? {
        var input = sp_pad_in()
        var any = false
        var touchedPad = false
        for c in GCController.controllers() {
            guard let g = c.extendedGamepad else { continue }
            any = true
            var bits: UInt32 = 0
            func set(_ b: GCControllerButtonInput?, _ bit: UInt32) { if b?.isPressed == true { bits |= bit } }
            set(g.buttonA, 1 << 0); set(g.buttonB, 1 << 1); set(g.buttonX, 1 << 2); set(g.buttonY, 1 << 3)
            set(g.dpad.up, 1 << 4); set(g.dpad.down, 1 << 5); set(g.dpad.left, 1 << 6); set(g.dpad.right, 1 << 7)
            set(g.leftShoulder, 1 << 8); set(g.rightShoulder, 1 << 9)
            set(g.leftTrigger, 1 << 10); set(g.rightTrigger, 1 << 11)
            set(g.buttonOptions, 1 << 12); set(g.buttonMenu, 1 << 13); set(g.buttonHome, 1 << 14)
            set(g.leftThumbstickButton, 1 << 15); set(g.rightThumbstickButton, 1 << 16)
            input.buttons |= bits
            // Whichever stick or trigger is pushed further, as the Deck merges two pads.
            func further(_ x: Float, _ y: Float, _ ox: inout Float, _ oy: inout Float) {
                if x * x + y * y > ox * ox + oy * oy { ox = x; oy = y }
            }
            further(g.leftThumbstick.xAxis.value, g.leftThumbstick.yAxis.value, &input.lx, &input.ly)
            further(g.rightThumbstick.xAxis.value, g.rightThumbstick.yAxis.value, &input.rx, &input.ry)
            input.lt = max(input.lt, g.leftTrigger.value)
            input.rt = max(input.rt, g.rightTrigger.value)
            // A PlayStation pad's touchpad: where a thumb is, centred when there is none, and
            // pressed as a whole as a button.
            let touchpad: (GCControllerDirectionPad, GCControllerButtonInput)?
            if let d = g as? GCDualShockGamepad { touchpad = (d.touchpadPrimary, d.touchpadButton) }
            else if let d = g as? GCDualSenseGamepad { touchpad = (d.touchpadPrimary, d.touchpadButton) }
            else { touchpad = nil }
            if let (pad, button) = touchpad {
                let x = pad.xAxis.value, y = pad.yAxis.value
                let down = x != 0 || y != 0 || button.isPressed
                if down && !touchedPad {
                    touchedPad = true
                    input.touch_x = x; input.touch_y = y
                    input.touched = 1
                }
                if button.isPressed { input.clicked = 1 }
            }
            if let motion = c.motion, motion.hasRotationRate, input.has_gyro == 0 {
                let r = motion.rotationRate
                let deg = Float(180.0 / Double.pi)
                input.has_gyro = 1
                input.gyro = (Float(r.x) * deg, Float(r.y) * deg, Float(r.z) * deg)
            }
        }
        // Where GameController says nothing of a touchpad, the pad's own reports do.
        if !touchedPad, let t = PadTouchpad.shared.current() {
            any = true
            if t.touched { input.touch_x = t.x; input.touch_y = t.y; input.touched = 1 }
            if t.clicked { input.clicked = 1 }
            // The PS button, which macOS does not always pass on through GameController.
            if t.ps { input.buttons |= 1 << 14 }
        }
        return any ? input : nil
    }

    // MARK: each frame

    private func targetWindow() -> UInt16? {
        if model.room.active { return model.room.core.focused }
        for (id, surface) in model.windows where (surface as? RemoteWindow)?.view.window?.isKeyWindow == true { return id }
        return nil
    }

    private func appName(of id: UInt16?) -> String {
        guard let id else { return "" }
        if MacWindows.isMac(id) { return "mac." + (model.room.macInfo(id)?.bundle ?? "") }
        return model.infos[id]?.app ?? ""
    }

    private func tick() {
        if enabled != claimed {
            claimed = enabled
            for c in GCController.controllers() { claimSystemButtons(c, enabled) }
        }
        guard enabled, let core else { return }
        guard var input = read() else {
            // The pad has gone: let go of anything it was holding.
            if lastReport != nil || lastPointer != 0 || lastTarget != nil { release() }
            return
        }
        let room = model.room
        let target = targetWindow()
        let app = appName(of: target)
        if app != focusedApp {
            focusedApp = app
            sp_pads_focus(core, app)
        }
        input.suspended = room.menu.isOpen ? 1 : 0
        var out = sp_pad_out()
        sp_pads_step(core, &input, &out)
        debugLine = String(format: "buttons %05x sticks %.2f,%.2f %.2f,%.2f triggers %.2f %.2f touch %@ at %.2f,%.2f click %d gyro %@ -> pointer %d motion %.1f,%.1f keys %d pad %04x",
                           input.buttons, input.lx, input.ly, input.rx, input.ry, input.lt, input.rt,
                           input.touched != 0 ? "DOWN" : "up", input.touch_x, input.touch_y, input.clicked,
                           input.has_gyro != 0 ? String(format: "%.0f,%.0f,%.0f", input.gyro.0, input.gyro.1, input.gyro.2) : "none",
                           out.pointer, out.motion_x, out.motion_y, out.key_count, out.pad_buttons)

        // The pad, as a gamepad, to the host that owns the window in front.
        if target != lastTarget, let old = lastTarget, !MacWindows.isMac(old) { sendPad(neutral: true) }
        lastTarget = target
        if let id = target, !MacWindows.isMac(id), model.infos[id] != nil {
            let report: [String: Any] = [
                "buttons": Int(out.pad_buttons), "dpad": Int(out.dpad),
                "left": [out.left.0, out.left.1], "right": [out.right.0, out.right.1],
                "triggers": [out.triggers.0, out.triggers.1], "extra": [0, 0, 0, 0],
            ]
            if lastReport.map({ NSDictionary(dictionary: $0).isEqual(to: report) }) != true {
                lastReport = report
                model.link.say(["Pad": report])
            }
        }

        // Keys, to the window in front.
        let count = Int(out.key_count)
        withUnsafeBytes(of: &out.key_code) { codes in
            withUnsafeBytes(of: &out.key_down) { downs in
                for i in 0..<min(count, 32) {
                    key(code: codes.load(fromByteOffset: i * 4, as: UInt32.self), down: downs[i] != 0, in: target)
                }
            }
        }

        if room.active {
            // The pointer.
            if out.motion_x != 0 || out.motion_y != 0 { room.pointerMoved(dx: Double(out.motion_x), dy: Double(out.motion_y)) }
            let changed = out.pointer ^ lastPointer
            for (bit, code) in [(UInt32(1), 0x110), (2, 0x111), (4, 0x112)] where changed & bit != 0 {
                if out.pointer & bit != 0 { room.buttonDown(code, grab: false) } else { room.buttonUp(code) }
            }
            lastPointer = out.pointer
            if out.wheel_x != 0 || out.wheel_y != 0 { room.scrolled(dx: Double(out.wheel_x) * 40, dy: Double(out.wheel_y) * 40, precise: true) }
            // The PS button is the Deck's STEAM: the settings; held, its ⋯: the launcher.
            if out.guide != 0 || out.commands & 1 != 0 { room.toggleSettings() }
            if out.guide_held != 0 || out.commands & 2 != 0 { room.toggleMenu() }
            if out.commands & 16 != 0 { room.recentre() }
            if out.menu_presses != 0 { room.menu.padPress(out.menu_presses) }
        }
    }

    private func key(code: UInt32, down: Bool, in target: UInt16?) {
        guard let id = target else { return }
        if MacWindows.isMac(id) {
            guard let info = model.room.macInfo(id), let mac = Self.macKey[code] else { return }
            MacInput.key(code: mac, flags: [], down: down, pid: info.pid)
        } else {
            model.link.say(["InputAt": ["window": Int(id), "input": ["Key": ["code": Int(code), "pressed": down]],
                                        "time_ms": Int(ProcessInfo.processInfo.systemUptime * 1000)]])
        }
    }

    private static let macKey: [UInt32: UInt16] = {
        var table: [UInt32: UInt16] = [:]
        for (mac, evdev) in KeyMap.evdev where table[evdev] == nil { table[evdev] = mac }
        return table
    }()

    private func sendPad(neutral: Bool) {
        lastReport = nil
        model.link.say(["Pad": ["buttons": 0, "dpad": 0, "left": [0, 0], "right": [0, 0], "triggers": [0, 0], "extra": [0, 0, 0, 0]]])
    }

    /// Let go of everything: a pad unplugged mid-press must not leave a button down or a stick over.
    private func release() {
        if lastReport != nil { sendPad(neutral: true) }
        if lastPointer != 0 {
            for (bit, code) in [(UInt32(1), 0x110), (2, 0x111), (4, 0x112)] where lastPointer & bit != 0 { model.room.buttonUp(code) }
            lastPointer = 0
        }
        lastTarget = nil
    }

    // MARK: rumble

    /// What a game asked the motors to do, strong and weak, out of 65535; played on the pad.
    func rumble(strong: Int, weak: Int) {
        try? player?.stop(atTime: CHHapticTimeImmediate)
        player = nil
        let level = Float(max(strong, weak)) / 65535
        guard level > 0.01 else { return }
        for c in GCController.controllers() {
            guard let haptics = c.haptics else { continue }
            let id = ObjectIdentifier(c)
            let engine = engines[id] ?? haptics.createEngine(withLocality: .default)
            guard let engine else { continue }
            engines[id] = engine
            do {
                try engine.start()
                let event = CHHapticEvent(eventType: .hapticContinuous, parameters: [
                    CHHapticEventParameter(parameterID: .hapticIntensity, value: level),
                    CHHapticEventParameter(parameterID: .hapticSharpness, value: strong > weak ? 0.2 : 0.8),
                ], relativeTime: 0, duration: 1.0)
                let p = try engine.makeAdvancedPlayer(with: CHHapticPattern(events: [event], parameters: []))
                try p.start(atTime: CHHapticTimeImmediate)
                player = p
            } catch {
                print("pad: rumble failed: \(error)")
            }
        }
    }
}
