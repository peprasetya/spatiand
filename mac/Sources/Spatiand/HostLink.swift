//  HostLink.swift — this Mac as a host, as Swift sees the Rust transport.
//
//  The other half of `Link`: that one is a session talking to a host, this one is a host
//  hearing a session. Messages are JSON; pictures go the other way as bytes.

import CSpatiand
import Foundation

final class HostLink {
    /// A device has connected: who, a short name, where from, the six digits to compare, and whether it is paired.
    var onJoined: (([String: Any]) -> Void)?
    /// A session has been let in and should be greeted.
    var onAttached: (() -> Void)?
    /// What the session said: a `ClientMessage` as serde writes it.
    var onSaid: (([String: Any]) -> Void)?
    /// The session is gone; `refused` if it never got in.
    var onLeft: ((Bool) -> Void)?

    private var core: OpaquePointer?

    init?(identityDir: String, port: UInt16) {
        try? FileManager.default.createDirectory(atPath: identityDir, withIntermediateDirectories: true)
        let me = Unmanaged.passUnretained(self).toOpaque()
        core = sp_host_start(identityDir, port, me) { user, json in
            guard let user, let json else { return }
            let link = Unmanaged<HostLink>.fromOpaque(user).takeUnretainedValue()
            let text = String(cString: json)
            guard let object = (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [String: Any] else { return }
            DispatchQueue.main.async {
                if let joined = object["joined"] as? [String: Any] {
                    link.onJoined?(joined)
                } else if object["attached"] != nil {
                    link.onAttached?()
                } else if let said = object["said"] {
                    if let message = said as? [String: Any] {
                        link.onSaid?(message)
                    } else if let name = said as? String {
                        // A message with no fields is its bare name.
                        link.onSaid?([name: NSNull()])
                    }
                } else if let left = object["left"] as? [String: Any] {
                    link.onLeft?((left["refused"] as? Bool) ?? false)
                }
            }
        }
        if core == nil { return nil }
    }

    deinit { if let core { sp_host_stop(core) } }

    var fingerprint: String {
        guard let core, let raw = sp_host_fingerprint(core) else { return "" }
        defer { sp_free_string(raw) }
        return String(cString: raw)
    }

    /// Say a `HostMessage`, as the JSON serde reads it.
    func say(_ message: [String: Any]) {
        guard let core, let data = try? JSONSerialization.data(withJSONObject: message),
              let text = String(data: data, encoding: .utf8) else { return }
        sp_host_say(core, text)
    }

    func video(window: UInt16, keyframe: Bool, capturedMicros: UInt64, _ data: Data) {
        guard let core else { return }
        data.withUnsafeBytes { raw in
            if let base = raw.bindMemory(to: UInt8.self).baseAddress {
                sp_host_video(core, window, keyframe ? 1 : 0, capturedMicros, base, data.count)
            }
        }
    }

    func openPairing(_ open: Bool) { if let core { sp_host_open_pairing(core, open ? 1 : 0) } }
    func decide(_ admit: Bool) { if let core { sp_host_decide(core, admit ? 1 : 0) } }
    func trust(_ fingerprint: String) { if let core { sp_host_trust(core, fingerprint) } }
    var pairedCount: Int { core.map { Int(sp_host_paired_count($0)) } ?? 0 }
    func forgetAll() { if let core { sp_host_forget_all(core) } }
}
