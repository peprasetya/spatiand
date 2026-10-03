//  AudioOut.swift — what the host's applications sound like on this Mac.
//
//  Without glasses the sound is flat stereo: each application's stream is folded down to two
//  channels, mixed with the others, and played into the device the owner chose (or the Mac's own
//  output). With glasses it will be placed in the room instead; that is the glasses path's work.

import AVFoundation
import CoreAudio

/// A surround layout folded to stereo. Channels come in the wire order of
/// `spatiand_audio::stage::Layout`: front pair, centre, LFE, rears, sides, then the four above.
enum Downmix {
    /// `(left weight, right weight)` for each channel of a layout.
    static func weights(channels: Int) -> [(Float, Float)] {
        let h: Float = 0.7071
        switch channels {
        case 2: return [(1, 0), (0, 1)]
        case 6: return [(1, 0), (0, 1), (h, h), (0, 0), (h, 0), (0, h)]
        case 8: return [(1, 0), (0, 1), (h, h), (0, 0), (h, 0), (0, h), (h, 0), (0, h)]
        case 12:
            return [(1, 0), (0, 1), (h, h), (0, 0), (h, 0), (0, h), (h, 0), (0, h),
                    (0.5, 0), (0, 0.5), (0.5, 0), (0, 0.5)]
        default: return [(1, 0), (0, 1)]
        }
    }

    /// Interleaved signed 16-bit in, interleaved stereo float out, kept below clipping by the
    /// sum of the weights that can land on one side.
    static func stereo(_ pcm: Data, channels: Int) -> [Float] {
        let w = weights(channels: channels)
        let norm = 1 / max(1, w.reduce(0) { $0 + $1.0 })
        let frames = pcm.count / (channels * 2)
        var out = [Float](repeating: 0, count: frames * 2)
        pcm.withUnsafeBytes { raw in
            let s = raw.bindMemory(to: Int16.self)
            for f in 0..<frames {
                var l: Float = 0, r: Float = 0
                for c in 0..<channels {
                    let v = Float(Int16(littleEndian: s[f * channels + c])) / 32768
                    l += v * w[c].0
                    r += v * w[c].1
                }
                // A stereo stream is untouched; a wider one is brought down to where it fits.
                let scale = channels == 2 ? 1 : norm
                out[f * 2] = l * scale
                out[f * 2 + 1] = r * scale
            }
        }
        return out
    }
}

final class AudioOut {
    static let shared = AudioOut()

    private let engine = AVAudioEngine()
    private var source: AVAudioSourceNode?
    private let lock = NSLock()
    /// Each application's waiting sound, interleaved stereo.
    private var rings: [String: [Float]] = [:]
    /// Longest to wait, in frames: past a quarter of a second the listener hears the past.
    private let limit = 48_000 / 4 * 2
    private var started = false

    /// Called from the link's threads, as sound arrives.
    func feed(app: String, channels: Int, data: Data) {
        let stereo = Downmix.stereo(data, channels: channels)
        lock.lock()
        var ring = rings[app, default: []]
        ring.append(contentsOf: stereo)
        if ring.count > limit { ring.removeFirst(ring.count - limit / 2) }
        rings[app] = ring
        lock.unlock()
        if !started { DispatchQueue.main.async { self.start() } }
    }

    private func start() {
        guard !started else { return }
        started = true
        let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 48_000, channels: 2, interleaved: false)!
        let node = AVAudioSourceNode(format: format) { [weak self] _, _, frames, list in
            guard let self else { return noErr }
            let buffers = UnsafeMutableAudioBufferListPointer(list)
            let n = Int(frames)
            for b in buffers { memset(b.mData, 0, Int(b.mDataByteSize)) }
            guard buffers.count >= 2,
                  let left = buffers[0].mData?.assumingMemoryBound(to: Float.self),
                  let right = buffers[1].mData?.assumingMemoryBound(to: Float.self) else { return noErr }
            self.lock.lock()
            for (app, ring) in self.rings {
                let take = min(n, ring.count / 2)
                for f in 0..<take {
                    left[f] += ring[f * 2]
                    right[f] += ring[f * 2 + 1]
                }
                self.rings[app] = Array(ring.dropFirst(take * 2))
            }
            self.lock.unlock()
            return noErr
        }
        source = node
        engine.attach(node)
        engine.connect(node, to: engine.mainMixerNode, format: format)
        applyDevice()
        do { try engine.start() } catch { print("sound: could not start: \(error)"); started = false }
    }

    /// Play into the device the owner chose, or the Mac's own output when none is chosen.
    func applyDevice() {
        guard let unit = engine.outputNode.audioUnit else { return }
        let wanted = Settings.audioOutputUID.flatMap { uid in AudioDevices.outputs().first { $0.uid == uid } }
            ?? AudioDevices.systemDefault()
        guard var id = wanted?.id else { return }
        AudioUnitSetProperty(unit, kAudioOutputUnitProperty_CurrentDevice,
                             kAudioUnitScope_Global, 0, &id, UInt32(MemoryLayout<AudioDeviceID>.size))
    }
}
