import Foundation

/// A place the camera has been. Stores the node id plus the windows inside it,
/// so it still resolves after the tree changes shape.
public struct Visit: Codable, Equatable, Sendable {
    public var node: NodeID
    public var windows: [WindowID]
    public var includesDesktop: Bool
    /// "Everything": always resolves to the current root.
    public var isRoot: Bool
    /// A run of neighbouring children of `node` (a split), framed together
    /// without the rest of the split. Nil frames all of `node`.
    public var range: [NodeID]? = nil

    public static let root = Visit(node: 0, windows: [], includesDesktop: true, isRoot: true)
}

/// The camera of one display: what is framed, back/forward history, saved views.
public final class Navigator: Codable {
    public private(set) var framed: Visit = .root
    public private(set) var backStack: [Visit] = []
    public private(set) var forwardStack: [Visit] = []
    public private(set) var savedViews: [Int: Visit] = [:]
    public static let historyLimit = 64
    public static let savedViewSlots = 1...9

    public init() {}

    /// An independent copy, for previewing what a change would frame.
    public func copy() -> Navigator {
        let n = Navigator()
        n.framed = framed
        n.backStack = backStack
        n.forwardStack = forwardStack
        n.savedViews = savedViews
        return n
    }

    // MARK: Resolution

    public func visit(for node: Node, in a: Arrangement) -> Visit {
        if node === a.root { return .root }
        return Visit(node: node.id, windows: node.allWindows,
                     includesDesktop: node.desktopNode != nil, isRoot: false)
    }

    public func visit(forRange nodes: [Node], in a: Arrangement) -> Visit {
        guard nodes.count > 1, let p = nodes.first?.parent, nodes.count < p.children.count else {
            return visit(for: nodes.first ?? a.root, in: a)
        }
        return Visit(node: p.id, windows: nodes.flatMap(\.allWindows),
                     includesDesktop: nodes.contains { $0.desktopNode != nil }, isRoot: false, range: nodes.map(\.id))
    }

    /// The children a range visit frames, in order, if they are still
    /// neighbouring children of one split (and not all of them).
    public func resolveRange(_ v: Visit, in a: Arrangement) -> [Node]? {
        guard let ids = v.range, !v.isRoot else { return nil }
        let nodes = ids.compactMap { a.node($0) }
        guard nodes.count == ids.count, nodes.count > 1, let p = nodes.first?.parent,
              nodes.allSatisfy({ $0.parent === p }) else { return nil }
        let idx = nodes.compactMap(\.indexInParent).sorted()
        guard idx.count == nodes.count, idx.last! - idx.first! == idx.count - 1, idx.count < p.children.count else { return nil }
        return idx.map { p.children[$0] }
    }

    /// The framed run of children, when a range is framed.
    public func framedRange(in a: Arrangement) -> [Node]? { resolveRange(framed, in: a) }

    private func currentVisit(in a: Arrangement) -> Visit {
        if let r = framedRange(in: a) { return visit(forRange: r, in: a) }
        return visit(for: framedNode(in: a), in: a)
    }

    private func same(_ v: Visit, _ w: Visit, in a: Arrangement) -> Bool {
        let r1 = resolveRange(v, in: a)?.map(\.id), r2 = resolveRange(w, in: a)?.map(\.id)
        if r1 != nil || r2 != nil { return r1 == r2 }
        return resolve(v, in: a) === resolve(w, in: a)
    }

    public func resolve(_ v: Visit, in a: Arrangement) -> Node {
        if v.isRoot { return a.root }
        if let n = a.node(v.node) { return n }
        var nodes = v.windows.compactMap { a.tile(of: $0) }
        if v.includesDesktop { nodes.append(a.desktop) }
        return Node.lowestCommonAncestor(nodes) ?? a.root
    }

    public func framedNode(in a: Arrangement) -> Node { resolve(framed, in: a) }

    public var canGoBack: Bool { !backStack.isEmpty }
    public var canGoForward: Bool { !forwardStack.isEmpty }

    // MARK: Moving the camera

    /// Frames `node`, pushing history. Returns false when it was already framed.
    @discardableResult
    public func frame(_ node: Node, in a: Arrangement) -> Bool {
        push(visit(for: node, in: a), in: a)
    }

    /// Frames a run of neighbouring children of one split.
    @discardableResult
    public func frame(range nodes: [Node], in a: Arrangement) -> Bool {
        push(visit(forRange: nodes, in: a), in: a)
    }

    private func push(_ v: Visit, in a: Arrangement) -> Bool {
        if same(v, framed, in: a) { return false }
        backStack.append(currentVisit(in: a))
        if backStack.count > Self.historyLimit { backStack.removeFirst(backStack.count - Self.historyLimit) }
        forwardStack.removeAll()
        framed = v
        return true
    }

