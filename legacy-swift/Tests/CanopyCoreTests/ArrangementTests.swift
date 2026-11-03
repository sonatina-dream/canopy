import Foundation
import Testing
@testable import CanopyCore

private func near(_ a: Double, _ b: Double, _ eps: Double = 1e-9) -> Bool { abs(a - b) < eps }
private func near(_ a: [Double], _ b: [Double]) -> Bool { a.count == b.count && zip(a, b).allSatisfy { near($0, $1) } }

@Suite("Docking (SPEC §4)")
struct DockingTests {
    @Test("A fresh world is just the Desktop")
    func freshWorld() {
        let a = Arrangement()
        #expect(a.root.isDesktop)
        #expect(a.isEmpty)
    }

    @Test("Docking at the left screen edge splits the screen with the Desktop")
    func firstDock() {
        let a = Arrangement()
        a.dockAtScreenEdge(1, edge: .left)
        #expect(a.root.description == "H(1 D)")
        #expect(near(a.root.weights, [0.5, 0.5]))
    }

    @Test("Docking beside a window in the same axis inserts a sibling and neighbours make room")
    func siblingInsert() {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        a.dock(2, beside: t1, edge: .right)
        #expect(a.root.description == "H(1 2 D)")
        #expect(near(a.root.weights, [1.0 / 3, 1.0 / 3, 1.0 / 3]))
    }

    @Test("Docking on a cross-axis edge nests a new split in place of the target")
    func nesting() {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        a.dock(2, beside: t1, edge: .bottom)
        #expect(a.root.description == "H(V(1 2) D)")
        let v = a.root.children[0]
        #expect(near(v.weights, [0.5, 0.5]))
        #expect(near(a.root.weights, [0.5, 0.5]), "the nested split keeps the target's share")
    }

    @Test("Nesting goes as deep as you like")
    func deepNesting() {
        let a = Arrangement()
        var t = a.dockAtScreenEdge(1, edge: .left)
        for w: WindowID in 2...12 {
            t = a.dock(w, beside: t, edge: w % 2 == 0 ? .bottom : .right)
        }
        #expect(a.windows.count == 12)
        #expect(a.tile(of: 12)!.depth == 12)
    }

    @Test("Docking at a screen edge goes beside everything")
    func besideEverything() {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        a.dock(2, beside: t1, edge: .bottom)
        a.dockAtScreenEdge(3, edge: .top)
        #expect(a.root.description == "V(3 H(V(1 2) D))")
        a.dockAtScreenEdge(4, edge: .bottom)
        #expect(a.root.description == "V(3 H(V(1 2) D) 4)")
        #expect(near(a.root.weights.reduce(0, +), 1))
    }

    @Test("Docking an already docked window moves it instead of duplicating it")
    func redockMoves() {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        a.dock(2, beside: t1, edge: .right)
        a.dock(1, beside: a.tile(of: 2)!, edge: .bottom)
        #expect(a.windows.sorted() == [1, 2])
        #expect(a.root.description == "H(V(2 1) D)")
    }

    @Test("There is no limit on docked windows")
    func unlimited() {
        let a = Arrangement()
        for w: WindowID in 1...40 { a.autoDock(w, rects: Solver.solve(a.root, in: Rect(x: 0, y: 0, width: 4000, height: 3000), gap: 0).nodeRects) }
        #expect(a.windows.count == 40)
    }
}

@Suite("Removing and restoring (SPEC §4)")
struct RemovalTests {
    @Test("Closing a window exactly undoes adding it when space is shared")
    func closeUndoesAdd() {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        let t2 = a.dock(2, beside: t1, edge: .right)
        a.resizeSeam(a.root, index: 0, delta: 100, available: 1000)
        let before = a.root.weights
        a.dock(3, beside: t2, edge: .right)
        a.remove(3)
        #expect(a.root.description == "H(1 2 D)")
        #expect(near(a.root.weights, before))
    }

    @Test("Without sharing, the next neighbour takes all the space")
    func neighbourTakesAll() {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        a.dock(2, beside: t1, edge: .right)
        // H(1 2 D) with thirds; remove 1 -> 2 gets 2/3.
        a.remove(1, shareSpace: false)
        #expect(a.root.description == "H(2 D)")
        #expect(near(a.root.weights, [2.0 / 3, 1.0 / 3]))
    }

    @Test("The last child's space goes to the previous neighbour without sharing")
    func lastChildGivesToPrevious() {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        let t2 = a.dock(2, beside: t1, edge: .bottom)
        a.dock(3, beside: t2, edge: .bottom)
        a.remove(3, shareSpace: false)
        #expect(near(a.root.children[0].weights, [1.0 / 3, 2.0 / 3]))
    }

    @Test("A split left with one child collapses")
    func collapse() {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        a.dock(2, beside: t1, edge: .bottom)
        a.remove(2)
        #expect(a.root.description == "H(1 D)")
        a.remove(1)
        #expect(a.root.description == "D")
    }

