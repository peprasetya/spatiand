//  RecordTest.swift — `Spatiand --selftest-record`
//
//  The HUD's Record row, without glasses: the room is drawn for two seconds into a video, which is then opened again
//  and looked at -- its size, its length, whether it has a picture track and a sound track, and one frame of it.
//  The video is made in a scratch folder under /tmp and left there to look at.

import AppKit
import AVFoundation

enum RecordTest {
    static func run() {
        let room = Model.shared.room
        var failures = 0
        func check(_ name: String, _ ok: Bool) {
            print((ok ? "ok    " : "FAIL  ") + name)
            if !ok { failures += 1 }
        }
        func after(_ seconds: Double, _ body: @escaping () -> Void) { DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: body) }
        let folder = URL(fileURLWithPath: "/tmp/spatiand-record-test")
        try? FileManager.default.removeItem(at: folder)
        after(0.5) {
            room.core.holdHead(yaw: 0, pitch: 0)
            room.core.perEye(1920, 1080)
            room.setActive(true)
            room.core.useDiskEnvironments()
        }
        after(2.0) {
            guard let renderer = room.renderer, let recorder = Recorder(renderer: renderer, width: 3840, height: 1080, sideBySide: true, folder: folder) else {
                check("a recorder can be made", false)
                exit(1)
            }
            Task {
                let started = await recorder.start()
                await MainActor.run {
                    check("the recording starts", started)
                    // Two seconds of frames, at about the pace of a display, with the head turning so the picture moves.
                    var n = 0
                    let timer = Timer(timeInterval: 1.0 / 30, repeats: true) { t in
                        n += 1
                        room.core.holdHead(yaw: sin(Double(n) / 20) * 20, pitch: 0)
                        room.tick()
                        recorder.capture(sideBySide: true)
                        if n >= 60 {
                            t.invalidate()
                            recorder.stop { saved in
                                DispatchQueue.main.async { inspect(recorder, saved) }
                            }
                        }
                    }
                    RunLoop.main.add(timer, forMode: .common)
                }
            }
        }
        func inspect(_ recorder: Recorder, _ saved: Bool) {
            check("it finishes and saves a file (\(recorder.frames) frames, \(recorder.dropped) dropped)", saved && FileManager.default.fileExists(atPath: recorder.url.path))
            check("most of the frames made it", recorder.frames >= 40)
            let asset = AVURLAsset(url: recorder.url)
            Task {
                let videos = (try? await asset.loadTracks(withMediaType: .video)) ?? []
                let audios = (try? await asset.loadTracks(withMediaType: .audio)) ?? []
                let length = (try? await asset.load(.duration).seconds) ?? 0
                let natural = (try? await videos.first?.load(.naturalSize)) ?? .zero
                await MainActor.run {
                    check("it has a picture of 3840 by 1080 (\(Int(natural.width))x\(Int(natural.height)))", videos.count == 1 && Int(natural.width) == 3840 && Int(natural.height) == 1080)
                    check("about two seconds long (\(String(format: "%.1f", length)) s)", length > 1.0 && length < 4.0)
                    print("note  sound track: \(audios.isEmpty ? "none (Screen Recording may not be allowed to this build)" : "yes")")
                    let generator = AVAssetImageGenerator(asset: asset)
                    generator.appliesPreferredTrackTransform = true
                    generator.requestedTimeToleranceBefore = .positiveInfinity
                    generator.requestedTimeToleranceAfter = .positiveInfinity
                    if let frame = try? generator.copyCGImage(at: CMTime(seconds: max(0.1, length - 0.6), preferredTimescale: 600), actualTime: nil) {
                        try? NSBitmapImageRep(cgImage: frame).representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: "/tmp/spatiand-record-frame.png"))
                        print("wrote /tmp/spatiand-record-frame.png (\(frame.width)x\(frame.height))")
                        check("a frame can be read back", frame.width == 3840)
                    } else {
                        check("a frame can be read back", false)
                    }
                    print(failures == 0 ? "all passed" : "\(failures) failed")
                    exit(failures == 0 ? 0 : 1)
                }
            }
        }
        NSApplication.shared.setActivationPolicy(.accessory)
        NSApplication.shared.run()
    }
}
