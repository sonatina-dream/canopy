import Foundation

/// A point in top-left-origin screen coordinates (y grows downward), the same
/// space the Accessibility API uses.
public struct Point: Codable, Equatable, Hashable, Sendable {
    public var x: Double
    public var y: Double

    public init(x: Double, y: Double) {
        self.x = x
        self.y = y
    }

    public func distance(to other: Point) -> Double {
        ((x - other.x) * (x - other.x) + (y - other.y) * (y - other.y)).squareRoot()
    }
}

public struct Size: Codable, Equatable, Hashable, Sendable {
    public var width: Double
    public var height: Double

    public init(width: Double, height: Double) {
        self.width = width
        self.height = height
    }

    public static let zero = Size(width: 0, height: 0)
}

/// An axis-aligned rectangle in top-left-origin coordinates.
public struct Rect: Codable, Equatable, Hashable, Sendable, CustomStringConvertible {
    public var x: Double
    public var y: Double
    public var width: Double
    public var height: Double

    public init(x: Double, y: Double, width: Double, height: Double) {
        self.x = x
        self.y = y
        self.width = width
        self.height = height
    }

    public static let zero = Rect(x: 0, y: 0, width: 0, height: 0)

    public var minX: Double { x }
    public var minY: Double { y }
    public var maxX: Double { x + width }
    public var maxY: Double { y + height }
    public var midX: Double { x + width / 2 }
    public var midY: Double { y + height / 2 }
    public var center: Point { Point(x: midX, y: midY) }
    public var size: Size { Size(width: width, height: height) }
    public var area: Double { max(0, width) * max(0, height) }
    public var isEmpty: Bool { width <= 0 || height <= 0 }

    public func contains(_ p: Point) -> Bool {
        p.x >= minX && p.x < maxX && p.y >= minY && p.y < maxY
    }

    public func contains(_ r: Rect) -> Bool {
        r.minX >= minX && r.maxX <= maxX && r.minY >= minY && r.maxY <= maxY
    }

    public func intersection(_ r: Rect) -> Rect {
        let x0 = max(minX, r.minX), y0 = max(minY, r.minY)
        let x1 = min(maxX, r.maxX), y1 = min(maxY, r.maxY)
        if x1 <= x0 || y1 <= y0 { return .zero }
        return Rect(x: x0, y: y0, width: x1 - x0, height: y1 - y0)
    }

    public func intersects(_ r: Rect) -> Bool { !intersection(r).isEmpty }

    public func union(_ r: Rect) -> Rect {
        if isEmpty { return r }
        if r.isEmpty { return self }
        let x0 = min(minX, r.minX), y0 = min(minY, r.minY)
        let x1 = max(maxX, r.maxX), y1 = max(maxY, r.maxY)
        return Rect(x: x0, y: y0, width: x1 - x0, height: y1 - y0)
    }

    public func insetBy(_ d: Double) -> Rect {
        Rect(x: x + d, y: y + d, width: max(0, width - 2 * d), height: max(0, height - 2 * d))
    }

    /// The extent along an axis: width for horizontal, height for vertical.
    public func length(along axis: Axis) -> Double {
        axis == .horizontal ? width : height
    }

    public func start(along axis: Axis) -> Double {
        axis == .horizontal ? x : y
    }

    /// Linear interpolation between two rects, `t` in 0...1.
    public static func lerp(_ a: Rect, _ b: Rect, _ t: Double) -> Rect {
        Rect(x: a.x + (b.x - a.x) * t,
             y: a.y + (b.y - a.y) * t,
             width: a.width + (b.width - a.width) * t,
             height: a.height + (b.height - a.height) * t)
    }

    /// The rect moved so it lies inside `bounds` where possible.
    public func clamped(into bounds: Rect) -> Rect {
        var r = self
        r.width = min(r.width, bounds.width)
        r.height = min(r.height, bounds.height)
        r.x = min(max(r.x, bounds.minX), bounds.maxX - r.width)
        r.y = min(max(r.y, bounds.minY), bounds.maxY - r.height)
        return r
    }

    public var description: String {
        "(\(Int(x)), \(Int(y)), \(Int(width))×\(Int(height)))"
    }
}

/// How a split lays out its children: `horizontal` places them left to right,
/// `vertical` top to bottom.
public enum Axis: String, Codable, Sendable, CaseIterable {
    case horizontal
    case vertical

    public var other: Axis { self == .horizontal ? .vertical : .horizontal }
}

/// A side of a rectangle; also used for directions (left/right/up/down).
public enum Edge: String, Codable, Sendable, CaseIterable {
    case left
    case right
    case top
    case bottom

    /// The split axis that docking on this edge creates.
    public var axis: Axis { (self == .left || self == .right) ? .horizontal : .vertical }

    /// Left and top come first in their split.
    public var isLeading: Bool { self == .left || self == .top }

    public var opposite: Edge {
        switch self {
        case .left: return .right
        case .right: return .left
        case .top: return .bottom
        case .bottom: return .top
        }
    }
}

/// Navigation direction for the neighbouring space.
public enum Direction: String, Codable, Sendable, CaseIterable {
    case left, right, up, down

    public var edge: Edge {
        switch self {
        case .left: return .left
        case .right: return .right
        case .up: return .top
        case .down: return .bottom
        }
    }
}

/// Maps rect `from` onto rect `to` (independent x and y scale). Used to place
/// windows that lie outside the framed node during camera motion.
public struct AffineMap: Equatable, Sendable {
    public var sx: Double, sy: Double, tx: Double, ty: Double

    public init(from: Rect, to: Rect) {
        sx = from.width > 0 ? to.width / from.width : 1
        sy = from.height > 0 ? to.height / from.height : 1
        tx = to.x - from.x * sx
        ty = to.y - from.y * sy
    }

    public static let identity = AffineMap(from: Rect(x: 0, y: 0, width: 1, height: 1),
                                           to: Rect(x: 0, y: 0, width: 1, height: 1))

    public func apply(_ r: Rect) -> Rect {
        Rect(x: r.x * sx + tx, y: r.y * sy + ty, width: r.width * sx, height: r.height * sy)
    }

    public func apply(_ p: Point) -> Point {
        Point(x: p.x * sx + tx, y: p.y * sy + ty)
    }
}
