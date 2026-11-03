import AppKit
import CanopyCore

// MARK: - The input thread

/// What the input thread may read. The main thread publishes a new copy
/// whenever it changes.
struct InputState {
    var started = false
    var rectZoom = true
    var greenButton = true
    var titleDoubleClick = true
    var optionPinch = true
    var managed = Set<WindowID>()
    var docked = Set<WindowID>()
    /// A stage is up after a change; a press ends it.
    var landing = false
}

/// Decides, on the event tap's own thread, which events Canopy takes. It only
/// asks the window server (fast) and, for clicks in a title bar's corner or a
/// double-click, Accessibility at that point. Everything else is handed to
/// the main thread without waiting, so the pointer never stalls.
final class InputRouter {
    weak var coordinator: Coordinator?
    private let lock = NSLock()
    private var state = InputState()

    // Input thread only.
    private var marqueeStart: CanopyCore.Point?
    private var swallowUp = false
    private var pinch = 0.0
    private var pinching = false
    private var tracking = false
    private var latestPinch: (total: Double, at: CanopyCore.Point)?
    private var pinchScheduled = false

    // Drag coalescing: the main thread takes the newest point.
    private var latestDrag: CanopyCore.Point?
    private var dragScheduled = false

    func publish(_ s: InputState) {
        lock.lock()
        state = s
        lock.unlock()
    }

    private var snapshot: InputState {
        lock.lock()
        defer { lock.unlock() }
        return state
    }

    private func main(_ body: @escaping (Coordinator) -> Void) {
        DispatchQueue.main.async { [weak self] in
            if let c = self?.coordinator { body(c) }
        }
    }

    func handle(_ type: CGEventType, _ e: CGEvent) -> Bool {
        let s = snapshot
        guard s.started else { return false }
        let raw = type.rawValue
        if raw == EventTap.magnifyType { return handlePinch(e, s) }
        if raw == EventTap.gestureType { return false }
        let p = CanopyCore.Point(e.location)
        switch type {
        case .keyDown:
            if marqueeStart != nil, e.getIntegerValueField(.keyboardEventKeycode) == 53 {
                marqueeStart = nil
                main { $0.marqueeCancelled() }
                return true
            }
            return false
        case .leftMouseDown: return down(p, e, s)
        case .leftMouseDragged: return dragged(p)
        case .leftMouseUp: return up(p)
        default: return false
        }
    }

    private func down(_ p: CanopyCore.Point, _ e: CGEvent, _ s: InputState) -> Bool {
        if s.landing { main { $0.stage.interrupt() } }
        let flags = e.flags
        let option = flags.contains(.maskAlternate)
        if s.rectZoom, flags.contains(.maskCommand), flags.contains(.maskShift), !option, !flags.contains(.maskControl) {
            marqueeStart = p
            main { $0.marqueeBegan(p) }
            return true
        }
        let clicks = e.getIntegerValueField(.mouseEventClickState)
        let top = Hit.window(at: p)

        // The green button docks and floats; ⌥ and fn keep their usual meaning.
        if s.greenButton, clicks == 1, !option, !flags.contains(.maskSecondaryFn), let (id, frame) = top,
           s.managed.contains(id), p.x - frame.minX < 90, p.y - frame.minY < 40, Hit.isFullScreenButton(at: p) {
            swallowUp = true
            main { $0.toggleDocked(id) }
            return true
        }
        if clicks == 2, !option {
            // A docked window's title bar: that window on its own, and back.
            if s.titleDoubleClick, let (id, frame) = top, s.docked.contains(id), p.y - frame.minY < 32, !Hit.isControl(at: p) {
                swallowUp = true
                main { $0.titleDoubleClicked(id) }
                return true
            }
            // The bare Desktop: the Desktop on its own, and back.
            if top == nil, Hit.isBareDesktop(at: p) {
                main { $0.desktopDoubleClicked(p) }
                return false
            }
        }
        if clicks == 1, let (id, frame) = top, s.managed.contains(id) {
            tracking = true
            main { $0.pressBegan(id, frame: CanopyCore.Rect(frame), at: p) }
        }
        return false
    }

