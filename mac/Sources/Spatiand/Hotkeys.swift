//  Hotkeys.swift — Ctrl-Space and Ctrl-Tab, from anywhere.
//
//  On the Deck the launcher is the `⋯` button and the settings are STEAM. A Mac has neither, so
//  two key chords stand in: Ctrl-Space opens Spatiand's menu and Ctrl-Tab opens its settings.
//
//  Registered as Carbon hot keys, which need no Accessibility permission -- the system delivers
//  them to whoever registered, and asks for nothing. The cost is that a chord the system itself
//  has claimed cannot be had, and Ctrl-Space is exactly that on a Mac with a second input source
//  switched on in Keyboard settings; `status` says so rather than failing quietly.

import Carbon.HIToolbox
import Foundation

final class Hotkeys {
    enum Chord: UInt32 {
        case menu = 1       // Ctrl-Space
        case settings = 2   // Ctrl-Tab
    }

    var onMenu: (() -> Void)?
    var onSettings: (() -> Void)?
    /// Whether each chord was granted, for the settings window to say.
    private(set) var status: [Chord: Bool] = [:]
    private var refs: [Chord: EventHotKeyRef] = [:]
    private var handler: EventHandlerRef?

    func enable() {
        disable()
        var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        let me = Unmanaged.passUnretained(self).toOpaque()
        InstallEventHandler(GetApplicationEventTarget(), { _, event, user in
            guard let event, let user else { return noErr }
            var id = EventHotKeyID()
            GetEventParameter(event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID),
                              nil, MemoryLayout<EventHotKeyID>.size, nil, &id)
            let keys = Unmanaged<Hotkeys>.fromOpaque(user).takeUnretainedValue()
            DispatchQueue.main.async {
                switch Chord(rawValue: id.id) {
                case .menu: keys.onMenu?()
                case .settings: keys.onSettings?()
                case nil: break
                }
            }
            return noErr
        }, 1, &spec, me, &handler)
        register(.menu, key: UInt32(kVK_Space))
        register(.settings, key: UInt32(kVK_Tab))
    }

    func disable() {
        for ref in refs.values { UnregisterEventHotKey(ref) }
        refs = [:]
        status = [:]
        if let handler { RemoveEventHandler(handler) }
        handler = nil
    }

    private func register(_ chord: Chord, key: UInt32) {
        var ref: EventHotKeyRef?
        let id = EventHotKeyID(signature: OSType(0x5370_6174), id: chord.rawValue)   // 'Spat'
        let result = RegisterEventHotKey(key, UInt32(controlKey), id, GetApplicationEventTarget(), 0, &ref)
        status[chord] = result == noErr
        if result == noErr, let ref { refs[chord] = ref }
    }
}


/// Ctrl-Option and a letter, registered while the room has the mouse and keyboard. With a window of this Mac
/// in front the keyboard is that application's and Spatiand hears no keys of its own, so its chords are
/// asked of the system instead.
final class RoomChords {
    var onChord: ((Int) -> Void)?
    private var refs: [EventHotKeyRef] = []
    private var handler: EventHandlerRef?
    private static let keys = [kVK_ANSI_G, kVK_ANSI_R, kVK_ANSI_P, kVK_ANSI_B, kVK_ANSI_C, kVK_ANSI_S, kVK_ANSI_W]

    func enable() {
        disable()
        var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        let me = Unmanaged.passUnretained(self).toOpaque()
        InstallEventHandler(GetApplicationEventTarget(), { _, event, user in
            guard let event, let user else { return noErr }
            var id = EventHotKeyID()
            GetEventParameter(event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID),
                              nil, MemoryLayout<EventHotKeyID>.size, nil, &id)
            guard id.signature == OSType(0x5370_6143) else { return OSStatus(eventNotHandledErr) }   // 'SpaC'
            let chords = Unmanaged<RoomChords>.fromOpaque(user).takeUnretainedValue()
            DispatchQueue.main.async { chords.onChord?(Int(id.id)) }
            return noErr
        }, 1, &spec, me, &handler)
        for key in Self.keys {
            var ref: EventHotKeyRef?
            let id = EventHotKeyID(signature: OSType(0x5370_6143), id: UInt32(key))
            if RegisterEventHotKey(UInt32(key), UInt32(controlKey | optionKey), id, GetApplicationEventTarget(), 0, &ref) == noErr, let ref { refs.append(ref) }
        }
    }

    func disable() {
        for ref in refs { UnregisterEventHotKey(ref) }
        refs = []
        if let handler { RemoveEventHandler(handler) }
        handler = nil
    }
}
