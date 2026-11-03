import AppKit
import CanopyCore

@_silgen_name("CGSCopyManagedDisplaySpaces")
private func CGSCopyManagedDisplaySpaces(_ cid: CGSConnectionID) -> Unmanaged<CFArray>?

@_silgen_name("CGSCopySpacesForWindows")
private func CGSCopySpacesForWindows(_ cid: CGSConnectionID, _ mask: Int32, _ windows: CFArray) -> Unmanaged<CFArray>?

/// macOS Spaces (the desktops in Mission Control), from the window server.
/// Canopy keeps one arrangement per display per Space.
enum Spaces {
    struct Space: Equatable {
        /// The window server's id for the Space; changes across restarts.
        let id: UInt64
        /// A stable name: the Space's uuid, or "main" for the first desktop.
        let key: String
        /// A full-screen app's Space: Canopy leaves it alone.
        let fullScreen: Bool
    }

    /// The current Space of each display, by display UUID. When displays share
    /// Spaces, every display gets the same one. Nil if the lookup is unavailable.
    static func current(displays: [Display]) -> [String: Space]? {
        guard let raw = CGSCopyManagedDisplaySpaces(_CGSDefaultConnection())?.takeRetainedValue() as? [[String: Any]] else {
            return nil
        }
        var out: [String: Space] = [:]
        for entry in raw {
            guard let current = entry["Current Space"] as? [String: Any] else { continue }
            let id = (current["ManagedSpaceID"] as? NSNumber)?.uint64Value ?? (current["id64"] as? NSNumber)?.uint64Value ?? 0
            let uuid = current["uuid"] as? String ?? ""
            let type = (current["type"] as? NSNumber)?.intValue ?? 0
            let space = Space(id: id, key: uuid.isEmpty ? "main" : uuid, fullScreen: type == 4)
            let identifier = entry["Display Identifier"] as? String ?? "Main"
            if let d = displays.first(where: { $0.uuid.caseInsensitiveCompare(identifier) == .orderedSame }) {
                out[d.uuid] = space
            } else {
                // "Main": displays share one set of Spaces.
                for d in displays where out[d.uuid] == nil { out[d.uuid] = space }
            }
        }
        return out
    }

    /// The Spaces a window is on (more than one for windows on all desktops).
    static func of(_ window: WindowID) -> [UInt64] {
        let ids = [NSNumber(value: UInt32(truncatingIfNeeded: window))] as CFArray
        guard let raw = CGSCopySpacesForWindows(_CGSDefaultConnection(), 0x7, ids)?.takeRetainedValue() as? [NSNumber] else {
            return []
        }
        return raw.map(\.uint64Value)
    }
}