    private func dragged(_ p: CanopyCore.Point) -> Bool {
        if let start = marqueeStart {
            main { $0.marqueeMoved(start, p) }
            return true
        }
        guard tracking else { return false }
        lock.lock()
        latestDrag = p
        let schedule = !dragScheduled
        dragScheduled = true
        lock.unlock()
        if schedule {
            main { [weak self] c in
                guard let self else { return }
                self.lock.lock()
                let q = self.latestDrag
                self.latestDrag = nil
                self.dragScheduled = false
                self.lock.unlock()
                if let q { c.pressDragged(q) }
            }
        }
        return false
    }

    private func up(_ p: CanopyCore.Point) -> Bool {
        if swallowUp {
            swallowUp = false
            return true
        }
        if let start = marqueeStart {
            marqueeStart = nil
            main { $0.marqueeEnded(start, p) }
            return true
        }
        if tracking {
            tracking = false
            main { $0.pressEnded(p) }
        }
        return false
    }

    /// ⌥-pinch drives the camera continuously; the main thread gets the
    /// newest total, coalesced, and a final call when the fingers lift.
    private func handlePinch(_ e: CGEvent, _ s: InputState) -> Bool {
        guard let ns = NSEvent(cgEvent: e) else { return false }
        let ending = ns.phase == .ended || ns.phase == .cancelled
        guard s.optionPinch, e.flags.contains(.maskAlternate) || (pinching && ending) else {
            if pinching { finishPinch(CanopyCore.Point(e.location)) }
            return false
        }
        let p = CanopyCore.Point(e.location)
        if ns.phase == .began || !pinching {
            pinching = true
            pinch = 0
        }
        if ending {
            finishPinch(p)
            return true
        }
        pinch += Double(ns.magnification)
        lock.lock()
        latestPinch = (pinch, p)
        let schedule = !pinchScheduled
        pinchScheduled = true
        lock.unlock()
        if schedule {
            main { [weak self] c in
                guard let self else { return }
                self.lock.lock()
                let q = self.latestPinch
                self.pinchScheduled = false
                self.lock.unlock()
                if let q { c.pinch(total: q.total, at: q.at, ended: false) }
            }
        }
        return true
    }

    private func finishPinch(_ p: CanopyCore.Point) {
        let total = pinch
        pinching = false
        pinch = 0
        main { $0.pinch(total: total, at: p, ended: true) }
    }
}

/// Hit testing that is safe on any thread.
enum Hit {
    /// The frontmost ordinary window under the pointer that isn't Canopy's.
    static func window(at p: CanopyCore.Point) -> (WindowID, CGRect)? {
        let own = ProcessInfo.processInfo.processIdentifier
        for o in WindowServer.onScreen() where o.pid != own && o.frame.contains(p) {
            return (o.id, o.frame.cg)
        }
        return nil
    }

    static func isFullScreenButton(at p: CanopyCore.Point) -> Bool {
        guard let el = AX.element(at: p.cg) else { return false }
        return AX.string(el, kAXSubroleAttribute) == "AXFullScreenButton"
    }

    static func isControl(at p: CanopyCore.Point) -> Bool {
        guard let el = AX.element(at: p.cg), let role = AX.string(el, kAXRoleAttribute) else { return false }
        let controls: Set<String> = ["AXButton", "AXTextField", "AXPopUpButton", "AXMenuButton", "AXCheckBox",
                                     "AXRadioButton", "AXSlider", "AXComboBox", "AXLink", "AXSegmentedControl",
                                     "AXTabGroup", "AXDisclosureTriangle", "AXIncrementor", "AXTextArea"]
        return controls.contains(role)
    }

    static func isBareDesktop(at p: CanopyCore.Point) -> Bool {
        guard let el = AX.element(at: p.cg) else { return true }
        let role = AX.string(el, kAXRoleAttribute) ?? ""
        return !["AXImage", "AXTextField", "AXStaticText", "AXButton", "AXMenuBarItem"].contains(role)
    }
}

// MARK: - Presses and drags (main thread)

/// A left-button press on a managed window, which may become a move or a resize.
struct Press {
    let window: WindowID
    let start: CanopyCore.Point
    let startFrame: CanopyCore.Rect
    var moving = false
    var resizing = false
    var session: DragSession?
}

