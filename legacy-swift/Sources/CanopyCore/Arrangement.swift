import Foundation

/// Where a window goes back to when it is restored (after minimise, or when a
/// floating window is docked again with the green button).
public enum RestoreTarget: Codable, Equatable, Sendable {
    /// Beside node `beside` on `edge`, taking `fraction` of that split.
    /// `fallback` is a window that was in that neighbour, used if the node is gone.
    case docked(beside: NodeID, edge: Edge, fraction: Double, fallback: WindowID?)
    /// A tab at `index` in the group that contains `in`.
    case tab(in: WindowID, index: Int)
    case floating
}

/// One display's tree of docked windows, plus every operation that changes it.
public final class Arrangement: Codable {
    public private(set) var root: Node
    private var nextID: NodeID
    /// When a split collapses, its id points at the node that took its place,
    /// so visits and restore targets that named it still resolve.
    private var aliases: [NodeID: NodeID] = [:]

    public init() {
        root = Node(id: 1, kind: .desktop)
        nextID = 2
    }

    private enum CodingKeys: String, CodingKey { case root, aliases }

    public required init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        root = try c.decode(Node.self, forKey: .root)
        let saved = try c.decodeIfPresent([String: NodeID].self, forKey: .aliases) ?? [:]
        aliases = Dictionary(uniqueKeysWithValues: saved.compactMap { k, v in NodeID(k).map { ($0, v) } })
        nextID = (root.allNodes.map(\.id).max() ?? 0) + 1
        if root.desktopNode == nil {
            // A tree without a desktop is invalid; give it one on the right.
            let d = Node(id: nextID, kind: .desktop)
            nextID += 1
            insert(d, beside: root, edge: .right)
        }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(root, forKey: .root)
        try c.encode(Dictionary(uniqueKeysWithValues: aliases.map { (String($0.key), $0.value) }), forKey: .aliases)
    }

    /// A deep copy, used to preview the outcome of a drop.
    public func clone() -> Arrangement {
        let a = Arrangement()
        a.root = root.clone()
        a.nextID = nextID
        a.aliases = aliases
        return a
    }

    private func makeID() -> NodeID {
        defer { nextID += 1 }
        return nextID
    }

    // MARK: Queries

    public var desktop: Node { root.desktopNode! }
    public var windows: [WindowID] { root.allWindows }
    public var tiles: [Node] { root.tiles }
    public var isEmpty: Bool { root.tiles.isEmpty }
    public func contains(_ window: WindowID) -> Bool { root.tile(containing: window) != nil }
    public func tile(of window: WindowID) -> Node? { root.tile(containing: window) }
    /// The node with `id`, or the node that took its place after a collapse.
    public func node(_ id: NodeID) -> Node? {
        var id = id
        var seen = Set<NodeID>()
        while true {
            if let n = root.find(id) { return n }
            guard let next = aliases[id], seen.insert(id).inserted else { return nil }
            id = next
        }
    }

    // MARK: Docking

    /// Docks `window` in a new tile beside `target` on `edge`. Returns the new tile.
    @discardableResult
    public func dock(_ window: WindowID, beside target: Node, edge: Edge, fraction: Double? = nil) -> Node {
        if let existing = tile(of: window) {
            // Already docked: move it rather than duplicate it.
            if existing.windows.count == 1 {
                move(existing, beside: target, edge: edge, fraction: fraction)
                return existing
            }
            remove(window)
        }
        let tile = Node(id: makeID(), kind: .tile, windows: [window])
        insert(tile, beside: target, edge: edge, fraction: fraction)
        return tile
    }

    /// Docks beside everything at a screen edge.
    @discardableResult
    public func dockAtScreenEdge(_ window: WindowID, edge: Edge) -> Node {
        dock(window, beside: root, edge: edge)
    }

    /// Inserts a detached node beside `target`. Neighbours make room.
    public func insert(_ node: Node, beside target: Node, edge: Edge, fraction: Double? = nil) {
        precondition(node.parent == nil, "insert expects a detached node")
        let axis = edge.axis

        // Same axis as the target's split: become a sibling.
        if let p = target.parent, p.isSplit, p.axis == axis, let i = target.indexInParent {
            let f = Self.clampFraction(fraction ?? 1 / Double(p.children.count + 1))
            let total = p.weights.reduce(0, +)
            p.weights = p.weights.map { $0 / total * (1 - f) }
            let at = edge.isLeading ? i : i + 1
            p.children.insert(node, at: at)
            p.weights.insert(f, at: at)
            node.parent = p
            return
        }

        // Target is itself a split on this axis: join it at the near end.
        if target.isSplit, target.axis == axis {
            let f = Self.clampFraction(fraction ?? 1 / Double(target.children.count + 1))
            let total = target.weights.reduce(0, +)
            target.weights = target.weights.map { $0 / total * (1 - f) }
            let at = edge.isLeading ? 0 : target.children.count
            target.children.insert(node, at: at)
            target.weights.insert(f, at: at)
            node.parent = target
            return
        }

        // Otherwise wrap the target in a new split.
        let f = Self.clampFraction(fraction ?? 0.5)
        let parent = target.parent
        let index = target.indexInParent
        let split = Node(id: makeID(), kind: .split, axis: axis)
        target.parent = nil
        split.children = edge.isLeading ? [node, target] : [target, node]
        split.weights = edge.isLeading ? [f, 1 - f] : [1 - f, f]
        node.parent = split
        target.parent = split
        if let parent, let index {
            parent.children[index] = split
            split.parent = parent
        } else {
            root = split
        }
    }

    /// Moves an existing node (tile or subtree) beside `target`.
    public func move(_ node: Node, beside target: Node, edge: Edge, fraction: Double? = nil) {
        guard node !== target, !node.contains(target) else { return }
        var target = target
        if let survivor = detach(node, shareSpace: true), survivor.replaced === target {
            target = survivor.by
        }
        insert(node, beside: target, edge: edge, fraction: fraction)
    }

    // MARK: Tabs

    /// Adds `window` as a selected tab of `tile`.
    public func addTab(_ window: WindowID, to tile: Node, at index: Int? = nil) {
        guard tile.isTile else { return }
        if let current = self.tile(of: window) {
            if current === tile { return }
            remove(window)
        }
        let at = min(max(0, index ?? tile.windows.count), tile.windows.count)
        tile.windows.insert(window, at: at)
        tile.selected = at
    }

    /// Merges every window of `source` into `target` as tabs.
    public func merge(_ source: Node, into target: Node) {
        guard source.isTile, target.isTile, source !== target else { return }
        let moving = source.windows
        let selectedWindow = source.selectedWindow
        detach(source, shareSpace: true)
        target.windows.append(contentsOf: moving)
        if let s = selectedWindow, let i = target.windows.firstIndex(of: s) { target.selected = i }
    }

    public func select(_ window: WindowID) {
        guard let t = tile(of: window), let i = t.windows.firstIndex(of: window) else { return }
        t.selected = i
    }

    // MARK: Removal

    /// Removes `window` from the tree and returns where it could go back to.
    @discardableResult
    public func remove(_ window: WindowID, shareSpace: Bool = true) -> RestoreTarget? {
        guard let tile = tile(of: window), let i = tile.windows.firstIndex(of: window) else { return nil }
        if tile.windows.count > 1 {
            let wasSelected = tile.selectedWindow
            tile.windows.remove(at: i)
            if wasSelected == window {
                tile.selected = min(i, tile.windows.count - 1)
            } else if let w = wasSelected, let j = tile.windows.firstIndex(of: w) {
                tile.selected = j
            }
            return .tab(in: tile.windows[min(i, tile.windows.count - 1)], index: i)
        }
        let target = restoreTarget(forDetaching: tile)
        detach(tile, shareSpace: shareSpace)
        return target
    }

    /// Removes every window that is not in `live`. Returns the removed windows.
    @discardableResult
    public func retain(only live: Set<WindowID>, shareSpace: Bool = true) -> [WindowID] {
        let gone = windows.filter { !live.contains($0) }
        for w in gone { remove(w, shareSpace: shareSpace) }
        return gone
    }

    /// Replaces window ids (after a restart the same window can have a new id).
    public func remap(_ mapping: [WindowID: WindowID]) {
        for t in tiles {
            t.windows = t.windows.map { mapping[$0] ?? $0 }
        }
    }

    private func restoreTarget(forDetaching node: Node) -> RestoreTarget? {
        guard let p = node.parent, let i = node.indexInParent, p.children.count > 1 else { return nil }
        let total = p.weights.reduce(0, +)
        let fraction = total > 0 ? p.weights[i] / total : 0.5
        let neighbour: Node
        let edge: Edge
        if i > 0 {
            neighbour = p.children[i - 1]
            edge = p.axis == .horizontal ? .right : .bottom
        } else {
            neighbour = p.children[i + 1]
            edge = p.axis == .horizontal ? .left : .top
        }
        return .docked(beside: neighbour.id, edge: edge, fraction: fraction, fallback: neighbour.allWindows.first)
    }

    /// Detaches `node` from its parent. A split left with one child collapses;
    /// returns that collapse so callers can follow the surviving node.
    @discardableResult
    private func detach(_ node: Node, shareSpace: Bool) -> (replaced: Node, by: Node)? {
        guard let p = node.parent, let i = node.indexInParent else { return nil }
        let w = p.weights[i]
        p.children.remove(at: i)
        p.weights.remove(at: i)
        node.parent = nil
        if !shareSpace, !p.children.isEmpty {
            let j = i < p.children.count ? i : i - 1
            p.weights[j] += w
        }
        let total = p.weights.reduce(0, +)
        if total > 0 { p.weights = p.weights.map { $0 / total } }
        if p.children.count == 1 {
            let only = p.children[0]
            if let g = p.parent, let k = p.indexInParent {
                g.children[k] = only
                only.parent = g
            } else {
                only.parent = nil
                root = only
            }
            p.children = []
            p.weights = []
            p.parent = nil
            recordAlias(p.id, only.id)
            return (p, only)
        }
        return nil
    }

    /// Puts `window` back where `target` says. Returns nil if it could not, in
    /// which case the caller auto-docks it.
    @discardableResult
    public func restore(_ window: WindowID, to target: RestoreTarget) -> Node? {
        switch target {
        case let .tab(other, index):
            guard let t = tile(of: other) else { return nil }
            addTab(window, to: t, at: index)
            return t
        case let .docked(nodeID, edge, fraction, fallback):
            if let n = node(nodeID) {
                return dock(window, beside: n, edge: edge, fraction: fraction)
            }
            if let fallback, let t = tile(of: fallback) {
                return dock(window, beside: t, edge: edge, fraction: fraction)
            }
            return nil
        case .floating:
            return nil
        }
    }

    /// Docks `window` by splitting the largest visible tile along its longer
    /// side, or at the left screen edge when nothing is docked yet.
    @discardableResult
    public func autoDock(_ window: WindowID, rects: [NodeID: Rect]) -> Node {
        let candidates = tiles.compactMap { t -> (Node, Rect)? in
            guard let r = rects[t.id], !r.isEmpty else { return nil }
            return (t, r)
        }
        guard let (largest, r) = candidates.max(by: { $0.1.area < $1.1.area }) else {
            return dockAtScreenEdge(window, edge: .left)
        }
        return dock(window, beside: largest, edge: r.width >= r.height ? .right : .bottom)
    }

    // MARK: Resizing and the desktop

    /// Moves the seam between child `index` and child `next` of `split` by
    /// `delta` points, given the split's available length in points. Neither
    /// side goes below `minPoints`, or below its own minimum (`minBefore`,
    /// `minAfter`) when known: the seam stops where a window would refuse.
    public func resizeSeam(_ split: Node, index: Int, next: Int? = nil, delta: Double, available: Double,
                           minFraction: Double = 0.05, minPoints: Double = 60,
                           minBefore: Double = 0, minAfter: Double = 0) {
        let next = next ?? index + 1
        guard split.isSplit, index >= 0, next > index, next < split.children.count, available > 0 else { return }
        let total = split.weights.reduce(0, +)
        let lowA = max(minFraction * total, max(minPoints, minBefore) / available * total)
        let lowB = max(minFraction * total, max(minPoints, minAfter) / available * total)
        let a = split.weights[index], b = split.weights[next]
        let dw = delta / available * total
        var na = a + dw
        // Never squeeze a side that already fits; never push one that doesn't further.
        if dw > 0 { na = min(na, max(a, a + b - lowB)) } else { na = max(na, min(a, lowA)) }
        guard na > 0, a + b - na > 0 else { return }
        split.weights[index] = na
        split.weights[next] = a + b - na
    }

    public func setDesktopHidden(_ hidden: Bool) {
        desktop.hidden = hidden
    }

    private func recordAlias(_ from: NodeID, _ to: NodeID) {
        aliases[from] = to
        if aliases.count > 512 {
            let live = Set(root.allNodes.map(\.id))
            aliases = aliases.filter { node($0.value).map { live.contains($0.id) } ?? false }
        }
    }

    static func clampFraction(_ f: Double) -> Double { min(max(f, 0.05), 0.95) }
}
