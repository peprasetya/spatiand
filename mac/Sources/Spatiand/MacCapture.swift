//  MacCapture.swift — this Mac's own windows, brought into the room.
//
//  A host's windows arrive as video. A window of this Mac needs no video at all: ScreenCaptureKit
//  hands over its picture as a GPU surface, which the room draws as it is. So a Mac window in the
//  glasses costs a capture and a texture, with no encode, no decode and no network.
//
//  Needs the Screen Recording permission, which macOS asks for the first time a window is
//  captured. Clicking and typing into one needs Accessibility as well, to post events at it; see
//  `MacInput`.

import AppKit
import ApplicationServices
import CoreMedia
import ScreenCaptureKit

/// The window of an accessibility element, by number: not in any header, and what every tool that
/// maps one kind of window to the other uses.
@_silgen_name("_AXUIElementGetWindow")
private func _AXUIElementGetWindow(_ element: AXUIElement, _ id: UnsafeMutablePointer<CGWindowID>) -> AXError

/// A window of this Mac that could be brought into the room.
struct MacWindowInfo: Equatable {
    let windowID: CGWindowID
    let pid: pid_t
    let bundle: String
    let app: String
    let title: String
    /// In points, in the global space with its origin at the top left of the main display.
    let frame: CGRect
}

enum MacWindows {
    /// The ids of Mac windows in the room start here, clear of the host's (which are small) and of
    /// Spatiand's own panels (which are at the top).
    static let firstID: UInt16 = 0x8000
    static func isMac(_ id: UInt16) -> Bool { id >= firstID && id < 0xFFF0 }

    /// Whether the permission to capture is in hand, asking for it if it is not.
    static func allowed(ask: Bool) -> Bool {
        if CGPreflightScreenCaptureAccess() { return true }
        if ask { CGRequestScreenCaptureAccess() }
        return false
    }

    /// The accessibility element of a window, for closing it and sizing it. Needs Accessibility.
    static func axWindow(_ info: MacWindowInfo) -> AXUIElement? {
        let app = AXUIElementCreateApplication(info.pid)
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &value) == .success,
              let windows = value as? [AXUIElement] else { return nil }
        return windows.first { w in
            var id: CGWindowID = 0
            return _AXUIElementGetWindow(w, &id) == .success && id == info.windowID
        }
    }

    /// Ask a window to be this size, in points. It may decline: a window has its own limits.
    @discardableResult
    static func resize(_ info: MacWindowInfo, toPoints size: CGSize) -> Bool {
        guard let window = axWindow(info) else { return false }
        var wanted = size
        guard let value = AXValueCreate(.cgSize, &wanted) else { return false }
        return AXUIElementSetAttributeValue(window, kAXSizeAttribute as CFString, value) == .success
    }

    /// The windows worth offering: ordinary, on screen, big enough to be a window and not one of
    /// Spatiand's own.
    static func list() async -> [MacWindowInfo] {
        guard let content = try? await SCShareableContent.excludingDesktopWindows(true, onScreenWindowsOnly: true) else { return [] }
        let mine = ProcessInfo.processInfo.processIdentifier
        return content.windows.compactMap { w in
            guard w.windowLayer == 0, w.isOnScreen, w.frame.width >= 160, w.frame.height >= 100,
                  let app = w.owningApplication, app.processID != mine, !app.applicationName.isEmpty else { return nil }
            return MacWindowInfo(windowID: w.windowID, pid: app.processID, bundle: app.bundleIdentifier, app: app.applicationName,
                                 title: w.title ?? "", frame: w.frame)
        }
        .sorted { ($0.app, $0.title) < ($1.app, $1.title) }
    }
}

/// One Mac window being captured: its newest picture, kept for the room to draw.
final class MacCapture: NSObject, PictureSource, SCStreamOutput, SCStreamDelegate {
    let info: MacWindowInfo
    private var stream: SCStream?
    private let lock = NSLock()
    private var newest: CVPixelBuffer?
    private var generation = 0
    /// How many pixels a point of the window is: the picture is captured at the display's density.
    private(set) var scale: CGFloat = 2
    /// Said, off the main thread, when the first picture and each change of size arrives.
    var onSize: ((CGSize) -> Void)?
    var onEnded: (() -> Void)?
    /// Every complete picture, off the main thread, with its time in microseconds.
    var onFrame: ((CVPixelBuffer, UInt64) -> Void)?
    private var config: SCStreamConfiguration?
    private var configuredSize = CGSize.zero
    private var lastResize = Date.distantPast
    private var lastSize = CGSize.zero

