import Foundation

/// Where every window of one display goes for the current camera.
public struct Plan: Equatable, Sendable {
    public var framed: NodeID
    /// Layout of the framed subtree.
    public var layout: Layout
    /// Real windows shown on screen (the selected tab of each roomy tile).
    public var visible: [WindowID: Rect]
    /// Tiles too small for their window: drawn as a cover, window parked.
    public var covers: [NodeID: Rect]
    /// Docked windows moved out of view: outside the framed node, hidden tabs, cramped.
    public var parked: Set<WindowID>
    /// The desktop's rect when it is in view and not hidden.
    public var desktopRect: Rect?
    /// Every node's rect as the camera sees it, including nodes outside the
    /// framed subtree (off screen). Used to animate windows into and out of view.
    public var projected: [NodeID: Rect]
}

public enum Planner {
    /// Below this, a window is not usable and shows as a cover.
    public static let minimumUsable = Size(width: 160, height: 100)

    /// A window may be a little bigger than its tile and reach into the gap;
    /// only a real shortfall makes it a cover.
    public static func isCramped(_ rect: Rect, minimum: Size?, tolerance: Double = 12) -> Bool {
        let m = minimum ?? .zero
        let w = max(m.width - tolerance, minimumUsable.width), h = max(m.height - tolerance, minimumUsable.height)
        return rect.width + 0.5 < w || rect.height + 0.5 < h
    }

    public static func plan(_ a: Arrangement, _ nav: Navigator, visibleFrame: Rect, gap: Double,
                            minimumSize: (WindowID) -> Size? = { _ in nil }) -> Plan {
        let bounds = Solver.bounds(for: visibleFrame, gap: gap)
        let framed = nav.framedNode(in: a)
        let range = nav.framedRange(in: a)
        var layoutRoot = framed
        if let range, let p = range.first?.parent {
            // Lay out just the run of children, as if it were the whole split.
            let weights = range.map { p.weights[$0.indexInParent!] }
            layoutRoot = Node(id: -1, kind: .split, axis: p.axis, children: range.map { $0.clone() }, weights: weights)
        }
        var layout = Solver.solve(layoutRoot, in: bounds, gap: gap)
        if let range, let p = range.first?.parent {
            // Seams of the run belong to the real split. Its weights cover more
            // children than the run, so scale the length a drag is measured in.
            let runTotal = range.reduce(0.0) { $0 + p.weights[$1.indexInParent!] }
            let total = p.weights.reduce(0, +)
            layout.seams = layout.seams.map { seam in
                guard seam.split == -1 else { return seam }
                var s = seam
                s.split = p.id
                s.index = range[seam.index].indexInParent!
                s.next = range[seam.next].indexInParent!
                s.available = runTotal > 0 ? seam.available * total / runTotal : seam.available
                return s
            }
        }

        // Project the whole tree so the framed node (or run) fills the bounds.
        let world = Solver.solve(a.root, in: bounds, gap: gap)
        var projected: [NodeID: Rect] = [:]
        let framedWorld = range.map { r in r.compactMap { world.nodeRects[$0.id] }.reduce(Rect.zero) { $0.union($1) } }
            ?? world.nodeRects[framed.id]
        if let framedWorld {
            let map = AffineMap(from: framedWorld, to: bounds)
            for (id, r) in world.nodeRects { projected[id] = map.apply(r) }
        }
        for (id, r) in layout.nodeRects where id >= 0 { projected[id] = r }

        var visible: [WindowID: Rect] = [:]
        var covers: [NodeID: Rect] = [:]
        var parked = Set<WindowID>()
        for tile in a.tiles {
            guard let windowRect = layout.windowRects[tile.id], let selected = tile.selectedWindow else {
                parked.formUnion(tile.windows)
                continue
            }
            for w in tile.windows where w != selected { parked.insert(w) }
            if isCramped(windowRect, minimum: minimumSize(selected)) {
                covers[tile.id] = layout.nodeRects[tile.id]
                parked.insert(selected)
            } else {
                visible[selected] = windowRect
            }
        }
        let desktop = a.desktop
        let desktopRect = desktop.hidden ? nil : layout.nodeRects[desktop.id]
        return Plan(framed: framed.id, layout: layout, visible: visible, covers: covers, parked: parked,
                    desktopRect: desktopRect, projected: projected)
    }

    /// Whether `tile`'s window would be usable with `framing` framed.
    public static func fits(_ tile: Node, framing: Node, visibleFrame: Rect, gap: Double, minimum: Size?) -> Bool {
        guard framing.contains(tile) else { return false }
        let layout = Solver.solve(framing, in: Solver.bounds(for: visibleFrame, gap: gap), gap: gap)
        guard let r = layout.windowRects[tile.id] else { return false }
        return !isCramped(r, minimum: minimum)
    }
}
