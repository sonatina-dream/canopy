import Foundation
import Testing
@testable import CanopyCore

private let screen = Rect(x: 0, y: 25, width: 1512, height: 920)

@Suite("Layout solver (SPEC §3)")
struct SolverTests {
    @Test("Children share a split by weight with a gap between and around them")
    func gaps() {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        let bounds = Solver.bounds(for: screen, gap: 12)
        let l = Solver.solve(a.root, in: bounds, gap: 12)
        let r1 = l.nodeRects[t1.id]!
        let rd = l.nodeRects[a.desktop.id]!
        #expect(r1.minX == 12)
        #expect(r1.minY == 37)
        #expect(rd.minX - r1.maxX == 12)
        #expect(rd.maxX == 1500)
        #expect(abs(r1.width - rd.width) <= 1)
        #expect(l.seams.count == 1)
        #expect(l.seams[0].rect == Rect(x: r1.maxX, y: 37, width: 12, height: 896))
    }

    @Test("Edges are whole points and the last child absorbs rounding")
    func rounding() {
        let a = Arrangement()
        var t = a.dockAtScreenEdge(1, edge: .left)
        t = a.dock(2, beside: t, edge: .right)
        a.dock(3, beside: t, edge: .right)
        let l = Solver.solve(a.root, in: Rect(x: 0, y: 0, width: 1001, height: 500), gap: 7)
        let xs = a.root.children.map { l.nodeRects[$0.id]! }
        for r in xs { #expect(r.minX == r.minX.rounded() && r.maxX == r.maxX.rounded()) }
        #expect(xs.last!.maxX == 1001)
    }

    @Test("A tab group reserves a strip on top for its tabs")
    func strip() {
        let a = Arrangement()
        let t = a.dockAtScreenEdge(1, edge: .left)
        a.addTab(2, to: t)
        let l = Solver.solve(a.root, in: Rect(x: 0, y: 0, width: 1000, height: 600), gap: 0)
        #expect(l.stripRects[t.id]!.height == Solver.tabStripHeight)
        #expect(l.windowRects[t.id]!.minY == Solver.tabStripHeight)
        #expect(l.windowRects[t.id]!.maxY == 600)
    }

    @Test("A hidden Desktop gives its space to its neighbours")
    func hiddenDesktop() {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        a.setDesktopHidden(true)
        let l = Solver.solve(a.root, in: Rect(x: 0, y: 0, width: 1000, height: 600), gap: 10)
        #expect(l.nodeRects[t1.id] == Rect(x: 0, y: 0, width: 1000, height: 600))
        #expect(l.nodeRects[a.desktop.id] == nil)
        #expect(l.seams.isEmpty)
    }
}

@Suite("Planner: cramped windows and parking (SPEC §2)")
struct PlannerTests {
    @Test("Windows outside the framed node are parked")
    func parkOutside() {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        a.dock(2, beside: t1, edge: .right)
        let nav = Navigator()
        nav.frame(t1, in: a)
        let p = Planner.plan(a, nav, visibleFrame: screen, gap: 12)
        #expect(p.visible.keys.sorted() == [1])
        #expect(p.parked == [2])
        #expect(p.visible[1] == screen.insetBy(12))
        #expect(p.desktopRect == nil)
        // The out-of-view neighbour is projected off screen to its right.
        #expect(p.projected[a.tile(of: 2)!.id]!.minX >= screen.maxX)
    }

    @Test("A window too small for its app becomes a cover until there's room")
    func cramped() {
        let a = Arrangement()
        var t = a.dockAtScreenEdge(1, edge: .left)
        for w: WindowID in 2...8 { t = a.dock(w, beside: t, edge: .right) }
        let nav = Navigator()
        let p = Planner.plan(a, nav, visibleFrame: screen, gap: 12, minimumSize: { _ in Size(width: 400, height: 300) })
        #expect(p.visible.isEmpty)
        #expect(p.covers.count == 8)
        #expect(p.parked.count == 8)
        // Framing one window gives it room again.
        nav.frame(a.tile(of: 3)!, in: a)
        let q = Planner.plan(a, nav, visibleFrame: screen, gap: 12, minimumSize: { _ in Size(width: 400, height: 300) })
        #expect(q.visible.keys.sorted() == [3])
    }

