import AppKit
import CanopyCore

// MARK: - Cover

/// Stands in for a window too small for its app: its latest picture tinted
/// with the app's colour, and the app icon. Double-click to go in.
final class CoverPanel: OverlayPanel {
    private let view = CoverView()
    var onDoubleClick: (() -> Void)? {
        get { view.onDoubleClick }
        set { view.onDoubleClick = newValue }
    }

    init() {
        super.init(level: .normal, mouse: true)
        contentView = view
    }

    func update(image: CGImage?, colour: NSColor, icon: NSImage, name: String, tint: Double) {
        view.image = image
        view.colour = colour
        view.icon = icon
        view.name = name
        view.tint = tint
        view.needsDisplay = true
    }
}

private final class CoverView: NSView {
    var image: CGImage?
    var colour: NSColor = .gray
    var icon: NSImage?
    var name = ""
    var tint = 0.35
    var onDoubleClick: (() -> Void)?

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func mouseDown(with event: NSEvent) {
        if event.clickCount == 2 { onDoubleClick?() }
    }

    override func draw(_ dirtyRect: NSRect) {
        let path = NSBezierPath(roundedRect: bounds, xRadius: 10, yRadius: 10)
        path.addClip()
        colour.setFill()
        bounds.fill()
        if let image, let ctx = NSGraphicsContext.current?.cgContext {
            // Aspect fill.
            let s = max(bounds.width / CGFloat(image.width), bounds.height / CGFloat(image.height))
            let w = CGFloat(image.width) * s, h = CGFloat(image.height) * s
            ctx.draw(image, in: CGRect(x: bounds.midX - w / 2, y: bounds.midY - h / 2, width: w, height: h))
            colour.withAlphaComponent(tint).setFill()
            bounds.fill(using: .sourceOver)
        }
        let side = max(16, min(72, min(bounds.width, bounds.height) * 0.45))
        let iconRect = NSRect(x: bounds.midX - side / 2, y: bounds.midY - side / 2 + (bounds.height > 120 ? 10 : 0),
                              width: side, height: side)
        icon?.draw(in: iconRect)
        if bounds.height > 120, bounds.width > 90 {
            let attrs: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: 12, weight: .medium),
                .foregroundColor: NSColor.white,
            ]
            let s = NSAttributedString(string: name, attributes: attrs)
            let size = s.size()
            s.draw(at: NSPoint(x: bounds.midX - min(size.width, bounds.width - 12) / 2, y: iconRect.minY - size.height - 6))
        }
    }
}

// MARK: - Tab strip

struct TabInfo {
    var window: WindowID
    var title: String
    var icon: NSImage
    var selected: Bool
}

/// The strip of tabs on top of a tab group: glass, like a title bar.
final class TabStripPanel: OverlayPanel {
    let view = TabStripView()

    init() {
        super.init(level: .normal, mouse: true)
        let glass = NSVisualEffectView()
        glass.material = .hudWindow
        glass.blendingMode = .behindWindow
        glass.state = .active
        glass.wantsLayer = true
        glass.layer?.cornerRadius = 10
        glass.layer?.masksToBounds = true
        view.autoresizingMask = [.width, .height]
        glass.addSubview(view)
        contentView = glass
    }

    override func place(_ r: CanopyCore.Rect) {
        super.place(r)
        view.frame = contentView?.bounds ?? .zero
    }
}

final class TabStripView: NSView {
    var tabs: [TabInfo] = [] { didSet { needsDisplay = true } }
    var showsClose = true
    var onSelect: ((WindowID) -> Void)?
    var onClose: ((WindowID) -> Void)?
    var onDragBegan: ((WindowID) -> Void)?
    var onDragMoved: (() -> Void)?
    var onDragEnded: (() -> Void)?
    var menuFor: ((WindowID) -> NSMenu)?

    private var pressed: (window: WindowID, at: NSPoint, onClose: Bool)?
    private var dragging = false
    private var hovered: Int?
    private var tracking: NSTrackingArea?

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func updateTrackingAreas() {
        if let tracking { removeTrackingArea(tracking) }
        let t = NSTrackingArea(rect: bounds, options: [.mouseMoved, .mouseEnteredAndExited, .activeAlways],
                               owner: self, userInfo: nil)
        addTrackingArea(t)
        tracking = t
    }

    private func tabRect(_ i: Int) -> NSRect {
        let inner = bounds.insetBy(dx: 4, dy: 4)
        let w = inner.width / CGFloat(max(1, tabs.count))
        return NSRect(x: inner.minX + CGFloat(i) * w, y: inner.minY, width: w, height: inner.height).insetBy(dx: 2, dy: 0)
    }

