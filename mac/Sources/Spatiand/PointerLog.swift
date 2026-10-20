//  PointerLog.swift — every movement of the Mac's pointer while it is in a screen of the room,
//  written to ~/Library/Logs/Spatiand/pointer.log, to find where it jumps.
//
//  One line each: the time in milliseconds, what it was, and its numbers --
//    hand    a movement the tap took: the event's own travel, where it says the cursor is, the
//            travel the room was given, and whether that was read from where the cursor was put
//    put     the Mac's cursor put where the room's pointer is, in a screen
//    raise   the cursor put for scrolling (readyToScroll)
//  Kept to 4 MB: then the old file is pointer.log.old.
//
//  Off unless SPATIAND_POINTER_LOG=1 is in Spatiand's environment
//  (launchctl setenv SPATIAND_POINTER_LOG 1, then Restart Spatiand): the pointer is fixed for now.

import Foundation

final class PointerLog {
    static let shared = PointerLog()
    private let queue = DispatchQueue(label: "spatiand.pointerlog", qos: .utility)
    private var handle: FileHandle?
    private var written = 0
    private let path = NSHomeDirectory() + "/Library/Logs/Spatiand/pointer.log"
    /// Whether the room's pointer is in a screen: lines are written only then.
    var inScreen = false

    private let on = ProcessInfo.processInfo.environment["SPATIAND_POINTER_LOG"] == "1"

    func note(_ line: @autoclosure () -> String) {
        guard on, inScreen else { return }
        let line = line()
        let ms = clock_gettime_nsec_np(CLOCK_UPTIME_RAW) / 1_000_000
        queue.async { [self] in
            if handle == nil || written > 4 << 20 {
                try? handle?.close()
                if written > 0 { try? FileManager.default.removeItem(atPath: path + ".old"); try? FileManager.default.moveItem(atPath: path, toPath: path + ".old") }
                FileManager.default.createFile(atPath: path, contents: nil)
                handle = FileHandle(forWritingAtPath: path)
                written = 0
            }
            let data = Data("\(ms) \(line)\n".utf8)
            handle?.write(data)
            written += data.count
        }
    }
}
