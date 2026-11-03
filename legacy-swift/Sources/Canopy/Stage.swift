import AppKit
import CanopyCore
import QuartzCore

/// A borderless panel Canopy draws in. Never activates the app.
class OverlayPanel: NSPanel {
    init(level: NSWindow.Level, mouse: Bool) {
        super.init(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        self.level = level
        isOpaque = false
        backgroundColor = .clear
        hasShadow = false
        ignoresMouseEvents = !mouse
        isReleasedWhenClosed = false
        hidesOnDeactivate = false
        animationBehavior = .none
        // Overlays belong to the Space they were shown on, like the windows
        // they decorate; transient ones (motion, previews) opt into every Space.
        collectionBehavior = [.stationary, .ignoresCycle]
    }

    /// For panels that must appear on whichever Space is current.
    func onEverySpace() {
        collectionBehavior = [.stationary, .ignoresCycle, .canJoinAllSpaces, .fullScreenAuxiliary]
    }

    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }

    func place(_ r: CanopyCore.Rect) {
        setFrame(Coordinates.ns(fromAX: r), display: true)
    }
}

/// One window drawn on the stage.
struct StageItem {
    var id: WindowID
    var rect: CanopyCore.Rect
    var image: CGImage?
    /// Placeholder colour when there is no picture.
    var colour: NSColor
    var icon: NSImage?
    /// What shows past the picture when the window grows.
    var edge: CGColor?
}

/// Draws motion for one display: its wallpaper and a picture of every visible
/// window, which slide while the real windows move out of sight underneath.
/// Above ordinary windows, below the menu bar, and never takes the mouse.
final class StagePanel {
    let display: Display
    private let panel = OverlayPanel(level: NSWindow.Level(rawValue: Int(CGWindowLevelForKey(.utilityWindow))), mouse: false)
    private let root: CALayer
    /// Holds the window pictures; its transform is the live pinch camera.
    private let content = CALayer()
    private var layers: [WindowID: CALayer] = [:]
    private var icons: [WindowID: CALayer] = [:]
    /// Picture layers of raised items (their outer layer carries the shadow).
    private var inners: [WindowID: CALayer] = [:]
    private let slot = CALayer()
    /// Inside a tab slot: where the new tab joins the strip.
    private let slotStrip = CALayer()
    /// Blurred, darkened desktop behind arranged windows, with the Desktop's
    /// own space cut out so it stays clear.
    private let glassLayer = CALayer()
    private let glassMask = CAShapeLayer()
    /// The blurred desktop, when it has been captured.
    var glass: ((Display) -> CGImage?)?
    private var generation = 0
    private(set) var isShown = false
    private var shownAt = CFAbsoluteTimeGetCurrent()
    /// The desktop as it is drawn, when Canopy may capture it.
    var backdrop: ((Display) -> CGImage?)?
    let curve = CAMediaTimingFunction(controlPoints: 0.25, 0.1, 0.1, 1.0)

    /// The area the stage covers: everything below the menu bar, Dock included.
    let area: CanopyCore.Rect

    init(display: Display) {
        self.display = display
        let f = display.frame
        area = CanopyCore.Rect(x: f.x, y: display.visible.y, width: f.width, height: f.maxY - display.visible.y)
        let view = NSView()
        view.wantsLayer = true
        panel.contentView = view
        panel.onEverySpace()
        root = view.layer!
        root.masksToBounds = true
        slot.cornerRadius = 12
        slot.borderWidth = 2
        slot.borderColor = NSColor.controlAccentColor.withAlphaComponent(0.9).cgColor
        slot.backgroundColor = NSColor.controlAccentColor.withAlphaComponent(0.16).cgColor
        slot.isHidden = true
        slotStrip.cornerRadius = 8
        slotStrip.backgroundColor = NSColor.controlAccentColor.withAlphaComponent(0.35).cgColor
        slotStrip.isHidden = true
        slot.addSublayer(slotStrip)
    }

