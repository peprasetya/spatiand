//  MacAudio.swift — one Mac application's sound, for a session that is using its windows.
//
//  ScreenCaptureKit can capture the sound of a single application, apart from everything else the
//  Mac is playing, which is what a host's window needs: the sound of that window's app, on a stream
//  of its own, so the device can put it where the window is. It is captured as the app's own
//  stereo mix and sent as 16-bit samples, as the Linux host does; a quiet application sends
//  nothing.
//
//  The Mac goes on playing it too. There is no way to take an application's sound away from the
//  speakers and leave it in the capture, so on the Mac itself it is heard as it always was.

import AVFoundation
import CoreMedia
import ScreenCaptureKit

final class MacAudio: NSObject, SCStreamOutput, SCStreamDelegate {
    let bundle: String
    private var stream: SCStream?
    private let queue = DispatchQueue(label: "spatiand.macaudio", qos: .userInteractive)
    /// Interleaved 16-bit stereo, at 48 kHz, off the capture's queue.
    var onSound: ((Data) -> Void)?
    var onEnded: (() -> Void)?

    init(bundle: String) { self.bundle = bundle }

    func start() async throws {
        let content = try await SCShareableContent.excludingDesktopWindows(true, onScreenWindowsOnly: false)
        guard let app = content.applications.first(where: { $0.bundleIdentifier == bundle }),
              let display = content.displays.first else { throw MacCapture.CaptureError.gone }
        let filter = SCContentFilter(display: display, including: [app], exceptingWindows: [])
        let config = SCStreamConfiguration()
        config.capturesAudio = true
        config.sampleRate = 48_000
        config.channelCount = 2
        config.excludesCurrentProcessAudio = true
        // There is no picture to take, and none is asked for beyond the least a stream will accept.
        config.width = 2
        config.height = 2
        config.minimumFrameInterval = CMTime(value: 1, timescale: 1)
        let s = SCStream(filter: filter, configuration: config, delegate: self)
        try s.addStreamOutput(self, type: .audio, sampleHandlerQueue: queue)
        try await s.startCapture()
        stream = s
    }

    func invalidate() {
        let s = stream
        stream = nil
        onSound = nil
        onEnded = nil
        Task { try? await s?.stopCapture() }
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sample: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .audio, CMSampleBufferIsValid(sample), let data = Self.pcm16(sample) else { return }
        // Silence is not sent; a window that makes no noise costs nothing.
        if data.allSatisfy({ $0 == 0 }) { return }
        onSound?(data)
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        DispatchQueue.main.async { [weak self] in self?.onEnded?() }
    }

    /// The samples of a buffer as interleaved signed 16-bit, whatever shape the capture gave them in.
    static func pcm16(_ sample: CMSampleBuffer) -> Data? {
        guard let format = CMSampleBufferGetFormatDescription(sample),
              let description = CMAudioFormatDescriptionGetStreamBasicDescription(format)?.pointee,
              description.mFormatID == kAudioFormatLinearPCM else { return nil }
        let frames = CMSampleBufferGetNumSamples(sample)
        guard frames > 0 else { return nil }
        let channels = Int(description.mChannelsPerFrame)
        guard channels >= 1 else { return nil }
        var size = 0
        CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
            sample, bufferListSizeNeededOut: &size, bufferListOut: nil, bufferListSize: 0,
            blockBufferAllocator: nil, blockBufferMemoryAllocator: nil, flags: 0, blockBufferOut: nil)
        let raw = UnsafeMutableRawPointer.allocate(byteCount: size, alignment: MemoryLayout<AudioBufferList>.alignment)
        defer { raw.deallocate() }
        let list = raw.bindMemory(to: AudioBufferList.self, capacity: 1)
        var block: CMBlockBuffer?
        guard CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
            sample, bufferListSizeNeededOut: nil, bufferListOut: list, bufferListSize: size,
            blockBufferAllocator: nil, blockBufferMemoryAllocator: nil, flags: 0, blockBufferOut: &block) == noErr else { return nil }
        let buffers = UnsafeMutableAudioBufferListPointer(list)
        let isFloat = description.mFormatFlags & kAudioFormatFlagIsFloat != 0
        let planar = description.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0
        func value(_ channel: Int, _ frame: Int) -> Float {
            let c = min(channel, channels - 1)
            if planar {
                guard c < buffers.count, let p = buffers[c].mData else { return 0 }
                return isFloat ? p.assumingMemoryBound(to: Float.self)[frame] : Float(p.assumingMemoryBound(to: Int16.self)[frame]) / 32768
            }
            guard let p = buffers[0].mData else { return 0 }
            return isFloat ? p.assumingMemoryBound(to: Float.self)[frame * channels + c]
                           : Float(p.assumingMemoryBound(to: Int16.self)[frame * channels + c]) / 32768
        }
        var out = Data(count: frames * 4)
        out.withUnsafeMutableBytes { bytes in
            let samples = bytes.bindMemory(to: Int16.self)
            for f in 0..<frames {
                for c in 0..<2 {
                    let v = max(-1, min(1, value(c, f)))
                    samples[f * 2 + c] = Int16(v * 32767).littleEndian
                }
            }
        }
        return out
    }
}
