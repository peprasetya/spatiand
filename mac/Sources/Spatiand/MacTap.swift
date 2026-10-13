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

    init(bundle: String, pid: pid_t) { self.bundle = bundle; appPID = pid; excluding = nil }

    /// Everything this Mac plays, but for some applications -- the ones whose sound is taken by
    /// taps of their own -- and Spatiand itself: the sound of a whole screen.
    private let excluding: [pid_t]?
    init(everythingBut pids: [pid_t]) {
        bundle = MacScreens.bundle
        appPID = 0
        excluding = pids + [ProcessInfo.processInfo.processIdentifier]
    }
    var isEverything: Bool { excluding != nil }

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
            // For a tap of everything, these are the ones left out of it.
            if let excluding { return pid > 0 && excluding.contains { Self.descends(pid, from: $0) } }
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
        guard !objects.isEmpty || excluding != nil else { lastError = "nothing of \(bundle) is playing yet"; return false }
        if ProcessInfo.processInfo.environment["SPATIAND_DEBUG_TAP"] != nil {
            for o in objects { print("tap: process object \(o) pid \(Self.property(o, kAudioProcessPropertyPID, as: pid_t.self, default: -1)) running output \(Self.property(o, kAudioProcessPropertyIsRunningOutput, as: UInt32.self, default: 9))") }
        }
        let description = excluding == nil ? CATapDescription(stereoMixdownOfProcesses: objects) : CATapDescription(stereoGlobalTapButExcludeProcesses: objects)
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
        // **The rate the sound arrives at is the device's it is read through**, which runs on the
        // output's clock: with headphones at 44.1 kHz it arrives at 44.1 kHz, whatever the tap
        // says its own format is. Taken for 48 kHz it was a twelfth short every second, and
        // the sound stopped and started as what was waiting to be played ran out.
        var nominal = Float64(0)
        var nominalAddress = AudioObjectPropertyAddress(mSelector: kAudioDevicePropertyNominalSampleRate, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        var nominalSize = UInt32(MemoryLayout<Float64>.size)
        AudioObjectGetPropertyData(aggregate, &nominalAddress, 0, nil, &nominalSize, &nominal)
        let rate = nominal > 0 ? nominal : format.mSampleRate > 0 ? format.mSampleRate : 48_000
        let interleaved = format.mFormatFlags & kAudioFormatFlagIsNonInterleaved == 0

        let micStreams = Self.inputStreams(ofDeviceWithUID: output)
        let debug = ProcessInfo.processInfo.environment["SPATIAND_DEBUG_TAP"] != nil
        if debug { print("tap: the output device has \(micStreams) input stream(s) of its own") }
        if debug { print("tap: read at \(nominal) Hz") }
        if debug { print("tap: \(objects.count) process(es), format \(format.mSampleRate) Hz, \(channels) channel(s), interleaved \(interleaved), output \(output)") }
        var calls = 0
        let result = AudioDeviceCreateIOProcIDWithBlock(&procID, aggregate, queue) { [weak self] _, input, _, _, _ in
            calls += 1
            if debug, calls == 1 || calls % 200 == 0 {
                let list = UnsafeMutableAudioBufferListPointer(UnsafeMutablePointer(mutating: input))
                let each = list.map { b -> String in
                    let n = Int(b.mDataByteSize) / 4
                    let p = b.mData?.assumingMemoryBound(to: Float.self)
                    var peak: Float = 0
                    for i in 0..<n { peak = max(peak, abs(p?[i] ?? 0)) }
                    return "\(b.mNumberChannels) ch \(b.mDataByteSize) bytes peak \(peak)"
                }
                print("tap: call \(calls), \(list.count) buffer(s): \(each.joined(separator: "; "))")
            }
            guard let self, let sink = self.onSound else { return }
            let buffers = UnsafeMutableAudioBufferListPointer(UnsafeMutablePointer(mutating: input))
            // **The tap's sound comes after the output device's own inputs.** A device that
            // plays and also listens -- the glasses have a microphone -- hands its microphone
            // over first, in the same list; read as the application's sound, that was the
            // room's noise played in place of a film. And each buffer says for itself how many
            // channels it holds: what the tap says of its format is not how it arrives here.
            let at = min(micStreams, buffers.count - 1)
            guard at >= 0, let data = buffers[at].mData else { return }
            let held = max(1, Int(buffers[at].mNumberChannels))
            let frames = Int(buffers[at].mDataByteSize) / (MemoryLayout<Float>.size * held)
            guard frames > 0 else { return }
            var out = [Float](repeating: 0, count: frames * 2)
            let p = data.assumingMemoryBound(to: Float.self)
            if held >= 2 {
                for i in 0..<frames {
                    out[i * 2] = p[i * held]
                    out[i * 2 + 1] = p[i * held + 1]
                }
            } else {
                // One channel to a buffer: the next is the right, if there is a next.
                let r = at + 1 < buffers.count && buffers[at + 1].mNumberChannels == 1 && buffers[at + 1].mDataByteSize == buffers[at].mDataByteSize
                    ? buffers[at + 1].mData?.assumingMemoryBound(to: Float.self) : nil
                for i in 0..<frames { out[i * 2] = p[i]; out[i * 2 + 1] = r?[i] ?? p[i] }
            }
            sink(out, rate)
        }
        let started = procID.map { AudioDeviceStart(aggregate, $0) } ?? -1
        if debug { print("tap: io proc \(result), start \(started)") }
        guard result == noErr, procID != nil, started == noErr else { lastError = "the tap could not be read (\(result), \(started))"; stop(); return false }
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
    func needsRefresh() -> Bool { excluding == nil ? !Set(processObjects()).isSubset(of: processes) : Set(processObjects()) != processes }

    /// How many streams of sound a device takes in, itself: a microphone's.
    private static func inputStreams(ofDeviceWithUID uid: String) -> Int {
        var address = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyTranslateUIDToDevice, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        var device = AudioObjectID(kAudioObjectUnknown)
        var size = UInt32(MemoryLayout<AudioObjectID>.size)
        var name = uid as CFString
        guard withUnsafePointer(to: &name, { AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &address, UInt32(MemoryLayout<CFString>.size), $0, &size, &device) }) == noErr,
              device != kAudioObjectUnknown else { return 0 }
        var streams = AudioObjectPropertyAddress(mSelector: kAudioDevicePropertyStreams, mScope: kAudioObjectPropertyScopeInput, mElement: kAudioObjectPropertyElementMain)
        var bytes: UInt32 = 0
        guard AudioObjectGetPropertyDataSize(device, &streams, 0, nil, &bytes) == noErr else { return 0 }
        return Int(bytes) / MemoryLayout<AudioObjectID>.size
    }

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