    /// Puts the stage up with `items` (back to front) where they are now.
    /// `desk` is the Desktop's clear space now (nil when it is out of view).
    func show(_ items: [StageItem], desk: CanopyCore.Rect? = nil) {
        generation += 1
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        root.sublayers?.forEach { $0.removeFromSuperlayer() }
        layers = [:]
        icons = [:]
        inners = [:]
        let backdrop = CALayer()
        backdrop.backgroundColor = NSColor.windowBackgroundColor.cgColor
        backdrop.frame = local(display.frame)
        if let desktop = self.backdrop?(display) {
            // The captured desktop matches the display pixel for pixel.
            backdrop.contents = desktop
            backdrop.contentsGravity = .resize
            if !isShown { Log.add("STAGE up on the captured desktop") }
        } else if let wallpaper = AppLook.wallpaper(for: display) {
            backdrop.contents = wallpaper
            backdrop.contentsGravity = .resizeAspectFill
            if !isShown { Log.add("STAGE up on the wallpaper file (no desktop capture yet)") }
        }
        root.addSublayer(backdrop)
        glassLayer.frame = local(display.frame)
        glassLayer.contentsGravity = .resize
        if let g = glass?(display) {
            glassLayer.contents = g
            glassLayer.backgroundColor = nil
        } else {
            glassLayer.contents = nil
            glassLayer.backgroundColor = NSColor.black.withAlphaComponent(0.3).cgColor
        }
        glassMask.frame = glassLayer.bounds
        glassMask.fillRule = .evenOdd
        glassMask.path = glassPath(desk)
        glassLayer.mask = glassMask
        root.addSublayer(glassLayer)
        content.transform = CATransform3DIdentity
        content.frame = CGRect(origin: .zero, size: CGSize(width: area.width, height: area.height))
        content.sublayers?.forEach { $0.removeFromSuperlayer() }
        root.addSublayer(content)
        for item in items { add(item) }
        slot.isHidden = true
        content.addSublayer(slot)
        CATransaction.commit()
        panel.alphaValue = 1
        panel.place(area)
        panel.orderFrontRegardless()
        if !isShown { shownAt = CFAbsoluteTimeGetCurrent() }
        isShown = true
    }

    private func add(_ item: StageItem) {
        let l = CALayer()
        l.cornerRadius = 10
        l.masksToBounds = true
        l.borderColor = NSColor.black.withAlphaComponent(0.18).cgColor
        l.borderWidth = 0.5
        l.backgroundColor = (item.image != nil ? item.edge : nil) ?? item.colour.cgColor
        l.frame = local(item.rect)
        if let image = item.image {
            l.contents = image
            l.contentsScale = PictureStore.scale
            l.contentsGravity = gravity(image, for: item.rect)
        } else if let icon = item.icon {
            let i = CALayer()
            i.contents = icon
            i.contentsGravity = .resizeAspect
            i.frame = iconFrame(in: l.bounds)
            l.addSublayer(i)
            icons[item.id] = i
        }
        content.addSublayer(l)
        layers[item.id] = l
    }

    func contains(_ id: WindowID) -> Bool { layers[id] != nil }

    /// Raises one item above the others (the window being dragged).
    func bringToFront(_ id: WindowID) {
        guard let l = layers[id] else { return }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        l.removeFromSuperlayer()
        content.insertSublayer(l, below: slot)
        l.shadowOpacity = 0.35
        l.shadowRadius = 18
        l.shadowOffset = CGSize(width: 0, height: -8)
        l.masksToBounds = false
        if let content = l.contents {
            // Keep rounded corners on the picture while the shadow shows.
            let inner = CALayer()
            inner.frame = l.bounds
            inner.contents = content
            inner.contentsScale = l.contentsScale
            inner.contentsGravity = l.contentsGravity
            inner.cornerRadius = 10
            inner.masksToBounds = true
            inner.autoresizingMask = [.layerWidthSizable, .layerHeightSizable]
            l.contents = nil
            l.backgroundColor = nil
            l.addSublayer(inner)
            inners[id] = inner
        }
        CATransaction.commit()
    }

    /// Moves an item at once (following the pointer).
    func move(_ id: WindowID, to r: CanopyCore.Rect) {
        guard let l = layers[id] else { return }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        l.frame = local(r)
        icons[id]?.frame = iconFrame(in: l.bounds)
        CATransaction.commit()
    }

    /// A picture keeps its top-left in place like a window being resized, and
    /// only scales down when the window shrinks a lot (into a cover).
    private func gravity(_ image: CGImage, for r: CanopyCore.Rect) -> CALayerContentsGravity {
        let w = CGFloat(image.width) / PictureStore.scale, h = CGFloat(image.height) / PictureStore.scale
        return r.width >= w * 0.5 && r.height >= h * 0.5 ? .topLeft : .resizeAspectFill
    }

