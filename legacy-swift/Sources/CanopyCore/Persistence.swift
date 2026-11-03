import Foundation

/// What identifies a window across restarts when its id has changed.
public struct WindowRef: Codable, Equatable, Hashable, Sendable {
    public var bundleID: String
    public var title: String

    public init(bundleID: String, title: String) {
        self.bundleID = bundleID
        self.title = title
    }
}

/// One display's saved arrangement, camera and the windows it referred to.
public final class DisplayState: Codable {
    public var arrangement: Arrangement
    public var navigator: Navigator
    /// Keyed by `String(WindowID)` so the JSON stays an object.
    public var windows: [String: WindowRef]

    public init(arrangement: Arrangement, navigator: Navigator, windows: [WindowID: WindowRef]) {
        self.arrangement = arrangement
        self.navigator = navigator
        self.windows = Dictionary(uniqueKeysWithValues: windows.map { (String($0.key), $0.value) })
    }

    public var windowRefs: [WindowID: WindowRef] {
        Dictionary(uniqueKeysWithValues: windows.compactMap { k, v in WindowID(k).map { ($0, v) } })
    }

    /// Re-attaches the saved layout to the windows that exist now. Windows that
    /// cannot be matched leave the layout and their space is shared.
    public func reattach(to live: [WindowID: WindowRef], shareSpace: Bool = true) {
        let mapping = WindowMatcher.match(saved: windowRefs, live: live)
        arrangement.retain(only: Set(mapping.keys), shareSpace: shareSpace)
        arrangement.remap(mapping)
        navigator.remap(mapping)
        navigator.prune(liveWindows: Set(arrangement.windows))
        var refs: [WindowID: WindowRef] = [:]
        for w in arrangement.windows { refs[w] = live[w] }
        windows = Dictionary(uniqueKeysWithValues: refs.map { (String($0.key), $0.value) })
    }
}

public struct SavedState: Codable {
    public static let currentVersion = 1
    public var version: Int
    /// Keyed by display UUID.
    public var displays: [String: DisplayState]

    public init(displays: [String: DisplayState]) {
        version = Self.currentVersion
        self.displays = displays
    }

    public func encoded() throws -> Data {
        let e = JSONEncoder()
        e.outputFormatting = [.prettyPrinted, .sortedKeys]
        return try e.encode(self)
    }

    public static func decode(_ data: Data) throws -> SavedState {
        try JSONDecoder().decode(SavedState.self, from: data)
    }
}

public enum WindowMatcher {
    /// Maps saved window ids to live ones:
    /// 1. same id and same app;
    /// 2. same app and same title;
    /// 3. the only unmatched saved window of an app to its only unmatched live window.
    public static func match(saved: [WindowID: WindowRef], live: [WindowID: WindowRef]) -> [WindowID: WindowID] {
        var mapping: [WindowID: WindowID] = [:]
        var usedLive = Set<WindowID>()
        let savedIDs = saved.keys.sorted()

        for id in savedIDs {
            if let l = live[id], l.bundleID == saved[id]!.bundleID {
                mapping[id] = id
                usedLive.insert(id)
            }
        }
        for id in savedIDs where mapping[id] == nil {
            let ref = saved[id]!
            if let l = live.keys.sorted().first(where: { !usedLive.contains($0) && live[$0] == ref }) {
                mapping[id] = l
                usedLive.insert(l)
            }
        }
        let unmatchedSaved = savedIDs.filter { mapping[$0] == nil }
        let bySavedApp = Dictionary(grouping: unmatchedSaved) { saved[$0]!.bundleID }
        let unmatchedLive = live.keys.sorted().filter { !usedLive.contains($0) }
        let byLiveApp = Dictionary(grouping: unmatchedLive) { live[$0]!.bundleID }
        for (app, ids) in bySavedApp where ids.count == 1 {
            if let candidates = byLiveApp[app], candidates.count == 1 {
                mapping[ids[0]] = candidates[0]
            }
        }
        return mapping
    }
}

/// User preferences. Missing keys decode to their defaults.
public struct Preferences: Codable, Equatable, Sendable {
    public static let currentVersion = 2
    public var version: Int = Preferences.currentVersion
    public var gap: Double = 12
    public var animate: Bool = true
    public var animationDuration: Double = 0.46
    public var redrawWait: Double = 0.15
    public var shareRemovedSpace: Bool = true
    public var tabHold: Double = 0.3
    public var ignoredBundleIDs: [String] = []
    public var menuBarControls: Bool = true
    public var tabCloseButtons: Bool = true
    public var optionPinch: Bool = true
    public var rectZoom: Bool = true
    public var titleDoubleClickFrames: Bool = true
    public var greenButtonToggles: Bool = true
    public var stepTowardPointer: Bool = false
    public var coverTint: Double = 0.35
    public var livePreviews: Bool = true

    public init() {}

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let d = Preferences()
        gap = try c.decodeIfPresent(Double.self, forKey: .gap) ?? d.gap
        animate = try c.decodeIfPresent(Bool.self, forKey: .animate) ?? d.animate
        animationDuration = try c.decodeIfPresent(Double.self, forKey: .animationDuration) ?? d.animationDuration
        redrawWait = try c.decodeIfPresent(Double.self, forKey: .redrawWait) ?? d.redrawWait
        shareRemovedSpace = try c.decodeIfPresent(Bool.self, forKey: .shareRemovedSpace) ?? d.shareRemovedSpace
        tabHold = try c.decodeIfPresent(Double.self, forKey: .tabHold) ?? d.tabHold
        ignoredBundleIDs = try c.decodeIfPresent([String].self, forKey: .ignoredBundleIDs) ?? d.ignoredBundleIDs
        menuBarControls = try c.decodeIfPresent(Bool.self, forKey: .menuBarControls) ?? d.menuBarControls
        tabCloseButtons = try c.decodeIfPresent(Bool.self, forKey: .tabCloseButtons) ?? d.tabCloseButtons
        optionPinch = try c.decodeIfPresent(Bool.self, forKey: .optionPinch) ?? d.optionPinch
        rectZoom = try c.decodeIfPresent(Bool.self, forKey: .rectZoom) ?? d.rectZoom
        titleDoubleClickFrames = try c.decodeIfPresent(Bool.self, forKey: .titleDoubleClickFrames) ?? d.titleDoubleClickFrames
        greenButtonToggles = try c.decodeIfPresent(Bool.self, forKey: .greenButtonToggles) ?? d.greenButtonToggles
        stepTowardPointer = try c.decodeIfPresent(Bool.self, forKey: .stepTowardPointer) ?? d.stepTowardPointer
        coverTint = try c.decodeIfPresent(Double.self, forKey: .coverTint) ?? d.coverTint
        livePreviews = try c.decodeIfPresent(Bool.self, forKey: .livePreviews) ?? d.livePreviews
        // Version 1 waited half a second over a window before offering a tab.
        let stored = try c.decodeIfPresent(Int.self, forKey: .version) ?? 1
        if stored < 2, tabHold == 0.5 { tabHold = d.tabHold }
        version = Self.currentVersion
    }
}
