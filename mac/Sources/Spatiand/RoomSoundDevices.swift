//  RoomSoundDevices.swift — this Mac's sound devices, for the room's settings.
//
//  With glasses on, the menu bar that would choose where sound comes out is out of sight. The
//  room's settings list the devices instead, and choosing one makes it the Mac's own default:
//  what the menu bar's choice would have done.

import CoreAudio
import CSpatiand

enum RoomSoundDevices {
    private static func address(_ selector: AudioObjectPropertySelector, _ scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress(mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain)
    }

    private static func all() -> [AudioDeviceID] {
        var a = address(kAudioHardwarePropertyDevices)
        var size: UInt32 = 0
        guard AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &a, 0, nil, &size) == noErr else { return [] }
        var ids = [AudioDeviceID](repeating: 0, count: Int(size) / MemoryLayout<AudioDeviceID>.size)
        guard AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &a, 0, nil, &size, &ids) == noErr else { return [] }
        return ids
    }

    private static func has(_ id: AudioDeviceID, _ scope: AudioObjectPropertyScope) -> Bool {
        var a = address(kAudioDevicePropertyStreams, scope)
        var size: UInt32 = 0
        return AudioObjectGetPropertyDataSize(id, &a, 0, nil, &size) == noErr && size > 0
    }

    private static func name(_ id: AudioDeviceID) -> String {
        var a = address(kAudioObjectPropertyName)
        var value: Unmanaged<CFString>?
        var size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
        guard AudioObjectGetPropertyData(id, &a, 0, nil, &size, &value) == noErr, let value else { return "Device \(id)" }
        return value.takeRetainedValue() as String
    }

    private static func current(_ selector: AudioObjectPropertySelector) -> AudioDeviceID {
        var a = address(selector)
        var id = AudioDeviceID(0)
        var size = UInt32(MemoryLayout<AudioDeviceID>.size)
        AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &a, 0, nil, &size, &id)
        return id
    }

    /// Tell the compositor what there is: where sound can come out, then the microphones.
    static func say() {
        let ids = all()
        let (out, mic) = (current(kAudioHardwarePropertyDefaultOutputDevice), current(kAudioHardwarePropertyDefaultInputDevice))
        sp_audio_devices_begin()
        for id in ids where has(id, kAudioObjectPropertyScopeOutput) { sp_audio_device(id, name(id), false, id == out) }
        for id in ids where has(id, kAudioObjectPropertyScopeInput) { sp_audio_device(id, name(id), true, id == mic) }
        sp_audio_devices_end()
    }

    /// Make a device the Mac's own, as the menu bar would, and say the list again.
    static func use(_ id: AudioDeviceID, input: Bool) {
        var a = address(input ? kAudioHardwarePropertyDefaultInputDevice : kAudioHardwarePropertyDefaultOutputDevice)
        var device = id
        let status = AudioObjectSetPropertyData(AudioObjectID(kAudioObjectSystemObject), &a, 0, nil, UInt32(MemoryLayout<AudioDeviceID>.size), &device)
        print("sound: \(input ? "the microphone" : "the output") is now \(name(id))\(status == noErr ? "" : " -- refused (\(status))")")
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { say() }
    }
}