    init(_ info: MacWindowInfo) { self.info = info }

    func start() async throws {
        let content = try await SCShareableContent.excludingDesktopWindows(true, onScreenWindowsOnly: false)
        guard let window = content.windows.first(where: { $0.windowID == info.windowID }) else { throw CaptureError.gone }
        let screenScale = NSScreen.screens.first { $0.frame.intersects(window.frame) }?.backingScaleFactor ?? NSScreen.main?.backingScaleFactor ?? 2
        scale = screenScale
        let filter = SCContentFilter(desktopIndependentWindow: window)
        let config = SCStreamConfiguration()
        config.width = Int(window.frame.width * scale)
        config.height = Int(window.frame.height * scale)
        config.pixelFormat = kCVPixelFormatType_32BGRA
        config.showsCursor = false
        config.queueDepth = 3
        config.minimumFrameInterval = CMTime(value: 1, timescale: 60)
        self.config = config
        configuredSize = window.frame.size
        let s = SCStream(filter: filter, configuration: config, delegate: self)
        try s.addStreamOutput(self, type: .screen, sampleHandlerQueue: DispatchQueue(label: "spatiand.maccapture", qos: .userInteractive))
        try await s.startCapture()
        stream = s
    }

    enum CaptureError: Error { case gone }

    func invalidate() {
        let s = stream
        stream = nil
        Task { try? await s?.stopCapture() }
    }

    func current() -> (CVPixelBuffer, Int)? {
        lock.lock()
        defer { lock.unlock() }
        return newest.map { ($0, generation) }
    }

    // MARK: where it is now

    private var cachedFrame: (CGRect, Date)?

    /// The window's frame at this moment, in points: it can be moved while it is in the room.
    func frame() -> CGRect {
        if let (rect, at) = cachedFrame, Date().timeIntervalSince(at) < 0.3 { return rect }
        var rect = info.frame
        if let list = CGWindowListCreateDescriptionFromArray([info.windowID] as CFArray) as? [[String: Any]],
           let bounds = list.first?[kCGWindowBounds as String] as? NSDictionary,
           let found = CGRect(dictionaryRepresentation: bounds) {
            rect = found
        }
        cachedFrame = (rect, Date())
        return rect
    }

    /// Where a point of the captured picture, in pixels from its top left, is on the Mac's screen.
    func screenPoint(x: Double, y: Double) -> CGPoint {
        let f = frame()
        let (w, h) = lastSize.width > 0 ? (lastSize.width, lastSize.height) : (f.width * scale, f.height * scale)
        return CGPoint(x: f.minX + CGFloat(x) / w * f.width, y: f.minY + CGFloat(y) / h * f.height)
    }

    // MARK: SCStreamOutput

    func stream(_ stream: SCStream, didOutputSampleBuffer sample: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, CMSampleBufferIsValid(sample), let pixels = CMSampleBufferGetImageBuffer(sample) else { return }
        // A frame the capture marks as complete carries a new picture; the others (idle, blank) do not.
        if let list = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: false) as? [[String: Any]],
           let raw = list.first?[SCStreamFrameInfo.status.rawValue] as? Int, let status = SCFrameStatus(rawValue: raw), status != .complete {
            return
        }
        lock.lock()
        newest = pixels
        generation += 1
        lock.unlock()
        let seconds = CMTimeGetSeconds(CMSampleBufferGetPresentationTimeStamp(sample))
        onFrame?(pixels, UInt64(max(0, seconds) * 1_000_000))
        followResize()
        let size = CGSize(width: CVPixelBufferGetWidth(pixels), height: CVPixelBufferGetHeight(pixels))
        if size != lastSize {
            lastSize = size
            onSize?(size)
        }
    }

    /// A window that has been resized is captured at its new size, or it would be scaled to the old.
    private func followResize() {
        guard let stream, let config, Date().timeIntervalSince(lastResize) > 0.4 else { return }
        let now = frame().size
        guard abs(now.width - configuredSize.width) > 1 || abs(now.height - configuredSize.height) > 1, now.width >= 50, now.height >= 50 else { return }
        lastResize = Date()
        configuredSize = now
        config.width = Int(now.width * scale)
        config.height = Int(now.height * scale)
        Task { try? await stream.updateConfiguration(config) }
    }

    // MARK: SCStreamDelegate

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        DispatchQueue.main.async { [weak self] in self?.onEnded?() }
    }
}