    @Test("A window a few points bigger than its tile stays a window")
    func smallShortfall() {
        #expect(!Planner.isCramped(Rect(x: 0, y: 0, width: 464, height: 769), minimum: Size(width: 466, height: 252)))
        #expect(Planner.isCramped(Rect(x: 0, y: 0, width: 306, height: 769), minimum: Size(width: 466, height: 252)))
    }

    @Test("Only the selected tab is shown; the others are parked")
    func tabsParked() {
        let a = Arrangement()
        let t = a.dockAtScreenEdge(1, edge: .left)
        a.addTab(2, to: t)
        a.addTab(3, to: t)
        a.select(2)
        let p = Planner.plan(a, Navigator(), visibleFrame: screen, gap: 12)
        #expect(p.visible.keys.sorted() == [2])
        #expect(p.parked == [1, 3])
    }
}

@Suite("Navigation (SPEC §6)")
struct NavigationTests {
    /// H( V(1 2) H... ) built so that there are levels to step through:
    /// root = H( V(1, H(2, 3)), D )
    private func world() -> (Arrangement, Navigator) {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        let t2 = a.dock(2, beside: t1, edge: .bottom)
        a.dock(3, beside: t2, edge: .right)
        return (a, Navigator())
    }

    private func rects(_ a: Arrangement) -> [NodeID: Rect] {
        Solver.solve(a.root, in: Solver.bounds(for: screen, gap: 12), gap: 12).nodeRects
    }

    @Test("Step in goes toward the focused window, step out goes back up a level")
    func stepInOut() {
        let (a, nav) = world()
        #expect(a.root.description == "H(V(1 H(2 3)) D)")
        #expect(nav.stepIn(in: a, focused: 3, pointer: nil, rects: rects(a)))
        #expect(nav.framedNode(in: a).description == "V(1 H(2 3))")
        nav.stepIn(in: a, focused: 3, pointer: nil, rects: rects(a))
        #expect(nav.framedNode(in: a).description == "H(2 3)")
        nav.stepIn(in: a, focused: 3, pointer: nil, rects: rects(a))
        #expect(nav.framedNode(in: a).description == "3")
        #expect(!nav.stepIn(in: a, focused: 3, pointer: nil, rects: rects(a)), "a tile has nothing inside")
        nav.stepOut(in: a)
        #expect(nav.framedNode(in: a).description == "H(2 3)")
        nav.everything(in: a)
        #expect(nav.framedNode(in: a) === a.root)
        #expect(!nav.stepOut(in: a), "nothing above everything")
    }

    @Test("Step in can go toward the pointer instead")
    func stepTowardPointer() {
        let (a, nav) = world()
        let r = rects(a)
        let desktopCentre = r[a.desktop.id]!.center
        nav.stepIn(in: a, focused: 1, pointer: desktopCentre, rects: r, preferPointer: true)
        #expect(nav.framedNode(in: a).isDesktop)
    }

    @Test("Back and forward work like a web browser")
    func history() {
        let (a, nav) = world()
        let t3 = a.tile(of: 3)!
        nav.frame(t3, in: a)
        nav.stepOut(in: a)
        #expect(nav.framedNode(in: a).description == "H(2 3)")
        nav.back(in: a)
        #expect(nav.framedNode(in: a) === t3)
        nav.back(in: a)
        #expect(nav.framedNode(in: a) === a.root)
        #expect(!nav.back(in: a))
        nav.forward(in: a)
        nav.forward(in: a)
        #expect(nav.framedNode(in: a).description == "H(2 3)")
        // A new move clears forward.
        nav.back(in: a)
        nav.everything(in: a)
        #expect(!nav.canGoForward)
    }

