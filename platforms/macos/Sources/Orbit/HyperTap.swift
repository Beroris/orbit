// The input tap behind HyperKey Internal and Synthetic modes and text expansion. It exists only
// while one of those is on. The callback runs on its own thread and only updates small in-memory
// state machines and hands recognised events to the main thread: macOS disables a slow tap, and
// a slow tap lags every keypress on the system. It stores and logs nothing it sees.

import AppKit
import Carbon
import COrbit

/// Marks events Orbit injected, so the tap passes them through instead of acting on them.
let injectedTag: Int64 = 0x4F52_4249 // "ORBI"

/// Left and right Command, Shift, Option and Control, with their device-dependent flag bits.
private let modifierBits: [UInt8: UInt64] = [0x37: 0x8, 0x36: 0x10, 0x38: 0x2, 0x3C: 0x4, 0x3B: 0x1, 0x3E: 0x2000, 0x3A: 0x20, 0x3D: 0x40]
private let hyperFlags: CGEventFlags = [.maskControl, .maskAlternate, .maskShift, .maskCommand]
private let navigationKeys: Set<Int64> = [36, 48, 53, 76, 115, 116, 119, 121, 123, 124, 125, 126]

/// Decides whether a disabled tap may be re-enabled. A tap that keeps timing out means something
/// is wrong, and re-enabling it in a loop would make typing worse, not better.
struct TapGuard {
    static let limit = 5
    static let windowMs: Double = 60_000
    private var recent: [Double] = []

    mutating func allow(nowMs: Double) -> Bool {
        recent.removeAll { nowMs - $0 > TapGuard.windowMs }
        if recent.count >= TapGuard.limit { return false }
        recent.append(nowMs)
        return true
    }
}

struct TapConfig {
    var triggerCode: UInt8?
    var mode: UInt32 = UInt32(ORBIT_HYPER_INTERNAL)
    var tap: UInt32 = UInt32(ORBIT_TAP_ESCAPE)
    var tapTimeoutMs: UInt32 = 250
    var boundCodes: [UInt8] = []
    /// The real key that a tap should send when it keeps its normal behaviour.
    var originalIsCapsLock = false
    var expansion = false
}

final class HyperTap {
    let config: TapConfig
    /// All called on the main thread.
    var onDispatch: ((UInt8, Int64) -> Void)?
    var onExpand: ((Int64, UInt64) -> Void)?
    var onGaveUp: (() -> Void)?

    private var thread: Thread?
    private var runLoop: CFRunLoop?
    private var port: CFMachPort?
    private var machine: OpaquePointer?
    private var triggers: OpaquePointer?
    private var guardState = TapGuard()
    private var syntheticActive = false
    private var suspended = false

    // Read by the diagnostics page. Word-sized, and exact values don't matter there.
    private(set) var maxCallbackTicks: Int64 = 0
    private(set) var reEnables = 0
    private(set) var running = false

    init(config: TapConfig) {
        self.config = config
    }

    /// False means macOS refused the tap: the Accessibility or Input Monitoring grant is missing.
    func start() -> Bool {
        // Asking for a tap without the grant makes macOS show its own prompt. Orbit doesn't do that
        // unprompted: Settings explains the feature first and opens the right pane on request.
        guard Permissions.accessibility, Permissions.inputMonitoring else { return false }
        var mask: CGEventMask = 1 << CGEventType.keyDown.rawValue | 1 << CGEventType.keyUp.rawValue
        if config.triggerCode != nil { mask |= 1 << CGEventType.flagsChanged.rawValue }
        // Expansion alone never swallows anything, so it needs only a listening tap.
        let options: CGEventTapOptions = config.triggerCode == nil ? .listenOnly : .defaultTap
        let info = Unmanaged.passUnretained(self).toOpaque()
        guard let created = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap, options: options, eventsOfInterest: mask, callback: { _, type, event, info in
            guard let info else { return Unmanaged.passUnretained(event) }
            return Unmanaged<HyperTap>.fromOpaque(info).takeUnretainedValue().handle(type: type, event: event)
        }, userInfo: info) else { return false }
        port = created

