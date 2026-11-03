import AppKit
import CanopyCore

/// End-to-end test driver: `Canopy --harness <bundle-id>` manages only that
/// app's windows and takes one command per line on stdin, answering with one
/// line of JSON. Used by scripts/e2e.py; not part of normal use.
///
///   dump                                  the arrangement and every managed window
///   dock <title> screen <edge>            dock at a screen edge
///   dock <title> <other> <edge>           dock beside another window's tile
///   tab <title> <other>                   join another window's tab group
///   float <title>                         drop on the Desktop
///   toggle <title>                        the green button
///   frame <title>                         double-click the title bar
///   select <title>                        click its tab
///   raise <title>                         bring it to the front
///   close <title> | minimize <title> | restore <title>
///   url <canopy://…>                      a command by URL
///   With --tap, real mouse input through the event tap (each press first
///   checks that a test window, or Canopy's own handle, is under the pointer):
///   drag <title> <x> <y> [hold]           drag the title bar to a point
///   green <title>                         click the green button
///   dblclick <title>                      double-click the title bar
///   seam <n> <delta>                      drag gap handle n along its axis
///   quit
final class Harness {
    private let coordinator: Coordinator
    private let delegate: AppDelegate
    private var buffer = Data()

    init(coordinator: Coordinator, delegate: AppDelegate) {
        self.coordinator = coordinator
        self.delegate = delegate
    }

    func start() {
        FileHandle.standardInput.readabilityHandler = { [weak self] h in
            let data = h.availableData
            DispatchQueue.main.async { self?.receive(data) }
        }
        reply(["ready": true])
    }

    private func receive(_ data: Data) {
        if data.isEmpty { NSApp.terminate(nil); return }
        buffer.append(data)
        while let nl = buffer.firstIndex(of: 0x0A) {
            let line = String(decoding: buffer[buffer.startIndex..<nl], as: UTF8.self)
            buffer.removeSubrange(buffer.startIndex...nl)
            handle(line.trimmingCharacters(in: .whitespaces))
        }
    }

    private func window(_ title: String) -> TrackedWindow? {
        coordinator.tracker.windows.values.first { $0.title == title && coordinator.manageable($0) }
            ?? coordinator.tracker.windows.values.first { $0.title == title }
    }

    private func edge(_ s: String) -> Edge? { Edge(rawValue: s) }

