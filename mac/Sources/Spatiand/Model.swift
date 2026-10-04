//  Model.swift — what Spatiand knows and is showing: the session, the host's applications, and
//  the windows open on this Mac.
//
//  One object, on the main thread, because everything it holds is only ever read to draw a menu
//  or a window. The link's callbacks arrive here already on the main queue.

import AppKit

final class Model {
    static let shared = Model()

    struct RemoteApp { let id: String; let name: String }
    struct WindowInfo {
        var app: String
        var title: String
        /// Set for a menu or a tooltip: the window it hangs off, and where on its picture.
        var parent: (id: UInt16, x: Double, y: Double)?
    }

    let link: Link
    /// `SPATIAND_DEBUG=1` says what arrives, for working out why a window is black.
    let debug = ProcessInfo.processInfo.environment["SPATIAND_DEBUG"] != nil
    let clipboard = ClipboardSync()
    /// The glasses' world, which windows go into when there are glasses.
    let room = RoomController()
    private(set) var host: PairedHost?
    private(set) var connected = false
    /// Another device took this host's windows over. Not answered by taking them straight back:
    /// two devices doing that would pass the windows between them for ever.
    private(set) var takenOver = false
    private(set) var apps: [RemoteApp] = []
    private(set) var windows: [UInt16: RemoteSurface] = [:]
    private var infos: [UInt16: WindowInfo] = [:]
    private var sizes: [UInt16: CGSize] = [:]
    /// A keyframe that arrived before its window did. The window's announcement and its first
    /// picture travel on different paths and either can win; a window must not wait for the
    /// next keyframe -- which, for one that is not changing, may be never -- because it lost.
    private var earlyKeyframes: [UInt16: (codec: Int32, captured: UInt64, data: Data)] = [:]
    private var watchdog: Timer?

    /// The menu rebuilds itself when it opens, so most changes need no notice; this is for the
    /// ones that should show without it (the icon, a pairing in progress).
    var onChange: (() -> Void)?
    /// Pairing, as the user sees it: the code to compare, and how it ended.
    var onCompare: ((String, String) -> Void)?
    var onPaired: ((PairedHost) -> Void)?
    var onPairFailed: ((String) -> Void)?
    var onProblem: ((String) -> Void)?

    private init() {
        let dir = NSString("~/Library/Application Support/Spatiand/identity").expandingTildeInPath
        link = Link(identityDir: dir)!
        link.onHost = { [unowned self] in handle($0) }
        link.onCore = { [unowned self] what, detail in core(what, detail) }
        link.onVideo = { [unowned self] window, codec, keyframe, captured, data in
            if debug {
                let sets = VideoSamples.nalUnits(data).map { Int($0.first ?? 0) }
                print("video: window \(window) codec \(codec) key \(keyframe) \(data.count) bytes, nal headers \(sets.prefix(6)), surface \(windows[window] != nil)")
            }
            if let surface = windows[window] {
                surface.video(data, codec: codec, captured: captured)
            } else if keyframe {
                earlyKeyframes[window] = (codec, captured, data)
            }
        }
        // Until a picture has been shown, ask again every second.
        watchdog = Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { [unowned self] _ in
            for (id, surface) in windows where surface.view.pictures == 0 {
                link.say(["WantKeyframe": ["window": Int(id)]])
            }
        }
        link.onAudio = { [unowned self] app, channels, data in
            // In the room the sound is placed where its window is, and follows the head.
            AudioOut.shared.feed(app: app, channels: Int(channels), data: data, placed: room.place(app: app, channels: Int(channels), pcm: data))
        }
        clipboard.send = { [unowned self] in link.say($0) }
        clipboard.start()
    }

    // MARK: asking

    /// Whether the owner has asked to be connected, so a link that drops is brought back and one
    /// the owner ended is not.
    private var wantConnection = false
    private var retry: Timer?

    func connect(_ host: PairedHost) {
        closeAll()
        wantConnection = true
        takenOver = false
        self.host = host
        connected = false
        link.connect(address: host.address, fingerprint: host.fingerprint)
        onChange?()
    }

    func disconnect() {
        wantConnection = false
        retry?.invalidate()
        link.disconnect()
        closeAll()
        connected = false
        apps = []
        onChange?()
    }

    /// Starting an application here is asking for the windows back, as it is on the Deck.
    func launch(_ app: RemoteApp) {
        if !connected, takenOver, let host {
            pendingLaunch = app
            connect(host)
        } else {
            link.launch(app.id)
        }
    }
    private var pendingLaunch: RemoteApp?

