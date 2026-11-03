import Foundation

/// The gap between two neighbouring children of a split; drag it to resize both.
public struct Seam: Equatable, Sendable {
    public var split: NodeID
    /// The seam sits between child `index` and child `next` (the next visible one).
    public var index: Int
    public var next: Int
    public var axis: Axis
    public var rect: Rect
    /// The split's length available to children (for converting drags to weights).
    public var available: Double
}

/// The solved geometry of one framed subtree.
public struct Layout: Equatable, Sendable {
    /// A rect for every visible node of the framed subtree.
    public var nodeRects: [NodeID: Rect]
    /// Where each tile's selected window goes (below the tab strip for groups).
    public var windowRects: [NodeID: Rect]
    /// Tab strip rects for tiles with more than one window.
    public var stripRects: [NodeID: Rect]
    public var seams: [Seam]
}

public enum Solver {
    public static let tabStripHeight: Double = 36

    /// Lays out `node` to fill `bounds` (already inset for the outer margin).
    public static func solve(_ node: Node, in bounds: Rect, gap: Double) -> Layout {
        var layout = Layout(nodeRects: [:], windowRects: [:], stripRects: [:], seams: [])
        place(node, in: bounds, gap: gap, into: &layout)
        return layout
    }

    /// The bounds for a display: its visible frame inset by the gap.
    public static func bounds(for visibleFrame: Rect, gap: Double) -> Rect {
        visibleFrame.insetBy(gap)
    }

    private static func place(_ node: Node, in rect: Rect, gap: Double, into layout: inout Layout) {
        layout.nodeRects[node.id] = rect
        switch node.kind {
        case .desktop:
            return
        case .tile:
            if node.windows.count > 1 {
                let strip = Rect(x: rect.x, y: rect.y, width: rect.width, height: min(tabStripHeight, rect.height))
                layout.stripRects[node.id] = strip
                layout.windowRects[node.id] = Rect(x: rect.x, y: rect.y + strip.height,
                                                   width: rect.width, height: max(0, rect.height - strip.height))
            } else {
                layout.windowRects[node.id] = rect
            }
        case .split:
            let visible = node.children.indices.filter { !(node.children[$0].isDesktop && node.children[$0].hidden) }
            guard !visible.isEmpty else { return }
            let axis = node.axis
            let length = rect.length(along: axis)
            let available = max(0, length - gap * Double(visible.count - 1))
            let total = visible.reduce(0.0) { $0 + max(0, node.weights[$1]) }
            var cursor = rect.start(along: axis)
            var exact = cursor
            for (k, i) in visible.enumerated() {
                let share = total > 0 ? max(0, node.weights[i]) / total : 1 / Double(visible.count)
                exact += available * share
                let isLast = k == visible.count - 1
                let end = isLast ? rect.start(along: axis) + length : exact.rounded()
                let size = max(0, end - cursor)
                let childRect: Rect = axis == .horizontal
                    ? Rect(x: cursor, y: rect.y, width: size, height: rect.height)
                    : Rect(x: rect.x, y: cursor, width: rect.width, height: size)
                place(node.children[i], in: childRect, gap: gap, into: &layout)
                if !isLast {
                    let seamRect: Rect = axis == .horizontal
                        ? Rect(x: end, y: rect.y, width: gap, height: rect.height)
                        : Rect(x: rect.x, y: end, width: rect.width, height: gap)
                    let next = visible[k + 1]
                    layout.seams.append(Seam(split: node.id, index: i, next: next, axis: axis,
                                             rect: seamRect, available: available))
                }
                cursor = end + gap
                exact += gap
            }
        }
    }
}
