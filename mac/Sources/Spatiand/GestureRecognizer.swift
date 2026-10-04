//  GestureRecognizer.swift — three, four and five fingers on the trackpad, as things to do to the room.
//
//  macOS keeps these for itself (Mission Control, Spaces, Launchpad), but it also hands the raw
//  touches to the window that has the keyboard, which is all this needs: where each finger is,
//  from 0 to 1 across the pad. A gesture is the fingers landing together, doing one thing, and
//  lifting; what it was is judged when they lift, from where they began and where they were last
//  all together: moved far enough is a swipe, spread or squeezed enough is a pinch, and neither
//  and brief is a tap.
//
//  Pure, so it can be tried with no trackpad. See `LocalTests`.

import Foundation

struct GestureRecognizer {
    struct Touch { var id: Int; var x: Double; var y: Double }

    enum Direction { case left, right, up, down }

    enum Event: Equatable {
        case swipe(Direction, fingers: Int)
        /// `spreading` is the fingers moving apart, otherwise together.
        case pinch(fingers: Int, spreading: Bool)
        case tap(fingers: Int)
    }

    /// How far, across the pad, fingers must travel to be swiping.
    static let swipeDistance = 0.12
    /// How much the fingers' spread must change to be pinching: a quarter.
    static let pinchRatio = 0.25
    /// How long fingers must be down together before it counts as a gesture, and the longest a tap lasts.
    static let settle = 0.05
    static let tapTime = 0.3

    private(set) var active = false
    private var firstSeen: Double?
    private var engagedAt = 0.0
    private var peak = 0
    private var start = (x: 0.0, y: 0.0, spread: 0.0)
    private var last = (x: 0.0, y: 0.0, spread: 0.0)

    private static func measure(_ touches: [Touch]) -> (x: Double, y: Double, spread: Double) {
        let n = Double(touches.count)
        let x = touches.reduce(0) { $0 + $1.x } / n
        let y = touches.reduce(0) { $0 + $1.y } / n
        let spread = touches.reduce(0) { $0 + hypot($1.x - x, $1.y - y) } / n
        return (x, y, spread)
    }

    /// The fingers as they are now, at time `time` seconds. Returns what a gesture turned out to be, once it ends.
    mutating func update(_ touches: [Touch], at time: Double) -> Event? {
        let n = touches.count
        if !active {
            guard n >= 3 else {
                firstSeen = nil
                return nil
            }
            if firstSeen == nil { firstSeen = time }
            guard time - (firstSeen ?? time) >= Self.settle else { return nil }
            active = true
            engagedAt = firstSeen ?? time
            peak = n
            start = Self.measure(touches)
            last = start
            return nil
        }
        if n >= 3 {
            if n > peak {
                // More fingers have landed: the gesture is theirs, measured from here.
                peak = n
                start = Self.measure(touches)
                last = start
            } else if n == peak {
                last = Self.measure(touches)
            }
            return nil
        }
        // The fingers are lifting: judge it by the last time they were all down together.
        defer {
            active = false
            firstSeen = nil
            peak = 0
        }
        let dx = last.x - start.x, dy = last.y - start.y
        let ratio = start.spread > 0.001 ? last.spread / start.spread : 1
        let moved = max(abs(dx), abs(dy))
        // Three fingers pinching is not anything anywhere; four and five are.
        if peak >= 4, abs(ratio - 1) > Self.pinchRatio, moved < Self.swipeDistance {
            return .pinch(fingers: peak, spreading: ratio > 1)
        }
        if moved >= Self.swipeDistance {
            if abs(dx) >= abs(dy) { return .swipe(dx > 0 ? .right : .left, fingers: peak) }
            return .swipe(dy > 0 ? .up : .down, fingers: peak)
        }
        if time - engagedAt < Self.tapTime { return .tap(fingers: peak) }
        return nil
    }
}