    /// Whether the wearer has glasses on: whether windows are in the room, and so whether a
    /// virtual-reality application should draw the world or an ordinary view. Told to the host
    /// when it changes and again each time a session begins, because a host forgets with the
    /// session.
    var glassesOn = false {
        didSet {
            if glassesOn != oldValue {
                sendGlasses()
                room.setActive(glassesOn)
            }
        }
    }

    /// Tell the host how much this link can carry; it lowers its own ceiling to it.
    func sendBandwidth() {
        guard connected else { return }
        link.say(["Bandwidth": ["max_kbit": Settings.maxKbit, "max_fps": 60, "idle_fps": 0, "idle_after_ms": 250]])
    }

    private func sendGlasses() {
        guard connected else { return }
        link.say(["Glasses": ["on": glassesOn]])
    }

    func pair(address: String) { link.pair(address: address) }

    func forget(_ host: PairedHost) {
        if self.host == host { disconnect(); self.host = nil }
        Hosts.forget(host)
    }

    var openWindows: [(id: UInt16, title: String)] {
        windows.keys.sorted().filter { infos[$0]?.parent == nil }.map { ($0, infos[$0]?.title ?? "Window") }
    }

    func raise(_ id: UInt16) {
        // In the room, raising a window is bringing it to where the wearer is looking.
        if room.active {
            room.core.bringHere(id)
            room.core.focused = id
        } else {
            windows[id]?.show()
        }
    }

    // MARK: hearing

    private func core(_ what: String, _ detail: Any) {
        switch what {
        case "connected":
            connected = true
            clipboard.connected = true
            sendGlasses()
            sendBandwidth()
            if let app = pendingLaunch { pendingLaunch = nil; link.launch(app.id) }
        case "disconnected":
            connected = false
            clipboard.connected = false
            closeAll()
            if (detail as? String) == "taken over" {
                wantConnection = false
                takenOver = true
                print("another device took \(host?.name ?? "the host")'s windows; not taking them back")
            }
            // Brought back every few seconds for as long as it is wanted: a host that restarts, a
            // laptop that woke up, a Wi-Fi that blinked. Said once, not each time it fails.
            if wantConnection, let host {
                if retry == nil, let reason = detail as? String, !reason.isEmpty {
                    print("lost \(host.name): \(reason); trying again")
                }
                retry?.invalidate()
                retry = Timer.scheduledTimer(withTimeInterval: 5, repeats: false) { [unowned self] _ in
                    retry = nil
                    if wantConnection, !connected, let host = self.host {
                        link.connect(address: host.address, fingerprint: host.fingerprint)
                    }
                }
            }
        case "compare":
            if let d = detail as? [String: Any], let code = d["code"] as? String, let fp = d["fingerprint"] as? String {
                onCompare?(code, fp)
            }
        case "paired":
            if let d = detail as? [String: Any], let name = d["name"] as? String,
               let fp = d["fingerprint"] as? String, let address = d["address"] as? String {
                onPaired?(PairedHost(name: name, address: address, fingerprint: fp))
            }
        case "pair_failed":
            onPairFailed?((detail as? String) ?? "pairing failed")
        default: break
        }
        onChange?()
    }

    private func handle(_ message: [String: Any]) {
        guard let (name, body) = message.first else { return }
        if debug, name != "Catalog", name != "Cursor" { print("host: \(name) \(body)") }
        let fields = body as? [String: Any] ?? [:]
        func window() -> UInt16? { (fields["window"] as? NSNumber).map { UInt16(truncatingIfNeeded: $0.intValue) } }
        switch name {
        case "Catalog":
            apps = ((fields["apps"] as? [[String: Any]]) ?? []).compactMap {
                guard let id = $0["id"] as? String, let name = $0["name"] as? String else { return nil }
                return RemoteApp(id: id, name: name)
            }
        case "Opened":
            guard let id = window() else { return }
            var parent: (id: UInt16, x: Double, y: Double)?
            if let p = fields["parent"] as? [Any], p.count == 3,
               let pid = (p[0] as? NSNumber)?.intValue, let x = (p[1] as? NSNumber)?.doubleValue,
               let y = (p[2] as? NSNumber)?.doubleValue {
                parent = (UInt16(truncatingIfNeeded: pid), x, y)
            }
            infos[id] = WindowInfo(app: fields["app"] as? String ?? "", title: fields["title"] as? String ?? "", parent: parent)
        case "Stream":
            guard let id = window(), let w = fields["width"] as? NSNumber, let h = fields["height"] as? NSNumber else { return }
            let size = CGSize(width: w.doubleValue, height: h.doubleValue)
            sizes[id] = size
            // A picture to start from. A window that is not changing sends nothing by itself, so
            // one that was already there when this session arrived would stay black for ever.
            link.say(["WantKeyframe": ["window": Int(id)]])
            room.windowSized(id, size: size)
            if let existing = windows[id] {
                existing.setSize(size)
            } else if let parent = infos[id]?.parent {
                openPopup(id, parent: parent, size: size)
            } else {
                open(id, size: size)
            }
        case "Retitled":
            if let id = window(), let title = fields["title"] as? String {
                infos[id]?.title = title
                (windows[id] as? RemoteWindow)?.window.title = title
            }
        case "Closed":
            if let id = window() {
                windows.removeValue(forKey: id)?.closeForReal()
                room.windowClosed(id)
                infos[id] = nil
                sizes[id] = nil
            }
        case "Clipboard":
            clipboard.fromHost(fields)
        case "Cursor":
            cursor(fields)
        case "Refused":
            onProblem?((fields["reason"] as? String) ?? "the computer refused this session")
        default: break
        }
        onChange?()
    }

