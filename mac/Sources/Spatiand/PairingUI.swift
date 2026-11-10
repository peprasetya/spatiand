//  PairingUI.swift — adding a computer.
//
//  Pairing is the one step that decides who is trusted, so it is kept honest: the other computer
//  shows a six-digit code when it is asked to pair (`spatiand-host --pair`), the person types it
//  here, and the two prove to each other that they know it (crates/spatiand-stream/src/pairing.rs).
//  The code is never sent and this Mac never shows one of its own; nothing is written down here
//  unless both ends proved it.

import AppKit

final class PairingUI {
    private var panel: NSPanel?
    private var waiting: NSTextField?

    func start() {
        let model = Model.shared
        model.onProving = { [weak self] in self?.proving() }
        model.onPaired = { [weak self] host in self?.finish(host) }
        model.onPairFailed = { [weak self] reason in
            self?.close()
            Self.alert("Could not pair", reason)
        }
    }

    /// Ask for an address and the code, and begin.
    func ask() {
        NSApp.activate(ignoringOtherApps: true)
        let alert = NSAlert()
        alert.messageText = "Add a computer"
        alert.informativeText = """
            On that computer, run \u{201C}spatiand-host --pair\u{201D} first: it opens pairing for \
            two minutes and shows a six-digit code. Type its name or address, and that code.
            """
        let field = NSTextField(frame: NSRect(x: 0, y: 30, width: 280, height: 24))
        field.placeholderString = "name or name:port"
        let codeField = NSTextField(frame: NSRect(x: 0, y: 0, width: 280, height: 24))
        codeField.placeholderString = "the code it shows, e.g. 482 913"
        let fields = NSView(frame: NSRect(x: 0, y: 0, width: 280, height: 54))
        fields.addSubview(field)
        fields.addSubview(codeField)
        alert.accessoryView = fields
        alert.addButton(withTitle: "Pair")
        alert.addButton(withTitle: "Cancel")
        alert.window.initialFirstResponder = field
        field.nextKeyView = codeField
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        let address = field.stringValue.trimmingCharacters(in: .whitespaces)
        let code = codeField.stringValue.filter { $0.isASCII && $0.isNumber }
        guard !address.isEmpty else { return }
        guard code.count == 6 else {
            Self.alert("Could not pair", "The code is the six digits \(address) shows.")
            return
        }
        Model.shared.pair(address: address, code: code)
        showWaiting(address)
    }

    private func showWaiting(_ address: String) {
        close()
        let text = NSTextField(wrappingLabelWithString: "Connecting to \(address)\u{2026}")
        text.font = .systemFont(ofSize: 15)
        text.frame = NSRect(x: 20, y: 40, width: 360, height: 90)
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 400, height: 150),
            styleMask: [.titled, .closable, .utilityWindow], backing: .buffered, defer: false)
        panel.title = "Pairing"
        panel.contentView?.addSubview(text)
        panel.center()
        panel.level = .floating
        panel.makeKeyAndOrderFront(nil)
        self.panel = panel
        waiting = text
    }

    private func proving() {
        waiting?.stringValue = "Connected. Proving the code\u{2026}"
    }

    private func finish(_ host: PairedHost) {
        close()
        Hosts.add(host)
        Model.shared.connect(host)
        Self.alert("Paired with \(host.name)", "Both computers proved the code. Its fingerprint begins \(host.fingerprint.prefix(8)).")
    }

    private func close() {
        panel?.close()
        panel = nil
        waiting = nil
    }

    static func alert(_ title: String, _ text: String) {
        NSApp.activate(ignoringOtherApps: true)
        let alert = NSAlert()
        alert.messageText = title
        alert.informativeText = text
        alert.runModal()
    }
}
