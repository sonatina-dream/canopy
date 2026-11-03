import Foundation

/// A running log in ~/Library/Logs/Canopy.log (the previous run's is kept as
/// Canopy.previous.log), plus the last lines in memory for the test harness.
enum Log {
    private static let lock = NSLock()
    private static var recent: [String] = []
    private static let start = CFAbsoluteTimeGetCurrent()
    private static var handle: FileHandle?
    static var folder = FileManager.default.urls(for: .libraryDirectory, in: .userDomainMask)[0]
        .appendingPathComponent("Logs", isDirectory: true)

    static func open() {
        let url = folder.appendingPathComponent("Canopy.log")
        let previous = folder.appendingPathComponent("Canopy.previous.log")
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try? FileManager.default.removeItem(at: previous)
        try? FileManager.default.moveItem(at: url, to: previous)
        FileManager.default.createFile(atPath: url.path, contents: nil)
        handle = try? FileHandle(forWritingTo: url)
    }

    static func add(_ message: @autoclosure () -> String) {
        let line = String(format: "%8.3f ", CFAbsoluteTimeGetCurrent() - start) + message()
        lock.lock()
        recent.append(line)
        if recent.count > 400 { recent.removeFirst(recent.count - 400) }
        let h = handle
        lock.unlock()
        h?.write(Data((line + "\n").utf8))
    }

    static func take() -> [String] {
        lock.lock()
        defer { recent = []; lock.unlock() }
        return recent
    }
}
