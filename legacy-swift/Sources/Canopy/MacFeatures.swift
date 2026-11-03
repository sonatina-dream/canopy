import Foundation
import ServiceManagement

/// macOS window features that move windows on their own and fight Canopy.
/// They live in the com.apple.WindowManager preferences domain.
enum MacFeature: String, CaseIterable, Identifiable {
    case edgeTiling
    case clickToShowDesktop
    case stageManager

    static let domain = "com.apple.WindowManager"
    var id: String { rawValue }

    var title: String {
        switch self {
        case .edgeTiling: return "Tiling by dragging to screen edges"
        case .clickToShowDesktop: return "Click wallpaper to reveal desktop"
        case .stageManager: return "Stage Manager"
        }
    }

    var detail: String {
        switch self {
        case .edgeTiling: return "snaps a window to half the screen when you drag it to an edge"
        case .clickToShowDesktop: return "sweeps every window aside when you click the desktop"
        case .stageManager: return "moves windows into a strip at the side of the screen"
        }
    }

    private var keys: [String] {
        switch self {
        case .edgeTiling: return ["EnableTilingByEdgeDrag", "EnableTopTilingByEdgeDrag"]
        case .clickToShowDesktop: return ["EnableStandardClickToShowDesktop"]
        case .stageManager: return ["GloballyEnabled"]
        }
    }

    /// What macOS assumes when the key is absent.
    private var defaultOn: Bool { self != .stageManager }

    var isOn: Bool {
        keys.contains { key in
            (CFPreferencesCopyAppValue(key as CFString, Self.domain as CFString) as? Bool) ?? defaultOn
        }
    }

    /// Writes through `defaults` so cfprefsd and WindowManager see the change.
    func set(_ on: Bool) {
        let remembered = "previous.\(rawValue)"
        if !on, UserDefaults.standard.object(forKey: remembered) == nil {
            UserDefaults.standard.set(isOn, forKey: remembered)
        }
        for key in keys {
            let p = Process()
            p.executableURL = URL(fileURLWithPath: "/usr/bin/defaults")
            p.arguments = ["write", Self.domain, key, "-bool", on ? "true" : "false"]
            try? p.run()
            p.waitUntilExit()
        }
        if on { UserDefaults.standard.removeObject(forKey: remembered) }
    }

    /// Whether Canopy turned this off and can turn it back on.
    var turnedOffByCanopy: Bool { UserDefaults.standard.object(forKey: "previous.\(rawValue)") != nil }
}

enum LoginItem {
    static var isEnabled: Bool { SMAppService.mainApp.status == .enabled }

    static func set(_ on: Bool) throws {
        if on { try SMAppService.mainApp.register() } else { try SMAppService.mainApp.unregister() }
    }
}