/// A window being dragged: its picture follows the pointer on the stage, and
/// the other windows make room live for wherever it would land.
struct DragSession {
    let display: Display
    let grab: CanopyCore.Point
    let size: CanopyCore.Size
    /// Where the other windows are on the stage when nothing would change.
    var baseline: [WindowID: CanopyCore.Rect]
    var stageUp: Bool
    /// Over a target, the dragged window shrinks to a thumbnail under the
    /// pointer, so the target stays in view.
    var thumbnail = false
    var target: DropTarget = .none
    var targetWorld: String?
    var holdTile: NodeID?
    var holdStart: CFAbsoluteTime = 0
    var held: NodeID?
    var pointer: CanopyCore.Point
}

/// A pinch in progress.
struct ZoomSession {
    let world: String
    let anchor: CanopyCore.Point
    var scale: Double
}

struct TabDrag {
    let window: WindowID
    let world: String
    var target: DropTarget = .none
    var targetWorld: String?
    var holdTile: NodeID?
    var holdStart: CFAbsoluteTime = 0
    var held: NodeID?
}

extension Coordinator {
    func pressBegan(_ id: WindowID, frame: CanopyCore.Rect, at p: CanopyCore.Point) {
        press = Press(window: id, start: p, startFrame: frame)
    }

    func pressDragged(_ p: CanopyCore.Point) {
        guard var pr = press else { return }
        if !pr.moving && !pr.resizing {
            guard let f = WindowServer.frame(of: pr.window) else { return }
            if abs(f.width - pr.startFrame.width) > 1 || abs(f.height - pr.startFrame.height) > 1 {
                pr.resizing = true
            } else if abs(f.x - pr.startFrame.x) > 2 || abs(f.y - pr.startFrame.y) > 2 {
                pr.moving = true
                Log.add("DRAG w\(pr.window) begins")
                pr.session = beginSession(pr.window, frame: f, pointer: p)
            }
        }
        if pr.moving, var s = pr.session {
            updateSession(&s, window: pr.window, pointer: p)
            pr.session = s
        }
        press = pr
    }

    func pressEnded(_ p: CanopyCore.Point) {
        defer { processMovedWhileMouseDown() }
        guard var pr = press else { return }
        press = nil
        holdTimer?.invalidate()
        if pr.moving, var s = pr.session {
            updateSession(&s, window: pr.window, pointer: p)
            pr.session = s
            finishSession(s, window: pr.window)
        } else if pr.resizing, let world = world(of: pr.window) {
            adoptResize(pr.window, in: world)
        }
        movedWhileMouseDown.remove(pr.window)
    }

    private func beginSession(_ id: WindowID, frame f: CanopyCore.Rect, pointer p: CanopyCore.Point) -> DragSession? {
        refreshFrames()
        guard let world = world(at: p) ?? world(of: id) else { return nil }
        var session = DragSession(display: world.display, grab: CanopyCore.Point(x: p.x - f.x, y: p.y - f.y),
                                  size: f.size, baseline: [:], stageUp: false, pointer: p)
        guard prefs.animate else { return session }
        let t0 = CFAbsoluteTimeGetCurrent()
        var included = Set<WindowID>()
        var items = onScreenItems(on: world.display, excluding: &included)
        let t1 = CFAbsoluteTimeGetCurrent()
        if let i = items.firstIndex(where: { $0.id == id }) {
            items[i].rect = f
        } else if let item = stageItem(id, at: f) {
            items.append(item)
        }
        let panel = stage.panel(for: world.display)
        let t2 = CFAbsoluteTimeGetCurrent()
        panel.show(items, desk: world.plan?.desktopRect)
        let t3 = CFAbsoluteTimeGetCurrent()
        panel.bringToFront(id)
        for item in items where item.id != id { session.baseline[item.id] = item.rect }
        session.stageUp = true
        pictures.refresh(items.map(\.id))
        publishInput()
        let t4 = CFAbsoluteTimeGetCurrent()
        Log.add(String(format: "DRAG stage up in %.0f ms (pictures %.0f, show %.0f; %d windows)",
                       (t4 - t0) * 1000, (t1 - t0) * 1000, (t3 - t2) * 1000, items.count))
        return session
    }

