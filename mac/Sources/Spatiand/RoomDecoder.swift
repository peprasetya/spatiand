//  RoomDecoder.swift — one window's pictures, decoded to something the GPU can draw.
//
//  A window on the Mac is shown by an AVSampleBufferDisplayLayer, which decodes and draws and
//  never gives the picture back. The room has to draw the picture itself, onto a curved surface
//  and twice over, so it decodes with VideoToolbox directly and keeps the newest picture as a
//  pixel buffer: BGRA, backed by an IOSurface, which Metal can use with no copy.

import CoreMedia
import CoreVideo
import Foundation
import VideoToolbox

/// Anything that has a newest picture the room can draw: a decoded stream from a host, or a Mac
/// window being captured here.
protocol PictureSource: AnyObject {
    /// The newest picture, and a number that changes when there is another.
    func current() -> (CVPixelBuffer, Int)?
    func invalidate()
}

final class RoomDecoder: PictureSource {
    private var session: VTDecompressionSession?
    private var format: CMFormatDescription?
    private let lock = NSLock()
    private var newest: CVPixelBuffer?
    private var generation = 0

    /// Called, off the main thread, when a picture has been decoded.
    var onPicture: ((CVPixelBuffer) -> Void)?
    /// The decoder has lost its place and needs a keyframe.
    var onTrouble: (() -> Void)?

    deinit { invalidate() }

    /// The newest picture, and a number that changes when there is another.
    func current() -> (CVPixelBuffer, Int)? {
        lock.lock()
        defer { lock.unlock() }
        return newest.map { ($0, generation) }
    }

    func invalidate() {
        if let session { VTDecompressionSessionInvalidate(session) }
        session = nil
        format = nil
    }

    func decode(_ sample: CMSampleBuffer) {
        guard let wanted = CMSampleBufferGetFormatDescription(sample) else { return }
        if let session, let format, CMFormatDescriptionEqual(format, otherFormatDescription: wanted),
           VTDecompressionSessionCanAcceptFormatDescription(session, formatDescription: wanted) {
            // Same stream; carry on.
        } else {
            invalidate()
            let attributes: [CFString: Any] = [
                kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_32BGRA,
                kCVPixelBufferMetalCompatibilityKey: true,
                kCVPixelBufferIOSurfacePropertiesKey: [:] as [String: Any],
            ]
            var made: VTDecompressionSession?
            let status = VTDecompressionSessionCreate(
                allocator: nil, formatDescription: wanted, decoderSpecification: nil,
                imageBufferAttributes: attributes as CFDictionary, outputCallback: nil,
                decompressionSessionOut: &made)
            guard status == noErr, let made else {
                onTrouble?()
                return
            }
            session = made
            format = wanted
        }
        guard let session else { return }
        let status = VTDecompressionSessionDecodeFrame(
            session, sampleBuffer: sample, flags: [._EnableAsynchronousDecompression], infoFlagsOut: nil
        ) { [weak self] status, _, image, _, _ in
            guard let self else { return }
            guard status == noErr, let image else {
                self.onTrouble?()
                return
            }
            self.lock.lock()
            self.newest = image
            self.generation += 1
            self.lock.unlock()
            self.onPicture?(image)
        }
        if status != noErr { onTrouble?() }
    }
}
