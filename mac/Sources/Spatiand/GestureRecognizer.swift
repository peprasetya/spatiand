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
    static let swipeDistance = 0.06
    /// How much the fingers' spread must change to be pinching: a quarter.
    static let pinchRatio = 0.15
    /// How long fingers must be down together before it counts as a gesture, and the longest a tap lasts.
    static let settle = 0.05
    static let tapTime = 0.3

    private(set) var active = false
    private var firstSeen: Double?
    private var engagedAt = 0.0
    private var peak = 0
    private var start = (x: 0.0, y: 0.0, spread: 0.0)
    private var last = (x: 0.0, y: 0.0, spread: 0.0)
    /// The fingers the gesture began with, and how the ones still down have moved since: a squeeze
    /// makes the pad lose fingers as they merge, so the gesture is followed by the fingers that
    /// stay, and the biggest squeeze or spread they made is what counts.
    private var startTouches: [Touch] = []
    private var travel = (x: 0.0, y: 0.0)
    private var bestRatio = 1.0

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
            startTouches = touches
            travel = (0, 0)
            bestRatio = 1
            return nil
        }
        if n >= 3 {
            if n > peak {
                // More fingers have landed: the gesture is theirs, measured from here.
                peak = n
                start = Self.measure(touches)
                last = start
                startTouches = touches
                travel = (0, 0)
                bestRatio = 1
            } else {
                let ids = Set(touches.map(\.id))
                let then = startTouches.filter { ids.contains($0.id) }
                if then.count >= 3, then.count == touches.filter({ t in startTouches.contains { $0.id == t.id } }).count {
                    let now = touches.filter { t in then.contains { $0.id == t.id } }
                    let a = Self.measure(then), b = Self.measure(now)
                    travel = (b.x - a.x, b.y - a.y)
                    let ratio = a.spread > 0.001 ? b.spread / a.spread : 1
                    if abs(ratio - 1) > abs(bestRatio - 1) { bestRatio = ratio }
                    last = b
                }
            }
            return nil
        }
        // The fingers are lifting: judge it by the last time they were all down together.
        defer {
            active = false
            firstSeen = nil
            peak = 0
        }
        let dx = travel.x, dy = travel.y
        let ratio = bestRatio
        let moved = max(abs(dx), abs(dy))
        // Three fingers pinching is not anything anywhere; four and five are.
        // (The thumb of a five-finger pinch moves the middle of the fingers too: it is a pinch
        // while the fingers' closing is more than the middle's travel.)
        if peak >= 4, abs(ratio - 1) > Self.pinchRatio, moved < Self.swipeDistance || abs(ratio - 1) * start.spread > moved * 0.6 {
            return .pinch(fingers: peak, spreading: ratio > 1)
        }
        if moved >= Self.swipeDistance {
            if abs(dx) >= abs(dy) { return .swipe(dx > 0 ? .right : .left, fingers: peak) }
            return .swipe(dy > 0 ? .up : .down, fingers: peak)
        }
        if moved < 0.04, time - engagedAt < Self.tapTime { return .tap(fingers: peak) }
        return nil
    }
}