    @Test("Neighbouring spaces are found left, right, up and down at the same depth")
    func neighbours() {
        let (a, nav) = world()
        let r = rects(a)
        nav.frame(a.tile(of: 2)!, in: a)
        nav.neighbour(.right, in: a, rects: r)
        #expect(nav.framedNode(in: a).description == "3")
        nav.neighbour(.up, in: a, rects: r)
        #expect(nav.framedNode(in: a).description == "1")
        nav.neighbour(.down, in: a, rects: r)
        // 1 sits at depth 2; the space below it at that depth is H(2 3).
        #expect(nav.framedNode(in: a).description == "H(2 3)")
        // From a deeper node, going up and down again lands at the same depth.
        nav.frame(a.tile(of: 3)!, in: a)
        nav.neighbour(.up, in: a, rects: r)
        nav.neighbour(.down, in: a, rects: r)
        #expect(nav.framedNode(in: a).description == "H(2 3)", "1 is a leaf, so down from it is its sibling space")
        nav.frame(a.tile(of: 1)!, in: a)
        nav.neighbour(.right, in: a, rects: r)
        #expect(nav.framedNode(in: a).isDesktop)
        #expect(!nav.neighbour(.right, in: a, rects: r), "nothing right of the Desktop")
    }

    @Test("Frame window toggles: again goes back")
    func toggleFrame() {
        let (a, nav) = world()
        let t2 = a.tile(of: 2)!
        nav.toggleFrame(t2, in: a)
        #expect(nav.framedNode(in: a) === t2)
        nav.toggleFrame(t2, in: a)
        #expect(nav.framedNode(in: a) === a.root)
    }

    @Test("Saved views 1–9 come straight back from anywhere")
    func savedViews() {
        let (a, nav) = world()
        nav.frame(a.tile(of: 2)!.parent!, in: a)
        nav.save(4, in: a)
        nav.save(10, in: a)
        #expect(nav.savedViews.keys.sorted() == [4])
        nav.everything(in: a)
        #expect(nav.go(to: 4, in: a))
        #expect(nav.framedNode(in: a).description == "H(2 3)")
        #expect(!nav.go(to: 5, in: a))
        nav.forget(4)
        #expect(nav.savedViews.isEmpty)
    }

    @Test("A saved view still resolves after its node is gone")
    func savedViewSurvivesReshape() {
        let (a, nav) = world()
        nav.frame(a.tile(of: 2)!.parent!, in: a)     // H(2 3)
        nav.save(1, in: a)
        a.dock(4, beside: a.tile(of: 2)!, edge: .bottom) // H(V(2 4) 3): H(2 3) node is still there
        a.remove(3)                                    // H(2 3) collapses into V(2 4)
        nav.everything(in: a)
        nav.go(to: 1, in: a)
        #expect(nav.framedNode(in: a).description == "V(2 4)", "the view follows the node that took its place")
        // A view whose node was removed outright falls back to its windows.
        nav.frame(a.tile(of: 4)!, in: a)
        nav.save(2, in: a)
        a.addTab(4, to: a.tile(of: 1)!)
        nav.everything(in: a)
        nav.go(to: 2, in: a)
        #expect(nav.framedNode(in: a).description == "[1,4]")
    }

    @Test("Zoom to a rectangle frames what it covers")
    func rectZoom() {
        let (a, nav) = world()
        let r = rects(a)
        let r2 = r[a.tile(of: 2)!.id]!, r3 = r[a.tile(of: 3)!.id]!
        nav.frame(rect: r2.union(r3).insetBy(30), in: a, rects: r)
        #expect(nav.framedNode(in: a).description == "H(2 3)")
        nav.frame(rect: r2.insetBy(40), in: a, rects: r)
        #expect(nav.framedNode(in: a).description == "2")
        // A sliver over a neighbour doesn't pull it in.
        nav.everything(in: a)
        let sliver = Rect(x: r2.minX + 20, y: r2.minY + 20, width: r2.width - 20 + 14, height: r2.height - 40)
        nav.frame(rect: sliver, in: a, rects: r)
        #expect(nav.framedNode(in: a).description == "2")
    }

