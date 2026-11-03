import AppKit
import CanopyCore

/// The arrangement and camera of one display on one Space (desktop).
final class World {
    var display: Display
    /// The Space's stable key ("main" for the first desktop).
    let space: String
    let arrangement: Arrangement
    let navigator: Navigator
    var plan: Plan?

    init(display: Display, space: String, arrangement: Arrangement = Arrangement(), navigator: Navigator = Navigator()) {
        self.display = display
        self.space = space
        self.arrangement = arrangement
        self.navigator = navigator
    }

    /// This world's identity: display and Space.
    var uuid: String { World.key(display.uuid, space) }

    static func key(_ display: String, _ space: String) -> String { display + "|" + space }
}

/// Ties windows, input, layout and motion together. Main thread only, and it
/// never waits on an app: every Accessibility call goes through `AXService`
/// and `WindowWriter`, frames come from the window server.
final class Coordinator: WindowTrackerDelegate {
    let tracker = WindowTracker()
    let writer = WindowWriter()
    let pictures = PictureStore()
    let stage = Stage()
    let hotkeys = HotkeyCenter()
    let store = Store()
    let resolver = DropResolver()
    let input = InputRouter()
    private let inputThread = RunLoopThread(name: "canopy.input")
    private let tap = EventTap()

    var prefs = Preferences() {
        didSet { if prefs != oldValue { preferencesChanged(from: oldValue) } }
    }
    var bindings: [Command: KeyCombo] = Shortcuts.defaults {
        didSet { store.saveBindings(bindings); if useHotkeys { hotkeys.register(bindings) } }
    }

    /// Menu bar refresh.
    var onStateChange: (() -> Void)?
    var onAskScreenRecording: (() -> Void)?
    var openSettings: ((SettingsPane?) -> Void)?

    /// Every display's arrangement on every Space, by world key.
    private(set) var worlds: [String: World] = [:]
    /// The world each display is showing now (its current Space), by display UUID.
    /// Absent on a full-screen app's Space.
    private var active: [String: String] = [:]
    /// The window server's id of each world's Space.
    private var spaceIDs: [String: UInt64] = [:]
    private(set) var started = false
    /// Test harness: manage only this app's windows, without the tap or hotkeys.
    var onlyBundleID: String?
    var useEventTap = true
    var useHotkeys = true
    private(set) var tapRunning = false

    private var detached: [String: DisplayState] = [:]
    /// Minimised, hidden or full-screen docked windows, and where they go back.
    private var away: [WindowID: (world: String, target: RestoreTarget)] = [:]
    /// Windows floated with the green button, and where they were docked.
    var floatRestore: [WindowID: (world: String, target: RestoreTarget)] = [:]
    /// A window's floating frame from before it was docked.
    var floatFrames: [WindowID: CanopyCore.Rect] = [:]
    /// Floating windows parked while their Desktop is hidden.
    private var parkedFloating: [WindowID: (world: String, frame: CanopyCore.Rect)] = [:]
    private(set) var parked = Set<WindowID>()
    /// The rect each docked window was last given.
    var applied: [WindowID: CanopyCore.Rect] = [:]
    private var lastWrite: [WindowID: CFAbsoluteTime] = [:]
    private var snapBacks: [WindowID: [CFAbsoluteTime]] = [:]
    var movedWhileMouseDown = Set<WindowID>()
    private var appMinimums: [String: CanopyCore.Size] = [:]
    private var relayoutWork: [String: DispatchWorkItem] = [:]

    private var desks: [String: DeskPanel] = [:]
    private var covers: [String: CoverPanel] = [:]
    private var strips: [String: TabStripPanel] = [:]
    private var handles: [String: GapHandlePanel] = [:]
    let preview = DropPreviewPanel()
    let marqueePanel = MarqueePanel()
    let ghost = GhostPanel()

    var press: Press?
    var tabDrag: TabDrag?
    var zoom: ZoomSession?
    /// The world whose gap is being dragged on the stage.
    private var seamWorld: String?
    var holdTimer: Timer?
    private var pendingSplit: (tile: NodeID?, world: String, until: Date)?
    private var saveWork: DispatchWorkItem?

    /// Main-thread responsiveness, for the harness: the longest gap between ticks.
    private(set) var worstStall: Double = 0
    private var lastTick = CFAbsoluteTimeGetCurrent()

    // MARK: - Lifecycle

