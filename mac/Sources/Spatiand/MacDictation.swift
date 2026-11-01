//
//  MacDictation.swift
//  Dictation with no glasses on: a switch in the menu bar, and while it is on the microphone is
//  open and what is said is typed into the Mac's front window. The work is the room's own
//  (`dictate::on_a_desktop`, docs/ai-integrations.md); what is here is the switch, what is in
//  front, the typing, and the card a terminal's words wait on.
//
//  In the glasses this gives way to the keyboard's mic key: the room holds the Mac's keyboard
//  then, and its windows are what is being spoken to.
//

import AppKit
import AVFoundation
import CSpatiand

final class MacDictation {
    static let shared = MacDictation()

    /// Called on the main thread as listening starts and stops.
    var onListening: ((Bool) -> Void)?

    /// The switch, kept across launches.
    var enabled: Bool {
        get { UserDefaults.standard.bool(forKey: "dictation") }
        set {
            UserDefaults.standard.set(newValue, forKey: "dictation")
            if newValue {
                allowThenApply()
            } else {
                apply()
            }
        }
    }

    private(set) var listening = false
    private var timer: Timer?
    private var sinceFocus = 0
    private let typing = DispatchQueue(label: "spatiand.dictation.typing", qos: .userInitiated)
    private let card = DictationCard()

    /// Begin following the switch, at launch.
    func begin() {
        NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main
        ) { [weak self] _ in
            if self?.listening == true { self?.tellFocus() }
        }
        timer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in self?.tick() }
        if enabled { allowThenApply() }
    }

    /// Asked where its questions can be seen: the switch is pressed here, not in the room.
    private func allowThenApply() {
        _ = RoomTap.allowed(ask: true)
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized:
            sp_microphone_allowed(true)
            apply()
        case .notDetermined:
            AVCaptureDevice.requestAccess(for: .audio) { granted in
                DispatchQueue.main.async {
                    sp_microphone_allowed(granted)
                    if !granted {
                        self.enabled = false
                        self.card.show("macOS has not allowed Spatiand the microphone.")
                    }
                    self.apply()
                }
            }
        default:
            enabled = false
            card.show("Allow Spatiand the microphone in System Settings \u{2192} Privacy & Security, then turn dictation on.")
        }
    }

    private func apply() {
        let wanted = enabled && !Room.shared.inGlasses
            && AVCaptureDevice.authorizationStatus(for: .audio) == .authorized
        guard wanted != listening else { return }
        listening = wanted
        if wanted { tellFocus() }
        sp_dictation_listen(wanted)
        print("dictation: \(wanted ? "listening" : "not listening")")
        onListening?(wanted)
    }

    private func tick() {
        apply()
        drain()
        guard listening else { return }
        // What is in the window now, for the next phrase's sake.
        sinceFocus += 1
        if sinceFocus >= 30 { tellFocus() }
    }

    private func tellFocus() {
        sinceFocus = 0
        guard let front = NSWorkspace.shared.frontmostApplication, let bundle = front.bundleIdentifier else { return }
        let pid = front.processIdentifier
        MacDescribe.describe(bundle: bundle) { json in
            json.withCString { sp_dictation_focus(pid, $0) }
        }
    }

    private func drain() {
        var kind: Int32 = 0
        while let words = sp_dictation_next(&kind) {
            let said = String(cString: words)
            sp_dictation_free(words)
            switch kind {
            case 1: type(said)
            case 2: card.show(said, heading: "Type into the terminal?") { [weak self] in self?.type(said) }
            case 4:
                enabled = false
                card.show(said)
            default: card.show(said)
            }
        }
    }

    private func type(_ words: String) {
        guard RoomTap.allowed(ask: false) else {
            card.show("Allow Spatiand Accessibility, to type what was said.")
            return
        }
        typing.async {
            guard let source = CGEventSource(stateID: .hidSystemState) else { return }
            SpatiKeyTyping.post(text: words, source: source)
        }
    }
}

/// A small card at the top right of the screen, which takes no focus from what is being typed in.
private final class DictationCard {
    private var panel: NSPanel?
    private var action: (() -> Void)?
    private var closing: DispatchWorkItem?

    /// With an action it stays until it is answered; without, it goes by itself.
    func show(_ words: String, heading: String = "Dictation", action: (() -> Void)? = nil) {
        self.panel?.orderOut(nil)
        self.action = action
        closing?.cancel()
        let width: CGFloat = 380
        let panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: width, height: 100),
                            styleMask: [.nonactivatingPanel, .titled, .fullSizeContentView], backing: .buffered, defer: false)
        panel.level = .floating
        panel.isReleasedWhenClosed = false
        panel.titleVisibility = .hidden
        panel.titlebarAppearsTransparent = true
        panel.hidesOnDeactivate = false
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]

        let title = NSTextField(labelWithString: heading)
        title.font = .boldSystemFont(ofSize: 13)
        let body = NSTextField(wrappingLabelWithString: words)
        body.preferredMaxLayoutWidth = width - 32
        let stack = NSStackView(views: [title, body])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 6
        stack.edgeInsets = NSEdgeInsets(top: 24, left: 16, bottom: 14, right: 16)
        if action != nil {
            let type = NSButton(title: "Type", target: self, action: #selector(answerType))
            type.keyEquivalent = ""
            let discard = NSButton(title: "Discard", target: self, action: #selector(answerDiscard))
            let buttons = NSStackView(views: [type, discard])
            stack.addArrangedSubview(buttons)
        }
        panel.contentView = stack
        stack.layoutSubtreeIfNeeded()
        let size = stack.fittingSize
        let screen = NSScreen.main?.visibleFrame ?? .zero
        panel.setFrame(NSRect(x: screen.maxX - width - 12, y: screen.maxY - size.height - 12, width: width, height: size.height), display: true)
        panel.orderFrontRegardless()
        self.panel = panel
        if action == nil {
            let later = DispatchWorkItem { [weak self] in self?.dismiss() }
            closing = later
            DispatchQueue.main.asyncAfter(deadline: .now() + 8, execute: later)
        }
    }

    private func dismiss() {
        panel?.orderOut(nil)
        panel = nil
        action = nil
    }

    @objc private func answerType() {
        let action = self.action
        dismiss()
        action?()
    }

    @objc private func answerDiscard() {
        dismiss()
    }
}
