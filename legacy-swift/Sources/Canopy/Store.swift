import Foundation
import CanopyCore

/// Arrangements in Application Support; preferences and shortcuts in UserDefaults.
struct Store {
    /// The test harness points these somewhere disposable.
    static var folder: URL = {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return base.appendingPathComponent("Canopy", isDirectory: true)
    }()
    static var defaults = UserDefaults.standard

    static var stateURL: URL { folder.appendingPathComponent("arrangements.json") }

    private var defaults: UserDefaults { Self.defaults }

    func loadState() -> SavedState? {
        guard let data = try? Data(contentsOf: Self.stateURL) else { return nil }
        do {
            return try SavedState.decode(data)
        } catch {
            NSLog("Canopy: could not read saved arrangements: \(error)")
            return nil
        }
    }

    func saveState(_ state: SavedState) {
        do {
            try FileManager.default.createDirectory(at: Self.folder, withIntermediateDirectories: true)
            try state.encoded().write(to: Self.stateURL, options: .atomic)
        } catch {
            NSLog("Canopy: could not save arrangements: \(error)")
        }
    }

    /// Smallest sizes apps have refused to go below, by bundle id.
    func loadAppMinimums() -> [String: CanopyCore.Size] {
        guard let data = defaults.data(forKey: "appMinimums"),
              let m = try? JSONDecoder().decode([String: CanopyCore.Size].self, from: data) else { return [:] }
        return m
    }

    func saveAppMinimums(_ m: [String: CanopyCore.Size]) {
        if let data = try? JSONEncoder().encode(m) { defaults.set(data, forKey: "appMinimums") }
    }

    func loadPreferences() -> Preferences {
        guard let data = defaults.data(forKey: "preferences"),
              let p = try? JSONDecoder().decode(Preferences.self, from: data) else { return Preferences() }
        return p
    }

    func savePreferences(_ p: Preferences) {
        if let data = try? JSONEncoder().encode(p) { defaults.set(data, forKey: "preferences") }
    }

    /// Shortcuts are stored by command URL. A command bound to nothing is
    /// stored explicitly so it doesn't fall back to its default.
    func loadBindings() -> [Command: KeyCombo] {
        guard let data = defaults.data(forKey: "shortcuts"),
              let raw = try? JSONDecoder().decode([String: KeyCombo?].self, from: data) else { return Shortcuts.defaults }
        var b = Shortcuts.defaults
        for (url, combo) in raw {
            guard let u = URL(string: url), let c = Command(url: u) else { continue }
            b[c] = combo
        }
        return b
    }

    func saveBindings(_ b: [Command: KeyCombo]) {
        var raw: [String: KeyCombo?] = [:]
        for c in Shortcuts.bindable { raw[c.url.absoluteString] = b[c] }
        if let data = try? JSONEncoder().encode(raw) { defaults.set(data, forKey: "shortcuts") }
    }
}
