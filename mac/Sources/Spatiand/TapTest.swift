//  TapTest.swift — `Spatiand --selftest-tap`
//
//  Whether an application's sound can be taken from the Mac's speakers: a short system sound is played over and over
//  by a program of its own (`afplay`), which is tapped, and what arrives is measured. The first time, macOS asks
//  whether Spatiand may take other applications' audio.

import AppKit
import AudioToolbox

enum TapTest {
    static func run() {
        guard #available(macOS 14.2, *) else { print("needs macOS 14.2"); exit(2) }
        var failures = 0
        func check(_ name: String, _ ok: Bool) {
            print((ok ? "ok    " : "FAIL  ") + name)
            if !ok { failures += 1 }
        }
        // Something to listen to: the sound repeated, until this test is done. Quiet, and for a short time.
        let player = Process()
        player.executableURL = URL(fileURLWithPath: "/bin/sh")
        player.arguments = ["-c", "for i in 1 2 3 4 5 6 7 8; do afplay -v 0.2 /System/Library/Sounds/Glass.aiff; done"]
        try? player.run()
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) {
            // afplay is a child of the shell, which is a child of this test.
            let tap = MacTap(bundle: "test.afplay", pid: player.processIdentifier)
            var samples = 0
            var peak: Float = 0
            let lock = NSLock()
            tap.onSound = { block, rate in
                lock.lock(); defer { lock.unlock() }
                samples += block.count / 2
                for v in block { peak = max(peak, abs(v)) }
                _ = rate
            }
            check("the player's audio process is found", !tap.processObjects().isEmpty)
            let started = tap.start()
            check("the tap starts (\(tap.lastError))", started)
            DispatchQueue.main.asyncAfter(deadline: .now() + 2.5) {
                lock.lock()
                let (n, p) = (samples, peak)
                lock.unlock()
                print("note  the player was producing sound, as Core Audio sees it: \(tap.isPlaying())")
                check("sound arrives (\(n) frames, peak \(String(format: "%.3f", p)))", n > 20_000 && p > 0.001)
                tap.stop()
                player.terminate()
                print(failures == 0 ? "all passed" : "\(failures) failed")
                exit(failures == 0 ? 0 : 1)
            }
        }
        NSApplication.shared.setActivationPolicy(.accessory)
        NSApplication.shared.run()
    }
}
