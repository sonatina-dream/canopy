import Foundation

/// A window's identity. On macOS this is the CGWindowID.
public typealias WindowID = UInt64
public typealias NodeID = Int

/// One node of a display's arrangement tree.
///
/// - `split`: children laid out along `axis`, sized by `weights` (sum 1).
/// - `tile`: one or more windows; more than one makes a tab group.
/// - `desktop`: the free-floating Desktop region. Exactly one per tree.
public final class Node: Codable, CustomStringConvertible {
    public enum Kind: String, Codable, Sendable {
        case split, tile, desktop
    }

    public let id: NodeID
    public internal(set) var kind: Kind
    public internal(set) var axis: Axis
    public internal(set) var children: [Node]
    public internal(set) var weights: [Double]
    public internal(set) var windows: [WindowID]
    public internal(set) var selected: Int
    /// Only meaningful for the desktop node.
    public internal(set) var hidden: Bool
    public internal(set) weak var parent: Node?

    init(id: NodeID, kind: Kind, axis: Axis = .horizontal, children: [Node] = [],
         weights: [Double] = [], windows: [WindowID] = [], selected: Int = 0, hidden: Bool = false) {
        self.id = id
        self.kind = kind
        self.axis = axis
        self.children = children
        self.weights = weights
        self.windows = windows
        self.selected = selected
        self.hidden = hidden
        for c in children { c.parent = self }
    }

    // MARK: Codable (parent links are rebuilt on decode)

    private enum CodingKeys: String, CodingKey {
        case id, kind, axis, children, weights, windows, selected, hidden
    }

    public required init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(NodeID.self, forKey: .id)
        kind = try c.decode(Kind.self, forKey: .kind)
        axis = try c.decodeIfPresent(Axis.self, forKey: .axis) ?? .horizontal
        children = try c.decodeIfPresent([Node].self, forKey: .children) ?? []
        weights = try c.decodeIfPresent([Double].self, forKey: .weights) ?? []
        windows = try c.decodeIfPresent([WindowID].self, forKey: .windows) ?? []
        selected = try c.decodeIfPresent(Int.self, forKey: .selected) ?? 0
        hidden = try c.decodeIfPresent(Bool.self, forKey: .hidden) ?? false
        for child in children { child.parent = self }
        if weights.count != children.count {
            weights = Array(repeating: 1 / Double(max(1, children.count)), count: children.count)
        }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(id, forKey: .id)
        try c.encode(kind, forKey: .kind)
        switch kind {
        case .split:
            try c.encode(axis, forKey: .axis)
            try c.encode(children, forKey: .children)
            try c.encode(weights, forKey: .weights)
        case .tile:
            try c.encode(windows, forKey: .windows)
            try c.encode(selected, forKey: .selected)
        case .desktop:
            try c.encode(hidden, forKey: .hidden)
        }
    }

    /// A deep copy with the same node ids.
    public func clone() -> Node {
        Node(id: id, kind: kind, axis: axis, children: children.map { $0.clone() },
             weights: weights, windows: windows, selected: selected, hidden: hidden)
    }

    // MARK: Queries

    public var isSplit: Bool { kind == .split }
    public var isTile: Bool { kind == .tile }
    public var isDesktop: Bool { kind == .desktop }

    public var selectedWindow: WindowID? {
        guard isTile, !windows.isEmpty else { return nil }
        return windows[min(max(0, selected), windows.count - 1)]
    }

    public var depth: Int {
        var d = 0
        var n = parent
        while let p = n { d += 1; n = p.parent }
        return d
    }

    public var indexInParent: Int? {
        parent?.children.firstIndex { $0 === self }
    }

    /// Ancestors from the parent up to the root.
    public var ancestors: [Node] {
        var out: [Node] = []
        var n = parent
        while let p = n { out.append(p); n = p.parent }
        return out
    }

    public var root: Node {
        var n = self
        while let p = n.parent { n = p }
        return n
    }

    public func isAncestor(of other: Node) -> Bool {
        var n = other.parent
        while let p = n {
            if p === self { return true }
            n = p.parent
        }
        return false
    }

    /// True if `other` is this node or inside it.
    public func contains(_ other: Node) -> Bool { other === self || isAncestor(of: other) }

    /// Preorder traversal of this subtree.
    public var allNodes: [Node] {
        var out: [Node] = [self]
        for c in children { out.append(contentsOf: c.allNodes) }
        return out
    }

    public var tiles: [Node] { allNodes.filter(\.isTile) }

    public var allWindows: [WindowID] { tiles.flatMap(\.windows) }

    public var desktopNode: Node? { allNodes.first(where: \.isDesktop) }

    public func find(_ id: NodeID) -> Node? { allNodes.first { $0.id == id } }

    public func tile(containing window: WindowID) -> Node? {
        tiles.first { $0.windows.contains(window) }
    }

    /// The deepest node containing every node in `nodes`.
    public static func lowestCommonAncestor(_ nodes: [Node]) -> Node? {
        guard let first = nodes.first else { return nil }
        var candidate: Node? = first
        while let c = candidate {
            if nodes.allSatisfy({ c.contains($0) }) { return c }
            candidate = c.parent
        }
        return nil
    }

    /// The least length along `axis` this subtree can take, given each
    /// window's minimum size (unknown minimums count as `fallback`).
    public func minimumLength(along axis: Axis, gap: Double, fallback: Double = 60,
                              minimum: (WindowID) -> Size?) -> Double {
        switch kind {
        case .desktop:
            return hidden ? 0 : fallback
        case .tile:
            guard let w = selectedWindow, let m = minimum(w) else { return fallback }
            let strip = axis == .vertical && windows.count > 1 ? Solver.tabStripHeight : 0
            return max(fallback, axis == .horizontal ? m.width : m.height) + strip
        case .split:
            let kids = children.filter { !($0.isDesktop && $0.hidden) }
            let lengths = kids.map { $0.minimumLength(along: axis, gap: gap, fallback: fallback, minimum: minimum) }
            if self.axis == axis { return lengths.reduce(0, +) + gap * Double(max(0, kids.count - 1)) }
            return lengths.max() ?? 0
        }
    }

    public var description: String {
        switch kind {
        case .desktop: return hidden ? "D(hidden)" : "D"
        case .tile:
            return windows.count == 1 ? "\(windows[0])" : "[" + windows.map(String.init).joined(separator: ",") + "]"
        case .split:
            let sym = axis == .horizontal ? "H" : "V"
            return sym + "(" + children.map(\.description).joined(separator: " ") + ")"
        }
    }
}
