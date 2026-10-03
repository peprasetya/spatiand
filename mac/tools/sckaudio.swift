// Probe: what does ScreenCaptureKit give us for one app's audio?
//   sckaudio list
//   sckaudio capture <bundle id> <seconds> <channels>
import Foundation
import ScreenCaptureKit
import AVFoundation
import CoreMedia

final class Probe: NSObject, SCStreamOutput, SCStreamDelegate {
    var buffers = 0
    var frames = 0
    var formatLine = ""
    var sumsq: [Double] = []
    var peak: [Float] = []
    var firstAt: Date?
    var lastAt: Date?

    func stream(_ s: SCStream, didOutputSampleBuffer sb: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .audio, sb.isValid else { return }
        if formatLine.isEmpty, let fd = CMSampleBufferGetFormatDescription(sb),
           let a = CMAudioFormatDescriptionGetStreamBasicDescription(fd)?.pointee {
            let nonint = (a.mFormatFlags & kAudioFormatFlagIsNonInterleaved) != 0
            formatLine = "rate \(a.mSampleRate) Hz, channels \(a.mChannelsPerFrame), bits \(a.mBitsPerChannel), float \((a.mFormatFlags & kAudioFormatFlagIsFloat) != 0), nonInterleaved \(nonint)"
        }
        if firstAt == nil { firstAt = Date() }
        lastAt = Date()
        buffers += 1
        frames += CMSampleBufferGetNumSamples(sb)
        try? sb.withAudioBufferList { abl, _ in
            for (c, b) in abl.enumerated() {
                guard let d = b.mData else { continue }
                let n = Int(b.mDataByteSize) / 4
                let p = d.assumingMemoryBound(to: Float.self)
                while sumsq.count <= c { sumsq.append(0); peak.append(0) }
                for i in 0..<n { let v = p[i]; sumsq[c] += Double(v * v); peak[c] = max(peak[c], abs(v)) }
            }
        }
    }
    func stream(_ s: SCStream, didStopWithError e: Error) { print("stream stopped: \(e)") }
}

func run() async {
    let args = CommandLine.arguments
    let content: SCShareableContent
    do { content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false) }
    catch { print("cannot read shareable content (screen recording permission?): \(error)"); exit(2) }

    if args.count >= 2 && args[1] == "list" {
        for app in content.applications.sorted(by: { $0.applicationName < $1.applicationName }) {
            let n = content.windows.filter { $0.owningApplication?.processID == app.processID }.count
            if n > 0 { print("\(app.bundleIdentifier)\t\(app.applicationName)\twindows:\(n)") }
        }
        exit(0)
    }
    guard args.count >= 5, args[1] == "capture" else { print("usage: list | capture <bundle id> <seconds> <channels>"); exit(1) }
    let bundle = args[2], seconds = Double(args[3]) ?? 5, channels = Int(args[4]) ?? 2
    guard let app = content.applications.first(where: { $0.bundleIdentifier == bundle }),
          let display = content.displays.first else { print("no such app or display"); exit(1) }

    let filter = SCContentFilter(display: display, including: [app], exceptingWindows: [])
    let cfg = SCStreamConfiguration()
    cfg.capturesAudio = true
    cfg.excludesCurrentProcessAudio = true
    cfg.sampleRate = 48000
    cfg.channelCount = channels
    // Audio is all we want; keep the picture as small and rare as it will go.
    cfg.width = 16; cfg.height = 16
    cfg.minimumFrameInterval = CMTime(value: 1, timescale: 1)

    let probe = Probe()
    let stream = SCStream(filter: filter, configuration: cfg, delegate: probe)
    do {
        try stream.addStreamOutput(probe, type: .audio, sampleHandlerQueue: DispatchQueue(label: "audio"))
        try await stream.startCapture()
    } catch { print("could not start: \(error)"); exit(3) }
    print("capturing \(app.applicationName) for \(seconds)s, asking for \(channels) channel(s)...")
    try? await Task.sleep(nanoseconds: UInt64(seconds * 1e9))
    try? await stream.stopCapture()

    print("buffers: \(probe.buffers), frames: \(probe.frames) (~\(Double(probe.frames) / 48000.0)s of audio)")
    print("format: \(probe.formatLine.isEmpty ? "(nothing arrived)" : probe.formatLine)")
    for c in 0..<probe.sumsq.count {
        let rms = probe.frames > 0 ? (probe.sumsq[c] / Double(probe.frames)).squareRoot() : 0
        print(String(format: "  channel %d: rms %.5f  peak %.4f", c, rms, probe.peak[c]))
    }
    exit(0)
}
Task { await run() }
RunLoop.main.run()