    private func handle(_ line: String) {
        let parts = line.split(separator: " ").map(String.init)
        guard let verb = parts.first else { return }
        let c = coordinator
        switch verb {
        case "dump":
            reply(dump())
            return
        case "session":
            // The drag in progress: its target, and what the stage shows for it.
            guard let s = c.press?.session else { reply(["ok": true, "dragging": false]); return }
            reply(["ok": true, "dragging": true, "target": "\(s.target)", "thumbnail": s.thumbnail,
                   "slot": c.stage.panel(for: s.display).slotStyle ?? "none"])
            return
        case "log":
            reply(["ok": true, "log": Log.take()])
            return
        case "stall":
            reply(["ok": true, "worstStallMs": Int(c.resetStallMeter() * 1000), "tap": c.tapRunning])
            return
        case "dock" where parts.count == 4:
            guard let w = window(parts[1]), let e = edge(parts[3]) else { return fail(line) }
            if parts[2] == "screen" {
                guard let world = c.currentWorld() else { return fail(line) }
                c.drop(w.id, .screenEdge(e), into: world, wholeTile: false)
            } else {
                guard let other = window(parts[2]), let world = c.world(of: other.id),
                      let tile = world.arrangement.tile(of: other.id) else { return fail(line) }
                c.drop(w.id, .beside(tile.id, e), into: world, wholeTile: false)
            }
        case "tab" where parts.count == 3:
            guard let w = window(parts[1]), let other = window(parts[2]), let world = c.world(of: other.id),
                  let tile = world.arrangement.tile(of: other.id) else { return fail(line) }
            c.drop(w.id, .tab(tile.id), into: world, wholeTile: false)
        case "float" where parts.count == 2:
            guard let w = window(parts[1]), let world = c.world(of: w.id) else { return fail(line) }
            c.drop(w.id, .float, into: world, wholeTile: false)
        case "toggle" where parts.count == 2:
            guard let w = window(parts[1]) else { return fail(line) }
            c.toggleDocked(w.id)
        case "frame" where parts.count == 2:
            guard let w = window(parts[1]), let world = c.world(of: w.id),
                  let tile = world.arrangement.tile(of: w.id) else { return fail(line) }
            if world.navigator.toggleFrame(tile, in: world.arrangement) { c.layout(world, animated: true) }
        case "raise" where parts.count == 2:
            guard let w = window(parts[1]) else { return fail(line) }
            c.focus(w.id)
        case "select" where parts.count == 2:
            guard let w = window(parts[1]), let world = c.world(of: w.id) else { return fail(line) }
            world.arrangement.select(w.id)
            c.layout(world, animated: false)
        case "close" where parts.count == 2:
            guard let w = window(parts[1]) else { return fail(line) }
            c.close(w.id)
        case "minimize" where parts.count == 2:
            guard let w = window(parts[1]) else { return fail(line) }
            AXService.shared.queue(for: w.pid).async { w.ax.minimize() }
        case "restore" where parts.count == 2:
            guard let w = window(parts[1]) else { return fail(line) }
            AXService.shared.queue(for: w.pid).async { AX.set(w.ax.element, kAXMinimizedAttribute, kCFBooleanFalse) }
        case "drag" where parts.count >= 4:
            guard let w = window(parts[1]), let f = WindowServer.frame(of: w.id)?.cg, let x = Double(parts[2]), let y = Double(parts[3]) else {
                return fail(line)
            }
            let start = CGPoint(x: f.midX, y: f.minY + 12)
            guard safe(start, for: w) else { return fail("unsafe: \(line)") }
            let hold = parts.count > 4 ? Double(parts[4]) ?? 0 : 0
            Mouse.drag(from: start, to: CGPoint(x: x, y: y), hold: hold) { [weak self] in self?.reply(["ok": true]) }
            return
        case "green" where parts.count == 2:
            guard let w = window(parts[1]), let b = AX.element(w.ax.element, "AXFullScreenButton"),
                  let o = AX.point(b, kAXPositionAttribute), let sz = AX.size(b, kAXSizeAttribute) else { return fail(line) }
            let p = CGPoint(x: o.x + sz.width / 2, y: o.y + sz.height / 2)
            guard safe(p, for: w) else { return fail("unsafe: \(line)") }
            Mouse.click(at: p, count: 1) { [weak self] in self?.reply(["ok": true]) }
            return
        case "dblclick" where parts.count == 2:
            guard let w = window(parts[1]), let f = WindowServer.frame(of: w.id)?.cg else { return fail(line) }
            let p = CGPoint(x: f.midX, y: f.minY + 12)
            guard safe(p, for: w) else { return fail("unsafe: \(line)") }
            Mouse.click(at: p, count: 2) { [weak self] in self?.reply(["ok": true]) }
            return
        case "marquee" where parts.count == 5:
            // ⌘⇧-drag a rectangle that starts inside a test window, so nothing else is ever clicked.
            guard let x0 = Double(parts[1]), let y0 = Double(parts[2]), let x1 = Double(parts[3]), let y1 = Double(parts[4]),
                  let (id, _) = Hit.window(at: CanopyCore.Point(x: x0, y: y0)), let w = c.tracker.windows[id],
                  safe(CGPoint(x: x0, y: y0), for: w) else { return fail("unsafe: \(line)") }
            Mouse.drag(from: CGPoint(x: x0, y: y0), to: CGPoint(x: x1, y: y1), hold: 0, flags: [.maskCommand, .maskShift]) { [weak self] in
                self?.reply(["ok": true])
            }
            return
        case "pinch" where parts.count == 4:
            // The pinch path after the event tap: a stream of totals, then the lift.
            guard let x = Double(parts[1]), let y = Double(parts[2]), let total = Double(parts[3]) else { return fail(line) }
            let p = CanopyCore.Point(x: x, y: y)
            for i in 1...12 {
                DispatchQueue.main.asyncAfter(deadline: .now() + Double(i) * 0.03) { c.pinch(total: total * Double(i) / 12, at: p, ended: false) }
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.45) {
                c.pinch(total: total, at: p, ended: true)
                self.reply(["ok": true])
            }
            return
        case "seam" where parts.count == 3:
            guard let n = Int(parts[1]), let delta = Double(parts[2]), let world = c.activeWorlds.first,
                  let seams = world.plan?.layout.seams, seams.indices.contains(n) else { return fail(line) }
            let seam = seams[n]
            let start = CGPoint(x: seam.rect.midX, y: seam.rect.midY)
            let end = seam.axis == .horizontal ? CGPoint(x: start.x + delta, y: start.y) : CGPoint(x: start.x, y: start.y + delta)
            Mouse.drag(from: start, to: end, hold: 0) { [weak self] in self?.reply(["ok": true]) }
            return
        case "key" where parts.count == 3:
            // A key press with modifiers, e.g. "key 124 ctrl" (next desktop).
            guard let code = UInt16(parts[1]) else { return fail(line) }
            var flags: CGEventFlags = []
            for m in parts[2].split(separator: ",") {
                switch m {
                case "ctrl": flags.insert(.maskControl)
                case "opt": flags.insert(.maskAlternate)
                case "shift": flags.insert(.maskShift)
                case "cmd": flags.insert(.maskCommand)
                default: break
                }
            }
            // Arrow keys from a real keyboard carry these too; shortcuts expect them.
            if (123...126).contains(code) { flags.formUnion([.maskNumericPad, .maskSecondaryFn]) }
            for down in [true, false] {
                let e = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down)
                e?.flags = flags
                e?.post(tap: .cghidEventTap)
            }
        case "url" where parts.count == 2:
            guard let url = URL(string: parts[1]) else { return fail(line) }
            delegate.application(NSApp, open: [url])
        case "quit":
            reply(["ok": true])
            NSApp.terminate(nil)
            return
        default:
            return fail(line)
        }
        reply(["ok": true])
    }

    /// The topmost window at `p` must be the test window we mean to touch.
    private func safe(_ p: CGPoint, for w: TrackedWindow) -> Bool {
        let own = ProcessInfo.processInfo.processIdentifier
        guard let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
            as? [[String: Any]] else { return false }
        for info in list {
            guard (info[kCGWindowOwnerPID as String] as? pid_t) != own,
                  (info[kCGWindowLayer as String] as? Int) == 0,
                  let b = info[kCGWindowBounds as String] as? NSDictionary,
                  let r = CGRect(dictionaryRepresentation: b), r.contains(p) else { continue }
            return (info[kCGWindowNumber as String] as? Int).map { WindowID($0) } == w.id
        }
        return false
    }

    private func fail(_ line: String) { reply(["ok": false, "error": "bad command: \(line)"]) }

    private func dump() -> [String: Any] {
        let c = coordinator
        let names: [WindowID: String] = Dictionary(uniqueKeysWithValues: c.tracker.windows.values.map { ($0.id, $0.title) })
        func named(_ s: String) -> String {
            // Replace window ids in a tree description with titles.
            var out = s
            for (id, name) in names.sorted(by: { String($0.key).count > String($1.key).count }) {
                out = out.replacingOccurrences(of: "\(id)", with: name)
            }
            return out
        }
        var worlds: [[String: Any]] = []
        for w in c.worlds.values.sorted(by: { c.isActive($0) && !c.isActive($1) }) {
            var entry: [String: Any] = [
                "active": c.isActive(w),
                "space": w.space,
                "visible": [w.display.visible.x, w.display.visible.y, w.display.visible.width, w.display.visible.height],
                "tree": named(w.arrangement.root.description),
                "framed": named(w.navigator.framedRange(in: w.arrangement)
                    .map { "range:" + $0.map(\.description).joined(separator: ",") }
                    ?? w.navigator.framedNode(in: w.arrangement).description),
                "desktopHidden": w.arrangement.desktop.hidden,
                "savedViews": w.navigator.savedViews.keys.sorted(),
                "canGoBack": w.navigator.canGoBack,
            ]
            if let p = w.plan {
                entry["visibleWindows"] = Dictionary(uniqueKeysWithValues: p.visible.map { (names[$0.key] ?? "\($0.key)", [$0.value.x, $0.value.y, $0.value.width, $0.value.height]) })
                entry["covers"] = p.covers.keys.compactMap { w.arrangement.node($0)?.selectedWindow.flatMap { names[$0] } }.sorted()
                entry["seams"] = p.layout.seams.count
            }
            worlds.append(entry)
        }
        var windows: [[String: Any]] = []
        let onScreen = Set(WindowServer.onScreen().map(\.id))
        for tw in c.tracker.windows.values where c.onlyBundleID == nil || tw.bundleID == c.onlyBundleID {
            let f = WindowServer.frame(of: tw.id)?.cg ?? tw.frame?.cg ?? .zero
            windows.append([
                "id": Int(tw.id),
                "title": tw.title,
                "onScreen": onScreen.contains(tw.id),
                "frame": [f.minX, f.minY, f.width, f.height],
                "docked": c.world(of: tw.id) != nil,
                "parked": c.parked.contains(tw.id),
                "minimized": tw.minimized,
            ])
        }
        return ["worlds": worlds, "windows": windows.sorted { ($0["title"] as! String) < ($1["title"] as! String) }]
    }

    private func reply(_ object: [String: Any]) {
        if let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]),
           let s = String(data: data, encoding: .utf8) {
            print(s)
            fflush(stdout)
        }
    }
}