    private func closeRect(_ i: Int) -> NSRect {
        let r = tabRect(i)
        return NSRect(x: r.minX + 6, y: r.midY - 7, width: 14, height: 14)
    }

    private func index(at p: NSPoint) -> Int? {
        tabs.indices.first { tabRect($0).contains(p) }
    }

    override func mouseMoved(with event: NSEvent) {
        let i = index(at: convert(event.locationInWindow, from: nil))
        if i != hovered { hovered = i; needsDisplay = true }
    }

    override func mouseExited(with event: NSEvent) {
        hovered = nil
        needsDisplay = true
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        guard let i = index(at: p) else { return }
        pressed = (tabs[i].window, p, showsClose && hovered == i && closeRect(i).contains(p))
        dragging = false
    }

    override func mouseDragged(with event: NSEvent) {
        guard let pressed, !pressed.onClose else { return }
        let p = convert(event.locationInWindow, from: nil)
        if !dragging, hypot(p.x - pressed.at.x, p.y - pressed.at.y) > 5 {
            dragging = true
            onDragBegan?(pressed.window)
        }
        if dragging { onDragMoved?() }
    }

    override func mouseUp(with event: NSEvent) {
        defer { pressed = nil; dragging = false }
        guard let pressed else { return }
        if dragging {
            onDragEnded?()
        } else if pressed.onClose {
            onClose?(pressed.window)
        } else {
            onSelect?(pressed.window)
        }
    }

    override func rightMouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        guard let i = index(at: p), let menu = menuFor?(tabs[i].window) else { return }
        NSMenu.popUpContextMenu(menu, with: event, for: self)
    }

    override func draw(_ dirtyRect: NSRect) {
        // The glass behind is the strip; tabs are chips on it.
        for (i, tab) in tabs.enumerated() {
            let r = tabRect(i)
            if tab.selected {
                NSColor.white.withAlphaComponent(0.16).setFill()
                NSBezierPath(roundedRect: r, xRadius: 7, yRadius: 7).fill()
            } else if hovered == i {
                NSColor.white.withAlphaComponent(0.07).setFill()
                NSBezierPath(roundedRect: r, xRadius: 7, yRadius: 7).fill()
            }
            var x = r.minX + 8
            if showsClose, hovered == i {
                let c = closeRect(i)
                let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 11, weight: .bold),
                                                            .foregroundColor: NSColor.secondaryLabelColor]
                NSAttributedString(string: "✕", attributes: attrs).draw(at: NSPoint(x: c.minX + 2, y: c.minY))
                x = c.maxX + 4
            } else {
                tab.icon.draw(in: NSRect(x: x, y: r.midY - 8, width: 16, height: 16))
                x += 22
            }
            let attrs: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: 12, weight: tab.selected ? .semibold : .regular),
                .foregroundColor: NSColor.white.withAlphaComponent(tab.selected ? 0.95 : 0.6),
            ]
            let title = NSAttributedString(string: tab.title.isEmpty ? " " : tab.title, attributes: attrs)
            let h = title.size().height
            title.draw(with: NSRect(x: x, y: r.midY - h / 2, width: max(0, r.maxX - x - 6), height: h),
                       options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
        }
    }
}

// MARK: - Gap handle

/// Lives in the gap between two neighbours; drag it to resize both.
final class GapHandlePanel: OverlayPanel {
    let view = GapHandleView()

    init() {
        super.init(level: .normal, mouse: true)
        contentView = view
    }
}

final class GapHandleView: NSView {
    var axis: Axis = .horizontal
    var onBegin: (() -> Void)?
    /// Called with the pointer's movement along the axis since the last call.
    var onDrag: ((Double) -> Void)?
    var onEnd: (() -> Void)?
    private var last: CanopyCore.Point?
    private var hovering = false
    private var tracking: NSTrackingArea?

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func updateTrackingAreas() {
        if let tracking { removeTrackingArea(tracking) }
        let t = NSTrackingArea(rect: bounds, options: [.mouseEnteredAndExited, .activeAlways, .cursorUpdate],
                               owner: self, userInfo: nil)
        addTrackingArea(t)
        tracking = t
    }

    private var cursor: NSCursor { axis == .horizontal ? .resizeLeftRight : .resizeUpDown }

    override func cursorUpdate(with event: NSEvent) { cursor.set() }
    override func mouseEntered(with event: NSEvent) { hovering = true; cursor.set(); needsDisplay = true }
    override func mouseExited(with event: NSEvent) {
        hovering = false
        if last == nil { NSCursor.arrow.set() }
        needsDisplay = true
    }

    override func mouseDown(with event: NSEvent) {
        last = Coordinates.mouse
        onBegin?()
    }

