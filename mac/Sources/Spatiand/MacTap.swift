//  MacTap.swift — one Mac application's sound, taken from the speakers and put in the room.
//
//  On the Deck every window's sound is placed where the window is and follows the head. A Mac application plays
//  straight to the Mac's output, so to place it the sound has to be taken away from there: a Core Audio process tap
//  (macOS 14.2) gives an application's output to Spatiand and, muted when tapped, takes it off the speakers while it
//  is held, so it is heard once, from where its window is. Letting go of the tap -- or Spatiand quitting -- gives
//  the application its speakers back.
//
//  An application's sound is made by more than its main process: a browser plays from a helper. So the tap takes
//  every process that is the application or one of its children.

import AudioToolbox
import CoreAudio
import Foundation

@available(macOS 14.2, *)
final class MacTap {
    let bundle: String
    private let appPID: pid_t
    private var tap = AudioObjectID(kAudioObjectUnknown)
    private var aggregate = AudioObjectID(kAudioObjectUnknown)
    private var procID: AudioDeviceIOProcID?
    private let queue = DispatchQueue(label: "spatiand.mactap", qos: .userInteractive)
    private var processes: Set<AudioObjectID> = []
    /// Interleaved stereo floats at the device's rate, from the audio thread.
    var onSound: (([Float], Double) -> Void)?
    private(set) var running = false
    private(set) var lastError = ""

    init(bundle: String, pid: pid_t) { self.bundle = bundle; appPID = pid }

    // MARK: finding who plays

    private static func property<T>(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector, as: T.Type, default value: T) -> T {
        var address = AudioObjectPropertyAddress(mSelector: selector, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        var out = value
        var size = UInt32(MemoryLayout<T>.size)
        AudioObjectGetPropertyData(object, &address, 0, nil, &size, &out)
        return out
    }

    /// Whether `pid` is `ancestor` or one of its descendants.
    static func descends(_ pid: pid_t, from ancestor: pid_t) -> Bool {
        var current = pid
        for _ in 0..<12 {
            if current == ancestor { return true }
            var info = kinfo_proc()
            var size = MemoryLayout<kinfo_proc>.size
            var mib: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PID, current]
            guard sysctl(&mib, 4, &info, &size, nil, 0) == 0, size > 0 else { return false }
            let parent = info.kp_eproc.e_ppid
            if parent <= 1 || parent == current { return false }
            current = parent
        }
        return false
    }

