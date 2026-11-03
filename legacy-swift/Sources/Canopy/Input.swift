import AppKit
import Carbon.HIToolbox
import CanopyCore

/// A thread with its own run loop, for work that must never wait on the main
/// thread: the event tap (so the pointer never stutters) and AX observers.
final class RunLoopThread: Thread {
    private(set) var runLoop: CFRunLoop?
    private let ready = DispatchSemaphore(value: 0)

    init(name: String) {
        super.init()
        self.name = name
        qualityOfService = .userInteractive
    }

    func startAndWait() {
        start()
        ready.wait()
    }

    override func main() {
        runLoop = CFRunLoopGetCurrent()
        // A port keeps the run loop alive before any source is added.
        RunLoop.current.add(NSMachPort(), forMode: .default)
        ready.signal()
        while !isCancelled {
            CFRunLoopRunInMode(.defaultMode, 60, false)
        }
    }

    /// Runs `block` on this thread.
    func perform(_ block: @escaping () -> Void) {
        guard let runLoop else { return }
        CFRunLoopPerformBlock(runLoop, CFRunLoopMode.defaultMode.rawValue, block)
        CFRunLoopWakeUp(runLoop)
    }
}

/// A session event tap for mouse buttons, drags, pinches and (while a
/// marquee is active) Escape. It runs on its own thread; the handler returns
/// true to swallow an event and must return quickly.
final class EventTap {
    /// Raw event types: NSEventTypeGesture and NSEventTypeMagnify.
    static let gestureType: UInt32 = 29
    static let magnifyType: UInt32 = 30

    var handler: ((CGEventType, CGEvent) -> Bool)?
    private var tap: CFMachPort?
    private var source: CFRunLoopSource?

    private var runLoop: CFRunLoop?

    /// Creates the tap on `runLoop`. Call from that run loop's thread.
    @discardableResult
    func start(on runLoop: CFRunLoop) -> Bool {
        guard tap == nil else { return true }
        self.runLoop = runLoop
        let types: [UInt32] = [
            CGEventType.leftMouseDown.rawValue, CGEventType.leftMouseUp.rawValue,
            CGEventType.leftMouseDragged.rawValue, CGEventType.keyDown.rawValue,
            Self.gestureType, Self.magnifyType,
        ]
        let mask = types.reduce(CGEventMask(0)) { $0 | (CGEventMask(1) << CGEventMask($1)) }
        let callback: CGEventTapCallBack = { _, type, event, refcon in
            guard let refcon else { return Unmanaged.passUnretained(event) }
            let me = Unmanaged<EventTap>.fromOpaque(refcon).takeUnretainedValue()
            if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
                if let tap = me.tap { CGEvent.tapEnable(tap: tap, enable: true) }
                return Unmanaged.passUnretained(event)
            }
            if me.handler?(type, event) == true { return nil }
            return Unmanaged.passUnretained(event)
        }
        guard let t = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap, options: .defaultTap,
                                        eventsOfInterest: mask, callback: callback,
                                        userInfo: Unmanaged.passUnretained(self).toOpaque()) else { return false }
        tap = t
        source = CFMachPortCreateRunLoopSource(nil, t, 0)
        CFRunLoopAddSource(runLoop, source, .commonModes)
        CGEvent.tapEnable(tap: t, enable: true)
        return true
    }

    func stop() {
        if let tap { CGEvent.tapEnable(tap: tap, enable: false); CFMachPortInvalidate(tap) }
        if let source, let runLoop { CFRunLoopRemoveSource(runLoop, source, .commonModes) }
        tap = nil
        source = nil
    }
}

// MARK: - Shortcuts

/// A key with modifiers, stored with Carbon modifier bits.
struct KeyCombo: Codable, Hashable {
    var keyCode: UInt32
    var modifiers: UInt32

    static let control = UInt32(controlKey), option = UInt32(optionKey)
    static let shift = UInt32(shiftKey), command = UInt32(cmdKey)

    init(_ keyCode: Int, _ modifiers: UInt32) {
        self.keyCode = UInt32(keyCode)
        self.modifiers = modifiers
    }

    init?(event: NSEvent) {
        var m: UInt32 = 0
        let f = event.modifierFlags
        if f.contains(.control) { m |= Self.control }
        if f.contains(.option) { m |= Self.option }
        if f.contains(.shift) { m |= Self.shift }
        if f.contains(.command) { m |= Self.command }
        // A shortcut needs ⌃, ⌥ or ⌘.
        guard m & (Self.control | Self.option | Self.command) != 0 else { return nil }
        self.init(Int(event.keyCode), m)
    }

    var display: String {
        var s = ""
        if modifiers & Self.control != 0 { s += "⌃" }
        if modifiers & Self.option != 0 { s += "⌥" }
        if modifiers & Self.shift != 0 { s += "⇧" }
        if modifiers & Self.command != 0 { s += "⌘" }
        return s + Self.keyName(Int(keyCode))
    }

    static func keyName(_ code: Int) -> String {
        switch code {
        case kVK_Space: return "Space"
        case kVK_Return: return "↩"
        case kVK_Escape: return "⎋"
        case kVK_Tab: return "⇥"
        case kVK_Delete: return "⌫"
        case kVK_LeftArrow: return "←"
        case kVK_RightArrow: return "→"
        case kVK_UpArrow: return "↑"
        case kVK_DownArrow: return "↓"
        default: return translate(code) ?? "#\(code)"
        }
    }

