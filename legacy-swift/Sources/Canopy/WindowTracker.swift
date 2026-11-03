import AppKit
import ApplicationServices
import CanopyCore

/// A window Canopy knows about. Main thread only.
final class TrackedWindow {
    let ax: AXWindow
    let bundleID: String
    let appName: String
    var title: String
    var minimumSize: CanopyCore.Size?
    var minimized = false
    var fullScreen = false
    var hidden = false
    /// Last known frame: from the window server, or what Canopy last wrote.
    var frame: CanopyCore.Rect?

    init(ax: AXWindow, bundleID: String, appName: String, title: String) {
        self.ax = ax
        self.bundleID = bundleID
        self.appName = appName
        self.title = title
    }

    var id: WindowID { WindowID(ax.id) }
    var pid: pid_t { ax.pid }
    var ref: CanopyCore.WindowRef { CanopyCore.WindowRef(bundleID: bundleID, title: title) }
}

protocol WindowTrackerDelegate: AnyObject {
    func windowAppeared(_ w: TrackedWindow)
    func windowDisappeared(_ id: WindowID)
    func windowMinimized(_ id: WindowID, _ minimized: Bool)
    func windowFullScreen(_ id: WindowID, _ fullScreen: Bool)
    func appHidden(_ pid: pid_t, _ hidden: Bool)
    func windowMovedOrResized(_ id: WindowID)
    func windowTitleChanged(_ id: WindowID)
    func focusChanged(_ id: WindowID?)
}

/// What one AX read of a window found. Built on the app's queue.
private struct Probe {
    let element: AXUIElement
    let id: WindowID
    let role: String?
    let subrole: String?
    let title: String
    let minimized: Bool
    let fullScreen: Bool
    let frame: CGRect?

    init?(_ el: AXUIElement) {
        guard let cg = AX.windowID(el) else { return nil }
        element = el
        id = WindowID(cg)
        role = AX.string(el, kAXRoleAttribute)
        subrole = AX.string(el, kAXSubroleAttribute)
        title = AX.string(el, kAXTitleAttribute) ?? ""
        minimized = AX.bool(el, kAXMinimizedAttribute) ?? false
        fullScreen = AX.bool(el, "AXFullScreen") ?? false
        if let p = AX.point(el, kAXPositionAttribute), let s = AX.size(el, kAXSizeAttribute) {
            frame = CGRect(origin: p, size: s)
        } else {
            frame = nil
        }
    }

    var isStandard: Bool { role == kAXWindowRole as String && subrole == kAXStandardWindowSubrole as String }
}

/// Watches every regular app's windows. AX observers live on their own
/// thread and every AX read runs on the app's queue; only results reach the
/// main thread, so a slow app never stalls Canopy.
final class WindowTracker {
    weak var delegate: WindowTrackerDelegate?
    private(set) var windows: [WindowID: TrackedWindow] = [:]
    /// The focused window of the frontmost app, as last reported.
    private(set) var focused: WindowID?
    private var apps: [pid_t: AppWatch] = [:]
    /// Watches of apps that quit; kept so a late callback never hits freed memory.
    private var retired: [AppWatch] = []
    private var rescanTimer: Timer?
    private let ownPID = ProcessInfo.processInfo.processIdentifier
    private let observerThread = RunLoopThread(name: "canopy.ax-observers")

    func start() {
        observerThread.startAndWait()
        let nc = NSWorkspace.shared.notificationCenter
        nc.addObserver(forName: NSWorkspace.didLaunchApplicationNotification, object: nil, queue: .main) { [weak self] n in
            if let app = n.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication { self?.watch(app) }
        }
        nc.addObserver(forName: NSWorkspace.didTerminateApplicationNotification, object: nil, queue: .main) { [weak self] n in
            if let app = n.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication { self?.unwatch(app.processIdentifier) }
        }
        nc.addObserver(forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main) { [weak self] n in
            if let app = n.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication {
                self?.refreshFocus(app.processIdentifier)
            }
        }
        nc.addObserver(forName: NSWorkspace.didHideApplicationNotification, object: nil, queue: .main) { [weak self] n in
            if let app = n.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication {
                self?.setHidden(app.processIdentifier, true)
            }
        }
        nc.addObserver(forName: NSWorkspace.didUnhideApplicationNotification, object: nil, queue: .main) { [weak self] n in
            if let app = n.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication {
                self?.setHidden(app.processIdentifier, false)
            }
        }
        for app in NSWorkspace.shared.runningApplications { watch(app) }
        rescanTimer = Timer.scheduledTimer(withTimeInterval: 3, repeats: true) { [weak self] _ in self?.rescanAll() }
        if let front = NSWorkspace.shared.frontmostApplication { refreshFocus(front.processIdentifier) }
    }

    func stop() {
        rescanTimer?.invalidate()
        for pid in apps.keys { unwatch(pid) }
    }