    /// Every audio process object that belongs to the application: itself and what it started.
    func processObjects() -> [AudioObjectID] {
        var address = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyProcessObjectList, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        var size: UInt32 = 0
        guard AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size) == noErr, size > 0 else { return [] }
        var list = [AudioObjectID](repeating: 0, count: Int(size) / MemoryLayout<AudioObjectID>.size)
        guard AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &list) == noErr else { return [] }
        return list.filter { object in
            let pid = Self.property(object, kAudioProcessPropertyPID, as: pid_t.self, default: -1)
            return pid > 0 && Self.descends(pid, from: appPID)
        }
    }

    // MARK: tapping

    /// Begin. False, with `lastError` saying why, if the sound cannot be taken (nothing of the application has played
    /// yet, or the permission was refused).
    @discardableResult
    func start() -> Bool {
        stop()
        let objects = processObjects()
        guard !objects.isEmpty else { lastError = "nothing of \(bundle) is playing yet"; return false }
        let description = CATapDescription(stereoMixdownOfProcesses: objects)
        description.uuid = UUID()
        description.muteBehavior = .mutedWhenTapped
        description.isPrivate = true
        var made = AudioObjectID(kAudioObjectUnknown)
        let status = AudioHardwareCreateProcessTap(description, &made)
        guard status == noErr else { lastError = "the tap was refused (\(status))"; return false }
        tap = made
        processes = Set(objects)

        // The tap is read through a private aggregate device that has it in its tap list.
        let output = Self.defaultOutputUID()
        let spec: [String: Any] = [
            kAudioAggregateDeviceNameKey: "Spatiand \(bundle)",
            kAudioAggregateDeviceUIDKey: UUID().uuidString,
            kAudioAggregateDeviceMainSubDeviceKey: output,
            kAudioAggregateDeviceIsPrivateKey: true,
            kAudioAggregateDeviceIsStackedKey: false,
            kAudioAggregateDeviceTapAutoStartKey: true,
            kAudioAggregateDeviceSubDeviceListKey: [[kAudioSubDeviceUIDKey: output]],
            kAudioAggregateDeviceTapListKey: [[kAudioSubTapDriftCompensationKey: true, kAudioSubTapUIDKey: description.uuid.uuidString]],
        ]
        var device = AudioObjectID(kAudioObjectUnknown)
        guard AudioHardwareCreateAggregateDevice(spec as CFDictionary, &device) == noErr else { lastError = "no device for the tap"; stop(); return false }
        aggregate = device

        var format = AudioStreamBasicDescription()
        var address = AudioObjectPropertyAddress(mSelector: kAudioTapPropertyFormat, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        var size = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
        AudioObjectGetPropertyData(tap, &address, 0, nil, &size, &format)
        let channels = max(1, Int(format.mChannelsPerFrame))
        let rate = format.mSampleRate > 0 ? format.mSampleRate : 48_000
        let interleaved = format.mFormatFlags & kAudioFormatFlagIsNonInterleaved == 0

        let result = AudioDeviceCreateIOProcIDWithBlock(&procID, aggregate, queue) { [weak self] _, input, _, _, _ in
            guard let self, let sink = self.onSound else { return }
            let buffers = UnsafeMutableAudioBufferListPointer(UnsafeMutablePointer(mutating: input))
            guard let first = buffers.first, let data = first.mData else { return }
            let frames = Int(first.mDataByteSize) / (MemoryLayout<Float>.size * (interleaved ? channels : 1))
            guard frames > 0 else { return }
            var out = [Float](repeating: 0, count: frames * 2)
            if interleaved {
                let p = data.assumingMemoryBound(to: Float.self)
                for i in 0..<frames {
                    out[i * 2] = p[i * channels]
                    out[i * 2 + 1] = p[i * channels + min(1, channels - 1)]
                }
            } else {
                let l = data.assumingMemoryBound(to: Float.self)
                let r = buffers.count > 1 ? buffers[1].mData?.assumingMemoryBound(to: Float.self) : nil
                for i in 0..<frames { out[i * 2] = l[i]; out[i * 2 + 1] = r?[i] ?? l[i] }
            }
            sink(out, rate)
        }
        guard result == noErr, let procID, AudioDeviceStart(aggregate, procID) == noErr else { lastError = "the tap could not be read"; stop(); return false }
        running = true
        return true
    }

    func stop() {
        running = false
        if let procID, aggregate != kAudioObjectUnknown {
            AudioDeviceStop(aggregate, procID)
            AudioDeviceDestroyIOProcID(aggregate, procID)
        }
        procID = nil
        if aggregate != kAudioObjectUnknown { AudioHardwareDestroyAggregateDevice(aggregate) }
        aggregate = AudioObjectID(kAudioObjectUnknown)
        if tap != kAudioObjectUnknown { AudioHardwareDestroyProcessTap(tap) }
        tap = AudioObjectID(kAudioObjectUnknown)
    }

    /// Whether any of the application's processes is producing sound at this moment, as Core Audio knows it.
    func isPlaying() -> Bool {
        processObjects().contains { Self.property($0, kAudioProcessPropertyIsRunningOutput, as: UInt32.self, default: 0) != 0 }
    }

    /// Whether more of the application's processes are playing now than when the tap was made: a browser opening a
    /// tab's helper. The tap is then made again, with them in it.
    func needsRefresh() -> Bool { !Set(processObjects()).isSubset(of: processes) }

    private static func defaultOutputUID() -> String {
        let device = property(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDefaultOutputDevice, as: AudioObjectID.self, default: 0)
        var address = AudioObjectPropertyAddress(mSelector: kAudioDevicePropertyDeviceUID, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        var uid: Unmanaged<CFString>?
        var size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
        AudioObjectGetPropertyData(device, &address, 0, nil, &size, &uid)
        return (uid?.takeRetainedValue() as String?) ?? ""
    }

    deinit { stop() }
}
