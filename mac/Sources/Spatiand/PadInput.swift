//  PadInput.swift — game controllers, handed to the room.
//
//  Whatever GameController finds -- a DualShock 4 on a cable or over Bluetooth, an Xbox pad, a
//  DualSense -- is read a hundred and twenty times a second and told to the compositor as it is:
//  buttons, sticks, triggers, a PlayStation pad's touchpad and gyro. What a button *does* is not
//  decided here. The compositor runs the pad through the same layouts as the Deck: its home
//  button is the settings, its D-pad and face buttons work the menus, its touchpad is the
//  pointer, and a game on a host is played with it.

import AppKit
import CSpatiand
import CoreHaptics
import GameController

final class PadInput {
    static let shared = PadInput()

    private var timer: Timer?
    /// Each controller's number for the compositor, and the buttons it last had down.
    private var numbers: [ObjectIdentifier: Int32] = [:]
    private var held: [Int32: UInt32] = [:]
    private var next: Int32 = 1
    private var engines: [ObjectIdentifier: CHHapticEngine] = [:]
    private var players: [ObjectIdentifier: CHHapticAdvancedPatternPlayer] = [:]
    private(set) var names: [String] = []
    private var claimed = false

    var enabled: Bool { Defaults.store.object(forKey: "gamepads") as? Bool ?? true }

    func start() {
        guard timer == nil else { return }
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
    }

    /// macOS gives a pad's PS or Home button to Game Center (and its Share button to the screen
    /// recorder). While Spatiand is using the pad, those buttons are Spatiand's.
    private func claimSystemButtons(_ c: GCController, _ claim: Bool) {
        for button in c.physicalInputProfile.allButtons {
            button.preferredSystemGestureState = claim ? .disabled : .enabled
        }
    }

    private func arrived(_ c: GCController) {
        claimSystemButtons(c, enabled)
        numbers[ObjectIdentifier(c)] = next
        next += 1
        names.append(c.vendorName ?? "controller")
        print("pad: \(c.vendorName ?? "a controller") arrived, motion \(c.motion != nil)")
        c.motion?.sensorsActive = true
    }

    private func left(_ c: GCController) {
        let key = ObjectIdentifier(c)
        if let n = numbers.removeValue(forKey: key) {
            held[n] = nil
            sp_pad_gone(n)
        }
        names.removeAll { $0 == (c.vendorName ?? "controller") }
        engines[key] = nil
        players[key] = nil
        print("pad: \(c.vendorName ?? "a controller") left")
    }

    private func tick() {
        if enabled != claimed {
            claimed = enabled
            for c in GCController.controllers() { claimSystemButtons(c, enabled) }
        }
        guard enabled else { return }
        // What the pad's own reports say where GameController says nothing: a DualShock clone's
        // touchpad, and the PS button, which macOS does not always pass on.
        let raw = PadTouchpad.shared.current()
        var first = true
        for c in GCController.controllers() {
            guard let g = c.extendedGamepad, let n = numbers[ObjectIdentifier(c)] else { continue }
            var bits: UInt32 = 0
            func set(_ b: GCControllerButtonInput?, _ bit: Int) { if b?.isPressed == true { bits |= 1 << UInt32(bit) } }
            set(g.buttonA, 0); set(g.buttonB, 1); set(g.buttonX, 2); set(g.buttonY, 3)
            set(g.leftShoulder, 4); set(g.rightShoulder, 5); set(g.leftTrigger, 6); set(g.rightTrigger, 7)
            set(g.buttonOptions, 8); set(g.buttonMenu, 9); set(g.buttonHome, 10)
            set(g.leftThumbstickButton, 11); set(g.rightThumbstickButton, 12)
            set(g.dpad.up, 13); set(g.dpad.down, 14); set(g.dpad.left, 15); set(g.dpad.right, 16)
            if first, raw?.ps == true { bits |= 1 << 10 }
            let before = held[n] ?? 0
            for bit in 0..<17 where (bits ^ before) & (1 << UInt32(bit)) != 0 {
                sp_pad_button(n, Int32(bit), bits & (1 << UInt32(bit)) != 0)
            }
            held[n] = bits
            sp_pad_axes(n, g.leftThumbstick.xAxis.value, g.leftThumbstick.yAxis.value, g.rightThumbstick.xAxis.value,
                        g.rightThumbstick.yAxis.value, g.leftTrigger.value, g.rightTrigger.value)

            let touchpad: (GCControllerDirectionPad, GCControllerButtonInput)?
            if let d = g as? GCDualShockGamepad { touchpad = (d.touchpadPrimary, d.touchpadButton) }
            else if let d = g as? GCDualSenseGamepad { touchpad = (d.touchpadPrimary, d.touchpadButton) }
            else { touchpad = nil }
            if let (pad, button) = touchpad, pad.xAxis.value != 0 || pad.yAxis.value != 0 || button.isPressed {
                sp_pad_touch(n, pad.xAxis.value, pad.yAxis.value, pad.xAxis.value != 0 || pad.yAxis.value != 0, button.isPressed)
            } else if first, let raw {
                sp_pad_touch(n, raw.x, raw.y, raw.touched, raw.clicked)
            } else if touchpad != nil {
                sp_pad_touch(n, 0, 0, false, false)
            }
            if let motion = c.motion, motion.hasRotationRate {
                let r = motion.rotationRate
                let deg = Float(180.0 / Double.pi)
                sp_pad_gyro(n, Float(r.x) * deg, Float(r.y) * deg, Float(r.z) * deg)
            }
            first = false
        }
        let rumble = sp_pad_rumble()
        if rumble >= 0 { play(strong: Float((rumble >> 16) & 0xFFFF) / 65535, weak: Float(rumble & 0xFFFF) / 65535) }
    }

    /// What a game asked the motors for, on every pad that has any.
    private func play(strong: Float, weak: Float) {
        let level = max(strong, weak)
        for c in GCController.controllers() {
            let key = ObjectIdentifier(c)
            guard let haptics = c.haptics else { continue }
            if level <= 0.01 {
                try? players[key]?.stop(atTime: CHHapticTimeImmediate)
                players[key] = nil
                continue
            }
            do {
                if engines[key] == nil {
                    let engine = haptics.createEngine(withLocality: .default)
                    try engine?.start()
                    engines[key] = engine
                }
                guard let engine = engines[key] else { continue }
                let event = CHHapticEvent(eventType: .hapticContinuous, parameters: [
                    CHHapticEventParameter(parameterID: .hapticIntensity, value: level),
                    CHHapticEventParameter(parameterID: .hapticSharpness, value: weak > strong ? 0.8 : 0.3),
                ], relativeTime: 0, duration: 10)
                try? players[key]?.stop(atTime: CHHapticTimeImmediate)
                let player = try engine.makeAdvancedPlayer(with: CHHapticPattern(events: [event], parameters: []))
                try player.start(atTime: CHHapticTimeImmediate)
                players[key] = player
            } catch {
                engines[key] = nil
            }
        }
    }
}
