// Orbit for macOS. One resident process, AppKit, no web runtime.

import AppKit
import COrbit
import Darwin
import notify

var dataDir = NSHomeDirectory() + "/Library/Application Support/Orbit"
var benchLog: String?
var hotkeyOverride: String?
var trimMs: Int? = 15_000
var noReindex = false
var waitPid: pid_t?
var indexOnly = false
var selfTest = false
var exportDir: String?
var hidutilMatching: String?
var pasteboardName: String?
var extraAppDir: String?
var fallbackHotkeys: [String]?
var noOpen = false

var arguments = CommandLine.arguments.dropFirst().makeIterator()
while let argument = arguments.next() {
    switch argument {
    case "--data-dir": dataDir = arguments.next() ?? dataDir
    case "--bench-log": benchLog = arguments.next()
    case "--hotkey": hotkeyOverride = arguments.next()
    case "--trim-ms": trimMs = arguments.next().flatMap { Int($0) }
    case "--no-trim": trimMs = nil
    case "--no-reindex": noReindex = true
    case "--wait-pid": waitPid = arguments.next().flatMap { pid_t($0) }
    case "--index-apps": indexOnly = true
    case "--self-test": selfTest = true
    case "--export-dir": exportDir = arguments.next()
    case "--hidutil-matching": hidutilMatching = arguments.next()
    case "--pasteboard": pasteboardName = arguments.next()
    case "--app-dir": extraAppDir = arguments.next()
    case "--fallback-hotkeys": fallbackHotkeys = arguments.next()?.split(separator: ",").map(String.init)
    case "--no-open": noOpen = true
    default: break
    }
}

try? FileManager.default.createDirectory(atPath: dataDir, withIntermediateDirectories: true)
orbit_install_panic_log(dataDir)

if selfTest {
    exit(SelfTest.run(dataDir: dataDir))
}
if indexOnly {
    // Benchmarks time the scan alone with this. The app itself scans on a background queue.
    let json = encodeApps(scanApplications())
    exit(orbit_apps_save(dataDir, json) ? 0 : 1)
}
if let waitPid {
    // Restart: give the old process a moment to let go of its shortcuts.
    var tries = 0
    while kill(waitPid, 0) == 0, tries < 100 {
        usleep(50_000)
        tries += 1
    }
}

/// A short stable name per data directory, so a test instance can run beside the real one.
func instanceName(_ path: String) -> String {
    var hash: UInt64 = 0xcbf2_9ce4_8422_2325
    for byte in path.lowercased().utf8 {
        hash ^= UInt64(byte)
        hash = hash &* 0x0000_0100_0000_01B3
    }
    return String(hash, radix: 16)
}

// One instance per data directory. A second launch just opens the first one's window. The lock is
// released by the kernel when the process ends, however it ends, so a crash can't block a restart.
let toggleName = "com.orbit.instance.\(instanceName(dataDir)).toggle"
let lockFile = open(dataDir + "/instance.lock", O_CREAT | O_RDWR, 0o600)
if lockFile >= 0, flock(lockFile, LOCK_EX | LOCK_NB) != 0 {
    notify_post(toggleName)
    exit(0)
}

let application = NSApplication.shared
application.setActivationPolicy(.accessory)
guard let controller = AppController(options: Options(dataDir: dataDir, benchLog: benchLog, hotkeyOverride: hotkeyOverride, trimAfterMs: trimMs, noReindex: noReindex, exportDir: exportDir, hidutilMatching: hidutilMatching, pasteboardName: pasteboardName, extraAppDir: extraAppDir, fallbackHotkeys: fallbackHotkeys, noOpen: noOpen)) else {
    FileHandle.standardError.write(Data("Orbit couldn't start: the core failed to load its settings.\n".utf8))
    exit(1)
}
AppController.shared = controller
application.delegate = controller

var toggleToken: Int32 = 0
notify_register_dispatch(toggleName, &toggleToken, DispatchQueue.main) { _ in
    AppController.shared.toggle(.root, source: "instance", t0: ticks())
}

application.run()
