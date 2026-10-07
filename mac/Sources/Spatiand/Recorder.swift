//  Recorder.swift — "Record a video": both eyes and every sound, to the Movies folder.
//
//  The Deck's HUD row of the same name. The picture is the room drawn once more, into a buffer the video encoder
//  reads, so what is in the file is what the glasses showed; the sound is what the Mac is playing at that moment,
//  its own applications' and Spatiand's placed sound together, as the wearer hears it, taken by ScreenCaptureKit
//  (the Screen Recording permission Spatiand already has). H.264 and AAC in an .mp4, at the display's rate.

import AVFoundation
import CoreMedia
import Metal
import ScreenCaptureKit

final class Recorder: NSObject, SCStreamOutput, SCStreamDelegate {
    private let renderer: RoomRenderer
    private let size: (Int, Int)
    private let stereo: Bool
    private let writer: AVAssetWriter
    private let video: AVAssetWriterInput
    private let audio: AVAssetWriterInput
    private let adaptor: AVAssetWriterInputPixelBufferAdaptor
    private var cache: CVMetalTextureCache?
    private var stream: SCStream?
    private let lock = NSLock()
    private let queue = DispatchQueue(label: "spatiand.recorder.audio", qos: .userInitiated)
    private var began: CFTimeInterval = 0
    private var started = false
    private var finished = false
    private(set) var frames = 0
    private(set) var dropped = 0
    let url: URL

    /// Opens the file; nil if the writer cannot be made.
    init?(renderer: RoomRenderer, width: Int, height: Int, sideBySide: Bool, folder: URL? = nil) {
        self.renderer = renderer
        size = (width, height)
        stereo = sideBySide
        let stamp = ISO8601DateFormatter().string(from: Date()).replacingOccurrences(of: ":", with: "-")
        let directory = folder ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Movies")
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        url = directory.appendingPathComponent("Spatiand \(stamp).mp4")
        guard let writer = try? AVAssetWriter(outputURL: url, fileType: .mp4) else { return nil }
        self.writer = writer
        video = AVAssetWriterInput(mediaType: .video, outputSettings: [
            AVVideoCodecKey: AVVideoCodecType.h264,
            AVVideoWidthKey: width, AVVideoHeightKey: height,
            AVVideoCompressionPropertiesKey: [AVVideoAverageBitRateKey: 24_000_000, AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel, AVVideoExpectedSourceFrameRateKey: 60],
        ])
        video.expectsMediaDataInRealTime = true
        audio = AVAssetWriterInput(mediaType: .audio, outputSettings: [
            AVFormatIDKey: kAudioFormatMPEG4AAC, AVSampleRateKey: 48_000, AVNumberOfChannelsKey: 2, AVEncoderBitRateKey: 160_000,
        ])
        audio.expectsMediaDataInRealTime = true
        adaptor = AVAssetWriterInputPixelBufferAdaptor(assetWriterInput: video, sourcePixelBufferAttributes: [
            kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
            kCVPixelBufferWidthKey as String: width, kCVPixelBufferHeightKey as String: height,
            kCVPixelBufferMetalCompatibilityKey as String: true,
            kCVPixelBufferIOSurfacePropertiesKey as String: [:],
        ])
        super.init()
        guard writer.canAdd(video), writer.canAdd(audio) else { return nil }
        writer.add(video)
        writer.add(audio)
        CVMetalTextureCacheCreate(nil, nil, renderer.device, nil, &cache)
    }

    /// Begin: the file is opened, and the sound starts being taken.
    func start() async -> Bool {
        guard writer.startWriting() else { print("record: could not start: \(writer.error.map(String.init(describing:)) ?? "?")"); return false }
        began = CACurrentMediaTime()
        writer.startSession(atSourceTime: .zero)
        started = true
        do {
            let content = try await SCShareableContent.excludingDesktopWindows(true, onScreenWindowsOnly: false)
            guard let display = content.displays.first else { return true }
            let config = SCStreamConfiguration()
            config.capturesAudio = true
            config.sampleRate = 48_000
            config.channelCount = 2
            // Everything that is playing, Spatiand's own placed sound included: that is what the wearer hears.
            config.excludesCurrentProcessAudio = false
            config.width = 2
            config.height = 2
            config.minimumFrameInterval = CMTime(value: 1, timescale: 1)
            let s = SCStream(filter: SCContentFilter(display: display, excludingWindows: []), configuration: config, delegate: self)
            try s.addStreamOutput(self, type: .audio, sampleHandlerQueue: queue)
            try await s.startCapture()
            stream = s
        } catch {
            print("record: the sound is not being taken: \(error)")
        }
        return true
    }

    /// One frame: the room is drawn into a buffer of the encoder's and handed over when the GPU has done.
    func capture(sideBySide: Bool) {
        guard started, !finished, video.isReadyForMoreMediaData, let pool = adaptor.pixelBufferPool, let cache else { dropped += 1; return }
        var buffer: CVPixelBuffer?
        guard CVPixelBufferPoolCreatePixelBuffer(nil, pool, &buffer) == kCVReturnSuccess, let pixels = buffer else { dropped += 1; return }
        var made: CVMetalTexture?
        guard CVMetalTextureCacheCreateTextureFromImage(nil, cache, pixels, nil, .bgra8Unorm, size.0, size.1, 0, &made) == kCVReturnSuccess,
              let made, let texture = CVMetalTextureGetTexture(made) else { dropped += 1; return }
        let at = CMTime(seconds: CACurrentMediaTime() - began, preferredTimescale: 90_000)
        renderer.render(into: texture, sideBySide: sideBySide) { [weak self] in
            guard let self else { return }
            _ = made   // held until the GPU is done with it
            self.lock.lock()
            defer { self.lock.unlock() }
            guard !self.finished, self.video.isReadyForMoreMediaData else { self.dropped += 1; return }
            if self.adaptor.append(pixels, withPresentationTime: at) { self.frames += 1 } else { self.dropped += 1 }
        }
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sample: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .audio, CMSampleBufferIsValid(sample), CMSampleBufferGetNumSamples(sample) > 0 else { return }
        let host = CMSampleBufferGetPresentationTimeStamp(sample)
        let relative = CMTimeSubtract(host, CMTime(seconds: began, preferredTimescale: 90_000))
        // Sound from before the recording began is not part of it.
        guard relative.seconds >= 0 else { return }
        var timing = CMSampleTimingInfo(duration: CMSampleBufferGetDuration(sample), presentationTimeStamp: relative, decodeTimeStamp: .invalid)
        var retimed: CMSampleBuffer?
        guard CMSampleBufferCreateCopyWithNewTiming(allocator: nil, sampleBuffer: sample, sampleTimingEntryCount: 1, sampleTimingArray: &timing, sampleBufferOut: &retimed) == noErr, let retimed else { return }
        lock.lock()
        defer { lock.unlock() }
        if !finished, audio.isReadyForMoreMediaData { audio.append(retimed) }
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {}

    /// Stop, and finish the file. `done` says whether there is a video to look at.
    func stop(_ done: @escaping (Bool) -> Void) {
        let s = stream
        stream = nil
        Task {
            try? await s?.stopCapture()
            self.lock.lock()
            self.finished = true
            self.lock.unlock()
            guard self.started, self.frames > 0 else { self.writer.cancelWriting(); done(false); return }
            self.video.markAsFinished()
            self.audio.markAsFinished()
            self.writer.finishWriting { done(self.writer.status == .completed) }
        }
    }
}
