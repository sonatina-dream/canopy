import AppKit
import CanopyCore

/// Canopy: dock, nest and zoom real windows. Menu-bar-only app.
final class AppDelegate: NSObject, NSApplicationDelegate {
    let coordinator = Coordinator()
    private var menuBar: MenuBar?
    private var settings: SettingsWindowController?
    private var trustTimer: Timer?
    private var signalSources: [DispatchSourceSignal] = []
    private var harness: Harness?
    private var welcome: WelcomeWindowController?
    private let arguments = CommandLine.arguments

    func applicationDidFinishLaunching(_ notification: Notification) {
        settings = SettingsWindowController(coordinator: coordinator)
        coordinator.openSettings = { [weak self] pane in self?.settings?.show(pane) }
        handleSignals()

        if arguments.contains("--smoke-test") {
            runSmokeTest()
            return
        }
        if let i = arguments.firstIndex(of: "--harness"), i + 1 < arguments.count {
            startHarness(bundleID: arguments[i + 1])
            return
        }
        menuBar = MenuBar(coordinator: coordinator)
        coordinator.onStateChange = { [weak self] in self?.menuBar?.refresh() }

        // First run, or no Accessibility yet: the welcome walks through setup.
        if WelcomeWindowController.shouldShow, !arguments.contains("--no-prompt") {
            let w = WelcomeWindowController()
            w.model.onTrusted = { [weak self] in self?.startCoordinator() }
            welcome = w
            w.show()
        }
        if AX.isTrusted {
            startCoordinator()
        } else {
            trustTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] t in
                guard AX.isTrusted else { return }
                t.invalidate()
                self?.startCoordinator()
            }
        }
    }

    private func startCoordinator() {
        guard !coordinator.started else { return }
        coordinator.start()
        menuBar?.refresh()
        coordinator.onAskScreenRecording = { [weak self] in self?.askScreenRecording() }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in
            Conflicts.warnIfNeeded()
            // Until Screen Recording is allowed, ask on every launch.
            if !Permissions.screenRecording, self?.welcome?.isVisible != true { self?.askScreenRecording() }
        }
    }

    private func askScreenRecording() {
        if let w = welcome, w.isVisible {
            w.model.step = 1
            NSApp.activate(ignoringOtherApps: true)
            return
        }
        let w = WelcomeWindowController()
        welcome = w
        w.showScreenRecording()
    }

    func applicationWillTerminate(_ notification: Notification) {
        coordinator.prepareForQuit()
    }

    /// canopy:// commands (SPEC §11).
    func application(_ application: NSApplication, open urls: [URL]) {
        for url in urls {
            guard let command = Command(url: url) else {
                NSLog("Canopy: ignoring \(url.absoluteString)")
                continue
            }
            coordinator.run(command)
        }
    }

    /// SIGTERM and SIGINT quit cleanly, so windows come back into view.
    private func handleSignals() {
        for sig in [SIGTERM, SIGINT] {
            signal(sig, SIG_IGN)
            let s = DispatchSource.makeSignalSource(signal: sig, queue: .main)
            s.setEventHandler { NSApp.terminate(nil) }
            s.resume()
            signalSources.append(s)
        }
    }

    /// Manages only one app's windows, with disposable state, driven over stdin.
    private func startHarness(bundleID: String) {
        let dir = ProcessInfo.processInfo.environment["CANOPY_HARNESS_DIR"]
            ?? NSTemporaryDirectory().appending("canopy-harness")
        Store.folder = URL(fileURLWithPath: dir, isDirectory: true)
        Log.folder = URL(fileURLWithPath: dir, isDirectory: true)
        Store.defaults = UserDefaults(suiteName: "app.canopy.wm.harness") ?? .standard
        if !arguments.contains("--keep-state") {
            try? FileManager.default.removeItem(at: Store.stateURL)
            Store.defaults.removePersistentDomain(forName: "app.canopy.wm.harness")
        }
        var p = Store().loadPreferences()
        p.animate = arguments.contains("--animate")
        Store().savePreferences(p)
        coordinator.onlyBundleID = bundleID
        coordinator.useEventTap = arguments.contains("--tap")
        coordinator.useHotkeys = false
        guard AX.isTrusted else {
            print(#"{"ready":false,"error":"Accessibility not allowed"}"#)
            exit(2)
        }
        coordinator.start()
        harness = Harness(coordinator: coordinator, delegate: self)
        // Answer only once the coordinator has scanned and started.
        Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] t in
            guard let self, self.coordinator.started else { return }
            t.invalidate()
            self.harness?.start()
        }
    }

    /// Launches every part that doesn't need permissions, reports, and quits.
    private func runSmokeTest() {
        let displays = Display.current()
        let report: [String: Any] = [
            "accessibilityTrusted": AX.isTrusted,
            "screenRecordingAllowed": CGPreflightScreenCaptureAccess(),
            "displays": displays.map { ["uuid": $0.uuid, "visible": "\($0.visible)"] },
            "urlScheme": Bundle.main.object(forInfoDictionaryKey: "CFBundleURLTypes") != nil,
            "defaultShortcuts": Shortcuts.defaults.count,
            "statePath": Store.stateURL.path.replacingOccurrences(of: NSHomeDirectory(), with: "~"),
        ]
        if let data = try? JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]),
           let s = String(data: data, encoding: .utf8) {
            print(s)
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { exit(0) }
    }
}

let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.setActivationPolicy(.accessory)
app.run()
