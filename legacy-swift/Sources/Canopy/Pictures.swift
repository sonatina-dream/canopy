import AppKit
import CanopyCore
import CoreImage
import ScreenCaptureKit

/// Still pictures of windows from ScreenCaptureKit, kept in memory only.
/// Used while windows move and on covers. Without Screen Recording
/// permission every picture is nil and callers draw app colour and icon.
final class PictureStore {
    private(set) var images: [WindowID: CGImage] = [:]
    /// The colour at each picture's right and bottom edges: what a window
    /// shows when it grows past its picture.
    private(set) var edgeColours: [WindowID: CGColor] = [:]
    private var inFlight = false
    private var queued = Set<WindowID>()
    /// Called on the main thread when a new picture arrives.
    var onUpdate: ((WindowID, CGImage) -> Void)?
    private var timer: Timer?
    /// Which windows to refresh on each tick.
    var wanted: () -> [WindowID] = { [] }
    var enabled = true

    var allowed: Bool { CGPreflightScreenCaptureAccess() }

    /// Pixels per point in pictures: half the backing store is plenty for motion.
    static var scale: CGFloat { (NSScreen.main?.backingScaleFactor ?? 2) * 0.5 }

    func start() {
        timer = Timer.scheduledTimer(withTimeInterval: 1.5, repeats: true) { [weak self] _ in
            self?.refresh()
        }
    }

    func forget(_ id: WindowID) {
        images[id] = nil
        edgeColours[id] = nil
    }

    // MARK: The desktop as it looks

    /// Each display's desktop exactly as it is drawn (wallpaper, dynamic or
    /// not, and desktop icons), captured with every window left out. The stage
    /// stands on this, so the background never changes under a drag.
    private(set) var backdrops: [CGDirectDisplayID: CGImage] = [:]
    /// The same desktop blurred and darkened: the glass behind arranged windows.
    private(set) var glass: [CGDirectDisplayID: CGImage] = [:]
    var onBackdrop: ((CGDirectDisplayID) -> Void)?
    private static let ciContext = CIContext(options: [.cacheIntermediates: false])
    private var backdropTimer: Timer?
    private var backdropInFlight = false

    func startBackdrops() {
        refreshBackdrops()
        backdropTimer = Timer.scheduledTimer(withTimeInterval: 15, repeats: true) { [weak self] _ in self?.refreshBackdrops() }
        let ws = NSWorkspace.shared.notificationCenter
        ws.addObserver(forName: NSWorkspace.activeSpaceDidChangeNotification, object: nil, queue: .main) { [weak self] _ in
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { self?.refreshBackdrops() }
        }
        NotificationCenter.default.addObserver(forName: NSApplication.didChangeScreenParametersNotification,
                                               object: nil, queue: .main) { [weak self] _ in
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { self?.refreshBackdrops() }
        }
    }

    /// Blur 20 pt and darken 30% (TrellisWM's dimBlur and dimOpacity), worked
    /// at a quarter size since a blur hides the detail anyway.
    static func makeGlass(_ image: CGImage) -> CGImage? {
        let small = CIImage(cgImage: image).transformed(by: CGAffineTransform(scaleX: 0.25, y: 0.25))
        let blurred = small.clampedToExtent().applyingGaussianBlur(sigma: 10).cropped(to: small.extent)
        let dark = CIImage(color: CIColor(red: 0, green: 0, blue: 0, alpha: 0.3)).cropped(to: small.extent)
        return ciContext.createCGImage(dark.composited(over: blurred), from: small.extent)
    }

