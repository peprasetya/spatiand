//  SessionTest.swift — `Spatiand --selftest-session <address> <fingerprint> <app>`
//
//  Connects to a host, starts one of its applications, decodes the first pictures that arrive
//  with VideoToolbox, and writes one of them to a PNG. The check that the whole path from the
//  link to a picture works, with no window and nobody looking.

import AppKit
import CoreImage
import VideoToolbox

final class SessionTest {
    private let link: Link
    private let samples = VideoSamples()
    private var session: VTDecompressionSession?
    private var decoded = 0
    private var received = 0
    private var wrote = false
    private let output: String

    init(identityDir: String, output: String) {
        link = Link(identityDir: identityDir)!
        self.output = output
    }

    func run(address: String, fingerprint: String, app: String, seconds: Double) -> Int32 {
        print("this computer is \(link.fingerprint.prefix(8))")
        link.onCore = { what, detail in print("core: \(what) \(detail)") }
        link.onHost = { message in
            if let name = message.keys.first {
                switch name {
                case "Catalog":
                    let apps = ((message[name] as? [String: Any])?["apps"] as? [[String: Any]]) ?? []
                    print("host offers: " + apps.compactMap { $0["id"] as? String }.joined(separator: ", "))
                case "Opened", "Stream", "Welcome", "Layer": print("host: \(name) \(message[name] ?? "")")
                default: break
                }
            }
        }
        var audioBytes: [String: Int] = [:]
        link.onAudio = { app, channels, data in audioBytes["\(app)/\(channels)ch", default: 0] += data.count }
        link.onVideo = { [self] window, codec, keyframe, captured, data in
            received += 1
            guard let sample = samples.sample(data, codec: codec, capturedMicros: captured) else { return }
            decode(sample)
        }
        link.connect(address: address, fingerprint: fingerprint)
        // Wait for the welcome, then ask for the application.
        RunLoop.main.run(until: Date().addingTimeInterval(2))
        link.launch(app)
        RunLoop.main.run(until: Date().addingTimeInterval(seconds))
        print("pictures received: \(received), decoded: \(decoded), sound: \(audioBytes)")
        link.say(["Detach": NSNull()])
        RunLoop.main.run(until: Date().addingTimeInterval(0.3))
        return wrote ? 0 : 1
    }

    private func decode(_ sample: CMSampleBuffer) {
        guard let format = CMSampleBufferGetFormatDescription(sample) else { return }
        if session == nil {
            var created: VTDecompressionSession?
            let attributes: [CFString: Any] = [kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_32BGRA]
            let status = VTDecompressionSessionCreate(
                allocator: kCFAllocatorDefault, formatDescription: format, decoderSpecification: nil,
                imageBufferAttributes: attributes as CFDictionary, outputCallback: nil, decompressionSessionOut: &created)
            guard status == noErr else { print("decoder: \(status)"); return }
            session = created
        }
        guard let session else { return }
        VTDecompressionSessionDecodeFrame(session, sampleBuffer: sample, flags: [], infoFlagsOut: nil) { [self] status, _, image, _, _ in
            guard status == noErr, let image else { return }
            decoded += 1
            if !wrote, decoded >= 1 {
                wrote = true
                let context = CIContext()
                let ci = CIImage(cvPixelBuffer: image)
                if let cg = context.createCGImage(ci, from: ci.extent) {
                    let rep = NSBitmapImageRep(cgImage: cg)
                    try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: output))
                    print("wrote \(output): \(cg.width)x\(cg.height)")
                }
            }
        }
    }
}