    override func mouseDragged(with event: NSEvent) {
        guard let l = last else { return }
        let p = Coordinates.mouse
        let d = axis == .horizontal ? p.x - l.x : p.y - l.y
        if d != 0 { onDrag?(d) }
        last = p
        cursor.set()
    }

    override func mouseUp(with event: NSEvent) {
        last = nil
        onEnd?()
    }

    override func draw(_ dirtyRect: NSRect) {
        guard hovering || last != nil else { return }
        let pill: NSRect = axis == .horizontal
            ? NSRect(x: bounds.midX - 2, y: bounds.midY - 24, width: 4, height: 48)
            : NSRect(x: bounds.midX - 24, y: bounds.midY - 2, width: 48, height: 4)
        NSColor.labelColor.withAlphaComponent(0.45).setFill()
        NSBezierPath(roundedRect: pill, xRadius: 2, yRadius: 2).fill()
    }
}

// MARK: - Drop preview, marquee, tab ghost

/// Shows where a dragged window will land.
final class DropPreviewPanel: OverlayPanel {
    private let view = HighlightView()

    init() {
        super.init(level: .floating, mouse: false)
        contentView = view
        onEverySpace()
    }

    func show(_ r: CanopyCore.Rect, tab: Bool) {
        view.tab = tab
        view.needsDisplay = true
        if isVisible {
            NSAnimationContext.runAnimationGroup { ctx in
                ctx.duration = 0.12
                animator().setFrame(Coordinates.ns(fromAX: r), display: true)
            }
        } else {
            place(r)
            orderFrontRegardless()
        }
    }
}

private final class HighlightView: NSView {
    var tab = false
    override func draw(_ dirtyRect: NSRect) {
        let r = bounds.insetBy(dx: 2, dy: 2)
        let path = NSBezierPath(roundedRect: r, xRadius: 10, yRadius: 10)
        NSColor.controlAccentColor.withAlphaComponent(0.18).setFill()
        path.fill()
        NSColor.controlAccentColor.withAlphaComponent(0.8).setStroke()
        path.lineWidth = 2
        path.stroke()
        if tab {
            let strip = NSRect(x: r.minX, y: r.maxY - Solver.tabStripHeight, width: r.width, height: Solver.tabStripHeight)
            NSColor.controlAccentColor.withAlphaComponent(0.35).setFill()
            NSBezierPath(roundedRect: strip, xRadius: 8, yRadius: 8).fill()
        }
    }
}

/// The rectangle drawn with ⌘⇧-drag.
final class MarqueePanel: OverlayPanel {
    private let view = MarqueeView()

    init() {
        super.init(level: .floating, mouse: false)
        contentView = view
        onEverySpace()
    }

    func show(_ r: CanopyCore.Rect) {
        place(r)
        if !isVisible { orderFrontRegardless() }
    }
}

private final class MarqueeView: NSView {
    override func draw(_ dirtyRect: NSRect) {
        let path = NSBezierPath(rect: bounds.insetBy(dx: 1, dy: 1))
        NSColor.controlAccentColor.withAlphaComponent(0.12).setFill()
        path.fill()
        NSColor.controlAccentColor.setStroke()
        path.setLineDash([6, 4], count: 2, phase: 0)
        path.lineWidth = 2
        path.stroke()
    }
}

/// Follows the pointer while a tab is dragged out of its strip.
final class GhostPanel: OverlayPanel {
    private let imageView = NSImageView()
    private let label = NSTextField(labelWithString: "")

    init() {
        super.init(level: .floating, mouse: false)
        let v = NSView()
        v.wantsLayer = true
        v.layer?.cornerRadius = 10
        v.layer?.masksToBounds = true
        v.layer?.backgroundColor = NSColor.windowBackgroundColor.withAlphaComponent(0.95).cgColor
        imageView.imageScaling = .scaleProportionallyUpOrDown
        label.font = .systemFont(ofSize: 12, weight: .medium)
        label.lineBreakMode = .byTruncatingTail
        v.addSubview(imageView)
        v.addSubview(label)
        contentView = v
        alphaValue = 0.9
        onEverySpace()
    }

    func show(image: NSImage, title: String, at p: CanopyCore.Point) {
        imageView.image = image
        label.stringValue = title
        let size = CGSize(width: 220, height: 150)
        imageView.frame = NSRect(x: 8, y: 30, width: size.width - 16, height: size.height - 38)
        label.frame = NSRect(x: 10, y: 6, width: size.width - 20, height: 18)
        move(to: p, size: size)
        orderFrontRegardless()
    }

    func move(to p: CanopyCore.Point, size: CGSize? = nil) {
        let s = size ?? frame.size
        place(CanopyCore.Rect(x: p.x - s.width / 2, y: p.y - 14, width: s.width, height: s.height))
    }
}
