import AppKit
import ApplicationServices
import CanopyCore

/// Maps an AX window element to its CGWindowID. Private but long-stable; used
/// by most macOS window managers.
@_silgen_name("_AXUIElementGetWindow")
@discardableResult
func _AXUIElementGetWindow(_ element: AXUIElement, _ id: UnsafeMutablePointer<CGWindowID>) -> AXError

enum AX {
    static let systemWide: AXUIElement = {
        let e = AXUIElementCreateSystemWide()
        AXUIElementSetMessagingTimeout(e, 0.3)
        return e
    }()

    static func value(_ el: AXUIElement, _ attribute: String) -> CFTypeRef? {
        var v: CFTypeRef?
        guard AXUIElementCopyAttributeValue(el, attribute as CFString, &v) == .success else { return nil }
        return v
    }

    static func string(_ el: AXUIElement, _ attribute: String) -> String? {
        value(el, attribute) as? String
    }

    static func bool(_ el: AXUIElement, _ attribute: String) -> Bool? {
        (value(el, attribute) as? NSNumber)?.boolValue
    }

    static func element(_ el: AXUIElement, _ attribute: String) -> AXUIElement? {
        guard let v = value(el, attribute), CFGetTypeID(v) == AXUIElementGetTypeID() else { return nil }
        return (v as! AXUIElement)
    }

    static func elements(_ el: AXUIElement, _ attribute: String) -> [AXUIElement] {
        guard let v = value(el, attribute) as? [AnyObject] else { return [] }
        return v.compactMap { item in
            CFGetTypeID(item) == AXUIElementGetTypeID() ? (item as! AXUIElement) : nil
        }
    }

    static func point(_ el: AXUIElement, _ attribute: String) -> CGPoint? {
        guard let v = value(el, attribute), CFGetTypeID(v) == AXValueGetTypeID() else { return nil }
        var p = CGPoint.zero
        return AXValueGetValue(v as! AXValue, .cgPoint, &p) ? p : nil
    }

    static func size(_ el: AXUIElement, _ attribute: String) -> CGSize? {
        guard let v = value(el, attribute), CFGetTypeID(v) == AXValueGetTypeID() else { return nil }
        var s = CGSize.zero
        return AXValueGetValue(v as! AXValue, .cgSize, &s) ? s : nil
    }

    @discardableResult
    static func set(_ el: AXUIElement, _ attribute: String, _ value: CFTypeRef) -> Bool {
        AXUIElementSetAttributeValue(el, attribute as CFString, value) == .success
    }

    @discardableResult
    static func setPoint(_ el: AXUIElement, _ p: CGPoint) -> Bool {
        var p = p
        guard let v = AXValueCreate(.cgPoint, &p) else { return false }
        return set(el, kAXPositionAttribute, v)
    }

    @discardableResult
    static func setSize(_ el: AXUIElement, _ s: CGSize) -> Bool {
        var s = s
        guard let v = AXValueCreate(.cgSize, &s) else { return false }
        return set(el, kAXSizeAttribute, v)
    }

    static func isSettable(_ el: AXUIElement, _ attribute: String) -> Bool {
        var settable: DarwinBoolean = false
        return AXUIElementIsAttributeSettable(el, attribute as CFString, &settable) == .success && settable.boolValue
    }

    static func windowID(_ el: AXUIElement) -> CGWindowID? {
        var id: CGWindowID = 0
        return _AXUIElementGetWindow(el, &id) == .success && id != 0 ? id : nil
    }

    static func pid(_ el: AXUIElement) -> pid_t? {
        var pid: pid_t = 0
        return AXUIElementGetPid(el, &pid) == .success ? pid : nil
    }

    static func element(at p: CGPoint) -> AXUIElement? {
        var el: AXUIElement?
        guard AXUIElementCopyElementAtPosition(systemWide, Float(p.x), Float(p.y), &el) == .success else { return nil }
        return el
    }

    static func press(_ el: AXUIElement) {
        AXUIElementPerformAction(el, kAXPressAction as CFString)
    }

    static var isTrusted: Bool { AXIsProcessTrusted() }

    static func requestTrust(prompt: Bool) -> Bool {
        let key = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
        return AXIsProcessTrustedWithOptions([key: prompt] as CFDictionary)
    }
}

/// One app window, controlled through Accessibility.
final class AXWindow {
    let element: AXUIElement
    let app: AXUIElement
    let pid: pid_t
    let id: CGWindowID

    init(element: AXUIElement, app: AXUIElement, pid: pid_t, id: CGWindowID) {
        self.element = element
        self.app = app
        self.pid = pid
        self.id = id
    }

    var frame: CGRect? {
        guard let p = AX.point(element, kAXPositionAttribute), let s = AX.size(element, kAXSizeAttribute) else { return nil }
        return CGRect(origin: p, size: s)
    }

    var title: String { AX.string(element, kAXTitleAttribute) ?? "" }
    var role: String? { AX.string(element, kAXRoleAttribute) }
    var subrole: String? { AX.string(element, kAXSubroleAttribute) }
    var isMinimized: Bool { AX.bool(element, kAXMinimizedAttribute) ?? false }
    var isFullScreen: Bool { AX.bool(element, "AXFullScreen") ?? false }
    var isResizable: Bool { AX.isSettable(element, kAXSizeAttribute) }

    /// Sets the frame: size, position, size again, so a window crossing
    /// displays isn't clamped by its old display. Browsers with
    /// AXEnhancedUserInterface on animate or ignore moves, so it is switched
    /// off around the move.
    func setFrame(_ r: CGRect) {
        withEnhancedUIOff {
            AX.setSize(element, r.size)
            AX.setPoint(element, r.origin)
            AX.setSize(element, r.size)
        }
    }

    func setPosition(_ p: CGPoint) {
        withEnhancedUIOff { AX.setPoint(element, p) }
    }

    func setSize(_ s: CGSize) {
        withEnhancedUIOff { AX.setSize(element, s) }
    }

    private static let enhancedLock = NSLock()
    private static var enhancedCache: [pid_t: (on: Bool, at: CFAbsoluteTime)] = [:]

    /// Whether the app has AXEnhancedUserInterface on, asked at most every 10 s.
    private var appIsEnhanced: Bool {
        let now = CFAbsoluteTimeGetCurrent()
        Self.enhancedLock.lock()
        let cached = Self.enhancedCache[pid]
        Self.enhancedLock.unlock()
        if let cached, now - cached.at < 10 { return cached.on }
        let on = AX.bool(app, "AXEnhancedUserInterface") ?? false
        Self.enhancedLock.lock()
        Self.enhancedCache[pid] = (on, now)
        Self.enhancedLock.unlock()
        return on
    }

    private func withEnhancedUIOff(_ body: () -> Void) {
        let enhanced = appIsEnhanced
        if enhanced { AX.set(app, "AXEnhancedUserInterface", kCFBooleanFalse) }
        body()
        if enhanced { AX.set(app, "AXEnhancedUserInterface", kCFBooleanTrue) }
    }

    func raise() {
        AXUIElementPerformAction(element, kAXRaiseAction as CFString)
    }

    func focus() {
        NSRunningApplication(processIdentifier: pid)?.activate()
        AX.set(element, kAXMainAttribute, kCFBooleanTrue)
        raise()
    }

    func close() {
        if let b = AX.element(element, kAXCloseButtonAttribute) { AX.press(b) }
    }

    func minimize() {
        AX.set(element, kAXMinimizedAttribute, kCFBooleanTrue)
    }
}