    /// Frames `node`, or goes back if it is already framed ("again to go back").
    @discardableResult
    public func toggleFrame(_ node: Node, in a: Arrangement) -> Bool {
        if framedRange(in: a) == nil, framedNode(in: a) === node { return back(in: a) }
        return frame(node, in: a)
    }

    @discardableResult
    public func everything(in a: Arrangement) -> Bool { frame(a.root, in: a) }

    @discardableResult
    public func stepOut(in a: Arrangement) -> Bool {
        // Out of a range: the whole split it is part of.
        if framedRange(in: a) != nil { return frame(framedNode(in: a), in: a) }
        guard let p = framedNode(in: a).parent else { return false }
        return frame(p, in: a)
    }

    /// Steps into the child of the framed split toward the focused window, or
    /// toward the pointer, or the first child.
    @discardableResult
    public func stepIn(in a: Arrangement, focused: WindowID?, pointer: Point?, rects: [NodeID: Rect],
                       preferPointer: Bool = false) -> Bool {
        let f = framedNode(in: a)
        guard f.isSplit else { return false }
        let visible = (framedRange(in: a) ?? f.children).filter { !($0.isDesktop && $0.hidden) }
        let byFocus: Node? = focused.flatMap { a.tile(of: $0) }.flatMap { t in visible.first { $0.contains(t) } }
        let byPointer: Node? = pointer.flatMap { p in visible.first { rects[$0.id]?.contains(p) == true } }
        let child = (preferPointer ? (byPointer ?? byFocus) : (byFocus ?? byPointer)) ?? visible.first
        guard let child else { return false }
        return frame(child, in: a)
    }

    /// Frames the neighbouring space in `direction`, at the same depth where possible.
    @discardableResult
    public func neighbour(_ direction: Direction, in a: Arrangement, rects: [NodeID: Rect]) -> Bool {
        guard let target = neighbourNode(direction, in: a, rects: rects) else { return false }
        return frame(target, in: a)
    }

    public func neighbourNode(_ direction: Direction, in a: Arrangement, rects: [NodeID: Rect]) -> Node? {
        let edge = direction.edge
        // From a range, look past its child on that side.
        let f = framedRange(in: a).map { edge.isLeading ? $0.first! : $0.last! } ?? framedNode(in: a)
        let reference = rects[f.id]?.center
        var c = f
        while let p = c.parent {
            if p.axis == edge.axis, var j = c.indexInParent {
                let step = edge.isLeading ? -1 : 1
                j += step
                while p.children.indices.contains(j), p.children[j].isDesktop, p.children[j].hidden { j += step }
                if p.children.indices.contains(j) {
                    return descend(p.children[j], toDepth: f.depth, edge: edge, reference: reference, rects: rects)
                }
            }
            c = p
        }
        return nil
    }

    private func descend(_ start: Node, toDepth depth: Int, edge: Edge, reference: Point?, rects: [NodeID: Rect]) -> Node {
        var s = start
        while s.depth < depth, s.isSplit {
            let visible = s.children.filter { !($0.isDesktop && $0.hidden) }
            guard let first = visible.first, let last = visible.last else { break }
            if s.axis == edge.axis {
                // Moving left enters from the right side, and so on.
                s = edge.isLeading ? last : first
            } else if let reference {
                s = visible.min { lhs, rhs in
                    crossDistance(rects[lhs.id], reference, axis: s.axis) < crossDistance(rects[rhs.id], reference, axis: s.axis)
                } ?? first
            } else {
                s = first
            }
        }
        return s
    }

    private func crossDistance(_ r: Rect?, _ p: Point, axis: Axis) -> Double {
        guard let r else { return .infinity }
        return axis == .horizontal ? abs(r.midX - p.x) : abs(r.midY - p.y)
    }

    @discardableResult
    public func back(in a: Arrangement) -> Bool {
        guard let v = backStack.popLast() else { return false }
        forwardStack.append(currentVisit(in: a))
        framed = v
        return true
    }

    @discardableResult
    public func forward(in a: Arrangement) -> Bool {
        guard let v = forwardStack.popLast() else { return false }
        backStack.append(currentVisit(in: a))
        framed = v
        return true
    }

    // MARK: Saved views

    public func save(_ slot: Int, in a: Arrangement) {
        guard Self.savedViewSlots.contains(slot) else { return }
        savedViews[slot] = currentVisit(in: a)
    }

    @discardableResult
    public func go(to slot: Int, in a: Arrangement) -> Bool {
        guard let v = savedViews[slot] else { return false }
        if let r = resolveRange(v, in: a) { return frame(range: r, in: a) }
        return frame(resolve(v, in: a), in: a)
    }

    public func forget(_ slot: Int) { savedViews[slot] = nil }

    // MARK: Following the layout

