import AppKit
import ApplicationServices
import CanopyCore

/// Every Accessibility call goes through here, off the main thread. Apps
/// answer AX at their own pace (Finder takes 40+ ms a call), so each app gets
/// its own serial queue: a slow app never holds up the pointer, the layout, or
/// any other app.
final class AXService {
    static let shared = AXService()

    private let lock = NSLock()
    private var queues: [pid_t: DispatchQueue] = [:]

    func queue(for pid: pid_t) -> DispatchQueue {
        lock.lock()
        defer { lock.unlock() }
        if let q = queues[pid] { return q }
        let q = DispatchQueue(label: "canopy.ax.\(pid)", qos: .userInteractive)
        queues[pid] = q
        return q
    }

    func forget(_ pid: pid_t) {
        lock.lock()
        queues[pid] = nil
        lock.unlock()
    }

    /// Runs `work` on the app's queue, then `then` on the main thread with its result.
    func run<T>(_ pid: pid_t, _ work: @escaping () -> T, then: @escaping (T) -> Void) {
        queue(for: pid).async {
            let result = work()
            DispatchQueue.main.async { then(result) }
        }
    }
}

/// Moves and resizes windows asynchronously. Writes are coalesced: if a window
/// is asked to move again before its last move finished, only the newest
/// target is written.
final class WindowWriter {
    struct Result {
        let id: WindowID
        let asked: CGRect
        /// The size the window actually took (nil when only the position was set).
        let actualSize: CGSize?
    }

    /// Called on the main thread after each write.
    var onWritten: ((Result) -> Void)?

    private enum Job {
        case frame(CGRect)
        case position(CGPoint)
    }

    private let lock = NSLock()
    private var pending: [WindowID: (AXWindow, Job)] = [:]
    private var running = Set<WindowID>()
    private var idleWaiters: [() -> Void] = []

    /// Whether a write to this window is queued or running.
    func isBusy(_ id: WindowID) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return running.contains(id) || pending[id] != nil
    }

    /// Writes synchronously, all apps in parallel, waiting at most `timeout`.
    /// Only for quitting, when there is no later.
    func flushSynchronously(_ jobs: [(AXWindow, CGRect)], timeout: TimeInterval) {
        let group = DispatchGroup()
        for (w, r) in jobs {
            AXService.shared.queue(for: w.pid).async(group: group) { w.setFrame(r) }
        }
        _ = group.wait(timeout: .now() + timeout)
    }

    var isIdle: Bool {
        lock.lock()
        defer { lock.unlock() }
        return running.isEmpty && pending.isEmpty
    }

    func setFrame(_ w: AXWindow, _ r: CGRect) { enqueue(w, .frame(r)) }
    func setPosition(_ w: AXWindow, _ p: CGPoint) { enqueue(w, .position(p)) }

    private func enqueue(_ w: AXWindow, _ job: Job) {
        let id = WindowID(w.id)
        lock.lock()
        pending[id] = (w, job)
        let start = running.insert(id).inserted
        lock.unlock()
        if start {
            AXService.shared.queue(for: w.pid).async { [weak self] in self?.drain(id) }
        }
    }

    private func drain(_ id: WindowID) {
        while true {
            lock.lock()
            guard let (w, job) = pending.removeValue(forKey: id) else {
                running.remove(id)
                let waiters = running.isEmpty && pending.isEmpty ? idleWaiters : []
                if !waiters.isEmpty { idleWaiters = [] }
                lock.unlock()
                if !waiters.isEmpty { DispatchQueue.main.async { waiters.forEach { $0() } } }
                return
            }
            lock.unlock()
            let result: Result
            let started = CFAbsoluteTimeGetCurrent()
            switch job {
            case .frame(let r):
                w.setFrame(r)
                result = Result(id: id, asked: r, actualSize: AX.size(w.element, kAXSizeAttribute))
            case .position(let p):
                w.setPosition(p)
                result = Result(id: id, asked: CGRect(origin: p, size: .zero), actualSize: nil)
            }
            let ms = (CFAbsoluteTimeGetCurrent() - started) * 1000
            if ms > 40 { Log.add("AX slow w\(id) \(Int(ms)) ms") }
            DispatchQueue.main.async { [weak self] in self?.onWritten?(result) }
        }
    }

    /// Calls `body` on the main thread once every write has finished, or after
    /// `timeout`, whichever comes first.
    func whenIdle(timeout: TimeInterval, _ body: @escaping () -> Void) {
        var fired = false
        let once = {
            guard !fired else { return }
            fired = true
            body()
        }
        lock.lock()
        let idle = running.isEmpty && pending.isEmpty
        if !idle { idleWaiters.append(once) }
        lock.unlock()
        if idle {
            DispatchQueue.main.async(execute: once)
        } else {
            DispatchQueue.main.asyncAfter(deadline: .now() + timeout, execute: once)
        }
    }
}

// MARK: - Private window-server calls

typealias CGSConnectionID = Int32

@_silgen_name("_CGSDefaultConnection")
func _CGSDefaultConnection() -> CGSConnectionID

@_silgen_name("CGSSetConnectionProperty")
@discardableResult
func CGSSetConnectionProperty(_ cid: CGSConnectionID, _ target: CGSConnectionID, _ key: CFString, _ value: CFTypeRef) -> Int32

enum WindowServer {
    /// Lets Canopy set the pointer (resize arrows over gaps) while another app
    /// is active, which macOS otherwise ignores.
    static func setCursorInBackground() {
        let cid = _CGSDefaultConnection()
        CGSSetConnectionProperty(cid, cid, "SetsCursorInBackground" as CFString, kCFBooleanTrue)
    }

    struct OnScreen {
        let id: WindowID
        let pid: pid_t
        let frame: CanopyCore.Rect
    }

    /// Ordinary on-screen windows, front to back. One call to the window
    /// server, well under a millisecond, safe from any thread.
    static func onScreen() -> [OnScreen] {
        guard let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
            as? [[String: Any]] else { return [] }
        var out: [OnScreen] = []
        out.reserveCapacity(list.count)
        for info in list {
            guard (info[kCGWindowLayer as String] as? Int) == 0,
                  let n = info[kCGWindowNumber as String] as? Int,
                  let pid = info[kCGWindowOwnerPID as String] as? pid_t,
                  let b = info[kCGWindowBounds as String] as? NSDictionary,
                  let r = CGRect(dictionaryRepresentation: b) else { continue }
            out.append(OnScreen(id: WindowID(n), pid: pid, frame: CanopyCore.Rect(r)))
        }
        return out
    }

    /// One window's current bounds, from the window server.
    static func frame(of id: WindowID) -> CanopyCore.Rect? {
        guard let list = CGWindowListCopyWindowInfo([.optionIncludingWindow], CGWindowID(id)) as? [[String: Any]],
              let b = list.first?[kCGWindowBounds as String] as? NSDictionary,
              let r = CGRect(dictionaryRepresentation: b) else { return nil }
        return CanopyCore.Rect(r)
    }
}