    @Test("Minimise and restore returns the window to its spot")
    func minimiseRestore() throws {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        let t2 = a.dock(2, beside: t1, edge: .bottom)
        a.dock(3, beside: t2, edge: .right)
        let shape = a.root.description
        let weights = a.root.allNodes.map(\.weights)
        let target = try #require(a.remove(3))
        #expect(a.root.description != shape)
        a.restore(3, to: target)
        #expect(a.root.description == shape)
        #expect(zip(a.root.allNodes.map(\.weights), weights).allSatisfy { near($0, $1) })
    }

    @Test("Restoring the first child of a split puts it back first")
    func restoreFirst() throws {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        a.dock(2, beside: t1, edge: .right)
        let target = try #require(a.remove(1))
        a.restore(1, to: target)
        #expect(a.root.description == "H(1 2 D)")
    }

    @Test("Restore falls back to the neighbour's window when its node is gone")
    func restoreFallback() throws {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        let t2 = a.dock(2, beside: t1, edge: .bottom)
        a.dock(4, beside: t2, edge: .right)            // H(V(1 H(2 4)) D)
        let target = try #require(a.remove(1))          // beside the H(2 4) node, top
        a.remove(4)                                     // that node collapses away
        #expect(a.restore(1, to: target) != nil)
        #expect(a.root.description == "H(V(1 2) D)")
    }

    @Test("Retaining only live windows drops the rest")
    func retain() {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        a.dock(2, beside: t1, edge: .right)
        a.dock(3, beside: t1, edge: .bottom)
        let gone = a.retain(only: [2])
        #expect(Set(gone) == [1, 3])
        #expect(a.root.description == "H(2 D)")
    }
}

@Suite("Tab groups (SPEC §7)")
struct TabTests {
    @Test("Adding a tab selects it; windows from any app share one tile")
    func addTab() {
        let a = Arrangement()
        let t = a.dockAtScreenEdge(1, edge: .left)
        a.addTab(2, to: t)
        #expect(t.windows == [1, 2])
        #expect(t.selectedWindow == 2)
        #expect(a.root.description == "H([1,2] D)")
    }

    @Test("Removing the selected tab selects the next one, or the previous when last")
    func removeSelected() {
        let a = Arrangement()
        let t = a.dockAtScreenEdge(1, edge: .left)
        a.addTab(2, to: t)
        a.addTab(3, to: t)
        a.select(2)
        a.remove(2)
        #expect(t.selectedWindow == 3)
        a.remove(3)
        #expect(t.selectedWindow == 1)
        #expect(a.root.description == "H(1 D)")
    }

    @Test("Removing an unselected tab keeps the selection")
    func removeUnselected() {
        let a = Arrangement()
        let t = a.dockAtScreenEdge(1, edge: .left)
        a.addTab(2, to: t)
        a.addTab(3, to: t)
        a.remove(1)
        #expect(t.selectedWindow == 3)
    }

    @Test("Tearing a tab off and restoring it puts it back at the same index")
    func tabRestore() throws {
        let a = Arrangement()
        let t = a.dockAtScreenEdge(1, edge: .left)
        a.addTab(2, to: t)
        a.addTab(3, to: t)
        let target = try #require(a.remove(2))
        #expect(target == .tab(in: 3, index: 1))
        a.restore(2, to: target)
        #expect(t.windows == [1, 2, 3])
    }

    @Test("Moving a whole tab group keeps its tabs together")
    func moveGroup() {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        a.addTab(2, to: t1)
        let t3 = a.dock(3, beside: t1, edge: .right)
        a.move(t1, beside: t3, edge: .bottom)
        #expect(a.root.description == "H(V(3 [1,2]) D)")
    }

    @Test("Merging one group into another makes one strip")
    func merge() {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        a.addTab(2, to: t1)
        let t3 = a.dock(3, beside: t1, edge: .right)
        a.merge(t1, into: t3)
        #expect(t3.windows == [3, 1, 2])
        #expect(t3.selectedWindow == 2)
        #expect(a.root.description == "H([3,1,2] D)")
    }
}

@Suite("Seams (SPEC §3)")
struct SeamTests {
    @Test("Dragging a seam resizes both neighbours and nothing else")
    func resize() {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        a.dock(2, beside: t1, edge: .right)
        a.resizeSeam(a.root, index: 0, delta: 100, available: 900)
        #expect(near(a.root.weights[0], 1.0 / 3 + 100.0 / 900))
        #expect(near(a.root.weights[1], 1.0 / 3 - 100.0 / 900))
        #expect(near(a.root.weights[2], 1.0 / 3))
    }

    @Test("A seam stops where a window would refuse to shrink")
    func stopsAtWindowMinimum() {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        a.dock(2, beside: t1, edge: .right)          // H(1 2 D), thirds of 900 = 300 each
        // Window 2 can't go below 250 wide: dragging 200 right stops 50 in.
        a.resizeSeam(a.root, index: 0, delta: 200, available: 900, minAfter: 250)
        #expect(near(a.root.weights[1] * 900, 250))
        // A side that's already too small isn't squeezed further, but can grow.
        a.resizeSeam(a.root, index: 0, delta: 30, available: 900, minAfter: 400)
        #expect(near(a.root.weights[1] * 900, 250))
        a.resizeSeam(a.root, index: 0, delta: -30, available: 900, minAfter: 400)
        #expect(near(a.root.weights[1] * 900, 280))
    }