    @Test("A rectangle over some of a row frames just that run of windows")
    func rangeFraming() throws {
        let a = Arrangement()
        var t = a.dockAtScreenEdge(1, edge: .left)
        t = a.dock(2, beside: t, edge: .right)
        a.dock(3, beside: t, edge: .right)               // H(1 2 3 D)
        let nav = Navigator()
        let r = rects(a)
        let r1 = r[a.tile(of: 1)!.id]!, r2 = r[a.tile(of: 2)!.id]!
        #expect(nav.frame(rect: r1.union(r2).insetBy(40), in: a, rects: r))
        let range = try #require(nav.framedRange(in: a))
        #expect(range.map(\.description) == ["1", "2"])
        // The run fills the screen; everything else is out of view.
        let p = Planner.plan(a, nav, visibleFrame: screen, gap: 12)
        #expect(p.visible.keys.sorted() == [1, 2])
        #expect(p.parked == [3])
        #expect(abs(p.visible[1]!.width - p.visible[2]!.width) <= 1)
        #expect(p.visible[1]!.minX == 12 && p.visible[2]!.maxX == screen.maxX - 12)
        // Dragging the run's gap resizes those two in the real split.
        let seam = try #require(p.layout.seams.first)
        #expect(seam.split == a.root.id && seam.index == 0 && seam.next == 1)
        a.resizeSeam(a.root, index: seam.index, next: seam.next, delta: 100, available: seam.available)
        let q = Planner.plan(a, nav, visibleFrame: screen, gap: 12)
        #expect(abs(q.visible[1]!.width - (p.visible[1]!.width + 100)) <= 1)
        // Neighbour right of the run is 3; stepping out frames the whole row.
        #expect(nav.neighbourNode(.right, in: a, rects: r)?.description == "3")
        nav.stepOut(in: a)
        #expect(nav.framedRange(in: a) == nil && nav.framedNode(in: a) === a.root)
        // Back returns to the run, and a saved run comes back too.
        nav.back(in: a)
        #expect(nav.framedRange(in: a)?.count == 2)
        nav.save(2, in: a)
        nav.everything(in: a)
        nav.go(to: 2, in: a)
        #expect(nav.framedRange(in: a)?.map(\.description) == ["1", "2"])
    }

    @Test("A pinch settles on the level that matches how far you pinched")
    func pinchSettles() {
        let (a, nav) = world()                       // H(V(1 H(2 3)) D)
        let bounds = Solver.bounds(for: screen, gap: 12)
        func projected() -> [NodeID: Rect] { Planner.plan(a, nav, visibleFrame: screen, gap: 12).projected }
        let t3 = a.tile(of: 3)!
        let p3 = projected()[t3.id]!.center
        // A small pinch stays put.
        #expect(nav.settleTarget(scale: 1.05, pointer: p3, in: a, rects: projected(), screen: bounds) === a.root)
        // Pinching in a lot over 3 settles on 3; a little, on a level above it.
        #expect(nav.settleTarget(scale: 8, pointer: p3, in: a, rects: projected(), screen: bounds) === t3)
        let mid = nav.settleTarget(scale: 2.2, pointer: p3, in: a, rects: projected(), screen: bounds)
        #expect(mid.contains(t3) && mid !== a.root && mid !== t3)
        // From 3, pinching out a lot settles on everything.
        nav.frame(t3, in: a)
        #expect(nav.settleTarget(scale: 0.15, pointer: p3, in: a, rects: projected(), screen: bounds) === a.root)
        // Pinching in over the Desktop settles on it.
        nav.everything(in: a)
        let pd = projected()[a.desktop.id]!.center
        #expect(nav.settleTarget(scale: 2.2, pointer: pd, in: a, rects: projected(), screen: bounds) === a.desktop)
    }