    private func open(_ id: UInt16, size: CGSize) {
        let title = infos[id]?.title ?? "Window"
        let remote = RemoteWindow(id: id, title: title, size: size)
        remote.view.send = { [unowned self] in link.say($0) }
        remote.view.isTerminal = Self.isTerminal(infos[id]?.app ?? "")
        remote.view.needKeyframe = { [unowned self] in link.say(["WantKeyframe": ["window": Int(id)]]) }
        remote.onClose = { [unowned self] id in link.say(["Close": ["window": Int(id)]]) }
        remote.onFocus = { [unowned self] id, focused in
            let target: Any = focused ? Int(id) : NSNull()
            link.say(["Focus": ["window": target]])
        }
        remote.onResize = { [unowned self] id, w, h in
            link.say(["Configure": ["window": Int(id), "width": w, "height": h]])
        }
        windows[id] = remote
        room.windowOpened(id, size: size, app: infos[id]?.app ?? "")
        feedEarly(id)
        if !room.active {
            remote.show()
            NSApp.activate(ignoringOtherApps: true)
        }
    }

    /// Terminals, where Control-C is an interrupt and copy is Control-Shift-C.
    private static func isTerminal(_ app: String) -> Bool {
        let name = app.lowercased()
        return ["terminal", "konsole", "xterm", "alacritty", "kitty", "foot", "wezterm", "tilix",
                "terminator", "urxvt", "rxvt", "st-256color", "ghostty", "lxterminal"].contains { name.contains($0) }
    }

    private func openPopup(_ id: UInt16, parent: (id: UInt16, x: Double, y: Double), size: CGSize) {
        guard let owner = windows[parent.id] as? RemoteWindow else { return }
        let popup = RemotePopup(id: id, parent: owner, offset: CGPoint(x: parent.x, y: parent.y), size: size)
        popup.view.needKeyframe = { [unowned self] in link.say(["WantKeyframe": ["window": Int(id)]]) }
        windows[id] = popup
        feedEarly(id)
        popup.show()
    }

    /// The host drew the pointer this way: premultiplied BGRA, with a hotspot, in its pixels.
    private func cursor(_ fields: [String: Any]) {
        guard let w = (fields["width"] as? NSNumber)?.intValue, let h = (fields["height"] as? NSNumber)?.intValue,
              w > 0, h > 0, let encoded = fields["pixels"] as? String, let pixels = Data(base64Encoded: encoded),
              pixels.count >= w * h * 4,
              let provider = CGDataProvider(data: pixels as CFData),
              let image = CGImage(
                width: w, height: h, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: w * 4,
                space: CGColorSpaceCreateDeviceRGB(),
                bitmapInfo: CGBitmapInfo(rawValue: CGBitmapInfo.byteOrder32Little.rawValue | CGImageAlphaInfo.premultipliedFirst.rawValue),
                provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent)
        else { return }
        let hot = (fields["hotspot"] as? [NSNumber]) ?? [0, 0]
        let scale = NSScreen.main?.backingScaleFactor ?? 2
        let size = NSSize(width: CGFloat(w) / scale, height: CGFloat(h) / scale)
        let picture = NSImage(cgImage: image, size: size)
        let cursor = NSCursor(image: picture, hotSpot: NSPoint(x: hot[0].doubleValue / scale, y: hot[1].doubleValue / scale))
        for surface in windows.values { surface.view.cursor = cursor }
    }

    private func feedEarly(_ id: UInt16) {
        if let early = earlyKeyframes.removeValue(forKey: id) {
            windows[id]?.video(early.data, codec: early.codec, captured: early.captured)
        }
    }

    private func closeAll() {
        room.sessionEnded()
        earlyKeyframes = [:]
        for window in windows.values { window.closeForReal() }
        windows = [:]
        infos = [:]
        sizes = [:]
    }
}
