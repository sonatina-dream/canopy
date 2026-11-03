// A throwaway app with a few plain windows for the end-to-end test.
//   CanopyTestWindows W1 W2:slow W3:700x600 W4    (name[:minWxminH][:slow])
// A slow window takes 45 ms for every frame change, like Finder answering
// Accessibility.
// stdin: "frames" prints each window's frame (top-left origin) as JSON;
// "new <title>" opens another window (on the current desktop); "quit" exits.
import AppKit

final class SlowWindow: NSWindow {
    var delay: useconds_t = 0
    override func setFrame(_ frameRect: NSRect, display flag: Bool) {
        if delay > 0 { usleep(delay) }
        super.setFrame(frameRect, display: flag)
    }
}

final class Delegate: NSObject, NSApplicationDelegate {
    var windows: [NSWindow] = []

    func applicationDidFinishLaunching(_ n: Notification) {
        for (i, spec) in CommandLine.arguments.dropFirst().enumerated() { open(String(spec), index: i) }
        NSApp.activate(ignoringOtherApps: true)
        FileHandle.standardInput.readabilityHandler = { h in
            let line = String(decoding: h.availableData, as: UTF8.self)
            DispatchQueue.main.async { self.handle(line) }
        }
        print("ready")
        fflush(stdout)
    }

    func open(_ spec: String, index i: Int) {
        do {
            let parts = spec.split(separator: ":")
            let title = String(parts[0])
            let w = SlowWindow(contentRect: NSRect(x: 120 + i * 40, y: 160 + i * 30, width: 520, height: 380),
                             styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
            w.title = title
            w.isReleasedWhenClosed = false
            w.isRestorable = false
            for p in parts.dropFirst() {
                if p == "slow" { w.delay = 45_000; continue }
                let m = p.split(separator: "x").compactMap { Double($0) }
                if m.count == 2 { w.minSize = NSSize(width: m[0], height: m[1]) }
            }
            let label = NSTextField(labelWithString: title)
            label.font = .systemFont(ofSize: 48, weight: .bold)
            label.frame = NSRect(x: 20, y: 20, width: 300, height: 60)
            w.contentView?.addSubview(label)
            w.makeKeyAndOrderFront(nil)
            windows.append(w)
        }
    }

    func handle(_ input: String) {
        if input.isEmpty { NSApp.terminate(nil) }
        for line in input.split(separator: "\n") {
            switch line.trimmingCharacters(in: .whitespaces) {
            case "frames":
                let h = NSScreen.screens.first?.frame.height ?? 0
                var out: [String: Any] = [:]
                for w in windows {
                    let f = w.frame
                    out[w.title] = ["frame": [f.minX, h - f.maxY, f.width, f.height],
                                    "visible": w.isVisible, "minimized": w.isMiniaturized]
                }
                let data = try! JSONSerialization.data(withJSONObject: out, options: [.sortedKeys])
                print(String(decoding: data, as: UTF8.self))
                fflush(stdout)
            case "quit":
                NSApp.terminate(nil)
            case let l where l.hasPrefix("new "):
                open(String(l.dropFirst(4)), index: windows.count)
                NSApp.activate(ignoringOtherApps: true)
                print("ok")
                fflush(stdout)
            default:
                break
            }
        }
    }
}

let app = NSApplication.shared
let delegate = Delegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