    @Test("Reveal widens the view so a newly docked window is never out of sight")
    func reveal() {
        let (a, nav) = world()
        let t1 = a.tile(of: 1)!
        nav.frame(t1, in: a)
        let t9 = a.dock(9, beside: t1, edge: .right)
        #expect(nav.reveal(t9, in: a))
        #expect(nav.framedNode(in: a).description == "H(1 9)")
    }

    @Test("Bringing a cramped window into view steps down until it fits")
    func bringIntoView() {
        let a = Arrangement()
        var t = a.dockAtScreenEdge(1, edge: .left)
        for w: WindowID in 2...6 { t = a.dock(w, beside: t, edge: .bottom) }
        // Six windows stacked: each ~140 pt tall, under 300 pt minimum.
        let nav = Navigator()
        let min = Size(width: 300, height: 300)
        let t4 = a.tile(of: 4)!
        nav.bringIntoView(t4, in: a) { Planner.fits(t4, framing: $0, visibleFrame: screen, gap: 12, minimum: min) }
        #expect(nav.framedNode(in: a) === t4)
    }
}

@Suite("Drop resolution (SPEC §5)")
struct DropTests {
    let resolver = DropResolver()
    let tileRect = Rect(x: 12, y: 37, width: 738, height: 896)
    let desktopRect = Rect(x: 762, y: 37, width: 738, height: 896)

    @Test("Near the screen edge docks beside everything")
    func screenEdge() {
        let t = resolver.resolve(pointer: Point(x: 5, y: 400), screen: screen, tiles: [(5, tileRect)],
                                 desktop: (1, desktopRect), held: nil)
        #expect(t == .screenEdge(.left))
        let b = resolver.resolve(pointer: Point(x: 1000, y: 940), screen: screen, tiles: [],
                                 desktop: (1, desktopRect), held: nil)
        #expect(b == .screenEdge(.bottom))
    }

    @Test("Pushing the pointer into the menu bar or the Dock is the screen edge")
    func pastVisibleFrame() {
        let display = Rect(x: 0, y: 0, width: 1512, height: 982)
        let top = resolver.resolve(pointer: Point(x: 700, y: 10), screen: screen, display: display,
                                   tiles: [(5, tileRect)], desktop: (1, desktopRect), held: nil)
        #expect(top == .screenEdge(.top))
        let dock = resolver.resolve(pointer: Point(x: 700, y: 970), screen: screen, display: display,
                                    tiles: [(5, tileRect)], desktop: (1, desktopRect), held: nil)
        #expect(dock == .screenEdge(.bottom))
        let elsewhere = resolver.resolve(pointer: Point(x: 3000, y: 10), screen: screen, display: display,
                                         tiles: [], desktop: nil, held: nil)
        #expect(elsewhere == .none, "another display's pointer is not this display's edge")
    }

    @Test("Over a window's edge docks beside it on that edge")
    func tileEdges() {
        func at(_ u: Double, _ v: Double) -> DropTarget {
            resolver.resolve(pointer: Point(x: tileRect.minX + u * tileRect.width, y: tileRect.minY + v * tileRect.height),
                             screen: screen, tiles: [(5, tileRect)], desktop: (1, desktopRect), held: nil)
        }
        #expect(at(0.2, 0.5) == .beside(5, .left))
        #expect(at(0.9, 0.5) == .beside(5, .right))
        #expect(at(0.5, 0.1) == .beside(5, .top))
        #expect(at(0.5, 0.85) == .beside(5, .bottom))
        #expect(at(0.15, 0.2) == .beside(5, .left), "nearest edge wins in a corner")
    }

    @Test("On a big window the edge zones stop at 200 pt, so most of it makes tabs")
    func bigWindowMiddle() {
        let big = Rect(x: 12, y: 37, width: 1400, height: 880)
        #expect(resolver.zone(of: Point(x: big.minX + 0.3 * big.width, y: big.midY), in: big) == .centre)
        #expect(resolver.zone(of: Point(x: big.minX + 150, y: big.midY), in: big) == .edge(.left))
        #expect(resolver.zone(of: Point(x: big.midX, y: big.minY + 0.15 * big.height), in: big) == .edge(.top))
        #expect(resolver.zone(of: Point(x: big.midX, y: big.minY + 0.30 * big.height), in: big) == .centre)
    }