    /// The average colour of a strip along a picture's right and bottom edges.
    static func edgeColour(of image: CGImage) -> CGColor? {
        let size = 8
        guard let ctx = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: size * 4,
                                  space: CGColorSpaceCreateDeviceRGB(),
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        ctx.interpolationQuality = .medium
        ctx.draw(image, in: CGRect(x: 0, y: 0, width: size, height: size))
        guard let data = ctx.data?.assumingMemoryBound(to: UInt8.self) else { return nil }
        var r = 0.0, g = 0.0, b = 0.0, n = 0.0
        for y in 0..<size {
            for x in 0..<size where x >= size - 2 || y <= 1 {   // right column, bottom rows (y is flipped)
                let i = (y * size + x) * 4
                r += Double(data[i]); g += Double(data[i + 1]); b += Double(data[i + 2]); n += 1
            }
        }
        return CGColor(red: r / n / 255, green: g / n / 255, blue: b / n / 255, alpha: 1)
    }

    func refreshBackdrops() {
        guard allowed, !backdropInFlight else { return }
        backdropInFlight = true
        let scales = Dictionary(uniqueKeysWithValues: NSScreen.screens.compactMap { s in s.displayID.map { ($0, s.backingScaleFactor) } })
        // All capture work stays off the main thread; only results come back.
        Task.detached(priority: .utility) {
            var results: [(CGDirectDisplayID, CGImage, CGImage?)] = []
            if let content = try? await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true) {
                // Everything at or above the normal window level is left out:
                // windows, panels, Canopy's own stage, the Dock and the menu bar.
                let above = content.windows.filter { $0.windowLayer >= 0 }
                for display in content.displays {
                    let scale = scales[display.displayID] ?? 2
                    let config = SCStreamConfiguration()
                    config.width = Int(CGFloat(display.width) * scale)
                    config.height = Int(CGFloat(display.height) * scale)
                    config.showsCursor = false
                    let filter = SCContentFilter(display: display, excludingWindows: above)
                    if let image = try? await SCScreenshotManager.captureImage(contentFilter: filter, configuration: config) {
                        results.append((display.displayID, image, PictureStore.makeGlass(image)))
                    }
                }
            }
            let finished = results
            await MainActor.run {
                for (id, image, glass) in finished {
                    self.backdrops[id] = image
                    self.glass[id] = glass
                    self.onBackdrop?(id)
                }
                self.backdropInFlight = false
            }
        }
    }

    func refresh(_ ids: [WindowID]? = nil) {
        guard enabled, allowed else { return }
        let ids = Set(ids ?? wanted())
        guard !ids.isEmpty else { return }
        if inFlight {
            queued.formUnion(ids)
            return
        }
        inFlight = true
        let scale = PictureStore.scale
        // All capture work, and sampling the pictures, stays off the main
        // thread; only finished pictures are handed back.
        Task.detached(priority: .utility) {
            var results: [(WindowID, CGImage, CGColor?)] = []
            if let content = try? await SCShareableContent.excludingDesktopWindows(true, onScreenWindowsOnly: true) {
                for w in content.windows where ids.contains(WindowID(w.windowID)) {
                    let config = SCStreamConfiguration()
                    config.width = max(1, Int(w.frame.width * scale))
                    config.height = max(1, Int(w.frame.height * scale))
                    config.showsCursor = false
                    config.ignoreShadowsSingleWindow = true
                    let filter = SCContentFilter(desktopIndependentWindow: w)
                    if let image = try? await SCScreenshotManager.captureImage(contentFilter: filter, configuration: config) {
                        results.append((WindowID(w.windowID), image, PictureStore.edgeColour(of: image)))
                    }
                }
            }
            let finished = results
            await MainActor.run {
                for (id, image, edge) in finished {
                    self.images[id] = image
                    if let edge { self.edgeColours[id] = edge }
                    self.onUpdate?(id, image)
                }
                self.inFlight = false
                if !self.queued.isEmpty {
                    let next = Array(self.queued)
                    self.queued = []
                    self.refresh(next)
                }
            }
        }
    }
}

/// App icon and colour, used for covers and for motion without pictures.
enum AppLook {
    private static var icons: [String: NSImage] = [:]
    private static var colours: [String: NSColor] = [:]