        if let code = config.triggerCode {
            var modifiers = Array(modifierBits.keys).filter { $0 != code }
            machine = orbit_hyper_new(code, config.mode, config.tap, config.tapTimeoutMs, &modifiers, modifiers.count)
            orbit_hyper_set_bound(machine, config.boundCodes, config.boundCodes.count)
        }

        let source = CFMachPortCreateRunLoopSource(nil, created, 0)
        let ready = DispatchSemaphore(value: 0)
        let thread = Thread { [weak self] in
            guard let self else { return }
            let loop = CFRunLoopGetCurrent()
            self.runLoop = loop
            CFRunLoopAddSource(loop, source, .commonModes)
            CGEvent.tapEnable(tap: created, enable: true)
            ready.signal()
            CFRunLoopRun()
        }
        thread.name = "orbit-tap"
        thread.qualityOfService = .userInteractive
        thread.stackSize = 256 * 1024
        self.thread = thread
        thread.start()
        ready.wait()
        running = true
        return true
    }

    /// Takes the tap out of the event stream at once. Keys go back to normal the moment this
    /// returns, even if a key is held. Nothing can be stuck: synthetic Hyper is only ever flags
    /// on individual events, never a held virtual key.
    func stop() {
        guard running else { return }
        running = false
        if let port { CGEvent.tapEnable(tap: port, enable: false) }
        let done = DispatchSemaphore(value: 0)
        if let runLoop {
            CFRunLoopPerformBlock(runLoop, CFRunLoopMode.commonModes.rawValue) {
                if let port = self.port { CFMachPortInvalidate(port) }
                orbit_hyper_free(self.machine)
                self.machine = nil
                orbit_triggers_free(self.triggers)
                self.triggers = nil
                CFRunLoopStop(CFRunLoopGetCurrent())
                done.signal()
            }
            CFRunLoopWakeUp(runLoop)
            _ = done.wait(timeout: .now() + 1)
        }
    }

    private func onTapThread(_ body: @escaping (HyperTap) -> Void) {
        guard running, let runLoop else { return }
        CFRunLoopPerformBlock(runLoop, CFRunLoopMode.commonModes.rawValue) { [weak self] in
            guard let self else { return }
            body(self)
        }
        CFRunLoopWakeUp(runLoop)
    }

    /// Takes ownership of `matcher`.
    func setTriggers(_ matcher: OpaquePointer?) {
        onTapThread { tap in
            orbit_triggers_free(tap.triggers)
            tap.triggers = matcher
        }
    }

    /// Passes everything through while the frontmost app is excluded or fullscreen, or finishes
    /// the current hold safely if it changes mid-chord.
    func setSuspended(_ value: Bool) {
        onTapThread { tap in
            tap.suspended = value
            tap.applyDecision(orbit_hyper_set_suspended(tap.machine, value))
            orbit_triggers_clear(tap.triggers)
        }
    }

    /// Forget all transient state. Called on lock, sleep and when focus changes app.
    func reset() {
        onTapThread { tap in
            tap.applyDecision(orbit_hyper_reset(tap.machine))
            orbit_triggers_clear(tap.triggers)
        }
    }

    private func recoverFromDisable() {
        applyDecision(orbit_hyper_reset(machine))
        orbit_triggers_clear(triggers)
        if guardState.allow(nowMs: Double(ticks()) * tickToMs) {
            reEnables += 1
            if let port { CGEvent.tapEnable(tap: port, enable: true) }
        } else {
            DispatchQueue.main.async { [weak self] in self?.onGaveUp?() }
        }
    }

    private func handle(type: CGEventType, event: CGEvent) -> Unmanaged<CGEvent>? {
        let start = ticks()
        defer {
            let spent = ticks() - start
            if spent > maxCallbackTicks { maxCallbackTicks = spent }
        }
        switch type {
        case .tapDisabledByTimeout, .tapDisabledByUserInput:
            recoverFromDisable()
            return Unmanaged.passUnretained(event)
        case .keyDown, .keyUp, .flagsChanged:
            break
        default:
            return Unmanaged.passUnretained(event)
        }
        if event.getIntegerValueField(.eventSourceUserData) == injectedTag {
            return Unmanaged.passUnretained(event)
        }

        let keycode = event.getIntegerValueField(.keyboardEventKeycode)
        var isDown = type == .keyDown
        if type == .flagsChanged {
            guard let bit = modifierBits[UInt8(truncatingIfNeeded: keycode)] else { return Unmanaged.passUnretained(event) }
            isDown = event.flags.rawValue & bit != 0
        }

        var swallow = false
        if machine != nil {
            let timeMs = UInt32(truncatingIfNeeded: Int64(Double(start) * tickToMs))
            let decision = orbit_hyper_on_key(machine, UInt8(truncatingIfNeeded: keycode), isDown, timeMs)
            swallow = decision.swallow
            applyDecision(decision, at: start)
        }

        // While synthetic Hyper is on, keys passing through carry the four modifiers. Only this
        // event changes, so there is no global state to get stuck.
        if syntheticActive, !swallow, type != .flagsChanged {
            event.flags.formUnion(hyperFlags)
        }
        if !swallow, type == .keyDown, !suspended, config.expansion, triggers != nil {
            feedExpansion(event, keycode: keycode)
        }
        return swallow ? nil : Unmanaged.passUnretained(event)
    }

    /// The rolling buffer lives in the core, is bounded, and is never written anywhere.
    private func feedExpansion(_ event: CGEvent, keycode: Int64) {
        if IsSecureEventInputEnabled() {
            orbit_triggers_clear(triggers)
            return
        }
        if keycode == 51 {
            orbit_triggers_backspace(triggers)
            return
        }
        if navigationKeys.contains(keycode) || !event.flags.intersection([.maskCommand, .maskControl]).isEmpty {
            orbit_triggers_clear(triggers)
            return
        }
        var length = 0
        var chars = [UniChar](repeating: 0, count: 4)
        event.keyboardGetUnicodeString(maxStringLength: 4, actualStringLength: &length, unicodeString: &chars)
        for scalar in String(utf16CodeUnits: chars, count: length).unicodeScalars {
            var matched: UInt64 = 0
            let id = orbit_triggers_push(triggers, scalar.value, &matched)
            if id > 0 {
                orbit_triggers_clear(triggers)
                DispatchQueue.main.async { [weak self] in self?.onExpand?(id, matched) }
            }
        }
    }

    private func applyDecision(_ decision: OrbitHyperDecision, at time: Int64 = 0) {
        let effects = [decision.effects.0, decision.effects.1]
        for effect in effects.prefix(Int(decision.count)) {
            switch Int(effect.kind) {
            case ORBIT_EFFECT_DISPATCH:
                let code = effect.code
                DispatchQueue.main.async { [weak self] in self?.onDispatch?(code, time) }
            case ORBIT_EFFECT_TAP:
                performTap(effect.tap)
            case ORBIT_EFFECT_SYNTHETIC_DOWN:
                syntheticActive = true
                postKey(code: CGKeyCode(effect.code), flags: hyperFlags)
            case ORBIT_EFFECT_SYNTHETIC_RELEASE:
                syntheticActive = false
            default:
                break
            }
        }
    }

    private func postKey(code: CGKeyCode, flags: CGEventFlags) {
        guard let down = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: true) else { return }
        down.flags = flags
        down.setIntegerValueField(.eventSourceUserData, value: injectedTag)
        down.post(tap: .cgSessionEventTap)
    }

    private func performTap(_ tap: UInt8) {
        if Int(tap) == ORBIT_TAP_ESCAPE {
            sendKeyPair(53)
        } else if Int(tap) == ORBIT_TAP_ORIGINAL {
            if config.originalIsCapsLock {
                CapsLockState.toggle()
            } else if let code = config.triggerCode {
                sendKeyPair(CGKeyCode(code))
            }
        }
    }

    private func sendKeyPair(_ code: CGKeyCode) {
        for isDown in [true, false] {
            guard let event = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: isDown) else { continue }
            event.setIntegerValueField(.eventSourceUserData, value: injectedTag)
            event.post(tap: .cgSessionEventTap)
        }
    }
}