    private func updateSession(_ s: inout DragSession, window: WindowID, pointer p: CanopyCore.Point) {
        s.pointer = p
        let r = resolveDrop(window: window, pointer: p, excludeOwnTile: true,
                            holdTile: &s.holdTile, holdStart: &s.holdStart, held: &s.held)
        let changed = r.target != s.target || r.world?.uuid != s.targetWorld
        s.target = r.target
        s.targetWorld = r.world?.uuid
        if s.stageUp {
            let panel = stage.panel(for: s.display)
            let overTarget: Bool
            switch r.target {
            case .none, .float: overTarget = false
            default: overTarget = true
            }
            let rect = overTarget ? thumbnailRect(for: window, size: s.size, pointer: p, panel: panel)
                : CanopyCore.Rect(x: p.x - s.grab.x, y: p.y - s.grab.y, width: s.size.width, height: s.size.height)
            if overTarget != s.thumbnail {
                s.thumbnail = overTarget
                panel.reshape(window, to: rect)
            } else {
                panel.move(window, to: rect)
            }
        }
        if changed { showTarget(s, window: window, in: r.world) }
    }

    /// A small copy of the window with its title bar under the pointer.
    private func thumbnailRect(for id: WindowID, size: CanopyCore.Size, pointer p: CanopyCore.Point,
                               panel: StagePanel) -> CanopyCore.Rect {
        let picture = panel.pictureSize(id) ?? CGSize(width: size.width, height: size.height)
        let scale = min(240 / max(1, picture.width), 170 / max(1, picture.height), 1)
        let w = picture.width * scale, h = picture.height * scale
        return CanopyCore.Rect(x: p.x - w / 2, y: p.y - 12, width: w, height: h)
    }

    /// Shows what the drop would do: on the stage, the other windows make
    /// room and a slot marks the landing place; without it, a highlight.
    private func showTarget(_ s: DragSession, window: WindowID, in world: World?) {
        guard s.stageUp else {
            showPreview(s.target, window: window, in: world, wholeTile: self.world(of: window) != nil)
            return
        }
        let panel = stage.panel(for: s.display)
        guard let world, world.display.uuid == s.display.uuid, let sim = simulate(s.target, window: window, in: world, wholeTile: true) else {
            panel.setSlot(nil)
            panel.animate(s.baseline, duration: 0.25)
            return
        }
        var targets = s.baseline
        for tile in sim.arrangement.tiles {
            guard let sel = tile.selectedWindow, sel != window, targets[sel] != nil else { continue }
            if let r = sim.plan.visible[sel] ?? sim.plan.covers[tile.id] ?? sim.plan.projected[tile.id] { targets[sel] = r }
        }
        panel.animate(targets, duration: 0.25)
        if case .tab = s.target { panel.setSlot(sim.slot, tab: true) } else { panel.setSlot(sim.slot) }
    }

    /// The arrangement and plan a drop would produce, worked out on a copy.
    private func simulate(_ target: DropTarget, window: WindowID, in world: World, wholeTile: Bool)
        -> (arrangement: Arrangement, plan: Plan, slot: CanopyCore.Rect)? {
        switch target {
        case .none, .float: return nil
        default: break
        }
        let a = world.arrangement.clone()
        let nav = world.navigator.copy()
        let whole = wholeTile && a.contains(window)
        guard a.apply(target, window: window, wholeTile: whole), let tile = a.tile(of: window) else { return nil }
        nav.reveal(tile, in: a)
        let plan = Planner.plan(a, nav, visibleFrame: world.display.visible, gap: prefs.gap) { [weak self] in
            self?.minimum(for: $0)
        }
        guard let slot = plan.layout.nodeRects[tile.id] else { return nil }
        return (a, plan, slot)
    }

    private func finishSession(_ s: DragSession, window: WindowID) {
        preview.orderOut(nil)
        let target = s.target
        let targetWorld = s.targetWorld.flatMap { worlds[$0] } ?? world(of: window) ?? world(at: s.pointer)
        let source = world(of: window)
        if s.stageUp, source == nil, target == .none || target == .float {
            // A floating window lands where it was let go; the real one is already there.
            let panel = stage.panel(for: s.display)
            if let f = WindowServer.frame(of: window) {
                tracker.windows[window]?.frame = f
                panel.animate([window: f], duration: 0.1)
            }
            panel.setSlot(nil)
            panel.animate(s.baseline, duration: 0.1)
            panel.land(after: 0.1, writer: writer, redraw: 0.02)
            return
        }
        guard let targetWorld else { stage.panel(for: s.display).fade(); return }
        drop(window, target, into: targetWorld, wholeTile: true)
        let panel = stage.panel(for: s.display)
        if panel.isShown, targetWorld.display.uuid != s.display.uuid, source?.display.uuid != s.display.uuid {
            panel.fade()
        }
    }

