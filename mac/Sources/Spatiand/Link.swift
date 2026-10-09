//  Link.swift — the Rust link to a host, as Swift sees it.
//
//  Wraps the C interface in `spatiand_core.h`. Events, pictures and sound all arrive on threads
//  of the core's own; this puts them on the main queue as Swift values, which is where the rest
//  of the app lives.

import CSpatiand
import Foundation

final class Link {
    /// A host message: `["Opened": {...}]`, `["Catalog": {...}]`, and so on, as serde writes it.
    var onHost: (([String: Any]) -> Void)?
    /// The core's own news: connected, disconnected, compare, paired, pair_failed.
    var onCore: ((String, Any) -> Void)?
    /// One whole picture: window, codec (0 H.264, 1 HEVC, 2 AV1), keyframe, host's capture time µs, Annex B bytes.
    var onVideo: ((UInt16, Int32, Bool, UInt64, Data) -> Void)?
    /// Sound: app, channels, signed 16-bit little-endian interleaved at 48 kHz.
    var onAudio: ((String, UInt16, Data) -> Void)?

    private var core: OpaquePointer?

    init?(identityDir: String) {
        try? FileManager.default.createDirectory(atPath: identityDir, withIntermediateDirectories: true)
        let me = Unmanaged.passUnretained(self).toOpaque()
        core = sp_start(identityDir, me,
            { user, json in
                guard let user, let json else { return }
                let link = Unmanaged<Link>.fromOpaque(user).takeUnretainedValue()
                let text = String(cString: json)
                guard let object = (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [String: Any] else { return }
                DispatchQueue.main.async {
                    if let host = object["host"] {
                        if let message = host as? [String: Any] {
                            link.onHost?(message)
                        } else if let name = host as? String {
                            // A message with no fields serialises as its bare name.
                            link.onHost?([name: NSNull()])
                        }
                    } else if let core = object["core"] as? [String: Any], let (what, detail) = core.first {
                        link.onCore?(what, detail)
                    }
                }
            },
            { user, window, codec, keyframe, captured, bytes, length in
                guard let user, let bytes else { return }
                let link = Unmanaged<Link>.fromOpaque(user).takeUnretainedValue()
                let data = Data(bytes: bytes, count: length)
                DispatchQueue.main.async { link.onVideo?(window, codec, keyframe != 0, captured, data) }
            },
            { user, app, channels, pcm, length in
                guard let user, let app, let pcm else { return }
                let link = Unmanaged<Link>.fromOpaque(user).takeUnretainedValue()
                let name = String(cString: app)
                let data = Data(bytes: pcm, count: length)
                // Sound does not wait for the main queue: it would be heard late.
                link.onAudio?(name, channels, data)
            })
        if core == nil { return nil }
    }

    deinit { if let core { sp_stop(core) } }

    var fingerprint: String {
        guard let core, let raw = sp_fingerprint(core) else { return "" }
        defer { sp_free_string(raw) }
        return String(cString: raw)
    }

    func pair(address: String) { if let core { sp_pair(core, address) } }
    func connect(address: String, fingerprint: String) { if let core { sp_connect(core, address, fingerprint) } }
    func disconnect() { if let core { sp_disconnect(core) } }

    /// Say a `ClientMessage`, as the JSON serde would write it.
    func say(_ message: [String: Any]) {
        guard let core, let data = try? JSONSerialization.data(withJSONObject: message),
              let text = String(data: data, encoding: .utf8) else { return }
        sp_say(core, text)
    }

    func launch(_ app: String) { say(["Launch": ["app": app]]) }
}
