//
//  MacDescribe.swift
//  What one of this Mac's applications shows, for the room's dictation and its assistant
//  (`SP_DESCRIBE`): the focused window's title and the field being typed in, read through
//  Accessibility -- a Mac window says nothing about itself through the room's protocol, so
//  this is its `describe` (docs/ai-integrations.md).
//
//  Off the main thread, and with a short timeout on every question: Accessibility asks the
//  application itself, and an application that is busy would otherwise hold up whoever asked.
//

import AppKit
import ApplicationServices
import CSpatiand

enum MacDescribe {
    private static let queue = DispatchQueue(label: "spatiand.describe", qos: .userInitiated)

    /// A terminal's whole scroll-back is its value: the end of it is what is being worked on.
    private static let longest = 6000

    /// Answer `SP_DESCRIBE` number `request`, for the application with this bundle id.
    static func describe(bundle: String, request: UInt32) {
        queue.async {
            let json = describing(bundle: bundle)
            json.withCString { sp_described(request, $0) }
        }
    }

    /// The same, to whoever is not the room.
    static func describe(bundle: String, answer: @escaping (String) -> Void) {
        queue.async { answer(describing(bundle: bundle)) }
    }

    private static func describing(bundle: String) -> String {
        var said: [String: Any] = ["kind": kind(of: bundle)]
        guard let running = NSRunningApplication.runningApplications(withBundleIdentifier: bundle).first else {
            return encode(said)
        }
        said["app"] = running.localizedName ?? bundle
        guard AXIsProcessTrusted() else { return encode(said) }
        let app = AXUIElementCreateApplication(running.processIdentifier)
        AXUIElementSetMessagingTimeout(app, 0.5)
        if let window = element(app, kAXFocusedWindowAttribute) {
            said["title"] = string(window, kAXTitleAttribute) ?? ""
            // A document's window knows its file: where it is, for the names near it.
            if let document = string(window, kAXDocumentAttribute), let url = URL(string: document), url.isFileURL {
                said["directory"] = url.deletingLastPathComponent().path
            }
        }
        if let focused = element(app, kAXFocusedUIElementAttribute) {
            var text = string(focused, kAXValueAttribute) ?? ""
            if text.count > longest { text = String(text.suffix(longest)) }
            let label = string(focused, kAXDescriptionAttribute) ?? string(focused, kAXPlaceholderValueAttribute) ?? string(focused, kAXTitleAttribute) ?? ""
            // The field last: it is what the next words go into.
            said["text"] = label.isEmpty || said["kind"] as? String == "terminal" ? text : label + ": " + text
        }
        return encode(said)
    }

    /// What kind of window an application has, by its bundle id: what dictation does with
    /// what is said depends on it.
    private static func kind(of bundle: String) -> String {
        let b = bundle.lowercased()
        let terminals = ["com.apple.terminal", "com.googlecode.iterm2", "com.mitchellh.ghostty", "net.kovidgoyal.kitty",
                         "org.alacritty", "com.github.wez.wezterm", "dev.warp.warp-stable", "co.zeit.hyper"]
        let browsers = ["com.apple.safari", "com.google.chrome", "org.mozilla.firefox", "company.thebrowser.browser",
                        "com.microsoft.edgemac", "com.brave.browser", "com.operasoftware.opera", "com.vivaldi.vivaldi"]
        if terminals.contains(where: { b.hasPrefix($0) }) { return "terminal" }
        if browsers.contains(where: { b.hasPrefix($0) }) { return "web" }
        return "text"
    }

    private static func element(_ of: AXUIElement, _ name: String) -> AXUIElement? {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(of, name as CFString, &value) == .success, let value,
              CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
        return (value as! AXUIElement)
    }

    private static func string(_ of: AXUIElement, _ name: String) -> String? {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(of, name as CFString, &value) == .success else { return nil }
        return value as? String
    }

    private static func encode(_ said: [String: Any]) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: said), let json = String(data: data, encoding: .utf8) else { return "{}" }
        return json
    }
}