    // MARK: Dropping

    func drop(_ id: WindowID, _ target: DropTarget, into world: World, wholeTile: Bool) {
        let source = self.world(of: id)
        Log.add("DROP w\(id) \(target) from \(source == nil ? "floating" : "docked")")
        switch target {
        case .none:
            if let s = source { applied[id] = nil; layout(s, animated: true, dragged: id) }
        case .float:
            guard let s = source else { return }
            let restore = s.arrangement.remove(id, shareSpace: prefs.shareRemovedSpace)
            floatRestore[id] = (s.uuid, restore ?? .floating)
            applied[id] = nil
            s.navigator.prune(liveWindows: Set(s.arrangement.windows))
            layout(s, animated: true, dragged: id)
        default:
            if source == nil, let f = tracker.windows[id]?.frame { floatFrames[id] = f }
            if let s = source, s !== world {
                s.arrangement.remove(id, shareSpace: prefs.shareRemovedSpace)
                layout(s, animated: true)
            }
            let whole = wholeTile && source === world
            guard world.arrangement.apply(target, window: id, wholeTile: whole) else {
                if let s = source { layout(s, animated: true, dragged: id) }
                return
            }
            applied[id] = nil
            reveal(id, in: world)
            layout(world, animated: true, confineFloating: true, dragged: id)
        }
    }

    /// Resolves where a drop at `pointer` would go, tracking the centre hold
    /// that turns a drop into a tab.
    func resolveDrop(window: WindowID, pointer p: CanopyCore.Point, excludeOwnTile: Bool,
                     holdTile: inout NodeID?, holdStart: inout CFAbsoluteTime,
                     held: inout NodeID?) -> (target: DropTarget, world: World?) {
        guard let world = world(at: p), let plan = world.plan else { return (.none, nil) }
        let own = world.arrangement.tile(of: window)
        let ownIsSingle = (own?.windows.count ?? 0) <= 1
        var tiles: [(NodeID, CanopyCore.Rect)] = []
        for t in world.arrangement.tiles {
            guard let r = plan.covers[t.id] ?? plan.layout.nodeRects[t.id] else { continue }
            if t === own && (excludeOwnTile || ownIsSingle) { continue }
            tiles.append((t.id, r))
        }
        let desktop = plan.desktopRect.map { (world.arrangement.desktop.id, $0) }
        let now = CFAbsoluteTimeGetCurrent()
        let centre = tiles.first { $0.1.contains(p) && resolver.zone(of: p, in: $0.1) == .centre }?.0
        if centre != holdTile {
            holdTile = centre
            holdStart = now
            held = nil
            if centre != nil {
                holdTimer?.invalidate()
                holdTimer = Timer.scheduledTimer(withTimeInterval: prefs.tabHold + 0.02, repeats: false) { [weak self] _ in
                    guard let self else { return }
                    if self.press?.session != nil {
                        self.pressDragged(Coordinates.mouse)
                    } else if self.tabDrag != nil {
                        self.updateTabDrag()
                    }
                }
            }
        } else if let c = centre, now - holdStart >= prefs.tabHold {
            held = c
        }
        let target = resolver.resolve(pointer: p, screen: world.display.visible, display: world.display.frame,
                                      tiles: tiles, desktop: desktop, held: held)
        return (target, world)
    }

    /// The plain highlight, for tab drags and when motion is off.
    func showPreview(_ target: DropTarget, window: WindowID, in world: World?, wholeTile: Bool) {
        guard let world, let sim = simulate(target, window: window, in: world, wholeTile: wholeTile) else {
            preview.orderOut(nil)
            return
        }
        if case .tab = target { preview.show(sim.slot, tab: true) } else { preview.show(sim.slot, tab: false) }
    }