    @Test("A subtree's minimum length adds along its axis and takes the largest across")
    func minimumLength() {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        a.dock(2, beside: t1, edge: .bottom)         // H(V(1 2) D)
        let mins: [WindowID: Size] = [1: Size(width: 400, height: 300), 2: Size(width: 466, height: 252)]
        let v = a.root.children[0]
        #expect(v.minimumLength(along: .horizontal, gap: 12) { mins[$0] } == 466)
        #expect(v.minimumLength(along: .vertical, gap: 12) { mins[$0] } == 300 + 12 + 252)
    }

    @Test("A seam can't squeeze a neighbour below its minimum")
    func clamp() {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        a.dock(2, beside: t1, edge: .right)
        a.resizeSeam(a.root, index: 0, delta: 10_000, available: 900)
        #expect(near(a.root.weights[1], 60.0 / 900))
    }
}

@Suite("Persistence (SPEC §8)")
struct PersistenceTests {
    @Test("An arrangement survives a JSON round trip")
    func roundTrip() throws {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        a.addTab(5, to: t1)
        a.dock(2, beside: t1, edge: .bottom)
        a.setDesktopHidden(true)
        let nav = Navigator()
        nav.frame(t1.parent!, in: a)
        nav.save(3, in: a)
        let state = SavedState(displays: ["UUID-1": DisplayState(arrangement: a, navigator: nav, windows: [
            1: WindowRef(bundleID: "a", title: "one"), 2: WindowRef(bundleID: "b", title: "two"),
            5: WindowRef(bundleID: "c", title: "five"),
        ])])
        let decoded = try SavedState.decode(try state.encoded())
        let d = try #require(decoded.displays["UUID-1"])
        #expect(d.arrangement.root.description == a.root.description)
        #expect(d.arrangement.desktop.hidden)
        #expect(d.navigator.framedNode(in: d.arrangement).description == "V([1,5] 2)")
        #expect(d.navigator.savedViews[3] != nil)
        // New nodes get fresh ids after decoding.
        let t = d.arrangement.dockAtScreenEdge(9, edge: .right)
        #expect(Set(d.arrangement.root.allNodes.map(\.id)).count == d.arrangement.root.allNodes.count)
        #expect(t.id > a.root.allNodes.map(\.id).max()!)
    }

    @Test("Windows are matched by id, then app and title, then the app's only window")
    func matching() {
        let saved: [WindowID: WindowRef] = [
            10: WindowRef(bundleID: "term", title: "zsh"),
            11: WindowRef(bundleID: "safari", title: "News"),
            12: WindowRef(bundleID: "notes", title: "Old title"),
            13: WindowRef(bundleID: "mail", title: "Inbox"),
        ]
        let live: [WindowID: WindowRef] = [
            10: WindowRef(bundleID: "term", title: "zsh — 80×24"),   // same id, same app
            90: WindowRef(bundleID: "safari", title: "News"),        // same app + title
            91: WindowRef(bundleID: "notes", title: "New title"),    // only notes window
            92: WindowRef(bundleID: "maps", title: "Inbox"),         // different app: no match
        ]
        let m = WindowMatcher.match(saved: saved, live: live)
        #expect(m == [10: 10, 11: 90, 12: 91])
    }

    @Test("Reattaching drops unmatched windows without id collisions")
    func reattach() {
        let a = Arrangement()
        let t1 = a.dockAtScreenEdge(1, edge: .left)
        a.dock(2, beside: t1, edge: .right)
        let state = DisplayState(arrangement: a, navigator: Navigator(), windows: [
            1: WindowRef(bundleID: "x", title: "X"),
            2: WindowRef(bundleID: "y", title: "Y"),
        ])
        // Window 1 is gone; window "x" now has id 2 and "y" has id 7.
        state.reattach(to: [2: WindowRef(bundleID: "x", title: "X"), 7: WindowRef(bundleID: "y", title: "Y")])
        #expect(state.arrangement.root.description == "H(2 7 D)")
    }

    @Test("Preferences fill in defaults for missing keys")
    func preferencesDefaults() throws {
        let p = try JSONDecoder().decode(Preferences.self, from: Data(#"{"gap": 20}"#.utf8))
        #expect(p.gap == 20)
        #expect(p.shareRemovedSpace)
        #expect(p.animationDuration == 0.46)
    }

    @Test("Preferences saved before version 2 get the shorter tab pause")
    func tabHoldMigration() throws {
        let old = try JSONDecoder().decode(Preferences.self, from: Data(#"{"tabHold": 0.5}"#.utf8))
        #expect(old.tabHold == 0.3 && old.version == Preferences.currentVersion)
        let chosen = try JSONDecoder().decode(Preferences.self, from: Data(#"{"tabHold": 0.8}"#.utf8))
        #expect(chosen.tabHold == 0.8)
        let current = try JSONDecoder().decode(Preferences.self, from: Data(#"{"version": 2, "tabHold": 0.5}"#.utf8))
        #expect(current.tabHold == 0.5)
    }
}
