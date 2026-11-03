import AppKit
import CanopyCore

/// A connected display, with frames in top-left-origin global coordinates
/// (the same space as Accessibility and CGWindowList).
struct Display: Equatable {
    let id: CGDirectDisplayID
    let uuid: String
    let frame: CanopyCore.Rect
    let visible: CanopyCore.Rect
    let name: String

    var screen: NSScreen? {
        NSScreen.screens.first { $0.displayID == id }
    }

    static func current() -> [Display] {
        NSScreen.screens.compactMap { s in
            guard let id = s.displayID else { return nil }
            let uuid: String
            if let u = CGDisplayCreateUUIDFromDisplayID(id)?.takeRetainedValue() {
                uuid = CFUUIDCreateString(nil, u) as String
            } else {
                uuid = "display-\(id)"
            }
            return Display(id: id, uuid: uuid, frame: Coordinates.ax(fromNS: s.frame),
                           visible: Coordinates.ax(fromNS: s.visibleFrame), name: s.localizedName)
        }
    }
}

extension NSScreen {
    var displayID: CGDirectDisplayID? {
        (deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber).map { CGDirectDisplayID($0.uint32Value) }
    }
}

/// Conversions between AppKit's bottom-left-origin screen space and the
/// top-left-origin space of Accessibility.
enum Coordinates {
    /// Height of the primary display (the one at the origin).
    static var primaryHeight: CGFloat {
        NSScreen.screens.first?.frame.height ?? 0
    }

    static func ax(fromNS r: CGRect) -> CanopyCore.Rect {
        CanopyCore.Rect(x: r.minX, y: primaryHeight - r.maxY, width: r.width, height: r.height)
    }

    static func ns(fromAX r: CanopyCore.Rect) -> CGRect {
        CGRect(x: r.x, y: primaryHeight - r.maxY, width: r.width, height: r.height)
    }

    static func axPoint(fromNS p: CGPoint) -> CanopyCore.Point {
        CanopyCore.Point(x: p.x, y: primaryHeight - p.y)
    }

    static var mouse: CanopyCore.Point { axPoint(fromNS: NSEvent.mouseLocation) }
}

extension CanopyCore.Rect {
    init(_ r: CGRect) { self.init(x: r.minX, y: r.minY, width: r.width, height: r.height) }
    var cg: CGRect { CGRect(x: x, y: y, width: width, height: height) }
}

extension CanopyCore.Point {
    init(_ p: CGPoint) { self.init(x: p.x, y: p.y) }
    var cg: CGPoint { CGPoint(x: x, y: y) }
}