    /// The user dragged a docked window's edge: move the matching seam.
    func adoptResize(_ id: WindowID, in world: World) {
        guard let f = WindowServer.frame(of: id), let old = applied[id], let plan = world.plan,
              let tile = world.arrangement.tile(of: id) else {
            layout(world, animated: false)
            return
        }
        tracker.windows[id]?.frame = f
        for edge in Edge.allCases {
            let delta: Double
            switch edge {
            case .left: delta = f.minX - old.minX
            case .right: delta = f.maxX - old.maxX
            case .top: delta = f.minY - old.minY
            case .bottom: delta = f.maxY - old.maxY
            }
            guard abs(delta) > 2 else { continue }
            var c: Node = tile
            seamSearch: while let p = c.parent {
                if p.axis == edge.axis, let i = c.indexInParent {
                    let visible = p.children.indices.filter { !(p.children[$0].isDesktop && p.children[$0].hidden) }
                    if let k = visible.firstIndex(of: i) {
                        let j: Int? = edge.isLeading ? (k > 0 ? visible[k - 1] : nil) : (k + 1 < visible.count ? visible[k + 1] : nil)
                        if let j, let seam = plan.layout.seams.first(where: { $0.split == p.id && $0.index == min(i, j) && $0.next == max(i, j) }) {
                            world.arrangement.resizeSeam(p, index: seam.index, next: seam.next, delta: delta, available: seam.available)
                            break seamSearch
                        }
                    }
                }
                c = p
            }
        }
        applied[id] = nil
        layout(world, animated: false)
    }

    // MARK: Tab drags

    func beginTabDrag(_ id: WindowID, world: World) {
        tabDrag = TabDrag(window: id, world: world.uuid)
        if let tw = tracker.windows[id] {
            let image = pictures.images[id].map { NSImage(cgImage: $0, size: .zero) } ?? AppLook.icon(for: tw.bundleID, pid: tw.pid)
            ghost.show(image: image, title: tw.title.isEmpty ? tw.appName : tw.title, at: Coordinates.mouse)
        }
    }

    func updateTabDrag() {
        guard var t = tabDrag else { return }
        let p = Coordinates.mouse
        ghost.move(to: p)
        let r = resolveDrop(window: t.window, pointer: p, excludeOwnTile: false,
                            holdTile: &t.holdTile, holdStart: &t.holdStart, held: &t.held)
        var target = r.target
        if case .tab(let id) = target, world(of: t.window)?.arrangement.tile(of: t.window)?.id == id { target = .none }
        t.target = target
        t.targetWorld = r.world?.uuid
        tabDrag = t
        showPreview(target, window: t.window, in: r.world, wholeTile: false)
    }

    func endTabDrag() {
        updateTabDrag()
        defer {
            tabDrag = nil
            ghost.orderOut(nil)
            preview.orderOut(nil)
            holdTimer?.invalidate()
        }
        guard let t = tabDrag else { return }
        switch t.target {
        case .none, .float:
            // Torn off: float it where it was dropped.
            guard let source = world(of: t.window), tracker.windows[t.window] != nil else { return }
            let p = Coordinates.mouse
            let size = floatFrames[t.window]?.size ?? applied[t.window]?.size ?? CanopyCore.Size(width: 800, height: 600)
            let bounds = (world(at: p) ?? source).display.visible
            let r = CanopyCore.Rect(x: p.x - size.width / 2, y: p.y - 14, width: min(size.width, bounds.width),
                                    height: min(size.height, bounds.height)).clamped(into: bounds)
            let target = source.arrangement.remove(t.window, shareSpace: prefs.shareRemovedSpace)
            floatRestore[t.window] = (source.uuid, target ?? .floating)
            applied[t.window] = nil
            layout(source, animated: true, floatTo: [t.window: r])
            focus(t.window)
        default:
            guard let uuid = t.targetWorld, let world = worlds[uuid] else { return }
            drop(t.window, t.target, into: world, wholeTile: false)
        }
    }

    // MARK: Clicks, marquee and gestures

    func titleDoubleClicked(_ id: WindowID) {
        guard let world = world(of: id), let tile = world.arrangement.tile(of: id) else { return }
        if world.navigator.toggleFrame(tile, in: world.arrangement) { layout(world, animated: true) }
    }

