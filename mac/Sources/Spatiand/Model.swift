//  Model.swift — what Spatiand knows and is showing: the session, the host's applications, and
//  the windows open on this Mac.
//
//  One object, on the main thread, because everything it holds is only ever read to draw a menu
//  or a window. The link's callbacks arrive here already on the main queue.

import AppKit

final class Model {
    static let shared = Model()

    struct RemoteApp { let id: String; let name: String }
    struct WindowInfo { var app: String; var title: String; var hasParent: Bool }

    let link: Link
    private(set) var host: PairedHost?
    private(set) var connected = false
    private(set) var apps: [RemoteApp] = []
    private(set) var windows: [UInt16: RemoteWindow] = [:]
    private var infos: [UInt16: WindowInfo] = [:]
    private var sizes: [UInt16: CGSize] = [:]

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
        link.onVideo = { [unowned self] window, codec, _, captured, data in
            windows[window]?.video(data, codec: codec, captured: captured)
        }
    }

    // MARK: asking

    func connect(_ host: PairedHost) {
        closeAll()
        self.host = host
        connected = false
        link.connect(address: host.address, fingerprint: host.fingerprint)
        onChange?()
    }

    func disconnect() {
        link.disconnect()
        closeAll()
        connected = false
        apps = []
        onChange?()
    }

    func launch(_ app: RemoteApp) { link.launch(app.id) }

    func pair(address: String) { link.pair(address: address) }

    func forget(_ host: PairedHost) {
        if self.host == host { disconnect(); self.host = nil }
        Hosts.forget(host)
    }

    var openWindows: [(id: UInt16, title: String)] {
        windows.keys.sorted().map { ($0, infos[$0]?.title ?? "Window") }
    }

    func raise(_ id: UInt16) { windows[id]?.show() }

    // MARK: hearing

    private func core(_ what: String, _ detail: Any) {
        switch what {
        case "connected":
            connected = true
        case "disconnected":
            connected = false
            closeAll()
            if let reason = detail as? String, !reason.isEmpty, host != nil { onProblem?(reason) }
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
            infos[id] = WindowInfo(
                app: fields["app"] as? String ?? "", title: fields["title"] as? String ?? "",
                hasParent: !(fields["parent"] is NSNull) && fields["parent"] != nil)
        case "Stream":
            guard let id = window(), let w = fields["width"] as? NSNumber, let h = fields["height"] as? NSNumber else { return }
            let size = CGSize(width: w.doubleValue, height: h.doubleValue)
            sizes[id] = size
            // A picture to start from. A window that is not changing sends nothing by itself, so
            // one that was already there when this session arrived would stay black for ever.
            link.say(["WantKeyframe": ["window": Int(id)]])
            if let existing = windows[id] {
                existing.setSize(size)
            } else if infos[id]?.hasParent != true {
                open(id, size: size)
            }
        case "Retitled":
            if let id = window(), let title = fields["title"] as? String {
                infos[id]?.title = title
                windows[id]?.window.title = title
            }
        case "Closed":
            if let id = window() {
                windows.removeValue(forKey: id)?.closeForReal()
                infos[id] = nil
                sizes[id] = nil
            }
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
        remote.show()
        NSApp.activate(ignoringOtherApps: true)
    }

    private func closeAll() {
        for window in windows.values { window.closeForReal() }
        windows = [:]
        infos = [:]
        sizes = [:]
    }
}
