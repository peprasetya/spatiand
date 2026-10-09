//  RoomRecorder.swift — a video of what the glasses show.
//
//  The compositor says when (the settings' Record row); the picture is taken from the glasses'
//  display by ScreenCaptureKit -- one eye of it, since a film of two is no use to anyone -- and
//  written with the Mac's sound to ~/Movies/Spatiand. Without glasses, in the test window, it
//  is that window which is filmed.

import AppKit
import ScreenCaptureKit

/// Needs macOS 15, which is where the system learned to write the file itself.
@available(macOS 15.0, *)
final class RoomRecorder: NSObject, SCStreamDelegate, SCRecordingOutputDelegate {
    static let shared = RoomRecorder()
    private var stream: SCStream?
    private var file: URL?
    var isRecording: Bool { stream != nil }

    func start(screen: GlassesScreen) {
        guard stream == nil else { return }
        Task {
            do {
                let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
                let config = SCStreamConfiguration()
                let filter: SCContentFilter
                if let id = screen.display, let display = content.displays.first(where: { $0.displayID == id }) {
                    filter = SCContentFilter(display: display, excludingWindows: [])
                    let mode = CGDisplayCopyDisplayMode(id)
                    let (w, h) = (mode?.pixelWidth ?? display.width, mode?.pixelHeight ?? display.height)
                    // The left eye of two, or all of one.
                    let two = w >= h * 3
                    config.sourceRect = CGRect(x: 0, y: 0, width: Double(display.width) / (two ? 2 : 1), height: Double(display.height))
                    config.width = two ? w / 2 : w
                    config.height = h
                } else if let number = screen.windowNumber, let window = content.windows.first(where: { $0.windowID == number }) {
                    filter = SCContentFilter(desktopIndependentWindow: window)
                    config.width = Int(window.frame.width) * 2
                    config.height = Int(window.frame.height) * 2
                } else {
                    print("recording: nothing of the room's to film")
                    return
                }
                config.minimumFrameInterval = CMTime(value: 1, timescale: 60)
                config.showsCursor = false
                config.capturesAudio = true
                let folder = FileManager.default.urls(for: .moviesDirectory, in: .userDomainMask)[0].appendingPathComponent("Spatiand")
                try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
                let stamp = DateFormatter()
                stamp.dateFormat = "yyyy-MM-dd HH.mm.ss"
                let url = folder.appendingPathComponent("Spatiand \(stamp.string(from: Date())).mov")
                let output = SCRecordingOutputConfiguration()
                output.outputURL = url
                output.outputFileType = .mov
                output.videoCodecType = .h264
                let s = SCStream(filter: filter, configuration: config, delegate: self)
                try s.addRecordingOutput(SCRecordingOutput(configuration: output, delegate: self))
                try await s.startCapture()
                stream = s
                file = url
                print("recording: started, \(config.width)x\(config.height), to \(url.path)")
            } catch {
                print("recording: could not start: \(error)")
            }
        }
    }

    func stop() {
        guard let s = stream else { return }
        stream = nil
        let url = file
        Task {
            try? await s.stopCapture()
            if let url {
                let size = (try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? Int) ?? 0
                print("recording: saved \(url.path) (\(size / 1024) KB)")
            }
        }
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        print("recording: stopped by the system: \(error)")
        self.stream = nil
    }
}