    func desktopDoubleClicked(_ p: CanopyCore.Point) {
        guard let world = world(at: p), let desk = world.plan?.desktopRect, desk.contains(p) else { return }
        if world.navigator.toggleFrame(world.arrangement.desktop, in: world.arrangement) { layout(world, animated: true) }
    }

    func marqueeBegan(_ p: CanopyCore.Point) {
        Log.add("MARQUEE begins at \(Int(p.x)),\(Int(p.y))")
        marqueePanel.show(CanopyCore.Rect(x: p.x, y: p.y, width: 1, height: 1))
    }

    func marqueeMoved(_ a: CanopyCore.Point, _ b: CanopyCore.Point) {
        marqueePanel.show(rect(a, b))
    }

    func marqueeCancelled() {
        marqueePanel.orderOut(nil)
    }

    func marqueeEnded(_ a: CanopyCore.Point, _ b: CanopyCore.Point) {
        marqueePanel.orderOut(nil)
        let r = rect(a, b)
        Log.add("MARQUEE \(r)")
        guard let world = world(at: a), let plan = world.plan, r.width > 8, r.height > 8 else { return }
        var rects = plan.layout.nodeRects
        for (id, c) in plan.covers { rects[id] = c }
        if world.navigator.frame(rect: r, in: world.arrangement, rects: rects) { layout(world, animated: true) }
    }

    private func rect(_ a: CanopyCore.Point, _ b: CanopyCore.Point) -> CanopyCore.Rect {
        CanopyCore.Rect(x: min(a.x, b.x), y: min(a.y, b.y), width: max(1, abs(a.x - b.x)), height: max(1, abs(a.y - b.y)))
    }

    /// A pinch: the stage zooms live around where it began, then the camera
    /// settles on the level nearest how far you pinched.
    func pinch(total: Double, at p: CanopyCore.Point, ended: Bool) {
        guard prefs.animate else {
            if ended, abs(total) >= 0.25 { zoomStep(inward: total > 0, pointer: p) }
            return
        }
        if zoom == nil {
            guard !ended else { return }
            beginZoom(at: p)
        }
        guard var z = zoom, let world = worlds[z.world] else { return }
        z.scale = min(max(exp(total), 0.05), 20)
        zoom = z
        let panel = stage.panel(for: world.display)
        panel.setCamera(scale: z.scale, around: z.anchor)
        guard ended else { return }
        zoom = nil
        guard let plan = world.plan else { panel.fade(); return }
        var rects = plan.projected
        for (id, r) in plan.covers { rects[id] = r }
        let target = world.navigator.settleTarget(scale: z.scale, pointer: z.anchor, in: world.arrangement, rects: rects,
                                                  screen: Solver.bounds(for: world.display.visible, gap: prefs.gap))
        Log.add("PINCH ×\(String(format: "%.2f", z.scale)) settles on \(target.id)")
        if world.navigator.frame(target, in: world.arrangement) {
            layout(world, animated: true)
        } else {
            panel.animate([:], duration: 0.3)
            panel.land(after: 0.3, writer: writer, redraw: 0.02)
        }
    }

    private func beginZoom(at p: CanopyCore.Point) {
        guard let world = world(at: p) ?? currentWorld(), let plan = world.plan else { return }
        refreshFrames()
        var included = Set<WindowID>()
        // Windows outside the view sit where the camera would see them.
        var items: [StageItem] = []
        for tile in world.arrangement.tiles {
            guard let sel = tile.selectedWindow, parked.contains(sel), let r = plan.projected[tile.id] else { continue }
            if let item = stageItem(sel, at: r) { items.append(item); included.insert(sel) }
        }
        items.append(contentsOf: onScreenItems(on: world.display, excluding: &included))
        stage.panel(for: world.display).show(items, desk: plan.desktopRect)
        zoom = ZoomSession(world: world.uuid, anchor: p, scale: 1)
        publishInput()
    }

    func zoomStep(inward: Bool, pointer: CanopyCore.Point) {
        guard let world = world(at: pointer) else { return }
        let a = world.arrangement, nav = world.navigator
        let moved = inward
            ? nav.stepIn(in: a, focused: nil, pointer: pointer, rects: world.plan?.layout.nodeRects ?? [:], preferPointer: true)
            : nav.stepOut(in: a)
        if moved { layout(world, animated: true) }
    }
}
