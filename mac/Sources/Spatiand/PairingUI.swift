//  PairingUI.swift — adding a computer.
//
//  Pairing is the one step that decides who is trusted, so it is kept honest: this Mac shows a
//  code, the person at the other computer is asked whether theirs shows the same, and nothing is
//  written down here until that computer has said yes *and* this one has been asked again.

import AppKit

final class PairingUI {
    private var panel: NSPanel?
    private var waiting: NSTextField?

    func start() {
        let model = Model.shared
        model.onCompare = { [weak self] code, fingerprint in self?.show(code: code, fingerprint: fingerprint) }
        model.onPaired = { [weak self] host in self?.finish(host) }
        model.onPairFailed = { [weak self] reason in
            self?.close()
            Self.alert("Could not pair", reason)
        }
    }

    /// Ask for an address, and begin.
    func ask() {
        NSApp.activate(ignoringOtherApps: true)
        let alert = NSAlert()
        alert.messageText = "Add a computer"
        alert.informativeText = """
            Type its name or address. On that computer, run \u{201C}spatiand-host --pair\u{201D} first: \
            it opens pairing for two minutes.
            """
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 280, height: 24))
        field.placeholderString = "name or name:port"
        alert.accessoryView = field
        alert.addButton(withTitle: "Pair")
        alert.addButton(withTitle: "Cancel")
        alert.window.initialFirstResponder = field
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        let address = field.stringValue.trimmingCharacters(in: .whitespaces)
        guard !address.isEmpty else { return }
        Model.shared.pair(address: address)
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

    private func show(code: String, fingerprint: String) {
        waiting?.stringValue = """
            The code is  \(code)

            On the other computer, check that it shows the same code and answer yes there. \
            This waits for it.
            """
        waiting?.font = .systemFont(ofSize: 15)
    }

    private func finish(_ host: PairedHost) {
        close()
        NSApp.activate(ignoringOtherApps: true)
        let alert = NSAlert()
        alert.messageText = "Trust \(host.name)?"
        alert.informativeText = "It said the codes match. Its fingerprint begins \(host.fingerprint.prefix(8))."
        alert.addButton(withTitle: "Trust")
        alert.addButton(withTitle: "Don\u{2019}t")
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        Hosts.add(host)
        Model.shared.connect(host)
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
