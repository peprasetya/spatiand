//  VideoSamples.swift — a host's pictures, as something macOS will decode and show.
//
//  The host sends each picture as one Annex B access unit: NAL units behind start codes, with
//  the parameter sets in front of every keyframe. VideoToolbox and AVSampleBufferDisplayLayer
//  want the other shape: a format description made once from those parameter sets, and samples
//  whose NAL units are each behind a four-byte length. This is the whole of that conversion.

import CoreMedia
import Foundation

final class VideoSamples {
    private(set) var format: CMVideoFormatDescription?
    private var parameterSets: [Data] = []
    private var codec: Int32 = -1

    /// Splits an Annex B stream into its NAL units, start codes removed.
    static func nalUnits(_ data: Data) -> [Data] {
        var units: [Data] = []
        let bytes = [UInt8](data)
        var i = 0
        var start: Int?
        while i + 2 < bytes.count {
            if bytes[i] == 0, bytes[i + 1] == 0, bytes[i + 2] == 1 {
                if let s = start {
                    // A four-byte start code leaves a zero on the end of the unit before it.
                    var end = i
                    while end > s, bytes[end - 1] == 0 { end -= 1 }
                    units.append(Data(bytes[s..<end]))
                }
                start = i + 3
                i += 3
            } else {
                i += 1
            }
        }
        if let s = start, s < bytes.count { units.append(Data(bytes[s...])) }
        return units
    }

    /// 0 H.264, 1 HEVC. Which kind of NAL unit this is, and whether it is a parameter set.
    private static func kind(_ nal: Data, codec: Int32) -> (type: Int, parameter: Bool) {
        guard let first = nal.first else { return (-1, false) }
        if codec == 1 {
            let type = Int(first >> 1) & 0x3f
            return (type, (32...34).contains(type))
        }
        let type = Int(first & 0x1f)
        return (type, type == 7 || type == 8)
    }

    /// A sample for one picture, or nil when it cannot be decoded yet -- before the first
    /// keyframe there is no format to decode with.
    func sample(_ data: Data, codec: Int32, capturedMicros: UInt64) -> CMSampleBuffer? {
        guard codec == 0 || codec == 1 else { return nil }
        let nals = Self.nalUnits(data)
        let sets = nals.filter { Self.kind($0, codec: codec).parameter }
        if !sets.isEmpty, sets != parameterSets || codec != self.codec {
            parameterSets = sets
            self.codec = codec
            format = Self.makeFormat(sets, codec: codec)
        }
        guard let format else { return nil }

        // Length-prefixed, parameter sets left out: they live in the format description.
        var avcc = Data()
        for nal in nals where !Self.kind(nal, codec: codec).parameter {
            var length = UInt32(nal.count).bigEndian
            avcc.append(Data(bytes: &length, count: 4))
            avcc.append(nal)
        }
        guard !avcc.isEmpty else { return nil }

        var block: CMBlockBuffer?
        let count = avcc.count
        guard CMBlockBufferCreateWithMemoryBlock(
            allocator: kCFAllocatorDefault, memoryBlock: nil, blockLength: count,
            blockAllocator: kCFAllocatorDefault, customBlockSource: nil, offsetToData: 0,
            dataLength: count, flags: 0, blockBufferOut: &block) == kCMBlockBufferNoErr, let block
        else { return nil }
        let copied = avcc.withUnsafeBytes {
            CMBlockBufferReplaceDataBytes(with: $0.baseAddress!, blockBuffer: block, offsetIntoDestination: 0, dataLength: count)
        }
        guard copied == kCMBlockBufferNoErr else { return nil }

        var timing = CMSampleTimingInfo(
            duration: .invalid,
            presentationTimeStamp: CMTime(value: CMTimeValue(capturedMicros), timescale: 1_000_000),
            decodeTimeStamp: .invalid)
        var size = count
        var sample: CMSampleBuffer?
        guard CMSampleBufferCreateReady(
            allocator: kCFAllocatorDefault, dataBuffer: block, formatDescription: format,
            sampleCount: 1, sampleTimingEntryCount: 1, sampleTimingArray: &timing,
            sampleSizeEntryCount: 1, sampleSizeArray: &size, sampleBufferOut: &sample) == noErr, let sample
        else { return nil }
        // Shown as it arrives: the host paced the picture to the display, and a timestamp from
        // another computer's clock is not one this Mac can schedule against.
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: true) as? [NSMutableDictionary],
           let first = attachments.first {
            first[kCMSampleAttachmentKey_DisplayImmediately] = true
        }
        return sample
    }

    private static func makeFormat(_ sets: [Data], codec: Int32) -> CMVideoFormatDescription? {
        var format: CMVideoFormatDescription?
        // The pointers must be valid together for the whole call; see withUnsafeBufferPointerList.
        let owned = sets.map { [UInt8]($0) }
        let sizes = owned.map { $0.count }
        var status: OSStatus = -1
        owned.withUnsafeBufferPointerList { bases in
            if codec == 1 {
                status = CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                    allocator: kCFAllocatorDefault, parameterSetCount: sets.count,
                    parameterSetPointers: bases, parameterSetSizes: sizes,
                    nalUnitHeaderLength: 4, extensions: nil, formatDescriptionOut: &format)
            } else {
                status = CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    allocator: kCFAllocatorDefault, parameterSetCount: sets.count,
                    parameterSetPointers: bases, parameterSetSizes: sizes,
                    nalUnitHeaderLength: 4, formatDescriptionOut: &format)
            }
        }
        return status == noErr ? format : nil
    }
}

extension Array where Element == [UInt8] {
    /// Calls `body` with a stable pointer to each inner array's bytes, all valid together.
    func withUnsafeBufferPointerList<R>(_ body: ([UnsafePointer<UInt8>]) -> R) -> R {
        func go(_ index: Int, _ acc: [UnsafePointer<UInt8>]) -> R {
            if index == count { return body(acc) }
            return self[index].withUnsafeBufferPointer { buffer in
                go(index + 1, acc + [buffer.baseAddress!])
            }
        }
        return go(0, [])
    }
}
