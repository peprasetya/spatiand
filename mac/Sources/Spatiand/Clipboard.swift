//  Clipboard.swift — copy here, paste there, and the other way round.
//
//  The session holds the clipboard for everything it can see (see spatiand-stream's
//  clipboard.rs), so this end does two things: when the Mac's pasteboard changes it says what is
//  on it, and when the host says its clipboard changed it puts that on the pasteboard. Small
//  text travels with the announcement; an image is fetched when it is announced, up to a limit,
//  because a pasteboard cannot wait on a network to answer a paste.
//
//  Text and PNG images. Anything else is announced by neither end.

import AppKit

final class ClipboardSync {
    private let board = NSPasteboard.general
    private var lastChange: Int
    private var timer: Timer?
    private var offeredText: String?
    private var offeredImage: Data?
    private let eagerText = 256 * 1024
    /// The largest image fetched without being asked: bigger waits for a person to want it, which
    /// this end cannot yet see, so it is left behind.
    private let eagerImage = 8 << 20

    /// Say something to the host.
    var send: (([String: Any]) -> Void)?
    var connected = false {
        didSet { if connected { lastChange = board.changeCount } }
    }

    init() { lastChange = NSPasteboard.general.changeCount }

    func start() {
        timer = Timer.scheduledTimer(withTimeInterval: 0.4, repeats: true) { [weak self] _ in self?.poll() }
    }

    // MARK: the Mac's clipboard changed

    private func poll() {
        let count = board.changeCount
        guard count != lastChange else { return }
        lastChange = count
        guard connected else { return }
        offer()
    }

    private func offer() {
        if let text = board.string(forType: .string), !text.isEmpty {
            let bytes = text.utf8.count
            offeredText = text
            offeredImage = nil
            send?(["Clipboard": ["Offer": [
                "mime_types": ["text/plain;charset=utf-8"],
                "text": bytes <= eagerText ? text as Any : NSNull(),
                "bytes": bytes,
            ]]])
        } else if let png = pngFromBoard() {
            offeredImage = png
            offeredText = nil
            send?(["Clipboard": ["Offer": [
                "mime_types": ["image/png"], "text": NSNull(), "bytes": png.count,
            ]]])
        }
    }

    private func pngFromBoard() -> Data? {
        if let png = board.data(forType: .png) { return png }
        guard let tiff = board.data(forType: .tiff), let rep = NSBitmapImageRep(data: tiff) else { return nil }
        return rep.representation(using: .png, properties: [:])
    }

    // MARK: the host's clipboard changed, or someone there pasted

    /// `body` is the Clipboard message's content: `["Offer": {...}]`, `["Want": {...}]` or `["Data": {...}]`.
    func fromHost(_ body: [String: Any]) {
        if let offer = body["Offer"] as? [String: Any] {
            let types = (offer["mime_types"] as? [String]) ?? []
            if let text = offer["text"] as? String {
                put { board.setString(text, forType: .string) }
            } else if types.contains(where: { $0.lowercased() == "image/png" }),
                      ((offer["bytes"] as? NSNumber)?.intValue ?? Int.max) <= eagerImage {
                send?(["Clipboard": ["Want": ["mime_type": "image/png"]]])
            }
        } else if let want = body["Want"] as? [String: Any], let mime = want["mime_type"] as? String {
            answer(mime)
        } else if let data = body["Data"] as? [String: Any], let mime = data["mime_type"] as? String,
                  mime.lowercased() == "image/png", let encoded = data["bytes"] as? String,
                  let png = Data(base64Encoded: encoded) {
            put { board.setData(png, forType: .png) }
        }
    }

    private func answer(_ mime: String) {
        var bytes: Data?
        if mime.lowercased() == "image/png" {
            bytes = offeredImage
        } else if let text = offeredText {
            bytes = Data(text.utf8)
        }
        guard let bytes else { return }
        send?(["Clipboard": ["Data": ["mime_type": mime, "bytes": bytes.base64EncodedString()]]])
    }

    /// Write to the pasteboard without taking our own change for the owner's.
    private func put(_ write: () -> Bool) {
        board.clearContents()
        _ = write()
        lastChange = board.changeCount
    }
}