/// With Caps Lock remapped to F18 the system no longer toggles it, so "keep its normal behaviour"
/// has to set the lock state directly.
enum CapsLockState {
    static func toggle() {
        let service = IOServiceGetMatchingService(kIOMainPortDefault, IOServiceMatching(kIOHIDSystemClass))
        guard service != 0 else { return }
        defer { IOObjectRelease(service) }
        var connection: io_connect_t = 0
        guard IOServiceOpen(service, mach_task_self_, UInt32(kIOHIDParamConnectType), &connection) == KERN_SUCCESS else { return }
        defer { IOServiceClose(connection) }
        var state = false
        guard IOHIDGetModifierLockState(connection, Int32(kIOHIDCapsLockState), &state) == KERN_SUCCESS else { return }
        IOHIDSetModifierLockState(connection, Int32(kIOHIDCapsLockState), !state)
    }
}

enum Permissions {
    /// What Orbit itself holds, without asking the system to prompt. When started from a terminal
    /// macOS answers for that terminal instead, which is why the app says how it was started.
    static var accessibility: Bool { AXIsProcessTrusted() }
    static var inputMonitoring: Bool { CGPreflightListenEventAccess() }
    static var postEvents: Bool { CGPreflightPostEventAccess() }

    static let accessibilityPane = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
    static let inputMonitoringPane = "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"

