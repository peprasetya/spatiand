//  HostEncoder.swift — a window's pictures, as HEVC for a session to decode.
//
//  VideoToolbox's hardware encoder, set up for a picture that is looked at and typed into: no
//  reordering, so a picture leaves as soon as it is made, and a keyframe only when one is asked
//  for (a window that is not changing sends nothing, and a session that arrives late has to be
//  able to ask). What comes out is length-prefixed, as MP4 wants it; the link carries Annex B,
//  with the parameter sets in front of every keyframe, so this puts it back the way it travels.

import CoreMedia
import Foundation
import VideoToolbox

final class HostEncoder {
    let width: Int
    let height: Int
    private var session: VTCompressionSession?
    /// One finished picture: Annex B bytes, whether it is a keyframe, and its time in microseconds.
    private let output: (Data, Bool, UInt64) -> Void

    init?(width: Int, height: Int, kbit: Int, output: @escaping (Data, Bool, UInt64) -> Void) {
        self.width = width
        self.height = height
        self.output = output
        var made: VTCompressionSession?
        let status = VTCompressionSessionCreate(
            allocator: nil, width: Int32(width), height: Int32(height), codecType: kCMVideoCodecType_HEVC,
            encoderSpecification: [kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder: true] as CFDictionary,
            imageBufferAttributes: nil, compressedDataAllocator: nil, outputCallback: nil, refcon: nil,
            compressionSessionOut: &made)
        guard status == noErr, let made else { return nil }
        session = made
        func set(_ key: CFString, _ value: Any) { VTSessionSetProperty(made, key: key, value: value as CFTypeRef) }
        set(kVTCompressionPropertyKey_RealTime, true)
        set(kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_HEVC_Main_AutoLevel)
        set(kVTCompressionPropertyKey_AllowFrameReordering, false)
        set(kVTCompressionPropertyKey_MaxKeyFrameInterval, 100_000)
        set(kVTCompressionPropertyKey_ExpectedFrameRate, 60)
        VTCompressionSessionPrepareToEncodeFrames(made)
        setBitrate(kbit)
    }

    deinit { invalidate() }

    func invalidate() {
        if let session {
            VTCompressionSessionCompleteFrames(session, untilPresentationTimeStamp: .invalid)
            VTCompressionSessionInvalidate(session)
        }
        session = nil
    }

    /// What this window may use, in kbit/s, with room for a burst of a quarter of a second's worth.
    func setBitrate(_ kbit: Int) {
        guard let session else { return }
        let bits = max(500, kbit) * 1000
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_AverageBitRate, value: bits as CFTypeRef)
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_DataRateLimits,
                             value: [bits / 8 / 4, 0.25] as CFArray)
    }

    func encode(_ pixels: CVPixelBuffer, microseconds: UInt64, key: Bool) {
        guard let session else { return }
        let time = CMTime(value: CMTimeValue(microseconds), timescale: 1_000_000)
        let options: CFDictionary? = key ? [kVTEncodeFrameOptionKey_ForceKeyFrame: true] as CFDictionary : nil
        VTCompressionSessionEncodeFrame(session, imageBuffer: pixels, presentationTimeStamp: time, duration: .invalid,
                                        frameProperties: options, infoFlagsOut: nil) { [output] status, _, sample in
            guard status == noErr, let sample, let (data, keyframe) = Self.annexB(sample) else { return }
            output(data, keyframe, microseconds)
        }
    }

    /// A finished sample as Annex B, and whether it is a keyframe.
    private static func annexB(_ sample: CMSampleBuffer) -> (Data, Bool)? {
        guard let block = CMSampleBufferGetDataBuffer(sample), let format = CMSampleBufferGetFormatDescription(sample) else { return nil }
        var keyframe = true
        if let list = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: false) as? [[CFString: Any]],
           let first = list.first, first[kCMSampleAttachmentKey_NotSync] as? Bool == true {
            keyframe = false
        }
        var out = Data()
        let start = Data([0, 0, 0, 1])
        if keyframe {
            var count = 0
            CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(format, parameterSetIndex: 0, parameterSetPointerOut: nil,
                                                               parameterSetSizeOut: nil, parameterSetCountOut: &count, nalUnitHeaderLengthOut: nil)
            for i in 0..<count {
                var pointer: UnsafePointer<UInt8>?
                var size = 0
                if CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(format, parameterSetIndex: i, parameterSetPointerOut: &pointer,
                                                                      parameterSetSizeOut: &size, parameterSetCountOut: nil, nalUnitHeaderLengthOut: nil) == noErr,
                   let pointer {
                    out.append(start)
                    out.append(pointer, count: size)
                }
            }
        }
        var length = 0
        var base: UnsafeMutablePointer<CChar>?
        guard CMBlockBufferGetDataPointer(block, atOffset: 0, lengthAtOffsetOut: nil, totalLengthOut: &length, dataPointerOut: &base) == noErr,
              let base else { return nil }
        var offset = 0
        while offset + 4 <= length {
            var n: UInt32 = 0
            memcpy(&n, base + offset, 4)
            let size = Int(UInt32(bigEndian: n))
            offset += 4
            guard size > 0, offset + size <= length else { break }
            out.append(start)
            out.append(Data(bytes: base + offset, count: size))
            offset += size
        }
        return out.isEmpty ? nil : (out, keyframe)
    }
}