    /// Widens the view just enough to include `node`, without touching history.
    /// Used after a dock so the new window never lands out of sight.
    @discardableResult
    public func reveal(_ node: Node, in a: Arrangement) -> Bool {
        let f = framedNode(in: a)
        if let r = framedRange(in: a) {
            if r.contains(where: { $0.contains(node) }) { return false }
            framed = visit(for: Node.lowestCommonAncestor([f, node]) ?? a.root, in: a)
            return true
        }
        if f.contains(node) { return false }
        framed = visit(for: Node.lowestCommonAncestor([f, node]) ?? a.root, in: a)
        return true
    }

    /// Brings `tile` into view at a size where it `fits`: from the common
    /// ancestor of the current view and the tile, step down toward the tile.
    @discardableResult
    public func bringIntoView(_ tile: Node, in a: Arrangement, fits: (_ framing: Node) -> Bool) -> Bool {
        let f = framedNode(in: a)
        if let r = framedRange(in: a), r.contains(where: { $0.contains(tile) }) { return false }
        if framedRange(in: a) == nil, f.contains(tile), fits(f) { return false }
        var n = Node.lowestCommonAncestor([f, tile]) ?? a.root
        while !fits(n), n !== tile, let next = n.children.first(where: { $0.contains(tile) }) {
            n = next
        }
        return frame(n, in: a)
    }

    /// Frames what a rectangle drawn on screen covers: the tiles (and the
    /// desktop) it covers a quarter of, that hold a fifth of it, or that hold
    /// its centre. When those
    /// are some of a split's children, just that run of children is framed.
    @discardableResult
    public func frame(rect: Rect, in a: Arrangement, rects: [NodeID: Rect]) -> Bool {
        let f = framedNode(in: a)
        var hits: [Node] = []
        for n in f.allNodes where n.isTile || (n.isDesktop && !n.hidden) {
            guard let r = rects[n.id], !r.isEmpty else { continue }
            let overlap = r.intersection(rect).area
            if overlap >= 0.25 * r.area || overlap >= 0.2 * rect.area || r.contains(rect.center) {
                hits.append(n)
            }
        }
        guard let target = Node.lowestCommonAncestor(hits) else { return false }
        if target.isSplit {
            let idx = Set(hits.compactMap { h in target.children.firstIndex { $0.contains(h) } })
            if let lo = idx.min(), let hi = idx.max(), hi > lo, hi - lo + 1 < target.children.count {
                return frame(range: Array(target.children[lo...hi]), in: a)
            }
        }
        return frame(target, in: a)
    }

    /// Where the camera settles after a pinch that scaled the view by `scale`
    /// (above 1 zooms in) around `pointer`. Candidates are the nodes under the
    /// pointer from the root down, and the current view's ancestors; the one
    /// whose own zoom to fill `screen` is nearest the pinch wins. `rects` are
    /// every node's rect under the current camera (`Plan.projected`).
    public func settleTarget(scale: Double, pointer: Point, in a: Arrangement, rects: [NodeID: Rect], screen: Rect) -> Node {
        let f = framedNode(in: a)
        guard scale > 0, abs(log(scale)) > 0.12 else { return f }
        var candidates: [Node] = [f] + f.ancestors
        var n: Node? = a.root
        while let node = n {
            candidates.append(node)
            n = node.children.first { !($0.isDesktop && $0.hidden) && rects[$0.id]?.contains(pointer) == true }
        }
        func zoom(_ node: Node) -> Double? {
            guard let r = rects[node.id], r.width > 1, r.height > 1 else { return nil }
            // How much the node grows to fill the screen, by area.
            return ((screen.width / r.width) * (screen.height / r.height)).squareRoot()
        }
        let target = log(scale)
        let best = candidates.compactMap { c in zoom(c).map { (c, abs(log($0) - target)) } }
            .min { $0.1 < $1.1 }
        return best?.0 ?? f
    }

    /// Drops saved views and history entries whose windows are all gone.
    public func prune(liveWindows: Set<WindowID>) {
        func alive(_ v: Visit) -> Bool { v.isRoot || v.includesDesktop || v.windows.contains(where: liveWindows.contains) }
        backStack.removeAll { !alive($0) }
        forwardStack.removeAll { !alive($0) }
        savedViews = savedViews.filter { alive($0.value) }
        if !alive(framed) { framed = .root }
    }

    /// Replaces window ids, matching `Arrangement.remap`.
    public func remap(_ mapping: [WindowID: WindowID]) {
        func map(_ v: Visit) -> Visit {
            var v = v
            v.windows = v.windows.map { mapping[$0] ?? $0 }
            return v
        }
        framed = map(framed)
        backStack = backStack.map(map)
        forwardStack = forwardStack.map(map)
        savedViews = savedViews.mapValues(map)
    }
}
