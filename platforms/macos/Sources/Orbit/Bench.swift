// What only exists for the benchmark driver and self-checks. BenchChannel is installed only with
// --bench-log, so it costs a normal run nothing. Darwin notifications need no permission and no
// socket, and they let the driver exercise everything except the OS key path when it has no grant
// to inject keys. Results it produces are labelled as such in the report.

import AppKit
import COrbit
import Darwin
import notify

enum BenchChannel {
    private static var tokens: [Int32] = []

    private static func on(_ command: String, _ body: @escaping (Int32) -> Void) {
        let name = "com.orbit.bench.\(getpid()).\(command)"
        var token: Int32 = 0
        if notify_register_dispatch(name, &token, DispatchQueue.main, { body($0) }) == NOTIFY_STATUS_OK {
            tokens.append(token)
        }
    }

    static func install(controller: AppController) {
        on("toggle") { _ in controller.benchToggle(t0: ticks()) }
        on("escape") { _ in controller.benchEscape() }
        on("enter") { _ in controller.benchEnter() }
        on("type") { token in
            var state: UInt64 = 0
            notify_get_state(token, &state)
            controller.benchType(UInt32(truncatingIfNeeded: state))
        }
        on("down") { _ in controller.benchDown() }
        on("trash-selected") { _ in controller.benchTrashSelected() }
        on("notes") { _ in
            controller.show(.root, source: "bench", t0: ticks())
            controller.performBuiltin(.searchNotes)
        }
        on("run-reset-shortcuts") { _ in controller.runBuiltin(.resetShortcuts) }
        on("run-delete-data") { _ in controller.runBuiltin(.deleteLocalData) }
        on("flush") { _ in
            controller.recordMemoryLine()
            controller.metrics.recordLine(controller.benchState())
            controller.metrics.flush()
        }
        on("trim") { _ in malloc_zone_pressure_relief(nil, 0) }
        on("snapshot") { _ in controller.benchSnapshot(path: controller.options.dataDir + "/snapshot.png") }
        on("window-shot") { _ in controller.benchWindowShot(path: controller.options.dataDir + "/window.png") }
        on("counts") { _ in orbit_worker_counts(controller.worker) }
        on("quit") { _ in controller.beginExit() }
        on("note") { _ in
            NoteEditor.show(worker: controller.worker, id: nil, body: "bench note")
        }
        on("settings") { _ in controller.performBuiltin(.settings) }
        on("diagnostics") { _ in
            controller.show(.root, source: "bench", t0: ticks())
            controller.performBuiltin(.diagnostics)
        }
        on("help") { _ in
            controller.show(.root, source: "bench", t0: ticks())
            controller.performBuiltin(.showHelp)
        }
        on("export") { _ in controller.exportData() }
        on("import") { _ in orbit_worker_import(controller.worker, controller.options.dataDir + "/import.json") }
        on("settings-shot") { _ in controller.benchSettingsShot() }
        on("settings-save") { _ in controller.benchSettingsSave() }
        on("reindex") { _ in controller.startIndexer() }
        on("lowres") { _ in controller.performBuiltin(.toggleLowResource) }
        on("suspend") { _ in controller.performBuiltin(controller.suspended ? .resumeShortcuts : .suspendShortcuts) }
    }
}

/// `Orbit --self-test`: host-level checks that need the real frameworks but no window, no key
/// injection and no changes to the machine. Exits non-zero if anything fails.
enum SelfTest {
    private static var failures = 0

    private static func check(_ name: String, _ condition: Bool) {
        print("\(condition ? "PASS" : "FAIL")  \(name)")
        if !condition { failures += 1 }
    }