/// Posts real mouse events, paced like a hand, then puts the pointer back.
enum Mouse {
    private static func post(_ type: CGEventType, _ p: CGPoint, clicks: Int64 = 1, flags: CGEventFlags = []) {
        guard let e = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: p, mouseButton: .left) else { return }
        e.setIntegerValueField(.mouseEventClickState, value: clicks)
        if !flags.isEmpty { e.flags = flags }
        e.post(tap: .cghidEventTap)
    }

    private static var home: CGPoint { CGEvent(source: nil)?.location ?? .zero }

    static func drag(from a: CGPoint, to b: CGPoint, hold: Double, flags: CGEventFlags = [], done: @escaping () -> Void) {
        let back = home
        post(.mouseMoved, a)
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) {
            post(.leftMouseDown, a, flags: flags)
            let steps = 24
            for i in 1...steps {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.15 + Double(i) * 0.025) {
                    let t = Double(i) / Double(steps)
                    post(.leftMouseDragged, CGPoint(x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t), flags: flags)
                }
            }
            let end = 0.15 + Double(steps) * 0.025 + 0.2 + hold
            if hold > 0 {
                // Keep a tiny movement going so the hold timer sees a live drag.
                DispatchQueue.main.asyncAfter(deadline: .now() + end - 0.1) { post(.leftMouseDragged, b) }
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + end) {
                post(.leftMouseUp, b, flags: flags)
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) {
                    post(.mouseMoved, back)
                    done()
                }
            }
        }
    }

    static func click(at p: CGPoint, count: Int, done: @escaping () -> Void) {
        let back = home
        post(.mouseMoved, p)
        var t = 0.15
        for n in 1...count {
            DispatchQueue.main.asyncAfter(deadline: .now() + t) { post(.leftMouseDown, p, clicks: Int64(n)) }
            DispatchQueue.main.asyncAfter(deadline: .now() + t + 0.04) { post(.leftMouseUp, p, clicks: Int64(n)) }
            t += 0.12
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + t + 0.1) {
            post(.mouseMoved, back)
            done()
        }
    }
}