    static func icon(for bundleID: String, pid: pid_t) -> NSImage {
        if let i = icons[bundleID] { return i }
        let i = NSRunningApplication(processIdentifier: pid)?.icon
            ?? NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleID).map { NSWorkspace.shared.icon(forFile: $0.path) }
            ?? NSImage(systemSymbolName: "macwindow", accessibilityDescription: nil)!
        icons[bundleID] = i
        return i
    }

    /// What a window without a picture shows: mostly grey, with a quarter of
    /// its app's colour (TrellisWM's appColourAmount).
    static func placeholder(for bundleID: String, pid: pid_t) -> NSColor {
        let app = colour(for: bundleID, pid: pid)
        return NSColor(deviceWhite: 0.3, alpha: 1).blended(withFraction: 0.25, of: app) ?? app
    }

    /// The icon's average opaque colour, a little more saturated.
    static func colour(for bundleID: String, pid: pid_t) -> NSColor {
        if let c = colours[bundleID] { return c }
        let icon = icon(for: bundleID, pid: pid)
        var colour = NSColor.systemGray
        let size = 16
        if let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size, bitsPerSample: 8,
                                      samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                                      bytesPerRow: 0, bitsPerPixel: 0) {
            NSGraphicsContext.saveGraphicsState()
            NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
            icon.draw(in: NSRect(x: 0, y: 0, width: size, height: size))
            NSGraphicsContext.restoreGraphicsState()
            var r = 0.0, g = 0.0, b = 0.0, n = 0.0
            for x in 0..<size {
                for y in 0..<size {
                    guard let c = rep.colorAt(x: x, y: y), c.alphaComponent > 0.5 else { continue }
                    r += c.redComponent; g += c.greenComponent; b += c.blueComponent; n += 1
                }
            }
            if n > 0 {
                let avg = NSColor(deviceRed: r / n, green: g / n, blue: b / n, alpha: 1)
                var h: CGFloat = 0, s: CGFloat = 0, v: CGFloat = 0, a: CGFloat = 0
                avg.getHue(&h, saturation: &s, brightness: &v, alpha: &a)
                colour = NSColor(deviceHue: h, saturation: min(1, s * 1.3), brightness: min(0.85, max(0.35, v)), alpha: 1)
            }
        }
        colours[bundleID] = colour
        return colour
    }

    /// Works out every running app's icon and colour ahead of time, one app
    /// per turn of the main loop, so the first drag doesn't pay for it.
    static func prepareApps() {
        var apps = NSWorkspace.shared.runningApplications.filter { $0.activationPolicy == .regular && $0.bundleIdentifier != nil }
        func next() {
            guard let app = apps.popLast() else { return }
            _ = colour(for: app.bundleIdentifier!, pid: app.processIdentifier)
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.03) { next() }
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { next() }
    }

    private static var wallpapers: [String: CGImage] = [:]

    /// The display's wallpaper, decoded ahead of time by `prepareWallpapers`
    /// so the first motion doesn't wait for a large image to decode.
    static func wallpaper(for display: Display) -> CGImage? {
        guard let screen = display.screen, let url = NSWorkspace.shared.desktopImageURL(for: screen) else { return nil }
        if let w = wallpapers[display.uuid + url.path] { return w }
        prepareWallpapers()
        return nil
    }

    /// Decodes every display's wallpaper off the main thread, at screen size.
    static func prepareWallpapers() {
        for display in Display.current() {
            guard let screen = display.screen, let url = NSWorkspace.shared.desktopImageURL(for: screen) else { continue }
            let key = display.uuid + url.path
            guard wallpapers[key] == nil else { continue }
            let maxSide = max(display.frame.width, display.frame.height) * screen.backingScaleFactor
            DispatchQueue.global(qos: .utility).async {
                guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
                      let image = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                          kCGImageSourceCreateThumbnailFromImageAlways: true,
                          kCGImageSourceThumbnailMaxPixelSize: maxSide,
                          kCGImageSourceShouldCacheImmediately: true,
                      ] as CFDictionary) else { return }
                DispatchQueue.main.async { wallpapers[key] = image }
            }
        }
    }
}
