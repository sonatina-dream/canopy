import Foundation

/// What letting go of a dragged window would do.
public enum DropTarget: Equatable, Sendable {
    /// Dock beside everything at a screen edge.
    case screenEdge(Edge)
    /// Dock beside a node on one of its edges.
    case beside(NodeID, Edge)
    /// Join a tile as a tab.
    case tab(NodeID)
    /// Float on the Desktop.
    case float
    /// No change: a floating window stays put, a docked one snaps back.
    case none
}

public struct DropResolver: Sendable {
    /// Outer share of a tile that counts as its edge, at most `maxTileEdge`
    /// points: on a big window most of it means "make a tab".
    public var tileEdgeFraction: Double = 0.22
    public var maxTileEdge: Double = 200
    /// Fraction of the screen's shorter side used as the screen-edge band.
    public var screenEdgeFraction: Double = 0.05
    public var minScreenEdgeBand: Double = 24
    /// Distance from an inner desktop edge that docks beside the desktop.
    public var desktopEdgeBand: Double = 40

    public init() {}

    public func screenEdgeBand(for screen: Rect) -> Double {
        max(minScreenEdgeBand, screenEdgeFraction * min(screen.width, screen.height))
    }

    /// - Parameters:
    ///   - pointer: the cursor, in screen coordinates.
    ///   - screen: the display's visible frame.
    ///   - display: the display's whole frame. The menu bar and the Dock lie
    ///     outside the visible frame; a pointer pushed into them is at that edge.
    ///   - tiles: rects of tiles that may be dropped on (exclude the dragged tile).
    ///   - desktop: the desktop's rect and id, when it is visible.
    ///   - held: the tile id the pointer has rested on in its centre long enough.
    public func resolve(pointer: Point, screen: Rect, display: Rect? = nil, tiles: [(NodeID, Rect)],
                        desktop: (NodeID, Rect)?, held: NodeID?) -> DropTarget {
        guard (display ?? screen).contains(pointer) || screen.contains(pointer) else { return .none }

        let band = screenEdgeBand(for: screen)
        // Negative when the pointer is past the visible frame (menu bar, Dock).
        let distances: [(Edge, Double)] = [
            (.left, pointer.x - screen.minX),
            (.right, screen.maxX - pointer.x),
            (.top, pointer.y - screen.minY),
            (.bottom, screen.maxY - pointer.y),
        ]
        if let nearest = distances.min(by: { $0.1 < $1.1 }), nearest.1 < band {
            return .screenEdge(nearest.0)
        }

        if let (id, r) = tiles.first(where: { $0.1.contains(pointer) }) {
            switch zone(of: pointer, in: r) {
            case .edge(let e): return .beside(id, e)
            case .centre: return held == id ? .tab(id) : .none
            }
        }

        if let (id, r) = desktop, r.contains(pointer) {
            // Inner edges of the desktop border docked tiles; dock beside it there.
            let inner: [(Edge, Double)] = [
                (.left, pointer.x - r.minX),
                (.right, r.maxX - pointer.x),
                (.top, pointer.y - r.minY),
                (.bottom, r.maxY - pointer.y),
            ].filter { edge, _ in !Self.touchesScreen(r, edge: edge, screen: screen, slack: band + 16) }
            if let nearest = inner.min(by: { $0.1 < $1.1 }), nearest.1 < desktopEdgeBand {
                return .beside(id, nearest.0)
            }
            return .float
        }
        return .none
    }

    public enum Zone: Equatable, Sendable { case edge(Edge), centre }

    /// Which part of a tile the pointer is over.
    public func zone(of p: Point, in r: Rect) -> Zone {
        guard r.width > 0, r.height > 0 else { return .centre }
        let bandX = min(tileEdgeFraction * r.width, maxTileEdge)
        let bandY = min(tileEdgeFraction * r.height, maxTileEdge)
        // How far into each edge band the pointer is, 0 at the edge, 1 at its inner side.
        let candidates: [(Edge, Double)] = [
            (.left, (p.x - r.minX) / bandX), (.right, (r.maxX - p.x) / bandX),
            (.top, (p.y - r.minY) / bandY), (.bottom, (r.maxY - p.y) / bandY),
        ].filter { $0.1 < 1 }
        if let nearest = candidates.min(by: { $0.1 < $1.1 }) { return .edge(nearest.0) }
        return .centre
    }

    private static func touchesScreen(_ r: Rect, edge: Edge, screen: Rect, slack: Double) -> Bool {
        switch edge {
        case .left: return r.minX - screen.minX <= slack
        case .right: return screen.maxX - r.maxX <= slack
        case .top: return r.minY - screen.minY <= slack
        case .bottom: return screen.maxY - r.maxY <= slack
        }
    }
}

extension Arrangement {
    /// Applies a drop of `window` (or its whole tile when `wholeTile`).
    /// Returns false when the target means "no change" or "float".
    @discardableResult
    public func apply(_ target: DropTarget, window: WindowID, wholeTile: Bool = false) -> Bool {
        let movingTile = wholeTile ? tile(of: window) : nil
        switch target {
        case .none, .float:
            return false
        case .screenEdge(let edge):
            if let t = movingTile { move(t, beside: root, edge: edge) } else { dockAtScreenEdge(window, edge: edge) }
            return true
        case let .beside(id, edge):
            guard let n = node(id) else { return false }
            if let t = movingTile { move(t, beside: n, edge: edge) } else { dock(window, beside: n, edge: edge) }
            return true
        case .tab(let id):
            guard let n = node(id), n.isTile else { return false }
            if let t = movingTile, t !== n { merge(t, into: n) } else { addTab(window, to: n) }
            return true
        }
    }
}
