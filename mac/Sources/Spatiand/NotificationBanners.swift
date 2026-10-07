//  NotificationBanners.swift — the Mac's notifications, in the glasses.
//
//  A notification is drawn by macOS, not given to applications, so there is nothing to subscribe to; but it is drawn, in
//  a window of the system's Notification Center that covers the screen and is clear except where a banner is. That window
//  is captured, the corner banners come in, are shown held to the head, top right under the clock, and a click on one is
//  made on the real banner: the application it is from opens, and its windows come into the room.
//
//  Needs the Screen Recording permission the room already has.

import AppKit
import CoreMedia
import ScreenCaptureKit

final class NotificationBanners: NSObject, SCStreamOutput, SCStreamDelegate {
    /// The part of the screen banners appear in, in points from the top right corner: a banner is about 344 wide, and a
    /// few of them stack down from the top.
    static let regionWidth: CGFloat = 440
    static let regionHeight: CGFloat = 420

    private var stream: SCStream?
    private let queue = DispatchQueue(label: "spatiand.banners", qos: .userInitiated)
    private(set) var running = false
    /// What is on show: the banners' picture (premultiplied BGRA), its size in pixels, and where its top left is on the
    /// Mac's screen, in points, with how many pixels a point is.
    struct Shown { var pixels: Data; var width: Int; var height: Int; var origin: CGPoint; var scale: CGFloat }
    private let lock = NSLock()
    private var shown: Shown?
    private var lastSent: Shown?
    /// The window the banners are in, for addressing events to it.
    private(set) var windowID: CGWindowID = 0
    private(set) var pid: pid_t = 0
    private var regionOrigin = CGPoint.zero
    private var scale: CGFloat = 2
    /// Said, on the main thread, when the picture changes: the picture, or nil when no banner is showing.
    var onChange: ((Shown?) -> Void)?

    func start() async {
        guard !running else { return }
        do {
            let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
            // The system's own full-screen layer, the biggest window Notification Center has.
            guard let window = content.windows
                .filter({ $0.owningApplication?.bundleIdentifier == "com.apple.notificationcenterui" && $0.windowLayer > 0 })
                .max(by: { $0.frame.width * $0.frame.height < $1.frame.width * $1.frame.height }) else {
                print("banners: no Notification Center window to watch")
                return
            }
            windowID = window.windowID
            pid = window.owningApplication?.processID ?? 0
            scale = NSScreen.screens.first { $0.frame.intersects(window.frame) }?.backingScaleFactor ?? 2
            let region = CGRect(x: window.frame.width - Self.regionWidth, y: 0, width: Self.regionWidth, height: Self.regionHeight)
            regionOrigin = CGPoint(x: window.frame.minX + region.minX, y: window.frame.minY + region.minY)
            let config = SCStreamConfiguration()
            config.sourceRect = region
            config.width = Int(region.width * scale)
            config.height = Int(region.height * scale)
            config.pixelFormat = kCVPixelFormatType_32BGRA
            config.showsCursor = false
            config.queueDepth = 3
            config.minimumFrameInterval = CMTime(value: 1, timescale: 20)
            config.backgroundColor = .clear
            let s = SCStream(filter: SCContentFilter(desktopIndependentWindow: window), configuration: config, delegate: self)
            try s.addStreamOutput(self, type: .screen, sampleHandlerQueue: queue)
            try await s.startCapture()
            stream = s
            running = true
        } catch {
            print("banners: could not watch: \(error)")
        }
    }

    func stop() {
        let s = stream
        stream = nil
        running = false
        lock.lock(); shown = nil; lastSent = nil; lock.unlock()
        Task { try? await s?.stopCapture() }
        DispatchQueue.main.async { [weak self] in self?.onChange?(nil) }
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sample: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, CMSampleBufferIsValid(sample), let buffer = CMSampleBufferGetImageBuffer(sample) else { return }
        if let list = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: false) as? [[String: Any]],
           let raw = list.first?[SCStreamFrameInfo.status.rawValue] as? Int, let status = SCFrameStatus(rawValue: raw), status != .complete { return }
        let found = Self.crop(buffer, origin: regionOrigin, scale: scale)
        lock.lock()
        let same = (found == nil && lastSent == nil) || (found != nil && lastSent != nil && found!.pixels == lastSent!.pixels && found!.origin == lastSent!.origin)
        if !same { lastSent = found }
        lock.unlock()
        if !same { DispatchQueue.main.async { [weak self] in self?.onChange?(found) } }
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        running = false
        print("banners: the capture stopped: \(error)")
    }

    /// The part of a picture that has banners in it, as its own picture, or nil if there is none: pixels that are more
    /// than half there are the banner (a shadow is far fainter), and their box, a little enlarged, is what is kept.
    static func crop(_ buffer: CVPixelBuffer, origin: CGPoint, scale: CGFloat) -> Shown? {
        CVPixelBufferLockBaseAddress(buffer, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(buffer, .readOnly) }
        guard let base = CVPixelBufferGetBaseAddress(buffer) else { return nil }
        let (w, h, rowBytes) = (CVPixelBufferGetWidth(buffer), CVPixelBufferGetHeight(buffer), CVPixelBufferGetBytesPerRow(buffer))
        var (minX, minY, maxX, maxY) = (w, h, -1, -1)
        // Every second pixel each way: a banner is large, and this is done for each picture.
        for y in stride(from: 0, to: h, by: 2) {
            let row = base.advanced(by: y * rowBytes).assumingMemoryBound(to: UInt8.self)
            for x in stride(from: 0, to: w, by: 2) where row[x * 4 + 3] > 140 {
                if x < minX { minX = x }
                if x > maxX { maxX = x }
                if y < minY { minY = y }
                if y > maxY { maxY = y }
            }
        }
        guard maxX >= 0 else { return nil }
        let pad = Int(8 * scale)
        let (x0, y0) = (max(0, minX - pad), max(0, minY - pad))
        let (x1, y1) = (min(w, maxX + pad), min(h, maxY + pad))
        let (cw, ch) = (x1 - x0, y1 - y0)
        guard cw > 8, ch > 8 else { return nil }
        var out = Data(count: cw * ch * 4)
        out.withUnsafeMutableBytes { dst in
            for y in 0..<ch {
                let src = base.advanced(by: (y0 + y) * rowBytes + x0 * 4)
                memcpy(dst.baseAddress!.advanced(by: y * cw * 4), src, cw * 4)
            }
        }
        return Shown(pixels: out, width: cw, height: ch, origin: CGPoint(x: origin.x + CGFloat(x0) / scale, y: origin.y + CGFloat(y0) / scale), scale: scale)
    }

    /// Where a point of the shown picture, in its pixels, is on the Mac's screen.
    static func screenPoint(_ shown: Shown, x: Double, y: Double) -> CGPoint {
        CGPoint(x: shown.origin.x + CGFloat(x) / shown.scale, y: shown.origin.y + CGFloat(y) / shown.scale)
    }
}