    static func open(pane: String) {
        if let url = URL(string: pane) { NSWorkspace.shared.open(url) }
    }
}

/// Runs `hidutil`, the supported way to remap Caps Lock to F18 without a kernel extension or
/// daemon. The mapping survives Orbit exiting but not a reboot, so a journal written before the
/// change lets the next start clean up after a crash. Only Orbit's own pair is ever removed.
final class HidRemap {
    private static let tool = "/usr/bin/hidutil"
    private let journal: String
    /// Replaces "all keyboards". The self-test sets a matcher for a device that doesn't exist.
    private let matching: String?
    private let queue = DispatchQueue(label: "orbit.hidutil", qos: .utility)

    init(dataDir: String, matching: String?) {
        journal = dataDir + "/hidutil-journal"
        self.matching = matching
    }

    private struct Plan: Decodable {
        struct Command: Decodable { var matching: String; var set: String }
        var conflict: String?
        var commands: [Command]
    }

    static func run(_ arguments: [String]) -> String? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: tool)
        process.arguments = arguments
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        guard (try? process.run()) != nil else { return nil }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        return process.terminationStatus == 0 ? String(decoding: data, as: UTF8.self) : nil
    }

    private func plan(remove: Bool) -> Plan? {
        let target = matching ?? takeString(orbit_hidutil_keyboards()) ?? ""
        guard let current = HidRemap.run(["property", "--matching", target, "--get", "UserKeyMapping"]) else { return nil }
        guard let json = takeString(orbit_hidutil_plan(current, remove, matching)), let data = json.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(Plan.self, from: data)
    }

    private func execute(_ plan: Plan) -> Bool {
        var ok = true
        for command in plan.commands {
            if HidRemap.run(["property", "--matching", command.matching, "--set", command.set]) == nil { ok = false }
        }
        return ok
    }

    /// `done` runs on the main thread with a message if the remap didn't happen.
    func apply(done: @escaping (String?) -> Void) {
        queue.async { [self] in
            let message: String?
            if let plan = plan(remove: false) {
                if let conflict = plan.conflict {
                    message = conflict
                } else {
                    FileManager.default.createFile(atPath: journal, contents: Data("caps-to-f18\n".utf8))
                    message = execute(plan) ? nil : "hidutil refused the Caps Lock remap."
                }
            } else {
                message = "Couldn't read the current key mapping with hidutil."
            }
            DispatchQueue.main.async { done(message) }
        }
    }

    var hasJournal: Bool { FileManager.default.fileExists(atPath: journal) }

    private func removeNow() {
        guard hasJournal else { return }
        // A refused removal keeps the journal, so the next start tries again.
        if let plan = plan(remove: true), plan.conflict == nil, execute(plan) {
            try? FileManager.default.removeItem(atPath: journal)
        }
    }

    func remove() {
        queue.async { [self] in removeNow() }
    }

    /// For quit: waits, within limits, so the keyboard is back to normal before the process ends.
    func removeAndWait(timeoutSeconds: Double) {
        let done = DispatchSemaphore(value: 0)
        queue.async { [self] in
            removeNow()
            done.signal()
        }
        _ = done.wait(timeout: .now() + timeoutSeconds)
    }
}
