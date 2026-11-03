import AppKit
import CanopyCore

/// Canopy's menu bar controls: back, forward, everything, and one button per
/// saved view, plus the Canopy icon and its menu. Right-click any of them for the menu.
final class MenuBar: NSObject, NSMenuDelegate {
    private let coordinator: Coordinator
    private let icon: NSStatusItem
    private var controls: [NSStatusItem] = []
    private var shownSignature = ""

    init(coordinator: Coordinator) {
        self.coordinator = coordinator
        icon = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        super.init()
        if let b = icon.button {
            b.image = NSImage(systemSymbolName: "square.split.2x2", accessibilityDescription: "Canopy")
            b.image?.isTemplate = true
        }
        let menu = NSMenu()
        menu.delegate = self
        icon.menu = menu
        refresh()
    }

    /// Rebuilds the controls when what they show has changed.
    func refresh() {
        let world = coordinator.started ? coordinator.currentWorld() : nil
        let views = world?.navigator.savedViews.keys.sorted() ?? []
        let show = coordinator.prefs.menuBarControls && coordinator.started
        let canBack = world?.navigator.canGoBack ?? false, canForward = world?.navigator.canGoForward ?? false
        let signature = "\(show)|\(views)|\(canBack)|\(canForward)"
        guard signature != shownSignature else { return }
        shownSignature = signature
        for c in controls { NSStatusBar.system.removeStatusItem(c) }
        controls = []
        guard show else { return }
        // Status items are added right to left.
        for n in views.reversed() { controls.append(control(title: "\(n)", symbol: nil, command: .goToView(n), enabled: true)) }
        controls.append(control(title: nil, symbol: "rectangle.3.group", command: .everything, enabled: true))
        controls.append(control(title: nil, symbol: "chevron.right", command: .forward, enabled: canForward))
        controls.append(control(title: nil, symbol: "chevron.left", command: .back, enabled: canBack))
    }

    private func control(title: String?, symbol: String?, command: Command, enabled: Bool) -> NSStatusItem {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        guard let b = item.button else { return item }
        if let symbol {
            b.image = NSImage(systemSymbolName: symbol, accessibilityDescription: command.title)
            b.image?.isTemplate = true
        }
        if let title { b.title = title }
        b.toolTip = command.title
        b.appearsDisabled = !enabled
        b.target = self
        b.action = #selector(controlClicked(_:))
        b.sendAction(on: [.leftMouseUp, .rightMouseUp])
        b.identifier = NSUserInterfaceItemIdentifier(command.url.absoluteString)
        return item
    }

    @objc private func controlClicked(_ sender: NSStatusBarButton) {
        if NSApp.currentEvent?.type == .rightMouseUp {
            icon.button?.performClick(nil)
            return
        }
        guard let raw = sender.identifier?.rawValue, let url = URL(string: raw), let c = Command(url: url) else { return }
        coordinator.run(c)
    }

    // MARK: Menu

    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        let c = coordinator
        guard c.started else {
            menu.addItem(withTitle: "Canopy needs Accessibility permission", action: nil, keyEquivalent: "")
            menu.addItem(ClosureMenuItem("Open Accessibility Settings…") {
                NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")!)
            })
            menu.addItem(.separator())
            menu.addItem(ClosureMenuItem("Quit Canopy", key: "q", modifiers: .command) { NSApp.terminate(nil) })
            return
        }
        if !Permissions.screenRecording {
            menu.addItem(ClosureMenuItem("Allow Screen Recording…") { [weak c] in c?.onAskScreenRecording?() })
            menu.addItem(.separator())
        }
        let world = c.currentWorld()
        menu.addItem(item(.everything))
        menu.addItem(item(.back, enabled: world?.navigator.canGoBack ?? false))
        menu.addItem(item(.forward, enabled: world?.navigator.canGoForward ?? false))

        let viewsItem = NSMenuItem(title: "Saved Views", action: nil, keyEquivalent: "")
        let views = NSMenu()
        let saved = world?.navigator.savedViews.keys.sorted() ?? []
        for n in saved { views.addItem(item(.goToView(n))) }
        if !saved.isEmpty { views.addItem(.separator()) }
        let saveItem = NSMenuItem(title: "Save This View", action: nil, keyEquivalent: "")
        let saveMenu = NSMenu()
        for n in 1...9 { saveMenu.addItem(item(.saveView(n), title: "As \(n)")) }
        saveItem.submenu = saveMenu
        views.addItem(saveItem)
        for n in saved {
            views.addItem(ClosureMenuItem("Forget View \(n)") { [weak c] in
                guard let c, let w = c.currentWorld() else { return }
                w.navigator.forget(n)
                c.scheduleSave()
                c.onStateChange?()
            })
        }
        viewsItem.submenu = views
        menu.addItem(viewsItem)

        let hidden = world?.arrangement.desktop.hidden ?? false
        menu.addItem(item(.toggleDesktop, title: hidden ? "Show Desktop in This Space" : "Hide Desktop in This Space"))
        menu.addItem(.separator())
        menu.addItem(item(.arrange))
        menu.addItem(item(.floatAll))
        if let front = NSWorkspace.shared.frontmostApplication, let bundle = front.bundleIdentifier,
           front.processIdentifier != ProcessInfo.processInfo.processIdentifier {
            let name = front.localizedName ?? bundle
            let ignored = c.prefs.ignoredBundleIDs.contains(bundle)
            menu.addItem(ClosureMenuItem(ignored ? "Arrange “\(name)” Again" : "Don’t Arrange “\(name)”") { [weak c] in
                c?.setIgnored(bundle, !ignored)
            })
        }
        menu.addItem(.separator())
        menu.addItem(ClosureMenuItem("Settings…", key: ",", modifiers: .command) { [weak c] in c?.run(.settings(nil)) })
        menu.addItem(ClosureMenuItem("Quit Canopy", key: "q", modifiers: .command) { NSApp.terminate(nil) })
    }

    private func item(_ command: Command, title: String? = nil, enabled: Bool = true) -> NSMenuItem {
        let i = ClosureMenuItem(title ?? command.title) { [weak coordinator] in coordinator?.run(command) }
        if let combo = coordinator.bindings[command], let (key, mods) = Self.keyEquivalent(combo) {
            i.keyEquivalent = key
            i.keyEquivalentModifierMask = mods
        }
        if !enabled { i.action = nil }
        return i
    }

    /// For showing a shortcut beside a menu item.
    static func keyEquivalent(_ c: KeyCombo) -> (String, NSEvent.ModifierFlags)? {
        var mods: NSEvent.ModifierFlags = []
        if c.modifiers & KeyCombo.control != 0 { mods.insert(.control) }
        if c.modifiers & KeyCombo.option != 0 { mods.insert(.option) }
        if c.modifiers & KeyCombo.shift != 0 { mods.insert(.shift) }
        if c.modifiers & KeyCombo.command != 0 { mods.insert(.command) }
        let special: [Int: Int] = [123: NSLeftArrowFunctionKey, 124: NSRightArrowFunctionKey,
                                   125: NSDownArrowFunctionKey, 126: NSUpArrowFunctionKey]
        let key: String
        if let f = special[Int(c.keyCode)] {
            key = String(Character(UnicodeScalar(f)!))
        } else if c.keyCode == 49 {
            key = " "
        } else if c.keyCode == 36 {
            key = "\r"
        } else {
            let name = KeyCombo.keyName(Int(c.keyCode))
            guard name.count == 1 else { return nil }
            key = name.lowercased()
        }
        return (key, mods)
    }
}