    // MARK: Apps

    private func watch(_ app: NSRunningApplication) {
        guard app.activationPolicy == .regular, app.processIdentifier != ownPID,
              apps[app.processIdentifier] == nil, let runLoop = observerThread.runLoop else { return }
        let watch = AppWatch(app: app, tracker: self)
        apps[app.processIdentifier] = watch
        watch.start(on: runLoop)
        scan(app.processIdentifier)
        // Some apps only answer once they have finished launching.
        if !app.isFinishedLaunching {
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak self] in self?.scan(app.processIdentifier) }
        }
    }

    private func unwatch(_ pid: pid_t) {
        guard let watch = apps.removeValue(forKey: pid) else { return }
        watch.stop()
        retired.append(watch)
        AXService.shared.forget(pid)
        for (id, w) in windows where w.pid == pid {
            windows[id] = nil
            delegate?.windowDisappeared(id)
        }
    }

    private func setHidden(_ pid: pid_t, _ hidden: Bool) {
        for w in windows.values where w.pid == pid { w.hidden = hidden }
        delegate?.appHidden(pid, hidden)
    }

    // MARK: Windows

    /// Finds an app's new windows and drops ones that have gone, off the main thread.
    func scan(_ pid: pid_t) {
        guard let watch = apps[pid] else { return }
        let known = Set(windows.values.filter { $0.pid == pid }.map(\.id))
        AXService.shared.run(pid, { () -> (probes: [Probe], gone: [WindowID]) in
            let probes = AX.elements(watch.element, kAXWindowsAttribute).compactMap(Probe.init)
            let seen = Set(probes.map(\.id))
            // AX lists only the current Space's windows; keep others that still answer.
            let gone = known.subtracting(seen).filter { id in
                guard let el = watch.element(for: id) else { return true }
                return AX.string(el, kAXRoleAttribute) == nil
            }
            return (probes, Array(gone))
        }, then: { [weak self] result in
            guard let self, self.apps[pid] != nil else { return }
            for p in result.probes where self.windows[p.id] == nil { self.adopt(p, watch: watch) }
            for id in result.gone where self.windows[id] != nil {
                self.windows[id] = nil
                self.delegate?.windowDisappeared(id)
            }
        })
    }

    private func rescanAll() {
        for pid in apps.keys { scan(pid) }
    }

    private func adopt(_ p: Probe, watch: AppWatch) {
        guard p.isStandard, windows[p.id] == nil else { return }
        let ax = AXWindow(element: p.element, app: watch.element, pid: watch.pid, id: CGWindowID(p.id))
        let w = TrackedWindow(ax: ax, bundleID: watch.bundleID, appName: watch.name, title: p.title)
        w.minimized = p.minimized
        w.fullScreen = p.fullScreen
        w.hidden = watch.app.isHidden
        w.frame = p.frame.map(CanopyCore.Rect.init)
        windows[w.id] = w
        watch.observeWindow(p.element, id: p.id)
        delegate?.windowAppeared(w)
    }

    /// A window that just appeared can report its subrole a moment late.
    fileprivate func consider(_ el: AXUIElement, watch: AppWatch, attempt: Int = 0) {
        AXService.shared.run(watch.pid, { Probe(el) }, then: { [weak self] probe in
            guard let self, let probe, self.windows[probe.id] == nil, self.apps[watch.pid] != nil else { return }
            if probe.isStandard {
                self.adopt(probe, watch: watch)
            } else if probe.role == kAXWindowRole as String, attempt < 3 {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) { self.consider(el, watch: watch, attempt: attempt + 1) }
            }
        })
    }

    /// Called on the observer thread; hands the work to the app's queue.
    fileprivate func handle(_ notification: String, element: AXUIElement, watch: AppWatch) {
        switch notification {
        case kAXWindowCreatedNotification:
            DispatchQueue.main.async { self.consider(element, watch: watch) }
        case kAXFocusedWindowChangedNotification, kAXMainWindowChangedNotification:
            DispatchQueue.main.async {
                self.consider(element, watch: watch)
                self.refreshFocus(watch.pid)
            }
        case kAXUIElementDestroyedNotification:
            let id = watch.id(of: element)
            DispatchQueue.main.async {
                guard let id, self.windows[id] != nil else { return }
                self.windows[id] = nil
                self.delegate?.windowDisappeared(id)
            }
        case kAXWindowMiniaturizedNotification, kAXWindowDeminiaturizedNotification:
            let minimized = notification == kAXWindowMiniaturizedNotification
            let id = watch.id(of: element)
            DispatchQueue.main.async {
                guard let id, let w = self.windows[id] else { return }
                w.minimized = minimized
                self.delegate?.windowMinimized(id, minimized)
            }
        case kAXWindowMovedNotification, kAXWindowResizedNotification:
            guard let id = watch.id(of: element) else { return }
            AXService.shared.run(watch.pid, { AX.bool(element, "AXFullScreen") ?? false }, then: { [weak self] fs in
                guard let self, let w = self.windows[id] else { return }
                if fs != w.fullScreen {
                    w.fullScreen = fs
                    self.delegate?.windowFullScreen(id, fs)
                } else {
                    self.delegate?.windowMovedOrResized(id)
                }
            })
        case kAXTitleChangedNotification:
            guard let id = watch.id(of: element) else { return }
            AXService.shared.run(watch.pid, { AX.string(element, kAXTitleAttribute) ?? "" }, then: { [weak self] title in
                guard let self, let w = self.windows[id], w.title != title else { return }
                w.title = title
                self.delegate?.windowTitleChanged(id)
            })
        default:
            break
        }
    }

    private func refreshFocus(_ pid: pid_t) {
        guard let watch = apps[pid] else { return }
        AXService.shared.run(pid, { () -> WindowID? in
            AX.element(watch.element, kAXFocusedWindowAttribute).flatMap(AX.windowID).map { WindowID($0) }
        }, then: { [weak self] id in
            guard let self, NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else { return }
            let tracked = id.flatMap { self.windows[$0] != nil ? $0 : nil }
            guard tracked != self.focused else { return }
            self.focused = tracked
            self.delegate?.focusChanged(tracked)
        })
    }

    /// The focused window of the frontmost app, if Canopy tracks it.
    func focusedWindowID() -> WindowID? {
        guard let f = focused, windows[f] != nil else { return nil }
        return f
    }
}

