import AppKit
import SwiftUI

enum Permissions {
    /// `--pretend-no-screen-recording` shows the request flow without touching
    /// the real permission (for checking the UI).
    static let pretendNoScreenRecording = CommandLine.arguments.contains("--pretend-no-screen-recording")

    static var screenRecording: Bool {
        !pretendNoScreenRecording && CGPreflightScreenCaptureAccess()
    }
}

/// First-run setup, one step at a time: the permissions Canopy needs, the
/// macOS features that fight it, and the first move.
final class WelcomeModel: ObservableObject {
    @Published var step = 0
    @Published var axTrusted = AX.isTrusted
    @Published var screenAllowed = Permissions.screenRecording
    /// Only the Screen Recording step: Canopy asking again on a later launch.
    @Published var screenOnly = false
    @Published var featuresOn: [MacFeature] = MacFeature.allCases.filter(\.isOn)
    /// Screen Recording takes effect only after a relaunch.
    @Published var needsReopen = false
    var onTrusted: (() -> Void)?
    var onDone: (() -> Void)?
    private var timer: Timer?

    init() {
        timer = Timer.scheduledTimer(withTimeInterval: 0.8, repeats: true) { [weak self] _ in self?.poll() }
    }

    func poll() {
        let trusted = AX.isTrusted
        if trusted, !axTrusted {
            axTrusted = true
            onTrusted?()
            if step == 0 { step = 1 }
        }
        let screen = Permissions.screenRecording
        if screen != screenAllowed { screenAllowed = screen }
        featuresOn = MacFeature.allCases.filter(\.isOn)
    }

    func askAccessibility() {
        _ = AX.requestTrust(prompt: true)
        NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")!)
    }

    /// macOS shows its own prompt the first time; after that, the Settings pane.
    func askScreenRecording() {
        if Permissions.pretendNoScreenRecording || !CGRequestScreenCaptureAccess() {
            NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")!)
        }
        needsReopen = true
    }

    /// Leaves the Screen Recording step.
    func afterScreenRecording() {
        if screenOnly {
            timer?.invalidate()
            onDone?()
        } else {
            step = 2
        }
    }

    func turnOffFeatures() {
        for f in featuresOn { f.set(false) }
        featuresOn = MacFeature.allCases.filter(\.isOn)
    }

    func reopen() {
        if !screenOnly { UserDefaults.standard.set(2, forKey: "welcomeResumeStep") }
        let path = Bundle.main.bundlePath
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/bin/sh")
        p.arguments = ["-c", "sleep 1; /usr/bin/open \"$0\"", path]
        try? p.run()
        NSApp.terminate(nil)
    }

    func finish() {
        UserDefaults.standard.set(true, forKey: "welcomed")
        timer?.invalidate()
        onDone?()
    }
}

final class WelcomeWindowController {
    private var window: NSWindow?
    let model = WelcomeModel()

    static var shouldShow: Bool {
        !UserDefaults.standard.bool(forKey: "welcomed") || !AX.isTrusted
    }

    /// Asks for Screen Recording on its own, on a launch after setup.
    func showScreenRecording() {
        model.screenOnly = true
        model.step = 1
        model.onDone = { [weak self] in self?.window?.close() }
        present()
    }

    func show() {
        if let resume = UserDefaults.standard.object(forKey: "welcomeResumeStep") as? Int {
            model.step = resume
            UserDefaults.standard.removeObject(forKey: "welcomeResumeStep")
        } else if model.axTrusted {
            model.step = 1
        }
        model.onDone = { [weak self] in self?.window?.close() }
        present()
    }

    var isVisible: Bool { window?.isVisible ?? false }

    private func present() {
        if window == nil {
            let w = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 520, height: 400),
                             styleMask: [.titled, .closable, .fullSizeContentView], backing: .buffered, defer: false)
            w.titlebarAppearsTransparent = true
            w.title = "Welcome to Canopy"
            w.isReleasedWhenClosed = false
            w.contentView = NSHostingView(rootView: WelcomeView(model: model))
            w.center()
            window = w
        }
        NSApp.activate(ignoringOtherApps: true)
        window?.makeKeyAndOrderFront(nil)
    }
}

