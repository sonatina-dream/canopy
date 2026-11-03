import AppKit
import CanopyCore
import SwiftUI

/// Bridges the coordinator's preferences and shortcuts to SwiftUI.
final class SettingsModel: ObservableObject {
    let coordinator: Coordinator
    @Published var pane: SettingsPane = .general
    @Published var prefs: Preferences {
        didSet { if prefs != coordinator.prefs { coordinator.prefs = prefs } }
    }
    @Published var bindings: [Command: KeyCombo]
    @Published var axTrusted = AX.isTrusted
    @Published var screenAllowed = Permissions.screenRecording
    @Published var featureStates: [MacFeature: Bool] = [:]
    @Published var loginEnabled = LoginItem.isEnabled
    @Published var loginError: String?
    @Published var recording: Command?
    private var timer: Timer?

    init(coordinator: Coordinator) {
        self.coordinator = coordinator
        prefs = coordinator.prefs
        bindings = coordinator.bindings
        refreshStatus()
        timer = Timer.scheduledTimer(withTimeInterval: 1.5, repeats: true) { [weak self] _ in self?.refreshStatus() }
    }

    func refreshStatus() {
        axTrusted = AX.isTrusted
        screenAllowed = Permissions.screenRecording
        featureStates = Dictionary(uniqueKeysWithValues: MacFeature.allCases.map { ($0, $0.isOn) })
        loginEnabled = LoginItem.isEnabled
        if prefs != coordinator.prefs { prefs = coordinator.prefs }
    }

    func setBinding(_ c: Command, _ combo: KeyCombo?) {
        var b = bindings
        // One combination does one thing: take it from any other command.
        if let combo { for (k, v) in b where v == combo { b[k] = nil } }
        b[c] = combo
        bindings = b
        coordinator.bindings = b
    }

    func restoreShortcuts() {
        bindings = Shortcuts.defaults
        coordinator.bindings = bindings
    }

    func setLogin(_ on: Bool) {
        do {
            try LoginItem.set(on)
            loginError = nil
        } catch {
            loginError = error.localizedDescription
        }
        loginEnabled = LoginItem.isEnabled
    }
}

final class SettingsWindowController {
    private var window: NSWindow?
    private let model: SettingsModel

    init(coordinator: Coordinator) {
        model = SettingsModel(coordinator: coordinator)
    }

    func show(_ pane: SettingsPane?) {
        if let pane { model.pane = pane }
        model.refreshStatus()
        if window == nil {
            let w = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 620, height: 480),
                             styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false)
            w.title = "Canopy Settings"
            w.isReleasedWhenClosed = false
            w.contentView = NSHostingView(rootView: SettingsView(model: model))
            w.center()
            window = w
        }
        NSApp.activate(ignoringOtherApps: true)
        window?.makeKeyAndOrderFront(nil)
    }
}

struct SettingsView: View {
    @ObservedObject var model: SettingsModel

    var body: some View {
        TabView(selection: $model.pane) {
            GeneralPane(model: model).tabItem { Label("General", systemImage: "gearshape") }.tag(SettingsPane.general)
            WindowsPane(model: model).tabItem { Label("Windows", systemImage: "rectangle.split.3x1") }.tag(SettingsPane.windows)
            TabsPane(model: model).tabItem { Label("Tabs", systemImage: "menubar.rectangle") }.tag(SettingsPane.tabs)
            AppearancePane(model: model).tabItem { Label("Appearance", systemImage: "paintbrush") }.tag(SettingsPane.appearance)
            GesturesPane(model: model).tabItem { Label("Gestures", systemImage: "hand.draw") }.tag(SettingsPane.gestures)
            AppsPane(model: model).tabItem { Label("Apps", systemImage: "app.badge") }.tag(SettingsPane.apps)
            ShortcutsPane(model: model).tabItem { Label("Shortcuts", systemImage: "keyboard") }.tag(SettingsPane.shortcuts)
        }
        .padding(16)
        .frame(width: 620, height: 480)
    }
}

private struct GeneralPane: View {
    @ObservedObject var model: SettingsModel