    /// The key's character on the current keyboard layout.
    private static func translate(_ code: Int) -> String? {
        guard let source = TISCopyCurrentKeyboardLayoutInputSource()?.takeRetainedValue(),
              let raw = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData) else { return nil }
        let data = Unmanaged<CFData>.fromOpaque(raw).takeUnretainedValue() as Data
        return data.withUnsafeBytes { buf -> String? in
            guard let layout = buf.baseAddress?.assumingMemoryBound(to: UCKeyboardLayout.self) else { return nil }
            var dead: UInt32 = 0
            var chars = [UniChar](repeating: 0, count: 4)
            var len = 0
            let status = UCKeyTranslate(layout, UInt16(code), UInt16(kUCKeyActionDisplay), 0, UInt32(LMGetKbdType()),
                                        OptionBits(kUCKeyTranslateNoDeadKeysBit), &dead, chars.count, &len, &chars)
            guard status == noErr, len > 0 else { return nil }
            return String(utf16CodeUnits: chars, count: len).uppercased()
        }
    }
}

/// Default bindings (SPEC §12), keyed by each command's URL.
enum Shortcuts {
    static let digitKeys = [kVK_ANSI_1, kVK_ANSI_2, kVK_ANSI_3, kVK_ANSI_4, kVK_ANSI_5,
                            kVK_ANSI_6, kVK_ANSI_7, kVK_ANSI_8, kVK_ANSI_9]

    static let bindable: [Command] = {
        var c: [Command] = [.everything, .stepOut, .stepIn, .neighbour(.left), .neighbour(.right),
                            .neighbour(.up), .neighbour(.down), .back, .forward, .frameWindow,
                            .toggleDesktop, .splitFinder]
        c += (1...9).map(Command.saveView)
        c += (1...9).map(Command.goToView)
        return c
    }()

    static var defaults: [Command: KeyCombo] {
        let co = KeyCombo.control | KeyCombo.option
        var d: [Command: KeyCombo] = [
            .everything: KeyCombo(kVK_Space, co),
            .stepOut: KeyCombo(kVK_UpArrow, co),
            .stepIn: KeyCombo(kVK_DownArrow, co),
            .neighbour(.left): KeyCombo(kVK_LeftArrow, co),
            .neighbour(.right): KeyCombo(kVK_RightArrow, co),
            .neighbour(.up): KeyCombo(kVK_UpArrow, co | KeyCombo.shift),
            .neighbour(.down): KeyCombo(kVK_DownArrow, co | KeyCombo.shift),
            .back: KeyCombo(kVK_ANSI_LeftBracket, co),
            .forward: KeyCombo(kVK_ANSI_RightBracket, co),
            .frameWindow: KeyCombo(kVK_Return, co),
            .toggleDesktop: KeyCombo(kVK_ANSI_D, co),
            .splitFinder: KeyCombo(kVK_ANSI_N, co),
        ]
        for (i, key) in digitKeys.enumerated() {
            d[.saveView(i + 1)] = KeyCombo(key, co | KeyCombo.shift)
            d[.goToView(i + 1)] = KeyCombo(key, co)
        }
        return d
    }
}

/// Global shortcuts through Carbon's RegisterEventHotKey.
final class HotkeyCenter {
    var onCommand: ((Command) -> Void)?
    private var refs: [EventHotKeyRef] = []
    private var commands: [UInt32: Command] = [:]
    private var handler: EventHandlerRef?
    private(set) var failed: [Command] = []

    init() {
        var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        InstallEventHandler(GetApplicationEventTarget(), { _, event, userData in
            guard let event, let userData else { return OSStatus(eventNotHandledErr) }
            let me = Unmanaged<HotkeyCenter>.fromOpaque(userData).takeUnretainedValue()
            var hk = EventHotKeyID()
            let status = GetEventParameter(event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID),
                                           nil, MemoryLayout<EventHotKeyID>.size, nil, &hk)
            guard status == noErr, let c = me.commands[hk.id] else { return OSStatus(eventNotHandledErr) }
            DispatchQueue.main.async { me.onCommand?(c) }
            return noErr
        }, 1, &spec, Unmanaged.passUnretained(self).toOpaque(), &handler)
    }

    func register(_ bindings: [Command: KeyCombo]) {
        unregisterAll()
        failed = []
        var next: UInt32 = 1
        for (command, combo) in bindings.sorted(by: { $0.key.url.absoluteString < $1.key.url.absoluteString }) {
            var ref: EventHotKeyRef?
            let id = EventHotKeyID(signature: OSType(0x434E_5059), id: next) // 'CNPY'
            if RegisterEventHotKey(combo.keyCode, combo.modifiers, id, GetApplicationEventTarget(), 0, &ref) == noErr, let ref {
                refs.append(ref)
                commands[next] = command
            } else {
                failed.append(command)
            }
            next += 1
        }
    }

    func unregisterAll() {
        for r in refs { UnregisterEventHotKey(r) }
        refs = []
        commands = [:]
    }
}