    static func run(dataDir: String) -> Int32 {
        check("ABI version matches the header", orbit_abi_version() == UInt32(ORBIT_ABI_VERSION))

        // Key names.
        check("Space is key code 49", keyCode(forKeyName: "Space") == 49)
        check("F13 is key code 105", keyCode(forKeyName: "F13") == 105)
        check("F18 is key code 79", keyCode(forKeyName: "F18") == 79)
        check("every letter maps to a key code", "ABCDEFGHIJKLMNOPQRSTUVWXYZ".allSatisfy { keyCode(forKeyName: String($0)) != nil })
        check("an unknown key has no code", keyCode(forKeyName: "F24") == nil)
        check("chords print in macOS notation", prettyChord("Ctrl+Alt+Shift+Cmd+K") == "⌃⌥⇧⌘K")

        // Actions.
        if case .focusOrLaunch(_, let name)? = Action(text: "app:Safari") { check("app action parses", name == "Safari") } else { check("app action parses", false) }
        if case .url(let url)? = Action(text: "url:https://a.b:8080/x") { check("URL keeps its own colons", url == "https://a.b:8080/x") } else { check("URL keeps its own colons", false) }
        check("unknown command is refused", Action(text: "cmd:nope") == nil)
        check("arguments split on quotes without a shell", AppController.splitArguments("\"a b\" c  $(rm)") == ["a b", "c", "$(rm)"])

        // View state, as in view.rs.
        var view = ViewState()
        view.rows = (0..<20).map { Row.info("row \($0)", "") }
        view.moveSelection(-1)
        check("selection wraps upward and stays visible", view.selected == 19 && view.scroll == 12)
        view.moveSelection(1)
        check("selection wraps downward", view.selected == 0 && view.scroll == 0)
        view.moveSelection(100)
        check("paging stops at the end", view.selected == 19)
        view.select(12)
        view.scrollBy(-20)
        check("scrolling pulls the selection into view", view.scroll == 0 && view.selected == 7)
        var empty = ViewState()
        empty.moveSelection(1)
        empty.scrollBy(3)
        empty.select(4)
        check("an empty list is safe", empty.selected == 0 && empty.selectedRow == nil)
        empty.staticRows = [.info("Alpha", "first"), .info("Béta", "second")]
        empty.filterStatic("beta")
        check("static filter ignores case and accents", empty.rows.count == 1 && empty.rows[0].title == "Béta")
        check("footer with nothing selected", empty.footerHints().hasSuffix("Esc Close"))

        // The tap's re-enable policy.
        var tapGuard = TapGuard()
        let allowed = (0..<7).map { tapGuard.allow(nowMs: Double($0) * 10) }
        check("a looping tap is re-enabled at most 5 times a minute", allowed == [true, true, true, true, true, false, false])
        check("the limit clears after a minute", tapGuard.allow(nowMs: 70_000))

        // FSEvents filter.
        let roots = ["/Applications"]
        check("a new bundle matters", AppWatcher.isRelevant("/Applications/New.app", roots: roots))
        check("a bundle in a vendor folder matters", AppWatcher.isRelevant("/Applications/Vendor/New.app/", roots: roots))
        check("a change inside a bundle does not", !AppWatcher.isRelevant("/Applications/Busy.app/Contents/Resources", roots: roots))
        check("a path elsewhere does not", !AppWatcher.isRelevant("/Users/x/Applicationsy", roots: roots))

        // The real scan, on this machine.
        let started = ticks()
        let apps = scanApplications()
        let scanMs = Double(ticks() - started) * tickToMs
        print(String(format: "      scan found %d apps in %.1f ms", apps.count, scanMs))
        check("the scan finds Calculator", apps.contains { $0.path.hasSuffix("/Calculator.app") })
        check("the scan finds Finder", apps.contains { $0.path.hasSuffix("/Finder.app") })
        check("app ids are unique", Set(apps.map(\.id)).count == apps.count)

        // Clipboard rules, on a private pasteboard so the real clipboard is never touched.
        let board = NSPasteboard(name: NSPasteboard.Name("com.orbit.selftest.\(getpid())"))
        defer { board.releaseGlobally() }
        func decide(types: [(String, String)], excluded: [String] = [], source: String? = nil, images: Bool = false, maxBytes: Int = 1024) -> ClipCapture {
            board.clearContents()
            board.declareTypes(types.map { NSPasteboard.PasteboardType($0.0) }, owner: nil)
            for (type, value) in types { board.setString(value, forType: NSPasteboard.PasteboardType(type)) }
            return evaluatePasteboard(board, excluded: excluded, sourceName: source, sourceBundle: nil, captureImages: images, maxBytes: maxBytes)
        }
        let string = NSPasteboard.PasteboardType.string.rawValue
        check("plain text is captured", decide(types: [(string, "hello")]) == .text("hello"))
        check("ConcealedType is skipped", decide(types: [(string, "secret"), ("org.nspasteboard.ConcealedType", "")]) == .skipConcealed)
        check("TransientType is skipped", decide(types: [(string, "temp"), ("org.nspasteboard.TransientType", "")]) == .skipConcealed)
        check("an excluded app is skipped, ignoring case", decide(types: [(string, "x")], excluded: ["1Password"], source: "1password") == .skipExcluded)
        check("a declared source bundle is honoured", decide(types: [(string, "x"), ("org.nspasteboard.source", "com.agile.1pw")], excluded: ["com.agile.1pw"]) == .skipExcluded)
        check("oversized text is skipped", decide(types: [(string, String(repeating: "x", count: 2000))]) == .skipTooLarge)
        board.clearContents()
        board.writeObjects([NSURL(fileURLWithPath: "/tmp/a.txt")])
        check("file references are captured as files", evaluatePasteboard(board, excluded: [], sourceName: nil, sourceBundle: nil, captureImages: false, maxBytes: 1024) == .files(["/tmp/a.txt"]))

        // hidutil, without changing any keyboard. Reads are real. The only write is aimed at a
        // device that doesn't exist, and the full mapping must read back identical afterwards.
        let absent = "{\"VendorID\":65534,\"ProductID\":65534}"
        let keyboards = takeString(orbit_hidutil_keyboards()) ?? ""
        let before = HidRemap.run(["property", "--get", "UserKeyMapping"])
        check("hidutil can read the current mapping", before != nil)
        if let before {
            let live = HidRemap.run(["property", "--matching", keyboards, "--get", "UserKeyMapping"]) ?? ""
            check("the keyboard matcher finds a keyboard", live.contains("UserKeyMapping"))
            let plan = takeString(orbit_hidutil_plan(live, false, keyboards)) ?? ""
            print("      caps remap on this machine right now: \(plan.contains("\"conflict\":\"") ? "would be refused, another tool holds Caps Lock" : "would be allowed (not performed by this test)")")
            let removal = takeString(orbit_hidutil_plan(live, true, keyboards)) ?? ""
            check("nothing to remove when Orbit hasn't mapped", removal.contains("\"commands\":[]"))
            let set = "{\"UserKeyMapping\":[{\"HIDKeyboardModifierMappingSrc\":0x700000039,\"HIDKeyboardModifierMappingDst\":0x70000006d}]}"
            _ = HidRemap.run(["property", "--matching", absent, "--set", set])
            check("a write to an absent device changes nothing", HidRemap.run(["property", "--get", "UserKeyMapping"]) == before)
        }
        print(failures == 0 ? "self-test passed" : "self-test FAILED: \(failures)")
        return failures == 0 ? 0 : 1
    }
}