    var body: some View {
        Form {
            Section("Permissions") {
                LabeledContent("Accessibility") {
                    HStack {
                        Text(model.axTrusted ? "Allowed" : "Not allowed").foregroundStyle(model.axTrusted ? .green : .red)
                        if !model.axTrusted {
                            Button("Allow…") {
                                _ = AX.requestTrust(prompt: true)
                                NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")!)
                            }
                        }
                    }
                }
                Text("Required: macOS only lets apps you trust move windows.").font(.caption).foregroundStyle(.secondary)
                LabeledContent("Screen Recording") {
                    HStack {
                        Text(model.screenAllowed ? "Allowed" : "Not allowed").foregroundStyle(model.screenAllowed ? .green : .secondary)
                        if !model.screenAllowed {
                            Button("Allow…") {
                                if !CGRequestScreenCaptureAccess() {
                                    NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")!)
                                }
                            }
                        }
                    }
                }
                Text("Optional: shows windows' real contents while they move. Nothing is recorded or kept. Reopen Canopy after allowing.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Section("macOS features") {
                ForEach(MacFeature.allCases) { f in
                    LabeledContent {
                        let on = model.featureStates[f] ?? false
                        Button(on ? "Turn Off" : (f.turnedOffByCanopy ? "Turn Back On" : "Turn On")) {
                            f.set(!on)
                            model.refreshStatus()
                        }
                    } label: {
                        VStack(alignment: .leading) {
                            Text(f.title)
                            Text(f.detail).font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
            }
            Section {
                Toggle("Open Canopy at login", isOn: Binding(get: { model.loginEnabled }, set: { model.setLogin($0) }))
                if let e = model.loginError { Text(e).font(.caption).foregroundStyle(.red) }
            }
        }
        .formStyle(.grouped)
    }
}

private struct WindowsPane: View {
    @ObservedObject var model: SettingsModel

    var body: some View {
        Form {
            Section("Layout") {
                LabeledContent("Gap") {
                    HStack {
                        Slider(value: $model.prefs.gap, in: 0...40, step: 1)
                        Text("\(Int(model.prefs.gap)) pt").monospacedDigit().frame(width: 44, alignment: .trailing)
                    }
                }
                Toggle("Share a leaving window's space with the whole row", isOn: $model.prefs.shareRemovedSpace)
                Text("So closing a window exactly undoes adding it. Otherwise the neighbour after it takes all its space.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Section("Motion") {
                Toggle("Animate layout changes", isOn: $model.prefs.animate)
                LabeledContent("Duration") {
                    HStack {
                        Slider(value: $model.prefs.animationDuration, in: 0.15...1.0)
                        Text("\(Int(model.prefs.animationDuration * 1000)) ms").monospacedDigit().frame(width: 60, alignment: .trailing)
                    }
                }
            }
            Section("Title bar") {
                Toggle("Double-click frames the window on its own", isOn: $model.prefs.titleDoubleClickFrames)
                Toggle("Green button docks and floats", isOn: $model.prefs.greenButtonToggles)
                Text("⌥-click still zooms, fn-click still goes full screen.").font(.caption).foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
    }
}

private struct TabsPane: View {
    @ObservedObject var model: SettingsModel

    var body: some View {
        Form {
            Section("Tab groups") {
                Text("Drop a window on the middle of another and pause to make them tabs of one window.")
                    .font(.caption).foregroundStyle(.secondary)
                LabeledContent("Hold over a target") {
                    HStack {
                        Slider(value: $model.prefs.tabHold, in: 0.2...1.5)
                        Text(String(format: "%.1f s", model.prefs.tabHold)).monospacedDigit().frame(width: 44, alignment: .trailing)
                    }
                }
                Toggle("Close buttons on tabs", isOn: $model.prefs.tabCloseButtons)
            }
        }
        .formStyle(.grouped)
    }
}

private struct AppearancePane: View {
    @ObservedObject var model: SettingsModel

    var body: some View {
        Form {
            Section("Menu bar") {
                Toggle("Show back, forward, everything and saved views", isOn: $model.prefs.menuBarControls)
            }
            Section("Previews") {
                Toggle("Live previews (needs Screen Recording)", isOn: $model.prefs.livePreviews)
                LabeledContent("App colour on covers") {
                    Slider(value: $model.prefs.coverTint, in: 0...1)
                }
                Text("Windows too small to use show their app's colour and icon until there's room. Double-click one to go in.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
    }
}

private struct GesturesPane: View {
    @ObservedObject var model: SettingsModel

    var body: some View {
        Form {
            Section("Trackpad") {
                Toggle("⌥-pinch anywhere to zoom in and out", isOn: $model.prefs.optionPinch)
            }
            Section("Mouse") {
                Toggle("⌘⇧-drag a rectangle to zoom to what it frames", isOn: $model.prefs.rectZoom)
            }
            Section("Stepping in") {
                Picker("Stepping in goes toward", selection: $model.prefs.stepTowardPointer) {
                    Text("The window you're using").tag(false)
                    Text("The pointer").tag(true)
                }
            }
        }
        .formStyle(.grouped)
    }
}

private struct AppsPane: View {
    @ObservedObject var model: SettingsModel

    private var running: [NSRunningApplication] {
        NSWorkspace.shared.runningApplications
            .filter { $0.activationPolicy == .regular && $0.bundleIdentifier != nil && $0.processIdentifier != ProcessInfo.processInfo.processIdentifier }
            .sorted { ($0.localizedName ?? "") < ($1.localizedName ?? "") }
    }

    var body: some View {
        Form {
            Section {
                Text("Windows of these apps are left alone: they float freely and are never arranged.")
                    .font(.caption).foregroundStyle(.secondary)
                if model.prefs.ignoredBundleIDs.isEmpty {
                    Text("No apps").foregroundStyle(.secondary)
                }
                ForEach(model.prefs.ignoredBundleIDs, id: \.self) { id in
                    HStack {
                        Text(NSWorkspace.shared.urlForApplication(withBundleIdentifier: id)?.deletingPathExtension().lastPathComponent ?? id)
                        Spacer()
                        Text(id).font(.caption).foregroundStyle(.secondary)
                        Button("Remove") { model.prefs.ignoredBundleIDs.removeAll { $0 == id } }
                    }
                }
                Menu("Add a Running App") {
                    ForEach(running.filter { !model.prefs.ignoredBundleIDs.contains($0.bundleIdentifier!) }, id: \.processIdentifier) { app in
                        Button(app.localizedName ?? app.bundleIdentifier!) { model.prefs.ignoredBundleIDs.append(app.bundleIdentifier!) }
                    }
                }
            }
        }
        .formStyle(.grouped)
    }
}

private struct ShortcutsPane: View {
    @ObservedObject var model: SettingsModel

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            List {
                ForEach(Shortcuts.bindable, id: \.self) { c in
                    HStack {
                        Text(c.title)
                        Spacer()
                        ShortcutField(model: model, command: c)
                    }
                }
            }
            HStack {
                Text("A shortcut needs ⌃, ⌥ or ⌘. Press ⌫ while recording to clear one.").font(.caption).foregroundStyle(.secondary)
                Spacer()
                Button("Restore Defaults") { model.restoreShortcuts() }
            }
        }
    }
}

private struct ShortcutField: View {
    @ObservedObject var model: SettingsModel
    let command: Command
    @State private var monitor: Any?

    var body: some View {
        Button(action: toggle) {
            Text(model.recording == command ? "Type shortcut…" : (model.bindings[command]?.display ?? "No shortcut"))
                .monospaced()
                .frame(minWidth: 110)
        }
        .onDisappear(perform: stop)
    }

    private func toggle() {
        if model.recording == command { stop(); return }
        stop()
        model.recording = command
        model.coordinator.hotkeys.unregisterAll()
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { e in
            if e.keyCode == 53 { stop(); return nil }
            if e.keyCode == 51 { model.setBinding(command, nil); stop(); return nil }
            if let combo = KeyCombo(event: e) {
                model.setBinding(command, combo)
                stop()
            } else {
                NSSound.beep()
            }
            return nil
        }
    }

    private func stop() {
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil
        if model.recording == command {
            model.recording = nil
            model.coordinator.hotkeys.register(model.coordinator.bindings)
        }
    }
}