    /// Zooms the whole picture by `scale` around `p`, at once (a pinch in progress).
    func setCamera(scale: Double, around p: CanopyCore.Point) {
        let q = CGPoint(x: p.x - area.x, y: area.maxY - p.y)
        let c = CGPoint(x: content.bounds.midX, y: content.bounds.midY)
        // The layer scales about its centre; shift so that `p` stays put.
        let shift = CGPoint(x: (1 - scale) * (q.x - c.x), y: (1 - scale) * (q.y - c.y))
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        content.transform = CATransform3DConcat(CATransform3DMakeScale(scale, scale, 1),
                                                CATransform3DMakeTranslation(shift.x, shift.y, 0))
        CATransaction.commit()
    }

    /// Slides items to new rects (and lets a pinch camera settle back).
    func animate(_ targets: [WindowID: CanopyCore.Rect], duration: Double) {
        CATransaction.begin()
        CATransaction.setAnimationDuration(duration)
        CATransaction.setAnimationTimingFunction(curve)
        if !CATransform3DIsIdentity(content.transform) { content.transform = CATransform3DIdentity }
        for (id, r) in targets {
            guard let l = layers[id] else { continue }
            let pictureLayer = inners[id] ?? l
            if let content = pictureLayer.contents {
                pictureLayer.contentsGravity = gravity(content as! CGImage, for: r)
            }
            l.frame = local(r)
            icons[id]?.frame = iconFrame(in: CGRect(origin: .zero, size: l.frame.size))
        }
        CATransaction.commit()
    }

    /// Moves items at once (a gap being dragged).
    func place(_ targets: [WindowID: CanopyCore.Rect]) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        for (id, r) in targets {
            guard let l = layers[id] else { continue }
            let pictureLayer = inners[id] ?? l
            if let content = pictureLayer.contents { pictureLayer.contentsGravity = gravity(content as! CGImage, for: r) }
            l.frame = local(r)
            icons[id]?.frame = iconFrame(in: l.bounds)
        }
        CATransaction.commit()
    }

    /// Slides the Desktop's clear space to `desk` (nil: glass everywhere).
    func setDesk(_ desk: CanopyCore.Rect?, duration: Double) {
        CATransaction.begin()
        CATransaction.setAnimationDuration(duration)
        CATransaction.setAnimationTimingFunction(curve)
        glassMask.path = glassPath(desk)
        CATransaction.commit()
    }

    private func glassPath(_ desk: CanopyCore.Rect?) -> CGPath {
        let path = CGMutablePath()
        let bounds = CGRect(origin: .zero, size: CGSize(width: display.frame.width, height: display.frame.height))
        path.addRect(bounds)
        if let desk {
            // In the glass layer's own space (it spans the whole display).
            let r = CGRect(x: desk.x - display.frame.x, y: display.frame.maxY - desk.maxY, width: desk.width, height: desk.height)
            path.addRoundedRect(in: r, cornerWidth: 10, cornerHeight: 10)
        } else {
            // An empty hole keeps the path's shape the same, so it animates.
            path.addRoundedRect(in: CGRect(x: bounds.midX, y: bounds.midY, width: 0, height: 0), cornerWidth: 0, cornerHeight: 0)
        }
        return path
    }

    /// Shows where the dragged window will land. To dock: a filled slot. To
    /// join as a tab: the whole target outlined, with the strip it will share.
    func setSlot(_ r: CanopyCore.Rect?, tab: Bool = false) {
        CATransaction.begin()
        CATransaction.setAnimationDuration(0.18)
        CATransaction.setAnimationTimingFunction(curve)
        if let r {
            if slot.isHidden {
                CATransaction.setDisableActions(true)
                slot.isHidden = false
            }
            let frame = local(r).insetBy(dx: -3, dy: -3)
            slot.frame = frame
            slot.borderWidth = tab ? 3 : 2
            slot.borderColor = NSColor.controlAccentColor.withAlphaComponent(tab ? 1 : 0.9).cgColor
            slot.backgroundColor = NSColor.controlAccentColor.withAlphaComponent(tab ? 0.08 : 0.16).cgColor
            slotStrip.isHidden = !tab
            let h = CGFloat(Solver.tabStripHeight)
            slotStrip.frame = CGRect(x: 6, y: frame.height - h - 3, width: frame.width - 12, height: h - 6)
        } else {
            slot.isHidden = true
        }
        CATransaction.commit()
    }

    /// Slides one item to a rect quickly (the dragged window becoming a
    /// thumbnail over a target, and back).
    func reshape(_ id: WindowID, to r: CanopyCore.Rect, duration: Double = 0.18) {
        animate([id: r], duration: duration)
    }

    /// What the slot shows now: nil, "dock" or "tab" (for the test harness).
    var slotStyle: String? { slot.isHidden ? nil : (slotStrip.isHidden ? "dock" : "tab") }

    /// The size of an item's picture, in points.
    func pictureSize(_ id: WindowID) -> CGSize? {
        guard let content = (inners[id] ?? layers[id])?.contents else { return nil }
        let image = content as! CGImage
        return CGSize(width: CGFloat(image.width) / PictureStore.scale, height: CGFloat(image.height) / PictureStore.scale)
    }

    func setImage(_ id: WindowID, _ image: CGImage) {
        guard let l = layers[id] else { return }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        if let inner = inners[id] {
            inner.contents = image
            inner.contentsScale = PictureStore.scale
        } else {
            l.contents = image
            l.contentsScale = PictureStore.scale
            l.contentsGravity = .topLeft
            icons[id]?.removeFromSuperlayer()
            icons[id] = nil
        }
        CATransaction.commit()
    }

    /// Takes the stage down after `delay`, once `writer` has finished moving
    /// the real windows (or a second has passed) and they have had `redraw`
    /// seconds to draw.
    func land(after delay: Double, writer: WindowWriter, redraw: Double) {
        let g = generation
        let landStart = CFAbsoluteTimeGetCurrent()
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in
            guard let self, self.generation == g else { return }
            writer.whenIdle(timeout: 1.0) {
                Log.add("LANDED writes done \(Int((CFAbsoluteTimeGetCurrent() - landStart) * 1000)) ms after release")
                DispatchQueue.main.asyncAfter(deadline: .now() + redraw) {
                    guard self.generation == g else { return }
                    self.fade()
                }
            }
        }
    }

    /// Takes the stage down now (a click during motion ends it).
    func fade(duration: Double = 0.14) {
        guard isShown else { return }
        Log.add("STAGE down after \(Int((CFAbsoluteTimeGetCurrent() - shownAt) * 1000)) ms")
        generation += 1
        let g = generation
        isShown = false
        NSAnimationContext.runAnimationGroup({ ctx in
            ctx.duration = duration
            panel.animator().alphaValue = 0
        }, completionHandler: { [weak self] in
            guard let self, self.generation == g else { return }
            self.panel.orderOut(nil)
            self.root.sublayers?.forEach { $0.removeFromSuperlayer() }
            self.layers = [:]
            self.icons = [:]
            self.inners = [:]
        })
    }

    func tearDown() {
        generation += 1
        isShown = false
        panel.orderOut(nil)
    }

    private func iconFrame(in bounds: CGRect) -> CGRect {
        let side = max(16, min(96, min(bounds.width, bounds.height) * 0.4))
        return CGRect(x: bounds.midX - side / 2, y: bounds.midY - side / 2, width: side, height: side)
    }

    /// AX rect to the panel's bottom-left local space.
    private func local(_ r: CanopyCore.Rect) -> CGRect {
        CGRect(x: r.x - area.x, y: area.maxY - r.maxY, width: r.width, height: r.height)
    }
}