/// The AXObserver of one app. Its callbacks arrive on the observer thread.
fileprivate final class AppWatch {
    let app: NSRunningApplication
    let pid: pid_t
    let element: AXUIElement
    let bundleID: String
    let name: String
    weak var tracker: WindowTracker?
    private var observer: AXObserver?
    private var runLoop: CFRunLoop?
    /// Window elements by id, for notifications about elements that can no longer answer.
    private let lock = NSLock()
    private var elements: [WindowID: AXUIElement] = [:]

    init(app: NSRunningApplication, tracker: WindowTracker) {
        self.app = app
        pid = app.processIdentifier
        element = AXUIElementCreateApplication(pid)
        bundleID = app.bundleIdentifier ?? "pid.\(app.processIdentifier)"
        name = app.localizedName ?? bundleID
        self.tracker = tracker
        AXUIElementSetMessagingTimeout(element, 1.0)
    }

    private static let appNotifications = [
        kAXWindowCreatedNotification, kAXFocusedWindowChangedNotification, kAXMainWindowChangedNotification,
    ]
    private static let windowNotifications = [
        kAXUIElementDestroyedNotification, kAXWindowMiniaturizedNotification, kAXWindowDeminiaturizedNotification,
        kAXWindowMovedNotification, kAXWindowResizedNotification, kAXTitleChangedNotification,
    ]

    func start(on runLoop: CFRunLoop) {
        let callback: AXObserverCallback = { _, element, notification, refcon in
            guard let refcon else { return }
            let watch = Unmanaged<AppWatch>.fromOpaque(refcon).takeUnretainedValue()
            watch.tracker?.handle(notification as String, element: element, watch: watch)
        }
        var obs: AXObserver?
        guard AXObserverCreate(pid, callback, &obs) == .success, let obs else { return }
        observer = obs
        self.runLoop = runLoop
        CFRunLoopAddSource(runLoop, AXObserverGetRunLoopSource(obs), .defaultMode)
        CFRunLoopWakeUp(runLoop)
        let refcon = Unmanaged.passUnretained(self).toOpaque()
        let element = self.element
        AXService.shared.queue(for: pid).async {
            for n in Self.appNotifications { AXObserverAddNotification(obs, element, n as CFString, refcon) }
        }
    }

    func observeWindow(_ el: AXUIElement, id: WindowID) {
        lock.lock()
        elements[id] = el
        lock.unlock()
        guard let observer else { return }
        let refcon = Unmanaged.passUnretained(self).toOpaque()
        AXService.shared.queue(for: pid).async {
            for n in Self.windowNotifications { AXObserverAddNotification(observer, el, n as CFString, refcon) }
        }
    }

    func element(for id: WindowID) -> AXUIElement? {
        lock.lock()
        defer { lock.unlock() }
        return elements[id]
    }

    /// The id of a window element, matched locally (no AX call).
    func id(of el: AXUIElement) -> WindowID? {
        lock.lock()
        defer { lock.unlock() }
        return elements.first { CFEqual($0.value, el) }?.key
    }

    func stop() {
        if let observer, let runLoop {
            CFRunLoopRemoveSource(runLoop, AXObserverGetRunLoopSource(observer), .defaultMode)
        }
        observer = nil
    }
}
