import AppKit

/// Other window managers that move windows too. Two at once fight over every
/// window, so Canopy says so instead of quietly losing.
enum Conflicts {
    static let known: [String: String] = [
        "app.trellisui.trelliswm": "TrellisWM",
        "bobko.aerospace": "AeroSpace",
        "com.knollsoft.Rectangle": "Rectangle",
        "com.knollsoft.Hookshot": "Hookshot",
        "com.amethyst.Amethyst": "Amethyst",
        "com.mizage.Divvy": "Divvy",
        "com.crowdcafe.windowmagnet": "Magnet",
        "com.lwouis.alt-tab-macos": "",
    ]

    /// Names of window managers running now.
    static func running() -> [String] {
        NSWorkspace.shared.runningApplications.compactMap { app in
            guard let id = app.bundleIdentifier, let name = known[id], !name.isEmpty else { return nil }
            return name
        }
    }

    static func warnIfNeeded() {
        let names = running()
        guard !names.isEmpty else { return }
        Log.add("CONFLICT \(names.joined(separator: ", ")) is running")
        let alert = NSAlert()
        alert.messageText = "\(names.joined(separator: " and ")) is also arranging windows"
        alert.informativeText = "Two window managers fight over every window: windows jump back, refuse to dock and feel slow. Quit \(names.joined(separator: " and ")) while you use Canopy."
        alert.addButton(withTitle: "Quit \(names.count == 1 ? names[0] : "Them")")
        alert.addButton(withTitle: "Keep Both")
        NSApp.activate(ignoringOtherApps: true)
        if alert.runModal() == .alertFirstButtonReturn {
            for app in NSWorkspace.shared.runningApplications where known[app.bundleIdentifier ?? ""]?.isEmpty == false {
                app.terminate()
            }
        }
    }
}
