// A small window for the benchmark driver: it holds focus while Orbit is tested, catches any key
// that leaks out of Orbit, and reports what it saw. It exists only during a benchmark run.

import AppKit
import Darwin
import notify

// `OrbitBenchSink --pasteboard NAME TYPE VALUE [TYPE VALUE...]` writes to a private named pasteboard
// and exits, so feature checks can exercise clipboard capture without touching the real clipboard.
if CommandLine.arguments.count >= 3, CommandLine.arguments[1] == "--pasteboard" {
    let board = NSPasteboard(name: NSPasteboard.Name(CommandLine.arguments[2]))
    let pairs = Array(CommandLine.arguments.dropFirst(3))
    board.clearContents()
    board.declareTypes(stride(from: 0, to: pairs.count, by: 2).map { NSPasteboard.PasteboardType(pairs[$0]) }, owner: nil)
    for index in stride(from: 0, to: pairs.count - 1, by: 2) {
        board.setString(pairs[index + 1], forType: NSPasteboard.PasteboardType(pairs[index]))
    }
    exit(0)
}

let reportPath = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "/dev/null"

final class Delegate: NSObject, NSApplicationDelegate {
    var window: NSWindow!
    var field: NSTextField!
    var tokens: [Int32] = []

    func applicationDidFinishLaunching(_ notification: Notification) {
        window = NSWindow(contentRect: NSRect(x: 40, y: 40, width: 520, height: 110), styleMask: [.titled], backing: .buffered, defer: false)
        window.title = "Orbit benchmark running. It ends by itself."
        field = NSTextField(frame: NSRect(x: 10, y: 40, width: 500, height: 26))
        window.contentView?.addSubview(field)
        window.setFrameOrigin(NSPoint(x: 40, y: 40))
        focus()
        register("focus") { [unowned self] in self.focus() }
        register("report") { [unowned self] in self.report() }
        register("clear") { [unowned self] in self.field.stringValue = "" }
        register("quit") { exit(0) }
        report()
    }

    private func register(_ command: String, _ body: @escaping () -> Void) {
        var token: Int32 = 0
        notify_register_dispatch("com.orbit.benchsink.\(getpid()).\(command)", &token, DispatchQueue.main) { _ in body() }
        tokens.append(token)
    }

    private func focus() {
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(field)
    }

    /// The driver waits for the sequence number to change, then reads the rest.
    private var sequence = 0
    private func report() {
        sequence += 1
        let line = "\(sequence),\(NSApp.isActive),\(window.isKeyWindow),\(field.stringValue)\n"
        FileManager.default.createFile(atPath: reportPath, contents: Data(line.utf8))
    }
}

let application = NSApplication.shared
application.setActivationPolicy(.regular)
let delegate = Delegate()
application.delegate = delegate
application.run()
