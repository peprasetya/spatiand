//  MacSound.swift — the sound of the Mac's applications whose windows are in the room, placed where those windows are.
//
//  One tap for each application that has a window in the room (see `MacTap`); what it takes is folded to two ears
//  through the room's own audio, which puts it where the window is and moves it as the head turns, and played by
//  Spatiand's engine. Off until the owner asks for it in Settings: macOS asks, the first time, whether Spatiand may take
//  other applications' sound, and an application whose sound is taken is silent on the Mac's own speakers.
//
//  If the permission is refused macOS does not say so; the tap simply delivers silence. So an application that Core
//  Audio says is playing while its tap has heard nothing for a few seconds is let go -- its sound comes back on the
//  speakers -- and the owner is told where to allow it.

import AppKit
import AudioToolbox

final class MacSound {
    private let room: RoomCore
    private var taps: [String: AnyObject] = [:]
    private var heard: [String: Date] = [:]
    private var playingSince: [String: Date] = [:]
    private let lock = NSLock()
    /// Said, on the main thread, when the sound could not be taken: the reason, for the hint.
    var onProblem: ((String) -> Void)?

    init(room: RoomCore) { self.room = room }

    var bundles: [String] { Array(taps.keys) }

    /// Make the taps match the applications that have a window in the room. Called about once a second.
    func reconcile(wanted: [String: pid_t]) {
        guard #available(macOS 14.2, *) else { return }
        for bundle in Array(taps.keys) where wanted[bundle] == nil { drop(bundle) }
        for (bundle, pid) in wanted {
            if let existing = taps[bundle] as? MacTap {
                // More of its processes are playing than when the tap was made: made again with them in it.
                if existing.needsRefresh() { existing.start() }
                check(bundle, existing)
                continue
            }
            let tap = MacTap(bundle: bundle, pid: pid)
            tap.onSound = { [weak self] block, rate in self?.sound(bundle, block, rate) }
            // Nothing of the application may have played yet; it is tried again next time.
            if tap.start() {
                taps[bundle] = tap
                heard[bundle] = Date()
            }
        }
    }

    @available(macOS 14.2, *)
    private func check(_ bundle: String, _ tap: MacTap) {
        let now = Date()
        lock.lock()
        let last = heard[bundle] ?? now
        lock.unlock()
        guard tap.isPlaying() else { playingSince[bundle] = nil; return }
        let since = playingSince[bundle] ?? now
        playingSince[bundle] = since
        if now.timeIntervalSince(since) > 4, now.timeIntervalSince(last) > 4 {
            // Playing, and nothing has come through the tap: the permission has not been given.
            stopAll()
            onProblem?("macOS has not let Spatiand take other applications' sound, so it is left on the Mac's speakers. Allow it in System Settings \u{2192} Privacy & Security \u{2192} Screen & System Audio Recording \u{2192} System Audio Recording Only.")
        }
    }

    private func sound(_ bundle: String, _ block: [Float], _ rate: Double) {
        guard block.contains(where: { abs($0) > 1e-5 }) else { return }
        lock.lock(); heard[bundle] = Date(); lock.unlock()
        let stereo = rate > 0 && abs(rate - 48_000) > 1 ? Self.resampled(block, from: rate) : block
        var pcm = Data(count: stereo.count * 2)
        pcm.withUnsafeMutableBytes { bytes in
            let out = bytes.bindMemory(to: Int16.self)
            for i in 0..<stereo.count { out[i] = Int16(max(-1, min(1, stereo[i])) * 32767) }
        }
        let placed = room.audio(app: bundle, channels: 2, pcm: pcm)
        AudioOut.shared.feed(app: bundle, channels: 2, data: pcm, placed: placed.isEmpty ? nil : placed)
    }

    /// Interleaved stereo to 48 kHz by straight lines between samples: a Mac whose output runs at 44.1 kHz.
    private static func resampled(_ block: [Float], from rate: Double) -> [Float] {
        let frames = block.count / 2
        let count = Int(Double(frames) * 48_000 / rate)
        guard frames > 1, count > 1 else { return block }
        var out = [Float](repeating: 0, count: count * 2)
        for i in 0..<count {
            let position = Double(i) * rate / 48_000
            let a = min(frames - 1, Int(position)), b = min(frames - 1, a + 1)
            let t = Float(position - Double(a))
            out[i * 2] = block[a * 2] * (1 - t) + block[b * 2] * t
            out[i * 2 + 1] = block[a * 2 + 1] * (1 - t) + block[b * 2 + 1] * t
        }
        return out
    }

    private func drop(_ bundle: String) {
        if #available(macOS 14.2, *) { (taps[bundle] as? MacTap)?.stop() }
        taps[bundle] = nil
        heard[bundle] = nil
        playingSince[bundle] = nil
    }

    /// Every application's sound back on the Mac's own speakers.
    func stopAll() {
        for bundle in Array(taps.keys) { drop(bundle) }
    }
}