private struct WelcomeView: View {
    @ObservedObject var model: WelcomeModel

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            if !model.screenOnly {
                HStack(spacing: 6) {
                    ForEach(0..<4) { i in
                        Capsule().fill(i <= model.step ? Color.accentColor : Color.secondary.opacity(0.25))
                            .frame(height: 4)
                    }
                }
            }
            switch model.step {
            case 0: accessibility
            case 1: screenRecording
            case 2: features
            default: start
            }
            Spacer(minLength: 0)
        }
        .padding(28)
        .padding(.top, 12)
        .frame(width: 520, height: 400)
    }

    private func heading(_ title: String, _ detail: String) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.title2.bold())
            Text(detail).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }
    }

    private var accessibility: some View {
        VStack(alignment: .leading, spacing: 18) {
            heading("Let Canopy arrange your windows",
                    "macOS only lets apps you trust move windows. Click Allow…, then switch Canopy on in System Settings ▸ Privacy & Security ▸ Accessibility.")
            HStack {
                Button("Allow…") { model.askAccessibility() }.keyboardShortcut(.defaultAction)
                if model.axTrusted { Label("Allowed", systemImage: "checkmark.circle.fill").foregroundStyle(.green) }
            }
            Text("Canopy carries on by itself once it's allowed.").font(.caption).foregroundStyle(.secondary)
        }
    }

    private var screenRecording: some View {
        VStack(alignment: .leading, spacing: 18) {
            heading(model.screenOnly ? "Let Canopy see your windows" : "See your windows as they are",
                    "With Screen Recording, windows keep their real contents while they move, instead of showing as their app's colour and icon. Canopy only takes still pictures to draw motion; nothing is recorded or kept.")
            if !model.screenAllowed {
                Text("Click Allow…, switch Canopy on in System Settings ▸ Privacy & Security ▸ Screen & System Audio Recording, then reopen Canopy.")
                    .font(.callout)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                if model.screenAllowed {
                    Label("Allowed", systemImage: "checkmark.circle.fill").foregroundStyle(.green)
                    Spacer()
                    Button("Continue") { model.afterScreenRecording() }.keyboardShortcut(.defaultAction)
                } else if model.needsReopen {
                    Button("Reopen Canopy") { model.reopen() }.keyboardShortcut(.defaultAction)
                    Text("macOS applies this when Canopy reopens.").font(.caption).foregroundStyle(.secondary)
                    Spacer()
                    Button("Not Now") { model.afterScreenRecording() }
                } else {
                    Button("Allow…") { model.askScreenRecording() }.keyboardShortcut(.defaultAction)
                    Spacer()
                    Button("Not Now") { model.afterScreenRecording() }
                }
            }
        }
    }

    private var features: some View {
        VStack(alignment: .leading, spacing: 18) {
            heading("Turn off what gets in the way",
                    "These macOS features move windows on their own and fight Canopy. You can turn them back on any time in Canopy's Settings ▸ General.")
            if model.featuresOn.isEmpty {
                Label("Nothing to turn off.", systemImage: "checkmark.circle.fill").foregroundStyle(.green)
            } else {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(model.featuresOn) { f in
                        Text("• \(f.title): \(f.detail)").font(.callout)
                    }
                }
            }
            HStack {
                if !model.featuresOn.isEmpty {
                    Button("Turn Them Off") { model.turnOffFeatures() }.keyboardShortcut(.defaultAction)
                }
                Spacer()
                Button(model.featuresOn.isEmpty ? "Continue" : "Keep Them") { model.step = 3 }
            }
        }
    }

    private var start: some View {
        VStack(alignment: .leading, spacing: 14) {
            heading("Your first move", "Nothing moves until you dock a window yourself.")
            VStack(alignment: .leading, spacing: 8) {
                Label("Drag a window by its title bar to the edge of the screen, or of another window. Let go and it docks.",
                      systemImage: "rectangle.split.2x1")
                Label("Drop a window on the middle of another and pause to make tabs.", systemImage: "menubar.rectangle")
                Label("⌃⌥Space shows everything. Double-click a title bar to give it the screen.", systemImage: "arrow.up.left.and.arrow.down.right")
                Label("The green button docks and floats a window.", systemImage: "circle.fill")
            }
            .font(.callout)
            HStack {
                Spacer()
                Button("Start Using Canopy") { model.finish() }.keyboardShortcut(.defaultAction)
            }
        }
    }
}