/// The stages of all displays.
final class Stage {
    private var panels: [String: StagePanel] = [:]
    var backdrop: ((Display) -> CGImage?)?
    var glass: ((Display) -> CGImage?)?
    var duration: Double = 0.46
    var redrawWait: Double = 0.15

    func panel(for display: Display) -> StagePanel {
        if let p = panels[display.uuid], p.display == display { return p }
        panels[display.uuid]?.tearDown()
        let p = StagePanel(display: display)
        p.backdrop = { [weak self] in self?.backdrop?($0) }
        p.glass = { [weak self] in self?.glass?($0) }
        panels[display.uuid] = p
        return p
    }

    var anyShown: Bool { panels.values.contains { $0.isShown } }

    /// A whole layout change: show where things are, slide them to where they
    /// go, run `apply` (which queues the real moves), and land.
    func transition(on display: Display, items: [StageItem], to targets: [WindowID: CanopyCore.Rect],
                    deskFrom: CanopyCore.Rect?, deskTo: CanopyCore.Rect?, writer: WindowWriter, apply: () -> Void) {
        let p = panel(for: display)
        p.show(items, desk: deskFrom)
        p.animate(targets, duration: duration)
        p.setDesk(deskTo, duration: duration)
        apply()
        p.land(after: duration, writer: writer, redraw: redrawWait)
    }

    /// Ends every landing at once.
    func interrupt() {
        for p in panels.values where p.isShown { p.fade(duration: 0.08) }
    }

    func tearDown() {
        for p in panels.values { p.tearDown() }
    }
}
