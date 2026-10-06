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

        // --- trackpad gestures
        func fingers(_ n: Int, centre: (Double, Double), spread: Double) -> [GestureRecognizer.Touch] {
            (0..<n).map { i in
                let a = Double(i) / Double(n) * 2 * .pi
                return .init(id: i, x: centre.0 + cos(a) * spread, y: centre.1 + sin(a) * spread)
            }
        }
        func run(_ frames: [[GestureRecognizer.Touch]]) -> GestureRecognizer.Event? {
            var g = GestureRecognizer()
            var t = 0.0
            var result: GestureRecognizer.Event?
            for frame in frames {
                if let e = g.update(frame, at: t) { result = e }
                t += 0.03
            }
            return result
        }
        let lift: [GestureRecognizer.Touch] = []
        let swipeRight3 = (0...12).map { fingers(3, centre: (0.3 + Double($0) * 0.03, 0.5), spread: 0.08) } + [lift]
        check("three fingers swiped right is a swipe right", run(swipeRight3) == .swipe(.right, fingers: 3))
        let swipeUp4 = (0...12).map { fingers(4, centre: (0.5, 0.3 + Double($0) * 0.03), spread: 0.08) } + [lift]
        check("four fingers swiped up is a swipe up", run(swipeUp4) == .swipe(.up, fingers: 4))
        let swipeLeft5 = (0...12).map { fingers(5, centre: (0.7 - Double($0) * 0.03, 0.5), spread: 0.1) } + [lift]
        check("five fingers swiped left is a swipe left", run(swipeLeft5) == .swipe(.left, fingers: 5))
        let pinch4 = (0...10).map { fingers(4, centre: (0.5, 0.5), spread: 0.16 - Double($0) * 0.01) } + [lift]
        check("four fingers drawn together is a pinch", run(pinch4) == .pinch(fingers: 4, spreading: false))
        let spread5 = (0...10).map { fingers(5, centre: (0.5, 0.5), spread: 0.08 + Double($0) * 0.01) } + [lift]
        check("five fingers spread is a spread", run(spread5) == .pinch(fingers: 5, spreading: true))
        let tap3 = (0...4).map { _ in fingers(3, centre: (0.5, 0.5), spread: 0.08) } + [lift]
        check("three fingers touched and lifted is a tap", run(tap3) == .tap(fingers: 3))
        let resting3 = (0...20).map { _ in fingers(3, centre: (0.5, 0.5), spread: 0.08) } + [lift]
        check("three fingers resting a while is nothing", run(resting3) == nil)
        let two = (0...12).map { fingers(2, centre: (0.3 + Double($0) * 0.03, 0.5), spread: 0.08) } + [lift]
        check("two fingers are not for this", run(two) == nil)
        let pinch3 = (0...10).map { fingers(3, centre: (0.5, 0.5), spread: 0.16 - Double($0) * 0.01) } + [lift]
        check("three fingers pinching is not a gesture", run(pinch3) == nil)
        // Fingers landing one after another, the fifth last, are judged as five.
        let staggered = (0...3).map { _ in fingers(3, centre: (0.5, 0.5), spread: 0.1) }
            + (0...3).map { _ in fingers(4, centre: (0.5, 0.5), spread: 0.1) }
            + (0...10).map { fingers(5, centre: (0.5, 0.5), spread: 0.1 + Double($0) * 0.01) } + [lift]
        check("fingers landing one by one are judged as the most that were down", run(staggered) == .pinch(fingers: 5, spreading: true))

        // --- clipboard
        let board = NSPasteboard.general
        let before = board.string(forType: .string)
        let sync = ClipboardSync()
        var said: [[String: Any]] = []
        sync.send = { said.append($0) }
        sync.connected = true
        said.removeAll()   // connecting offers what is already on the pasteboard
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

        // --- the applications of this Mac
        let apps = RoomController.installedApplications()
        check("the installed applications are found (\(apps.count))", apps.count > 10 && apps.contains { $0.id == "com.apple.finder" || $0.id == "com.apple.TextEdit" })
        check("an application that runs in the background is not offered", !apps.contains { $0.id == "com.apple.dock" })
        print("note  e.g. " + apps.map(\.name).sorted().prefix(8).joined(separator: ", "))

        let tap = KeyTap()
        print("note  a key tap for the menu can be made: \(tap.start()) (accessibility \(AXIsProcessTrusted()), input monitoring \(CGPreflightListenEventAccess()))")
        tap.stop()

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