    func start() {
        guard !started else { return }
        Log.open()
        Log.add("LAUNCH pid \(ProcessInfo.processInfo.processIdentifier) accessibility=\(AX.isTrusted) screen=\(Permissions.screenRecording)")
        prefs = store.loadPreferences()
        bindings = store.loadBindings()
        appMinimums = store.loadAppMinimums()
        applyPreferencesToParts()
        WindowServer.setCursorInBackground()
        AppLook.prepareWallpapers()
        AppLook.prepareApps()
        rebuildDisplays()

        writer.onWritten = { [weak self] in self?.written($0) }
        tracker.delegate = self
        tracker.start()

        hotkeys.onCommand = { [weak self] in self?.run($0) }
        if useHotkeys { hotkeys.register(bindings) } else { hotkeys.unregisterAll() }

        pictures.wanted = { [weak self] in self?.windowsOnScreen() ?? [] }
        pictures.onUpdate = { [weak self] id, image in self?.pictureArrived(id, image) }
        pictures.start()
        pictures.startBackdrops()
        stage.backdrop = { [weak self] display in self?.pictures.backdrops[display.id] }
        stage.glass = { [weak self] display in self?.pictures.glass[display.id] }
        pictures.onBackdrop = { [weak self] id in
            guard let self, let world = self.activeWorlds.first(where: { $0.display.id == id }), let plan = world.plan else { return }
            self.updateDesk(world, plan)
        }

        NotificationCenter.default.addObserver(forName: NSApplication.didChangeScreenParametersNotification,
                                               object: nil, queue: .main) { [weak self] _ in self?.rebuildDisplays() }
        Timer.scheduledTimer(withTimeInterval: 0.02, repeats: true) { [weak self] _ in
            guard let self else { return }
            let now = CFAbsoluteTimeGetCurrent()
            let gap = now - self.lastTick
            self.worstStall = max(self.worstStall, gap)
            if gap > 0.06 { Log.add("STALL main thread \(Int(gap * 1000)) ms") }
            self.lastTick = now
        }

        // Give the first scan a moment, then restore saved arrangements.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { [weak self] in
            guard let self else { return }
            self.restoreSaved()
            self.started = true
            self.startInput()
            for w in self.activeWorlds { self.layout(w, animated: false) }
            NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.activeSpaceDidChangeNotification,
                                                              object: nil, queue: .main) { [weak self] _ in self?.spacesChanged() }
            Timer.scheduledTimer(withTimeInterval: 3, repeats: true) { [weak self] _ in self?.checkWindowSpaces() }
            self.pictures.refresh()
            self.onStateChange?()
        }
    }

    private func startInput() {
        input.coordinator = self
        publishInput()
        guard useEventTap else { return }
        inputThread.startAndWait()
        let semaphore = DispatchSemaphore(value: 0)
        var ok = false
        let tap = self.tap
        let input = self.input
        inputThread.perform {
            if let runLoop = CFRunLoopGetCurrent() {
                tap.handler = { type, event in input.handle(type, event) }
                ok = tap.start(on: runLoop)
            }
            semaphore.signal()
        }
        semaphore.wait()
        tapRunning = ok
        Log.add(ok ? "INPUT event tap running on its own thread" : "INPUT event tap unavailable; drag-to-dock and gestures are off")
    }

    func resetStallMeter() -> Double {
        let w = worstStall
        worstStall = 0
        return w
    }

    /// Brings every parked window back into view and saves (quitting returns
    /// all windows to visible positions).
    func prepareForQuit() {
        guard started else { return }
        saveNow()
        tap.stop()
        hotkeys.unregisterAll()
        var jobs: [(AXWindow, CGRect)] = []
        var cascade = 0.0
        for world in worlds.values {
            let v = world.display.visible
            for id in world.arrangement.windows where parked.contains(id) {
                guard let tw = tracker.windows[id] else { continue }
                let size = applied[id]?.size ?? floatFrames[id]?.size ?? CanopyCore.Size(width: 900, height: 650)
                let w = min(max(size.width, 480), v.width * 0.8), h = min(max(size.height, 320), v.height * 0.8)
                let r = CanopyCore.Rect(x: v.midX - w / 2 + cascade, y: v.midY - h / 2 + cascade, width: w, height: h)
                jobs.append((tw.ax, r.clamped(into: v).cg))
                cascade = cascade >= 180 ? 0 : cascade + 30
            }
        }
        for (id, entry) in parkedFloating {
            if let tw = tracker.windows[id] { jobs.append((tw.ax, entry.frame.cg)) }
        }
        writer.flushSynchronously(jobs, timeout: 2.5)
        for p in covers.values { p.orderOut(nil) }
        for p in strips.values { p.orderOut(nil) }
        for p in handles.values { p.orderOut(nil) }
        for d in desks.values { d.hide() }
        stage.tearDown()
    }

    private func applyPreferencesToParts() {
        stage.duration = prefs.animationDuration
        stage.redrawWait = prefs.redrawWait
        pictures.enabled = prefs.livePreviews
    }

    private func preferencesChanged(from old: Preferences) {
        store.savePreferences(prefs)
        applyPreferencesToParts()
        guard started else { return }
        if prefs.ignoredBundleIDs != old.ignoredBundleIDs {
            let newlyIgnored = Set(prefs.ignoredBundleIDs).subtracting(old.ignoredBundleIDs)
            for w in tracker.windows.values where newlyIgnored.contains(w.bundleID) { float(w.id, animated: false) }
        }
        for w in worlds.values { layout(w, animated: false) }
        onStateChange?()
    }

    /// What the input thread may read.
    func publishInput() {
        var s = InputState()
        s.started = started
        s.rectZoom = prefs.rectZoom
        s.greenButton = prefs.greenButtonToggles
        s.titleDoubleClick = prefs.titleDoubleClickFrames
        s.optionPinch = prefs.optionPinch
        s.managed = Set(tracker.windows.values.filter(manageable).map(\.id))
        s.docked = Set(worlds.values.flatMap(\.arrangement.windows))
        s.landing = stage.anyShown
        input.publish(s)
    }

    // MARK: - Displays and saved state

    // MARK: - Spaces

    var activeWorlds: [World] { active.values.compactMap { worlds[$0] } }

    func isActive(_ world: World) -> Bool { active[world.display.uuid] == world.uuid }

    func activeWorld(for display: Display) -> World? { active[display.uuid].flatMap { worlds[$0] } }

    /// Points `display` at its world for `space`, creating it if new. A saved
    /// arrangement for that Space is restored separately (`restoreSpace`),
    /// once its windows have shown up. Returns true when the display changed world.
    @discardableResult
    private func activate(_ display: Display, space: Spaces.Space?) -> Bool {
        let space = space ?? Spaces.Space(id: 0, key: "main", fullScreen: false)
        guard !space.fullScreen else {
            // A full-screen app's Space: nothing to arrange.
            let changed = active[display.uuid] != nil
            active[display.uuid] = nil
            return changed
        }
        let key = World.key(display.uuid, space.key)
        spaceIDs[key] = space.id
        if active[display.uuid] == key { return false }
        if worlds[key] == nil, detached[key] == nil || !started {
            worlds[key] = World(display: display, space: space.key)
        }
        active[display.uuid] = key
        return true
    }

    /// Back on a Space Canopy arranged before (since launch, or saved): its
    /// windows return to their places.
    private func restoreSpace(_ display: Display) {
        guard let key = active[display.uuid], worlds[key] == nil else { return }
        if let saved = detached.removeValue(forKey: key) {
            saved.reattach(to: liveRefs(floatingOnly: true, onSpace: spaceIDs[key]), shareSpace: prefs.shareRemovedSpace)
            let space = String(key.split(separator: "|", maxSplits: 1).last ?? "main")
            worlds[key] = World(display: display, space: space, arrangement: saved.arrangement, navigator: saved.navigator)
            Log.add("SPACE restored \(saved.arrangement.windows.count) windows")
        } else {
            worlds[key] = World(display: display, space: String(key.split(separator: "|", maxSplits: 1).last ?? "main"))
        }
    }

    /// The user switched desktops: each display shows its world for the new
    /// Space. Windows on other Spaces are never touched.
    private func spacesChanged() {
        guard started else { return }
        let displays = Display.current()
        guard let spaces = Spaces.current(displays: displays) else { return }
        // A drag or pinch in progress belongs to the Space that just left.
        press = nil
        zoom = nil
        tabDrag = nil
        seamWorld = nil
        for d in displays {
            let before = active[d.uuid]
            guard activate(d, space: spaces[d.uuid]) else { continue }
            stage.panel(for: d).tearDown()
            Log.add("SPACE \(d.name) leaves \(before ?? "a full-screen Space") for \(active[d.uuid] ?? "a full-screen Space")")
            // The new Space's windows take a moment to show up; then arrange them.
            for pid in Set(tracker.windows.values.map(\.pid)) { tracker.scan(pid) }
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in
                guard let self else { return }
                self.restoreSpace(d)
                if let w = self.activeWorld(for: d) { self.layout(w, animated: false) }
                self.publishInput()
            }
        }
        publishInput()
        onStateChange?()
    }

    /// A docked window moved to another Space (Mission Control, or an app's
    /// own "move to desktop"): it leaves this Space's arrangement.
    private func checkWindowSpaces() {
        guard started else { return }
        for world in activeWorlds {
            guard let here = spaceIDs[world.uuid], here != 0 else { continue }
            var moved: [WindowID] = []
            for id in world.arrangement.windows where !parked.contains(id) || world.arrangement.tile(of: id)?.selectedWindow == id {
                let spaces = Spaces.of(id)
                if !spaces.isEmpty, !spaces.contains(here) { moved.append(id) }
            }
            guard !moved.isEmpty else { continue }
            for id in moved {
                Log.add("SPACE w\(id) moved to another desktop; it leaves this arrangement")
                world.arrangement.remove(id, shareSpace: prefs.shareRemovedSpace)
                parked.remove(id)
                applied[id] = nil
            }
            world.navigator.prune(liveWindows: Set(world.arrangement.windows))
            layout(world, animated: true)
        }
    }

    // MARK: - Displays and saved state

    private func rebuildDisplays() {
        let now = Display.current()
        let spaces = Spaces.current(displays: now)
        for d in now {
            for w in worlds.values where w.display.uuid == d.uuid { w.display = d }
            activate(d, space: spaces?[d.uuid])
        }
        let main = now.first
        for (key, w) in worlds where !now.contains(where: { $0.uuid == w.display.uuid }) {
            detached[key] = DisplayState(arrangement: w.arrangement, navigator: w.navigator, windows: refs(for: w.arrangement.windows))
            worlds[key] = nil
            active[w.display.uuid] = nil
            let uuid = key
            // Its windows float onto a remaining display until it returns.
            if let main {
                var offset = 0.0
                for id in w.arrangement.windows {
                    guard let tw = tracker.windows[id] else { continue }
                    let r = CanopyCore.Rect(x: main.visible.x + 60 + offset, y: main.visible.y + 60 + offset,
                                            width: min(900, main.visible.width * 0.6), height: min(650, main.visible.height * 0.6))
                    write(tw, r)
                    parked.remove(id)
                    offset += 30
                }
            }
            for (id, entry) in parkedFloating where entry.world == uuid {
                if let tw = tracker.windows[id] { write(tw, entry.frame) }
                parkedFloating[id] = nil
            }
            removeOverlays(forWorld: uuid, keeping: [], in: &covers)
            removeOverlays(forWorld: uuid, keeping: [], in: &strips)
            removeOverlays(forWorld: uuid, keeping: [], in: &handles)
        }
        if started {
            for w in activeWorlds { layout(w, animated: false) }
            scheduleSave()
        }
    }

    /// Saved arrangements come back for the Spaces on screen now; the others
    /// wait until their Space is opened. (Before Spaces were kept apart,
    /// arrangements were saved per display: those become the current Space's.)
    private func restoreSaved() {
        guard let state = store.loadState() else { return }
        var live = liveRefs(floatingOnly: false)
        for (savedKey, saved) in state.displays {
            var key = savedKey
            if !key.contains("|") {
                guard let current = active[savedKey] else { continue }
                key = current
            }
            if let world = worlds[key], isActive(world) {
                saved.reattach(to: live, shareSpace: prefs.shareRemovedSpace)
                for id in saved.arrangement.windows { live[id] = nil }
                worlds[key] = World(display: world.display, space: world.space, arrangement: saved.arrangement,
                                    navigator: saved.navigator)
            } else {
                detached[key] = saved
            }
        }
    }

    /// Windows that can be arranged, optionally only floating ones and only
    /// those on one Space.
    private func liveRefs(floatingOnly: Bool, onSpace space: UInt64? = nil) -> [WindowID: CanopyCore.WindowRef] {
        var out: [WindowID: CanopyCore.WindowRef] = [:]
        for w in tracker.windows.values where manageable(w) {
            if floatingOnly, world(of: w.id) != nil { continue }
            if let space, space != 0 {
                let spaces = Spaces.of(w.id)
                if !spaces.isEmpty, !spaces.contains(space) { continue }
            }
            out[w.id] = w.ref
        }
        return out
    }

    private func refs(for ids: [WindowID]) -> [WindowID: CanopyCore.WindowRef] {
        var out: [WindowID: CanopyCore.WindowRef] = [:]
        for id in ids { if let w = tracker.windows[id] { out[id] = w.ref } }
        return out
    }

    func scheduleSave() {
        saveWork?.cancel()
        let work = DispatchWorkItem { [weak self] in self?.saveNow() }
        saveWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 1, execute: work)
    }

    private func saveNow() {
        var displays = detached
        for (uuid, w) in worlds {
            displays[uuid] = DisplayState(arrangement: w.arrangement, navigator: w.navigator, windows: refs(for: w.arrangement.windows))
        }
        store.saveState(SavedState(displays: displays))
    }

    // MARK: - Lookups

    func world(of id: WindowID) -> World? {
        worlds.values.first { $0.arrangement.contains(id) }
    }

    /// The world shown on the display under `p`.
    func world(at p: CanopyCore.Point) -> World? {
        activeWorlds.first { $0.display.frame.contains(p) }
    }

    private var primaryWorld: World? {
        guard let main = Display.current().first else { return activeWorlds.first }
        return activeWorld(for: main) ?? activeWorlds.first
    }

    /// Commands act on the focused window's display, else the pointer's.
    func currentWorld() -> World? {
        if let f = tracker.focusedWindowID() {
            if let w = world(of: f), isActive(w) { return w }
            if let frame = tracker.windows[f]?.frame, let w = world(at: frame.center) { return w }
        }
        return world(at: Coordinates.mouse) ?? primaryWorld
    }

    func displayWorld(of w: TrackedWindow) -> World? {
        if let entry = parkedFloating[w.id] { return worlds[entry.world] }
        guard let f = w.frame else { return nil }
        return world(at: f.center)
    }

    func manageable(_ w: TrackedWindow) -> Bool {
        if let only = onlyBundleID, w.bundleID != only { return false }
        // The end-to-end test's windows belong to the test harness alone.
        if onlyBundleID == nil, w.bundleID == "app.canopy.e2e.windows" { return false }
        return !prefs.ignoredBundleIDs.contains(w.bundleID) && !w.minimized && !w.fullScreen && !w.hidden
    }

    /// Floating windows that live on a world's Desktop: on its Space now, or
    /// parked by it (excluded apps are left alone).
    func floatingWindows(in world: World) -> [TrackedWindow] {
        let here = Set(WindowServer.onScreen().map(\.id))
        return tracker.windows.values.filter { w in
            guard manageable(w), self.world(of: w.id) == nil else { return false }
            if let entry = parkedFloating[w.id] { return entry.world == world.uuid }
            return here.contains(w.id) && displayWorld(of: w) === world
        }
    }

    private func windowsOnScreen() -> [WindowID] {
        let own = ProcessInfo.processInfo.processIdentifier
        return WindowServer.onScreen().filter { $0.pid != own && !parked.contains($0.id) }.map(\.id)
    }

    /// The smallest size this window has accepted. Windows of one app can
    /// differ, so the app's minimum is only a floor for parking.
    func minimum(for id: WindowID) -> CanopyCore.Size? {
        tracker.windows[id]?.minimumSize
    }

    private func parkingFloor(for tw: TrackedWindow) -> CanopyCore.Size? {
        let a = tw.minimumSize, b = appMinimums[tw.bundleID]
        if a == nil && b == nil { return nil }
        return CanopyCore.Size(width: max(a?.width ?? 0, b?.width ?? 0), height: max(a?.height ?? 0, b?.height ?? 0))
    }

    /// Updates cached frames from the window server (well under a millisecond).
    func refreshFrames() {
        for o in WindowServer.onScreen() {
            guard let tw = tracker.windows[o.id] else { continue }
            if !writer.isBusy(o.id) { tw.frame = o.frame }
        }
    }

    // MARK: - Writing frames

    func write(_ tw: TrackedWindow, _ r: CanopyCore.Rect) {
        Log.add("WRITE w\(tw.id) \(tw.appName) \(r)")
        writer.setFrame(tw.ax, r.cg)
        tw.frame = r
    }

    private func written(_ result: WindowWriter.Result) {
        lastWrite[result.id] = CFAbsoluteTimeGetCurrent()
        guard let tw = tracker.windows[result.id], let actual = result.actualSize else { return }
        tw.frame = CanopyCore.Rect(x: result.asked.minX, y: result.asked.minY, width: actual.width, height: actual.height)
        // A window that refused to shrink tells us its minimum; remember it for the app too.
        let wider = actual.width > result.asked.width + 2, taller = actual.height > result.asked.height + 2
        guard wider || taller else { return }
        let learned = CanopyCore.Size(width: max(wider ? actual.width : 0, tw.minimumSize?.width ?? 0),
                                      height: max(taller ? actual.height : 0, tw.minimumSize?.height ?? 0))
        guard learned != tw.minimumSize else { return }
        let small = (wider ? actual.width - result.asked.width : 0) <= prefs.gap && (taller ? actual.height - result.asked.height : 0) <= prefs.gap
        Log.add("MINIMUM w\(tw.id) \(tw.appName) refused \(CanopyCore.Rect(result.asked)) → \(Int(actual.width))×\(Int(actual.height))")
        tw.minimumSize = learned
        let app = appMinimums[tw.bundleID]
        appMinimums[tw.bundleID] = CanopyCore.Size(width: max(app?.width ?? 0, learned.width), height: max(app?.height ?? 0, learned.height))
        store.saveAppMinimums(appMinimums)
        // Reaching a little into the gap changes nothing; a real shortfall makes a cover.
        if !small, !parked.contains(result.id), let world = world(of: result.id) { scheduleRelayout(world) }
    }

    private func scheduleRelayout(_ world: World) {
        relayoutWork[world.uuid]?.cancel()
        let work = DispatchWorkItem { [weak self, weak world] in
            guard let self, let world else { return }
            self.layout(world, animated: false)
        }
        relayoutWork[world.uuid] = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.05, execute: work)
    }

    /// Moves a window out of view: shrunk, at a corner of its display no other
    /// display touches, leaving the sliver on screen that macOS requires.
    private func park(_ id: WindowID, in world: World) {
        guard !parked.contains(id), let tw = tracker.windows[id] else { return }
        let current = tw.frame ?? applied[id] ?? CanopyCore.Rect(x: 0, y: 0, width: 800, height: 600)
        let floor = parkingFloor(for: tw)
        let size = CGSize(width: max(floor?.width ?? 0, min(current.width, 480)), height: max(floor?.height ?? 0, 128))
        let f = world.display.frame
        let others = activeWorlds.filter { $0.display.uuid != world.display.uuid }.map(\.display.frame)
        let candidates = [
            CGPoint(x: f.maxX - 1, y: f.maxY - 1),
            CGPoint(x: f.minX - size.width + 1, y: f.maxY - 1),
            CGPoint(x: f.maxX - 1, y: f.minY + 1),
        ]
        let spot = candidates.first { p in
            let r = CanopyCore.Rect(x: p.x, y: p.y, width: size.width, height: size.height)
            return !others.contains { $0.intersects(r) }
        } ?? candidates[0]
        Log.add("PARK w\(id) \(tw.appName)")
        writer.setFrame(tw.ax, CGRect(origin: spot, size: size))
        tw.frame = CanopyCore.Rect(CGRect(origin: spot, size: size))
        parked.insert(id)
        applied[id] = nil
    }

    // MARK: - Layout

    /// Solves `world` for its camera and moves every window there. Animated
    /// changes play on the stage; the real windows move underneath.
    func layout(_ world: World, animated: Bool, confineFloating: Bool = false,
                floatTo: [WindowID: CanopyCore.Rect] = [:], dragged: WindowID? = nil, quick: Bool = false) {
        // A world on another Space keeps its arrangement but moves nothing:
        // its windows are out of sight on that Space. It is laid out on return.
        guard started, isActive(world) else { scheduleSave(); return }
        refreshFrames()
        let a = world.arrangement
        let d = world.display
        let plan = Planner.plan(a, world.navigator, visibleFrame: d.visible, gap: prefs.gap) { [weak self] in
            self?.minimum(for: $0)
        }
        let old = world.plan
        world.plan = plan
        Log.add("LAYOUT \(a.root) framed \(world.navigator.framedNode(in: a).id) visible \(plan.visible.count) covers \(plan.covers.count) parked \(plan.parked.count)\(animated ? " animated" : "")")

        // Floating windows: hidden with the Desktop, kept on it otherwise.
        var floatMoves = floatTo
        var floatPark: [TrackedWindow] = []
        var floatReturn: [WindowID: CanopyCore.Rect] = [:]
        let floating = floatingWindows(in: world)
        if let desk = plan.desktopRect {
            for (id, entry) in parkedFloating where entry.world == world.uuid { floatReturn[id] = entry.frame }
            if confineFloating {
                for w in floating where parkedFloating[w.id] == nil && floatMoves[w.id] == nil && w.id != dragged {
                    guard let f = w.frame else { continue }
                    if f.intersection(desk).area < 0.6 * f.area {
                        floatMoves[w.id] = CanopyCore.Rect(x: f.x, y: f.y, width: min(f.width, desk.width),
                                                           height: min(f.height, desk.height)).clamped(into: desk)
                    }
                }
            }
        } else {
            floatPark = floating.filter { parkedFloating[$0.id] == nil }
        }

        let apply = { [weak self] in
            guard let self else { return }
            self.apply(plan, in: world)
            for (id, r) in floatMoves { if let tw = self.tracker.windows[id] { self.write(tw, r) } }
            for w in floatPark {
                if let f = w.frame { self.parkedFloating[w.id] = (world.uuid, f) }
                self.park(w.id, in: world)
            }
            for (id, r) in floatReturn {
                self.parkedFloating[id] = nil
                self.parked.remove(id)
                if let tw = self.tracker.windows[id] { self.write(tw, r) }
            }
        }

        let panel = stage.panel(for: d)
        if animated && prefs.animate {
            let motion = self.motion(world, plan: plan, old: old, floatMoves: floatMoves, floatPark: floatPark,
                                     floatReturn: floatReturn)
            if panel.isShown {
                // Continue on the stage a drag put up.
                let duration = quick ? 0.12 : stage.duration
                panel.setSlot(nil)
                panel.animate(motion.targets, duration: duration)
                panel.setDesk(plan.desktopRect, duration: duration)
                apply()
                panel.land(after: duration, writer: writer, redraw: stage.redrawWait)
            } else if motion.moving {
                stage.transition(on: d, items: motion.items, to: motion.targets, deskFrom: old?.desktopRect,
                                 deskTo: plan.desktopRect, writer: writer, apply: apply)
            } else {
                apply()
            }
        } else {
            // A landing in progress finishes on its own; it waits for these writes too.
            if panel.isShown, !prefs.animate { panel.fade() }
            apply()
        }
        updateOverlays(world, plan)
        publishInput()
        scheduleSave()
        onStateChange?()
    }

    /// Stage items for everything on the display, back to front, where it is
    /// now; and where each one goes.
    private func motion(_ world: World, plan: Plan, old: Plan?, floatMoves: [WindowID: CanopyCore.Rect],
                        floatPark: [TrackedWindow], floatReturn: [WindowID: CanopyCore.Rect])
        -> (items: [StageItem], targets: [WindowID: CanopyCore.Rect], moving: Bool) {
        let v = world.display.visible
        var items: [StageItem] = []
        var targets: [WindowID: CanopyCore.Rect] = [:]
        var included = Set<WindowID>()

        // Docked windows coming out of parking start from where the old camera had them.
        for tile in world.arrangement.tiles {
            guard let sel = tile.selectedWindow, parked.contains(sel),
                  let from = old?.covers[tile.id] ?? old?.projected[tile.id] else { continue }
            if let item = stageItem(sel, at: from) { items.append(item); included.insert(sel) }
        }
        // Everything on screen, back to front.
        items.append(contentsOf: onScreenItems(on: world.display, excluding: &included))
        // Floating windows returning with the Desktop come from below.
        for (id, back) in floatReturn where !included.contains(id) {
            if let item = stageItem(id, at: CanopyCore.Rect(x: back.x, y: v.maxY + 40, width: back.width, height: back.height)) {
                items.append(item)
                included.insert(id)
            }
        }

        for tile in world.arrangement.tiles {
            guard let sel = tile.selectedWindow else { continue }
            targets[sel] = plan.visible[sel] ?? plan.covers[tile.id] ?? plan.projected[tile.id]
        }
        for (id, r) in floatMoves { targets[id] = r }
        for w in floatPark {
            if let f = w.frame { targets[w.id] = CanopyCore.Rect(x: f.x, y: v.maxY + 40, width: f.width, height: f.height) }
        }
        for (id, r) in floatReturn { targets[id] = r }

        let moving = items.contains { item in
            guard let to = targets[item.id] else { return false }
            let from = item.rect
            return abs(from.x - to.x) + abs(from.y - to.y) + abs(from.width - to.width) + abs(from.height - to.height) > 2
        }
        return (items, targets, moving)
    }

    /// Every ordinary window on a display as it is now, back to front, plus
    /// the covers Canopy draws for cramped tiles.
    func onScreenItems(on display: Display, excluding included: inout Set<WindowID>) -> [StageItem] {
        let own = ProcessInfo.processInfo.processIdentifier
        var items: [StageItem] = []
        for o in WindowServer.onScreen().reversed() where o.pid != own && !parked.contains(o.id) && !included.contains(o.id) {
            guard o.frame.intersects(display.frame) else { continue }
            if let item = stageItem(o.id, at: tracker.windows[o.id]?.frame ?? o.frame, pid: o.pid) {
                items.append(item)
                included.insert(o.id)
            }
        }
        if let world = activeWorld(for: display) {
            for (tileID, r) in world.plan?.covers ?? [:] {
                guard let sel = world.arrangement.node(tileID)?.selectedWindow, !included.contains(sel) else { continue }
                if let item = stageItem(sel, at: r) {
                    items.insert(item, at: 0)
                    included.insert(sel)
                }
            }
        }
        return items
    }

    func stageItem(_ id: WindowID, at r: CanopyCore.Rect, pid: pid_t? = nil) -> StageItem? {
        let tw = tracker.windows[id]
        guard let pid = tw?.pid ?? pid else { return nil }
        let bundle = tw?.bundleID ?? NSRunningApplication(processIdentifier: pid)?.bundleIdentifier ?? "pid.\(pid)"
        return StageItem(id: id, rect: r, image: pictures.images[id], colour: AppLook.placeholder(for: bundle, pid: pid),
                         icon: AppLook.icon(for: bundle, pid: pid), edge: pictures.edgeColours[id])
    }

    private func pictureArrived(_ id: WindowID, _ image: CGImage) {
        for world in activeWorlds {
            let panel = stage.panel(for: world.display)
            if panel.isShown, panel.contains(id) { panel.setImage(id, image) }
        }
    }

    private func apply(_ plan: Plan, in world: World) {
        for (id, r) in plan.visible {
            guard let tw = tracker.windows[id] else { continue }
            if !parked.contains(id), applied[id] == r, let f = tw.frame, near(f, r) { continue }
            write(tw, r)
            applied[id] = r
            parked.remove(id)
        }
        for id in plan.parked { park(id, in: world) }
    }

    private func near(_ a: CanopyCore.Rect, _ b: CanopyCore.Rect) -> Bool {
        abs(a.x - b.x) < 1 && abs(a.y - b.y) < 1 && abs(a.width - b.width) < 1 && abs(a.height - b.height) < 1
    }

    // MARK: - Overlays

    private func key(_ world: World, _ id: NodeID) -> String { "\(world.uuid):\(id)" }

    private func removeOverlays<P: NSWindow>(forWorld uuid: String, keeping: Set<String>, in dict: inout [String: P]) {
        for (k, p) in dict where k.hasPrefix(uuid + ":") && !keeping.contains(k) {
            p.orderOut(nil)
            dict[k] = nil
        }
    }

    private func updateDesk(_ world: World, _ plan: Plan) {
        let desk = desks[world.uuid] ?? DeskPanel(display: world.display)
        desks[world.uuid] = desk
        desk.update(display: world.display, glass: pictures.glass[world.display.id], desk: plan.desktopRect,
                    anyDocked: !world.arrangement.isEmpty && (onlyBundleID == nil || CommandLine.arguments.contains("--glass")))
    }

    private func updateOverlays(_ world: World, _ plan: Plan) {
        let a = world.arrangement
        updateDesk(world, plan)

        // Covers for cramped tiles.
        var keepCovers = Set<String>()
        for (tileID, rect) in plan.covers {
            guard let tile = a.node(tileID), let sel = tile.selectedWindow, let tw = tracker.windows[sel] else { continue }
            let k = key(world, tileID)
            keepCovers.insert(k)
            let panel = covers[k] ?? CoverPanel()
            covers[k] = panel
            panel.update(image: pictures.images[sel], colour: AppLook.placeholder(for: tw.bundleID, pid: tw.pid),
                         icon: AppLook.icon(for: tw.bundleID, pid: tw.pid), name: tw.appName, tint: prefs.coverTint)
            panel.onDoubleClick = { [weak self, weak world] in
                guard let self, let world, let t = world.arrangement.node(tileID) else { return }
                if world.navigator.frame(t, in: world.arrangement) { self.layout(world, animated: true) }
                self.focus(sel)
            }
            panel.place(rect)
            panel.orderFrontRegardless()
        }
        removeOverlays(forWorld: world.uuid, keeping: keepCovers, in: &covers)

        // Tab strips for visible groups.
        var keepStrips = Set<String>()
        for (tileID, rect) in plan.layout.stripRects where plan.covers[tileID] == nil {
            guard let tile = a.node(tileID), let sel = tile.selectedWindow else { continue }
            let k = key(world, tileID)
            keepStrips.insert(k)
            let panel = strips[k] ?? TabStripPanel()
            strips[k] = panel
            configureStrip(panel.view, tile: tile, world: world)
            panel.place(rect)
            panel.order(.above, relativeTo: Int(sel))
            if !panel.isVisible { panel.orderFrontRegardless() }
        }
        removeOverlays(forWorld: world.uuid, keeping: keepStrips, in: &strips)

        // Gap handles, wider than the gap so they're easy to catch.
        var keepHandles = Set<String>()
        for seam in plan.layout.seams where prefs.gap >= 4 {
            let k = "\(world.uuid):\(seam.split):\(seam.index)"
            keepHandles.insert(k)
            let panel = handles[k] ?? GapHandlePanel()
            handles[k] = panel
            panel.view.axis = seam.axis
            panel.view.onBegin = { [weak self, weak world] in
                guard let self, let world else { return }
                self.beginSeamDrag(world)
            }
            panel.view.onDrag = { [weak self, weak world] delta in
                guard let self, let world else { return }
                self.dragSeam(world, seam: seam, delta: delta)
            }
            panel.view.onEnd = { [weak self, weak world] in
                guard let self, let world else { return }
                self.endSeamDrag(world)
            }
            let grow = 5.0
            let r = seam.axis == .horizontal
                ? CanopyCore.Rect(x: seam.rect.x - grow, y: seam.rect.y, width: seam.rect.width + 2 * grow, height: seam.rect.height)
                : CanopyCore.Rect(x: seam.rect.x, y: seam.rect.y - grow, width: seam.rect.width, height: seam.rect.height + 2 * grow)
            panel.place(r)
            panel.orderFrontRegardless()
        }
        removeOverlays(forWorld: world.uuid, keeping: keepHandles, in: &handles)
    }

    // MARK: Gap drags

    /// Gaps are dragged on the stage: the pictures resize at once under the
    /// pointer, and the real windows are written once, when the gap lets go.
    private func beginSeamDrag(_ world: World) {
        guard prefs.animate else { return }
        refreshFrames()
        var included = Set<WindowID>()
        let items = onScreenItems(on: world.display, excluding: &included)
        stage.panel(for: world.display).show(items, desk: world.plan?.desktopRect)
        seamWorld = world.uuid
        publishInput()
    }

    private func dragSeam(_ world: World, seam: Seam, delta: Double) {
        guard let split = world.arrangement.node(seam.split) else { return }
        let current = world.plan?.layout.seams.first { $0.split == seam.split && $0.index == seam.index } ?? seam
        // Stop at the window's own minimum, or its app's when it hasn't refused anything yet.
        let minimum: (WindowID) -> CanopyCore.Size? = { [weak self] id in
            guard let self, let tw = self.tracker.windows[id] else { return nil }
            return tw.minimumSize ?? self.parkingFloor(for: tw)
        }
        let before = split.children[current.index].minimumLength(along: split.axis, gap: prefs.gap, minimum: minimum)
        let after = split.children[current.next].minimumLength(along: split.axis, gap: prefs.gap, minimum: minimum)
        world.arrangement.resizeSeam(split, index: current.index, next: current.next, delta: delta,
                                     available: current.available, minBefore: before, minAfter: after)
        guard seamWorld == world.uuid else {
            layout(world, animated: false)
            return
        }
        let plan = Planner.plan(world.arrangement, world.navigator, visibleFrame: world.display.visible, gap: prefs.gap) {
            [weak self] in self?.minimum(for: $0)
        }
        world.plan = plan
        var targets = plan.visible
        for (tileID, r) in plan.covers { if let sel = world.arrangement.node(tileID)?.selectedWindow { targets[sel] = r } }
        stage.panel(for: world.display).place(targets)
    }

    private func endSeamDrag(_ world: World) {
        if seamWorld == world.uuid {
            seamWorld = nil
            layout(world, animated: true, quick: true)
        }
        scheduleSave()
    }

    private func configureStrip(_ view: TabStripView, tile: Node, world: World) {
        view.showsClose = prefs.tabCloseButtons
        view.tabs = tile.windows.compactMap { id in
            guard let tw = tracker.windows[id] else { return nil }
            return TabInfo(window: id, title: tw.title.isEmpty ? tw.appName : tw.title,
                           icon: AppLook.icon(for: tw.bundleID, pid: tw.pid), selected: id == tile.selectedWindow)
        }
        view.onSelect = { [weak self, weak world] id in
            guard let self, let world else { return }
            world.arrangement.select(id)
            self.layout(world, animated: false)
            self.focus(id)
        }
        view.onClose = { [weak self] id in self?.close(id) }
        view.onDragBegan = { [weak self, weak world] id in
            guard let self, let world else { return }
            self.beginTabDrag(id, world: world)
        }
        view.onDragMoved = { [weak self] in self?.updateTabDrag() }
        view.onDragEnded = { [weak self] in self?.endTabDrag() }
        view.menuFor = { [weak self] id in self?.tabMenu(id) ?? NSMenu() }
    }

    private func tabMenu(_ id: WindowID) -> NSMenu {
        let menu = NSMenu()
        menu.addItem(ClosureMenuItem("Close Tab") { [weak self] in self?.close(id) })
        menu.addItem(ClosureMenuItem("Move Tab to New Window") { [weak self] in self?.float(id, animated: true) })
        menu.addItem(ClosureMenuItem("Minimize Tab") { [weak self] in
            guard let self, let tw = self.tracker.windows[id] else { return }
            AXService.shared.queue(for: tw.pid).async { tw.ax.minimize() }
        })
        return menu
    }

    func focus(_ id: WindowID) {
        guard let tw = tracker.windows[id] else { return }
        NSRunningApplication(processIdentifier: tw.pid)?.activate()
        AXService.shared.queue(for: tw.pid).async {
            AX.set(tw.ax.element, kAXMainAttribute, kCFBooleanTrue)
            tw.ax.raise()
        }
    }

    func close(_ id: WindowID) {
        guard let tw = tracker.windows[id] else { return }
        AXService.shared.queue(for: tw.pid).async { tw.ax.close() }
    }

    // MARK: - Docking and floating

    /// Docks a floating window or floats a docked one (the green button).
    func toggleDocked(_ id: WindowID) {
        guard let tw = tracker.windows[id] else { return }
        if world(of: id) != nil {
            float(id, animated: true)
            return
        }
        refreshFrames()
        guard manageable(tw), let world = displayWorld(of: tw) ?? currentWorld() else { return }
        if let f = tw.frame { floatFrames[id] = f }
        if let r = floatRestore.removeValue(forKey: id), let w = worlds[r.world], w.arrangement.restore(id, to: r.target) != nil {
            reveal(id, in: w)
            layout(w, animated: true, confineFloating: true)
            return
        }
        world.arrangement.autoDock(id, rects: world.plan?.layout.nodeRects ?? [:])
        reveal(id, in: world)
        layout(world, animated: true, confineFloating: true)
    }

    /// Takes a window out of the layout and floats it on its Desktop.
    func float(_ id: WindowID, animated: Bool) {
        guard let world = world(of: id), let tw = tracker.windows[id] else { return }
        let target = world.arrangement.remove(id, shareSpace: prefs.shareRemovedSpace)
        floatRestore[id] = (world.uuid, target ?? .floating)
        let bounds = world.plan?.desktopRect ?? world.display.visible
        let current = applied[id] ?? tw.frame ?? bounds
        var r = floatFrames[id] ?? CanopyCore.Rect(x: bounds.midX - current.width * 0.4, y: bounds.midY - current.height * 0.4,
                                                   width: current.width * 0.8, height: current.height * 0.8)
        r.width = min(r.width, bounds.width)
        r.height = min(r.height, bounds.height)
        r = r.clamped(into: bounds)
        parked.remove(id)
        applied[id] = nil
        world.navigator.prune(liveWindows: Set(world.arrangement.windows))
        layout(world, animated: animated, floatTo: [id: r])
    }

    func reveal(_ id: WindowID, in world: World) {
        if let t = world.arrangement.tile(of: id) { world.navigator.reveal(t, in: world.arrangement) }
    }

    /// Docks every floating window of a display (Arrange Windows).
    func arrange(_ world: World) {
        refreshFrames()
        let floating = floatingWindows(in: world).filter { parkedFloating[$0.id] == nil }
            .sorted { ($0.frame?.minX ?? 0) < ($1.frame?.minX ?? 0) }
        guard !floating.isEmpty else { return }
        for w in floating {
            if let f = w.frame { floatFrames[w.id] = f }
            let rects = Solver.solve(world.arrangement.root, in: Solver.bounds(for: world.display.visible, gap: prefs.gap),
                                     gap: prefs.gap).nodeRects
            world.arrangement.autoDock(w.id, rects: rects)
        }
        world.navigator.everything(in: world.arrangement)
        layout(world, animated: true)
    }

    /// Floats every window on its Desktop: back to a normal Mac.
    func floatAll() {
        for world in worlds.values {
            let v = world.display.visible
            var moves: [WindowID: CanopyCore.Rect] = [:]
            var offset = 0.0
            for id in world.arrangement.windows {
                world.arrangement.remove(id, shareSpace: true)
                parked.remove(id)
                let size = floatFrames[id]?.size ?? applied[id]?.size ?? CanopyCore.Size(width: 900, height: 650)
                applied[id] = nil
                moves[id] = CanopyCore.Rect(x: v.x + 40 + offset, y: v.y + 40 + offset,
                                            width: min(max(size.width, 400), v.width * 0.8),
                                            height: min(max(size.height, 300), v.height * 0.8)).clamped(into: v)
                offset = offset >= 240 ? 0 : offset + 30
            }
            world.arrangement.setDesktopHidden(false)
            world.navigator.everything(in: world.arrangement)
            layout(world, animated: true, floatTo: moves)
        }
    }

    func setIgnored(_ bundleID: String, _ ignored: Bool) {
        var p = prefs
        p.ignoredBundleIDs.removeAll { $0 == bundleID }
        if ignored { p.ignoredBundleIDs.append(bundleID) }
        prefs = p
    }

    // MARK: - Commands

    func run(_ command: Command) {
        if case .settings(let pane) = command { openSettings?(pane); return }
        guard started else { return }
        if command == .floatAll { floatAll(); return }
        guard let world = currentWorld() else { return }
        let a = world.arrangement, nav = world.navigator
        var moved = false
        switch command {
        case .everything:
            moved = nav.everything(in: a)
        case .stepOut:
            moved = nav.stepOut(in: a)
        case .stepIn:
            moved = nav.stepIn(in: a, focused: tracker.focusedWindowID(), pointer: Coordinates.mouse,
                               rects: world.plan?.layout.nodeRects ?? [:], preferPointer: prefs.stepTowardPointer)
        case .back:
            moved = nav.back(in: a)
        case .forward:
            moved = nav.forward(in: a)
        case .neighbour(let dir):
            let rects = Solver.solve(a.root, in: Solver.bounds(for: world.display.visible, gap: prefs.gap), gap: prefs.gap).nodeRects
            moved = nav.neighbour(dir, in: a, rects: rects)
        case .frameWindow:
            if let f = tracker.focusedWindowID(), let t = a.tile(of: f) { moved = nav.toggleFrame(t, in: a) }
        case .frameDesktop:
            if a.desktop.hidden { a.setDesktopHidden(false) }
            nav.toggleFrame(a.desktop, in: a)
            moved = true
        case .toggleDesktop:
            let hide = !a.desktop.hidden
            if hide, nav.framedNode(in: a).isDesktop { nav.everything(in: a) }
            a.setDesktopHidden(hide)
            moved = true
        case .goToView(let n):
            moved = nav.go(to: n, in: a)
        case .saveView(let n):
            nav.save(n, in: a)
            scheduleSave()
            onStateChange?()
        case .splitFinder:
            splitFinder(in: world)
        case .arrange:
            arrange(world)
        case .floatAll, .settings:
            break
        }
        if moved { layout(world, animated: true) }
    }

    private func splitFinder(in world: World) {
        let tile = tracker.focusedWindowID().flatMap { world.arrangement.tile(of: $0) }
        pendingSplit = (tile?.id, world.uuid, Date().addingTimeInterval(6))
        DispatchQueue.global(qos: .userInitiated).async {
            let script = NSAppleScript(source: """
            tell application "Finder"
                make new Finder window to home
                activate
            end tell
            """)
            var error: NSDictionary?
            script?.executeAndReturnError(&error)
            if error != nil {
                DispatchQueue.main.async { NSWorkspace.shared.open(FileManager.default.homeDirectoryForCurrentUser) }
            }
        }
    }

    // MARK: - Tracker events

    func windowAppeared(_ w: TrackedWindow) {
        publishInput()
        guard started else { return }
        if let p = pendingSplit, Date() < p.until, w.bundleID == "com.apple.finder", let world = worlds[p.world] {
            pendingSplit = nil
            if let f = w.frame { floatFrames[w.id] = f }
            if let id = p.tile, let tile = world.arrangement.node(id), let r = world.plan?.layout.nodeRects[id] {
                world.arrangement.dock(w.id, beside: tile, edge: r.width >= r.height ? .right : .bottom)
            } else {
                world.arrangement.autoDock(w.id, rects: world.plan?.layout.nodeRects ?? [:])
            }
            reveal(w.id, in: world)
            layout(world, animated: true)
        }
        // Otherwise new windows float where they open.
    }

    func windowDisappeared(_ id: WindowID) {
        pictures.forget(id)
        parked.remove(id)
        applied[id] = nil
        parkedFloating[id] = nil
        away[id] = nil
        floatRestore[id] = nil
        floatFrames[id] = nil
        lastWrite[id] = nil
        publishInput()
        guard started, let world = world(of: id) else { return }
        world.arrangement.remove(id, shareSpace: prefs.shareRemovedSpace)
        world.navigator.prune(liveWindows: Set(world.arrangement.windows))
        layout(world, animated: true)
    }

    private func goAway(_ id: WindowID) {
        guard let world = world(of: id) else { return }
        let target = world.arrangement.remove(id, shareSpace: prefs.shareRemovedSpace)
        away[id] = (world.uuid, target ?? .floating)
        parked.remove(id)
        applied[id] = nil
        layout(world, animated: true)
    }

    private func comeBack(_ id: WindowID) {
        guard let entry = away.removeValue(forKey: id), let world = worlds[entry.world] else { return }
        if case .floating = entry.target { return }
        if world.arrangement.restore(id, to: entry.target) == nil {
            world.arrangement.autoDock(id, rects: world.plan?.layout.nodeRects ?? [:])
        }
        reveal(id, in: world)
        layout(world, animated: true)
    }

    func windowMinimized(_ id: WindowID, _ minimized: Bool) {
        publishInput()
        guard started else { return }
        minimized ? goAway(id) : comeBack(id)
    }

    func windowFullScreen(_ id: WindowID, _ fullScreen: Bool) {
        publishInput()
        guard started else { return }
        fullScreen ? goAway(id) : comeBack(id)
    }

    func appHidden(_ pid: pid_t, _ hidden: Bool) {
        publishInput()
        guard started else { return }
        for w in tracker.windows.values where w.pid == pid { hidden ? goAway(w.id) : comeBack(w.id) }
    }

    func windowMovedOrResized(_ id: WindowID) {
        guard started, let tw = tracker.windows[id] else { return }
        // Our own writes echo back as notifications; ignore them.
        if writer.isBusy(id) || CFAbsoluteTimeGetCurrent() - (lastWrite[id] ?? 0) < 0.4 { return }
        if press?.window == id { return }
        if let f = WindowServer.frame(of: id) { tw.frame = f }
        if NSEvent.pressedMouseButtons & 1 != 0 {
            movedWhileMouseDown.insert(id)
            return
        }
        guard let world = world(of: id), let want = applied[id], let f = tw.frame, !near(f, want) else { return }
        // Something else moved a docked window: put it back, a few times at most.
        let now = CFAbsoluteTimeGetCurrent()
        var recent = (snapBacks[id] ?? []).filter { now - $0 < 5 }
        guard recent.count < 3 else { return }
        recent.append(now)
        snapBacks[id] = recent
        applied[id] = nil
        scheduleRelayout(world)
    }

    func windowTitleChanged(_ id: WindowID) {
        guard let world = world(of: id), let plan = world.plan,
              let tile = world.arrangement.tile(of: id), plan.layout.stripRects[tile.id] != nil else { return }
        updateOverlays(world, plan)
    }

    func focusChanged(_ id: WindowID?) {
        guard started, let id else { return }
        guard let world = world(of: id), let tile = world.arrangement.tile(of: id) else {
            // A floating window parked with its hidden Desktop comes back with it.
            if let entry = parkedFloating[id], let w = worlds[entry.world] {
                w.arrangement.setDesktopHidden(false)
                w.navigator.reveal(w.arrangement.desktop, in: w.arrangement)
                layout(w, animated: true)
            }
            return
        }
        var changed = false
        if tile.selectedWindow != id {
            world.arrangement.select(id)
            changed = true
        }
        let min = minimum(for: id)
        let visible = world.display.visible, gap = prefs.gap
        if world.navigator.bringIntoView(tile, in: world.arrangement, fits: {
            Planner.fits(tile, framing: $0, visibleFrame: visible, gap: gap, minimum: min)
        }) {
            changed = true
        }
        if changed {
            layout(world, animated: true)
        } else if let panel = strips[key(world, tile.id)] {
            panel.order(.above, relativeTo: Int(id))
        }
    }

    func processMovedWhileMouseDown() {
        let ids = movedWhileMouseDown
        movedWhileMouseDown = []
        var touched: [String: World] = [:]
        for id in ids {
            guard let world = world(of: id) else { continue }
            applied[id] = nil
            touched[world.uuid] = world
        }
        for w in touched.values { layout(w, animated: false) }
    }
}

/// A menu item that runs a closure.
final class ClosureMenuItem: NSMenuItem {
    private let handler: () -> Void

    init(_ title: String, key: String = "", modifiers: NSEvent.ModifierFlags = [], handler: @escaping () -> Void) {
        self.handler = handler
        super.init(title: title, action: #selector(fire), keyEquivalent: key)
        keyEquivalentModifierMask = modifiers
        target = self
    }

    required init(coder: NSCoder) { fatalError("init(coder:) is not used") }

    @objc private func fire() { handler() }
}
