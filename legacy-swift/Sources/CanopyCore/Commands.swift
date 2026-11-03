import Foundation

public enum SettingsPane: String, CaseIterable, Codable, Sendable {
    case general, windows, tabs, appearance, gestures, apps, shortcuts
}

/// Every command Canopy can run, from a shortcut, the menu bar, or a URL.
public enum Command: Equatable, Hashable, Sendable {
    case everything
    case stepOut
    case stepIn
    case back
    case forward
    case neighbour(Direction)
    case frameWindow
    case frameDesktop
    case toggleDesktop
    case goToView(Int)
    case saveView(Int)
    case splitFinder
    case arrange
    case floatAll
    case settings(SettingsPane?)

    public static let scheme = "canopy"

    /// Parses `canopy://<command>[?args]`. Returns nil for anything invalid.
    public init?(url: URL) {
        guard url.scheme?.lowercased() == Self.scheme else { return nil }
        let components = URLComponents(url: url, resolvingAgainstBaseURL: false)
        var name = (url.host ?? "").lowercased()
        if name.isEmpty {
            // canopy:everything or canopy:///everything
            name = url.path.trimmingCharacters(in: CharacterSet(charactersIn: "/")).lowercased()
        }
        func query(_ key: String) -> String? {
            components?.queryItems?.first { $0.name.lowercased() == key }?.value
        }
        func slot() -> Int? {
            guard let raw = query("n"), let n = Int(raw), (1...9).contains(n) else { return nil }
            return n
        }
        switch name {
        case "everything": self = .everything
        case "zoom-out": self = .stepOut
        case "zoom-in": self = .stepIn
        case "back": self = .back
        case "forward": self = .forward
        case "neighbour", "neighbor":
            guard let raw = query("dir"), let d = Direction(rawValue: raw.lowercased()) else { return nil }
            self = .neighbour(d)
        case "frame-window": self = .frameWindow
        case "frame-desktop": self = .frameDesktop
        case "toggle-desktop": self = .toggleDesktop
        case "view":
            guard let n = slot() else { return nil }
            self = .goToView(n)
        case "save-view":
            guard let n = slot() else { return nil }
            self = .saveView(n)
        case "split-finder": self = .splitFinder
        case "arrange": self = .arrange
        case "float-all": self = .floatAll
        case "settings":
            self = .settings(query("pane").flatMap { SettingsPane(rawValue: $0.lowercased()) })
        default:
            return nil
        }
    }

    /// The canonical URL; `Command(url: c.url) == c`.
    public var url: URL {
        let path: String
        switch self {
        case .everything: path = "everything"
        case .stepOut: path = "zoom-out"
        case .stepIn: path = "zoom-in"
        case .back: path = "back"
        case .forward: path = "forward"
        case .neighbour(let d): path = "neighbour?dir=\(d.rawValue)"
        case .frameWindow: path = "frame-window"
        case .frameDesktop: path = "frame-desktop"
        case .toggleDesktop: path = "toggle-desktop"
        case .goToView(let n): path = "view?n=\(n)"
        case .saveView(let n): path = "save-view?n=\(n)"
        case .splitFinder: path = "split-finder"
        case .arrange: path = "arrange"
        case .floatAll: path = "float-all"
        case .settings(let pane): path = pane.map { "settings?pane=\($0.rawValue)" } ?? "settings"
        }
        return URL(string: "\(Self.scheme)://\(path)")!
    }

    public var title: String {
        switch self {
        case .everything: return "Show Everything"
        case .stepOut: return "Step Out a Level"
        case .stepIn: return "Step In a Level"
        case .back: return "Back"
        case .forward: return "Forward"
        case .neighbour(.left): return "Neighbouring Space to the Left"
        case .neighbour(.right): return "Neighbouring Space to the Right"
        case .neighbour(.up): return "Neighbouring Space Above"
        case .neighbour(.down): return "Neighbouring Space Below"
        case .frameWindow: return "Focused Window on Its Own"
        case .frameDesktop: return "Desktop on Its Own"
        case .toggleDesktop: return "Hide or Show the Desktop"
        case .goToView(let n): return "Go to View \(n)"
        case .saveView(let n): return "Save View \(n)"
        case .splitFinder: return "Split with a New Finder Window"
        case .arrange: return "Arrange Windows"
        case .floatAll: return "Float All Windows on Desktop"
        case .settings: return "Settings…"
        }
    }
}
