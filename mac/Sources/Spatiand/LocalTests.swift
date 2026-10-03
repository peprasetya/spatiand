//  LocalTests.swift — `Spatiand --selftest-local`: the parts that need no host.
//
//  Folding surround to stereo, and the clipboard's two directions. The clipboard test borrows the
//  real pasteboard for a moment and puts back what was on it.

import AppKit

enum LocalTests {
    static func run() -> Int32 {
        var failures = 0
        func check(_ name: String, _ ok: Bool) {
            print((ok ? "ok    " : "FAIL  ") + name)
            if !ok { failures += 1 }
        }

        // --- folding to stereo
        func pcm(_ values: [Int16]) -> Data { values.withUnsafeBytes { Data($0) } }
        let stereo = Downmix.stereo(pcm([16384, -16384, 8192, 0]), channels: 2)
        check("stereo passes through untouched", stereo == [0.5, -0.5, 0.25, 0])
        // 7.1.4, one frame, a signal only in the front-right channel: it stays on the right.
        var frame = [Int16](repeating: 0, count: 12); frame[1] = 20000
        let fr = Downmix.stereo(pcm(frame), channels: 12)
        check("front right stays right", fr[0] == 0 && fr[1] > 0.1)
        // Only the rear-left (index 4 in wire order): left.
        frame = [Int16](repeating: 0, count: 12); frame[4] = 20000
        let rl = Downmix.stereo(pcm(frame), channels: 12)
        check("rear left lands left", rl[0] > 0.05 && rl[1] == 0)
        // The LFE is dropped, as a stereo fold always has.
        frame = [Int16](repeating: 0, count: 6); frame[3] = 30000
        let lfe = Downmix.stereo(pcm(frame), channels: 6)
        check("the LFE is not folded in", lfe[0] == 0 && lfe[1] == 0)
        // Full-scale on every channel cannot clip once folded.
        frame = [Int16](repeating: 32767, count: 12)
        let loud = Downmix.stereo(pcm(frame), channels: 12)
        check("everything at full scale stays under clipping", loud.allSatisfy { abs($0) <= 1.0 })

        // --- clipboard
        let board = NSPasteboard.general
        let before = board.string(forType: .string)
        let sync = ClipboardSync()
        var said: [[String: Any]] = []
        sync.send = { said.append($0) }
        sync.connected = true
        sync.fromHost(["Offer": ["mime_types": ["text/plain;charset=utf-8"], "text": "from the host", "bytes": 13]])
        check("a host's text lands on the pasteboard", board.string(forType: .string) == "from the host")
        sync.fromHost(["Offer": ["mime_types": ["image/png"], "text": NSNull(), "bytes": 100]])
        check("a small image is asked for", (said.last?["Clipboard"] as? [String: Any])?["Want"] != nil)
        sync.fromHost(["Offer": ["mime_types": ["image/png"], "text": NSNull(), "bytes": 100_000_000]])
        check("a huge image is left alone", said.count == 1)
        // The Mac copies something: the poll notices and offers it, and a paste there is answered.
        sync.start()
        board.clearContents()
        board.setString("from the mac", forType: .string)
        RunLoop.main.run(until: Date().addingTimeInterval(1.0))
        let offer = ((said.last?["Clipboard"] as? [String: Any])?["Offer"] as? [String: Any])
        check("the Mac's text is offered", (offer?["text"] as? String) == "from the mac")
        sync.fromHost(["Want": ["mime_type": "UTF8_STRING"]])
        let data = ((said.last?["Clipboard"] as? [String: Any])?["Data"] as? [String: Any])
        check("a paste is answered with the text",
              (data?["bytes"] as? String).flatMap { Data(base64Encoded: $0) }.flatMap { String(data: $0, encoding: .utf8) } == "from the mac")

        // --- hot keys: whether the chords can be had here is a fact about this Mac, not a pass/fail
        let keys = Hotkeys()
        keys.enable()
        print("note  Ctrl-Space registered: \(keys.status[.menu] == true), Ctrl-Tab registered: \(keys.status[.settings] == true)")
        keys.disable()

        board.clearContents()
        if let before { board.setString(before, forType: .string) }
        print(failures == 0 ? "all passed" : "\(failures) failed")
        return failures == 0 ? 0 : 1
    }
}