    @Test("The middle of a window makes tabs only after a pause")
    func centreHold() {
        let p = tileRect.center
        #expect(resolver.resolve(pointer: p, screen: screen, tiles: [(5, tileRect)], desktop: nil, held: nil) == .none)
        #expect(resolver.resolve(pointer: p, screen: screen, tiles: [(5, tileRect)], desktop: nil, held: 5) == .tab(5))
    }

    @Test("Over the Desktop floats, except right by an inner edge")
    func desktop() {
        #expect(resolver.resolve(pointer: desktopRect.center, screen: screen, tiles: [(5, tileRect)],
                                 desktop: (1, desktopRect), held: nil) == .float)
        #expect(resolver.resolve(pointer: Point(x: desktopRect.minX + 10, y: 400), screen: screen,
                                 tiles: [(5, tileRect)], desktop: (1, desktopRect), held: nil) == .beside(1, .left))
    }

    @Test("Applying drops changes the arrangement as previewed")
    func apply() {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        #expect(a.apply(.beside(t1.id, .bottom), window: 2))
        #expect(a.root.description == "H(V(1 2) D)")
        #expect(a.apply(.tab(t1.id), window: 3))
        #expect(a.root.description == "H(V([1,3] 2) D)")
        #expect(!a.apply(.float, window: 4))
        #expect(a.apply(.screenEdge(.right), window: 1, wholeTile: true))
        #expect(a.root.description == "H(2 D [1,3])")
    }
}

@Suite("Commands by URL (SPEC §11)")
struct CommandTests {
    @Test("Every documented URL parses", arguments: [
        ("canopy://everything", Command.everything),
        ("canopy://zoom-out", .stepOut),
        ("canopy://zoom-in", .stepIn),
        ("canopy://back", .back),
        ("canopy://forward", .forward),
        ("canopy://neighbour?dir=left", .neighbour(.left)),
        ("canopy://neighbor?dir=DOWN", .neighbour(.down)),
        ("canopy://frame-window", .frameWindow),
        ("canopy://frame-desktop", .frameDesktop),
        ("canopy://toggle-desktop", .toggleDesktop),
        ("canopy://view?n=1", .goToView(1)),
        ("canopy://save-view?n=9", .saveView(9)),
        ("canopy://split-finder", .splitFinder),
        ("canopy://arrange", .arrange),
        ("canopy://float-all", .floatAll),
        ("canopy://settings?pane=shortcuts", .settings(.shortcuts)),
        ("canopy://settings", .settings(nil)),
        ("canopy://settings?pane=nope", .settings(nil)),
        ("CANOPY://Everything", .everything),
        ("canopy:everything", .everything),
    ])
    func parse(_ raw: String, _ expected: Command) {
        #expect(Command(url: URL(string: raw)!) == expected)
    }

    @Test("Invalid URLs are ignored", arguments: [
        "canopy://view", "canopy://view?n=0", "canopy://view?n=10", "canopy://view?n=two",
        "canopy://save-view?n=-1", "canopy://neighbour", "canopy://neighbour?dir=diagonal",
        "canopy://launch-missiles", "other://everything", "canopy://",
    ])
    func invalid(_ raw: String) {
        #expect(Command(url: URL(string: raw)!) == nil)
    }

    @Test("Every command's URL round-trips")
    func roundTrip() {
        var all: [Command] = [.everything, .stepOut, .stepIn, .back, .forward, .frameWindow, .frameDesktop,
                              .toggleDesktop, .splitFinder, .arrange, .floatAll, .settings(nil)]
        all += Direction.allCases.map(Command.neighbour)
        all += (1...9).flatMap { [Command.goToView($0), .saveView($0)] }
        all += SettingsPane.allCases.map { Command.settings($0) }
        for c in all { #expect(Command(url: c.url) == c, "\(c.url)") }
    }
}
