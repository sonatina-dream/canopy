import AppKit
import CanopyCore
import QuartzCore

/// The glass behind arranged windows while nothing moves: the desktop,
/// blurred and darkened, everywhere except the Desktop's own space. It sits
/// just above the desktop icons and below every window, and never takes the
/// mouse.
final class DeskPanel {
    private let panel = OverlayPanel(level: NSWindow.Level(rawValue: Int(CGWindowLevelForKey(.desktopIconWindow)) + 1), mouse: false)
    private let layer: CALayer
    private let mask = CAShapeLayer()
    private(set) var display: Display

    init(display: Display) {
        self.display = display
        let view = NSView()
        view.wantsLayer = true
        panel.contentView = view
        // The glass belongs to its Space's arrangement, so it stays on that Space.
        layer = view.layer!
        layer.contentsGravity = .resize
        mask.fillRule = .evenOdd
        layer.mask = mask
    }

    /// Shows glass with the Desktop's space (`desk`) clear. Hidden when
    /// nothing is docked, so a plain desktop stays plain.
    func update(display: Display, glass: CGImage?, desk: CanopyCore.Rect?, anyDocked: Bool) {
        self.display = display
        guard anyDocked else {
            panel.orderOut(nil)
            return
        }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        if let glass {
            layer.contents = glass
            layer.backgroundColor = nil
        } else {
            layer.contents = nil
            layer.backgroundColor = NSColor.black.withAlphaComponent(0.3).cgColor
        }
        let size = CGSize(width: display.frame.width, height: display.frame.height)
        layer.frame = CGRect(origin: .zero, size: size)
        mask.frame = layer.bounds
        let path = CGMutablePath()
        path.addRect(layer.bounds)
        if let desk {
            path.addRoundedRect(in: CGRect(x: desk.x - display.frame.x, y: display.frame.maxY - desk.maxY,
                                           width: desk.width, height: desk.height), cornerWidth: 10, cornerHeight: 10)
        }
        mask.path = path
        CATransaction.commit()
        panel.place(display.frame)
        if !panel.isVisible { panel.orderFrontRegardless() }
    }

    func hide() { panel.orderOut(nil) }
}
