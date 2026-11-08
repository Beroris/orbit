// The launcher's state and behaviour: the port of platforms/windows/src/app.rs. Everything here
// runs on the main thread. Disk work goes to the data thread, and scans and launches to queues.

import AppKit
import Carbon
import COrbit
import ServiceManagement

struct Options {
    var dataDir: String
    var benchLog: String?
    var hotkeyOverride: String?
    var trimAfterMs: Int? = 15_000
    var noReindex = false
    var exportDir: String?
    var hidutilMatching: String?
    var pasteboardName: String?
    var extraAppDir: String?
    /// Replaces the default fallback chain, so tests never touch a chord the owner may be using.
    var fallbackHotkeys: [String]?
    /// Reports what a link would do but opens nothing, so tests can't start an app or a dialog.
    var noOpen = false
}

/// True only while the process is run directly, not through LaunchServices, so TCC would answer
/// for the parent. The diagnostics page says so rather than reporting someone else's grant.
let launchedFromParent = getppid() != 1

private let hotkeyLauncher: UInt32 = 1
private let hotkeyShortcutBase: UInt32 = 100
private let hotkeyHyperBase: UInt32 = 1000
private let rowLimit: UInt32 = 200
private let defaultHotkeys = ["Alt+Space", "Ctrl+Alt+Space", "Ctrl+Shift+Space", "Ctrl+Alt+O"]

final class AppController: NSObject, NSApplicationDelegate, NSWindowDelegate, NSTextFieldDelegate, NSMenuDelegate {
    static var shared: AppController!

    let options: Options
    let core: Core
    private(set) var host: HostSettings
    private(set) var worker: OpaquePointer!
    private(set) var view = ViewState()
    private(set) var status: String?
    let metrics: Metrics
    let hotkeys = HotkeyCenter()
    private(set) var icons: IconCache

    private var panel: LauncherPanel!
    private var field: NSTextField!
    private var results: ResultsView!
    private var statusItem: NSStatusItem?
    private var settingsWindow: SettingsWindow?
    private let toast = Toast()

    private var visible = false
    private var previousApp: NSRunningApplication?
    private var seq: UInt64 = 0
    private var trimItem: DispatchWorkItem?
    private var exiting = false
    private var exitFallback: DispatchWorkItem?

    private(set) var activeHotkey = ""
    private(set) var hotkeyProblems: [String] = []
    private var shortcutActions: [UInt32: Action] = [:]
    private var hyperActions: [UInt8: Action] = [:]
    private(set) var suspended = false

    private var tap: HyperTap?
    private var deleteOnExit = false
    private(set) var tapFailed = false
    private let remap: HidRemap
    private var remapNotice: String?
    private var pendingTriggers: OpaquePointer?

    private let clipWatcher = ClipboardWatcher()
    private(set) var clipPaused = false

    private var appWatcher: AppWatcher?
    private(set) var indexing = false
    private var indexDirty = false
    private let indexQueue = DispatchQueue(label: "orbit.index", qos: .utility)

    private var lastCounts: (notes: Int64, snippets: Int64, clips: Int64, sqlite: Int64)?
    private var memorySource: DispatchSourceMemoryPressure?
    private(set) var shedActive = false
    private var pendingExpansion: (text: String, erase: Int, cursorBack: Int?)?
    private var insertTarget: NSRunningApplication?
    private var canUndoTrash = false
    /// The web search row, held back until the data thread says nothing local matched.
    private var webRowWaiting: Row?
    /// Bundle paths of running apps, so rows can say "Switch to". Gathered on first use, then kept
    /// current by workspace notifications, because a fresh list on each open took about 5 ms.
    private var runningApps: Set<String>?

    init?(options: Options) {
        guard let core = Core(dataDir: options.dataDir), let host = core.host() else { return nil }
        self.options = options
        self.core = core
        self.host = host
        metrics = Metrics(logPath: options.benchLog)
        icons = IconCache(capacity: host.lowResourceMode ? 16 : 64, dataDir: options.dataDir)
        remap = HidRemap(dataDir: options.dataDir, matching: options.hidutilMatching)
        super.init()
        if let name = options.pasteboardName { activePasteboard = NSPasteboard(name: NSPasteboard.Name(name)) }
        extraApplicationFolder = options.extraAppDir
    }

    // MARK: - Startup

    func applicationDidFinishLaunching(_ notification: Notification) {
        buildPanel()
        installMainMenu()

        let wake: orbit_wake_fn = { _ in
            DispatchQueue.main.async { AppController.shared.drainWorker() }
        }
        worker = orbit_worker_start(options.dataDir, host.clipPersist, host.clipMaxItems, host.clipRetentionDays, wake, nil)

        hotkeys.onHotkey = { [weak self] id, t0 in self?.onHotkey(id: id, t0: t0) }
        if !hotkeys.install() {
            hotkeyProblems.append("Orbit couldn't install its shortcut handler. Use the menu bar icon.")
        }
        registerHotkeys()
        buildStatusItem()
        icons.onLoaded = { [weak self] in
            guard let self, self.visible else { return }
            self.results.needsDisplay = true
        }

        clipWatcher.onCopy = { [weak self] capture, pasteboard, source in self?.onCopy(capture, pasteboard: pasteboard, source: source) }
        clipWatcher.limits = { [weak self] in
            guard let self else { return ([], false, 0) }
            return (self.host.clipExcluded, self.host.clipImages && !self.host.lowResourceMode, Int(self.host.clipMaxItemBytes))
        }
        applyClipboardWatcher()

        // A journal means the last run ended with the remap applied and didn't get to undo it.
        // applyHook applies it again if it's still wanted, so clean up first.
        if remap.hasJournal { remap.remove() }
        applyHook()
        observeSystem()
        orbit_worker_load_usage(worker)

        appWatcher = AppWatcher { [weak self] in self?.startIndexer() }
        appWatcher?.start()
        let stale = indexIsStale()
        if !orbit_engine_had_cached_apps(core.engine) || (stale && !options.noReindex) {
            startIndexer()
        }
        if let notice = takeString(orbit_engine_take_notice(core.engine)) {
            report(notice)
        }
        if !host.onboarded {
            let hint = activeHotkey.isEmpty ? "Use the menu bar icon to open it." : "Press \(prettyChord(activeHotkey)) to open it."
            toast.show("Orbit is running. \(hint)")
            _ = orbit_engine_set_flag(core.engine, "onboarded", true)
        }
        if metrics.benchMode {
            BenchChannel.install(controller: self)
            metrics.recordLine("ready,\(ticks())")
            recordMemoryLine()
            metrics.flush()
        }
    }

    private func indexIsStale() -> Bool {
        let path = options.dataDir + "/apps.json"
        guard let modified = (try? FileManager.default.attributesOfItem(atPath: path))?[.modificationDate] as? Date else { return true }
        return Date().timeIntervalSince(modified) > 86_400
    }

    private func buildPanel() {
        let rows = host.maxVisibleRows
        view.visibleRows = rows
        panel = LauncherPanel(rows: rows)
        panel.delegate = self
        panel.onKeyEquivalent = { [weak self] event in self?.keyEquivalent(event) ?? false }
        let frame = NSRect(x: 0, y: 0, width: Layout.width, height: Layout.height(rows: rows))
        results = ResultsView(controller: self, frame: frame)
        panel.contentView = results

        let x = Layout.padding + 10 + Layout.icon + 12
        field = NSTextField(frame: NSRect(x: x, y: (Layout.inputHeight - 30) / 2, width: Layout.width - x - 16, height: 30))
        field.isBordered = false
        field.drawsBackground = false
        field.focusRingType = .none
        field.font = NSFont.systemFont(ofSize: 20)
        field.delegate = self
        field.cell?.isScrollable = true
        field.cell?.wraps = false
        field.cell?.usesSingleLineMode = true
        results.addSubview(field)
        applyTheme()
    }

    private func applyTheme() {
        let theme = Theme.resolve(appearance: host.appearance)
        results.theme = theme
        field.textColor = theme.text
        setPlaceholder(theme: theme)
    }

    private func setPlaceholder(theme: Theme? = nil) {
        let theme = theme ?? results.theme
        field.placeholderAttributedString = NSAttributedString(string: view.scope.placeholder, attributes: [.foregroundColor: theme.secondary, .font: NSFont.systemFont(ofSize: 20)])
    }

    private func observeSystem() {
        let center = NSWorkspace.shared.notificationCenter
        center.addObserver(forName: NSWorkspace.willSleepNotification, object: nil, queue: .main) { [weak self] _ in self?.tap?.reset() }
        center.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main) { [weak self] _ in self?.afterWake() }
        center.addObserver(forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main) { [weak self] note in
            self?.frontmostChanged(note.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication)
        }
        center.addObserver(forName: NSWorkspace.activeSpaceDidChangeNotification, object: nil, queue: .main) { [weak self] _ in
            self?.frontmostChanged(NSWorkspace.shared.frontmostApplication)
        }
        center.addObserver(forName: NSWorkspace.didLaunchApplicationNotification, object: nil, queue: .main) { [weak self] note in
            guard let path = (note.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication)?.bundleURL?.path else { return }
            self?.runningApps?.insert(path)
        }
        center.addObserver(forName: NSWorkspace.didTerminateApplicationNotification, object: nil, queue: .main) { [weak self] note in
            guard let path = (note.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication)?.bundleURL?.path else { return }
            self?.runningApps?.remove(path)
        }
        let distributed = DistributedNotificationCenter.default()
        distributed.addObserver(forName: NSNotification.Name("com.apple.screenIsLocked"), object: nil, queue: .main) { [weak self] _ in
            self?.tap?.reset()
            self?.hide(restoreFocus: false)
            self?.clipWatcher.stop()
            // The lock screen's password field must get a normal Caps Lock.
            if self?.remap.hasJournal == true { self?.remap.remove() }
        }
        distributed.addObserver(forName: NSNotification.Name("com.apple.screenIsUnlocked"), object: nil, queue: .main) { [weak self] _ in
            self?.afterWake()
            self?.applyClipboardWatcher()
        }
        // Reacts to the kernel's signal instead of checking memory on a timer.
        let source = DispatchSource.makeMemoryPressureSource(eventMask: [.warning, .critical, .normal], queue: .main)
        source.setEventHandler { [weak self, weak source] in
            guard let self, let event = source?.data else { return }
            self.shedActive = event != .normal
            self.applyCacheLimits()
            if self.shedActive {
                orbit_worker_shed(self.worker)
                malloc_zone_pressure_relief(nil, 0)
            }
        }
        source.resume()
        memorySource = source
    }

    private func afterWake() {
        tap?.reset()
        if !suspended { registerHotkeys() }
        // Keyboards attached while asleep have no mapping yet, and hidutil state can be dropped.
        if wantsCapsRemap { applyRemap() }
    }

    private func frontmostChanged(_ app: NSRunningApplication?) {
        tap?.reset()
        guard let tap else { return }
        let names = [app?.localizedName, app?.bundleIdentifier].compactMap { $0 }
        let excluded = names.contains { name in host.hyperExcluded.contains { $0.caseInsensitiveCompare(name) == .orderedSame } }
            || (host.hyperPassGames && frontmostIsFullscreen(app))
        let expansionExcluded = names.contains { name in host.expansionExcluded.contains { $0.caseInsensitiveCompare(name) == .orderedSame } }
        tap.setSuspended(excluded || (tap.config.triggerCode == nil && expansionExcluded))
    }

    /// True when the app's frontmost normal window covers a whole screen. Owner and bounds need no
    /// Screen Recording grant. Runs on app or Space changes only.
    private func frontmostIsFullscreen(_ app: NSRunningApplication?) -> Bool {
        guard let app, let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] else { return false }
        for info in list {
            guard (info[kCGWindowLayer as String] as? Int) == 0 else { continue }
            guard (info[kCGWindowOwnerPID as String] as? pid_t) == app.processIdentifier else { return false }
            guard let raw = info[kCGWindowBounds as String] as? NSDictionary, let bounds = CGRect(dictionaryRepresentation: raw) else { return false }
            return NSScreen.screens.contains { $0.frame.size == bounds.size }
        }
        return false
    }

    // MARK: - Window lifecycle

    private func activeScreen() -> NSScreen? {
        let mouse = NSEvent.mouseLocation
        return NSScreen.screens.first { NSMouseInRect(mouse, $0.frame, false) } ?? NSScreen.main ?? NSScreen.screens.first
    }

    private func positionPanel() {
        guard let screen = activeScreen() else { return }
        let area = screen.visibleFrame
        let size = panel.frame.size
        let free = max(0, area.height - size.height)
        panel.setFrameOrigin(NSPoint(x: area.minX + (area.width - size.width) / 2, y: area.maxY - size.height - free / 3))
    }

    func toggle(_ scope: Scope, source: String, t0: Int64) {
        if visible, view.scope == scope, view.stack.isEmpty {
            hide(restoreFocus: true)
        } else {
            show(scope, source: source, t0: t0)
        }
    }

    /// The hot path. T0 to T3 are defined in docs/ORIGINAL_BRIEF.md section 15.
    func show(_ scope: Scope, source: String, t0: Int64) {
        var times = ShowTimes(t0: t0)
        trimItem?.cancel()
        if !visible {
            let front = NSWorkspace.shared.frontmostApplication
            previousApp = front?.processIdentifier == ProcessInfo.processInfo.processIdentifier ? nil : front
        }
        positionPanel()
        status = nil
        view.stack.removeAll()
        let keepQuery = host.preserveQuery && scope == .root && view.scope == .root
        applyTheme()
        setScope(scope, keepQuery: keepQuery)
        results.renderFrame()
        results.needsDisplay = false
        times.t1 = ticks()
        panel.orderFrontRegardless()
        visible = true
        results.displayIfNeeded()
        panel.invalidateShadow()
        CATransaction.flush()
        times.t2 = ticks()
        panel.makeKey()
        panel.makeFirstResponder(field)
        if keepQuery { field.currentEditor()?.selectAll(nil) }
        if panel.isKeyWindow, field.currentEditor() != nil { times.t3 = ticks() }
        metrics.recordShow(source: source, times: times)
    }

    func hide(restoreFocus: Bool) {
        guard visible else { return }
        visible = false
        panel.orderOut(nil)
        results.releaseFrame()
        metrics.commitPendingShow()
        view.stack.removeAll()
        // The panel never activated Orbit, so normally the previous app is still frontmost and
        // its window takes the keyboard back by itself. This covers Orbit having been active.
        if restoreFocus, let previous = previousApp, !previous.isTerminated, NSWorkspace.shared.frontmostApplication != previous {
            previous.activate()
        }
        scheduleTrim()
    }

    /// Returns freed heap to the system a while after hiding. A one-shot, not a timer that keeps
    /// running: it fires once and leaves idle wakeups at zero.
    private func scheduleTrim() {
        trimItem?.cancel()
        guard let delay = host.lowResourceMode ? 3000 : options.trimAfterMs else { return }
        let item = DispatchWorkItem { [weak self] in
            guard let self, !self.visible else { return }
            malloc_zone_pressure_relief(nil, 0)
        }
        trimItem = item
        DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(delay), execute: item)
    }

    func windowDidBecomeKey(_ notification: Notification) {
        if (notification.object as? NSWindow) === panel { metrics.panelBecameKey() }
    }

    func windowDidResignKey(_ notification: Notification) {
        if (notification.object as? NSWindow) === panel, visible { hide(restoreFocus: false) }
    }

    func windowDidChangeOcclusionState(_ notification: Notification) {
        if panel.occlusionState.contains(.visible) { metrics.panelBecameVisible() }
    }

    func focusField() {
        panel.makeFirstResponder(field)
    }

    private func redraw() {
        guard visible else { return }
        results.needsDisplay = true
    }

    // MARK: - Mouse

    func mouseSelect(_ index: Int) {
        guard index != view.selected else { return }
        view.select(index)
        afterSelectionChange()
        redraw()
    }

    func mouseClick(_ index: Int, secondary: Bool) {
        view.select(index)
        afterSelectionChange()
        if secondary {
            openActionMenu()
        } else {
            activateSelected(secondary: false, tEnter: ticks())
        }
    }

    func scroll(_ delta: Int) {
        view.scrollBy(delta)
        redraw()
    }

    // MARK: - Keys

    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        if textView.hasMarkedText() { return false } // the input method owns these keys
        let t = ticks()
        switch selector {
        case #selector(NSResponder.moveUp(_:)): view.moveSelection(-1)
        case #selector(NSResponder.moveDown(_:)): view.moveSelection(1)
        case #selector(NSResponder.scrollPageUp(_:)), #selector(NSResponder.pageUp(_:)): view.moveSelection(-view.visibleRows)
        case #selector(NSResponder.scrollPageDown(_:)), #selector(NSResponder.pageDown(_:)): view.moveSelection(view.visibleRows)
        case #selector(NSResponder.cancelOperation(_:)):
            goBackOrHide()
            return true
        case #selector(NSResponder.insertNewline(_:)):
            activateSelected(secondary: false, tEnter: t)
            return true
        case #selector(NSResponder.insertTab(_:)):
            openActionMenu()
            return true
        case #selector(NSResponder.deleteBackward(_:)):
            if field.stringValue.isEmpty, !view.stack.isEmpty {
                goBackOrHide()
                return true
            }
            return false
        default:
            return false
        }
        afterSelectionChange()
        redraw()
        return true
    }

    private func keyEquivalent(_ event: NSEvent) -> Bool {
        guard event.type == .keyDown, event.modifierFlags.contains(.command) else { return false }
        let editor = field.currentEditor() as? NSTextView
        switch (event.keyCode, event.modifierFlags.contains(.shift)) {
        case (36, _), (76, _):
            activateSelected(secondary: true, tEnter: ticks())
        case (40, false): // K
            openActionMenu()
        case (13, false): // W
            hide(restoreFocus: true)
        case (43, false): // comma
            performBuiltin(.settings)
        case (0, false): editor?.selectAll(nil)
        case (8, false): editor?.copy(nil)
        case (7, false): editor?.cut(nil)
        case (9, false): editor?.paste(nil)
        case (6, false): editor?.undoManager?.undo()
        case (6, true): editor?.undoManager?.redo()
        default:
            return false
        }
        return true
    }

    // MARK: - Queries

    private var query: String { field.stringValue }

    private func setQueryText(_ text: String) {
        field.stringValue = text
        field.currentEditor()?.selectedRange = NSRange(location: (text as NSString).length, length: 0)
    }

    func setScope(_ scope: Scope, keepQuery: Bool) {
        view.scope = scope
        view.resetPosition()
        view.staticRows.removeAll()
        setPlaceholder()
        if !keepQuery { setQueryText("") }
        buildStaticRows()
        refreshResults()
    }

    private func buildStaticRows() {
        switch view.scope {
        case .help:
            view.staticRows = helpRows(launcherHint: prettyChord(activeHotkey))
        case .diagnostics:
            view.staticRows = diagnosticsRows()
            orbit_worker_counts(worker)
        case .confirm(let command):
            let title = takeString(orbit_builtin_title(command.rawValue)) ?? "this"
            view.staticRows = [
                .op("Cancel", "Enter", glyph: ORBIT_GLYPH_INFO, .back),
                .op("Yes, \(title)", "Press ↓ then Enter", glyph: ORBIT_GLYPH_POWER, .confirmed(command)),
            ]
        default:
            break
        }
    }

    func controlTextDidChange(_ notification: Notification) {
        view.resetPosition()
        refreshResults()
    }

    private func refreshResults() {
        let t4 = ticks()
        let text = query
        view.lastQuery = text
        switch view.scope {
        case .root: buildRootRows(text)
        case .notes, .snippets, .clipboard: requestScopeRows(text)
        case .setAlias(let key, _):
            let alias = text.trimmingCharacters(in: .whitespaces)
            view.rows = alias.isEmpty || alias.contains(" ") ? [] : [.op("Use alias “\(alias)”", "", glyph: ORBIT_GLYPH_LINK, .saveAlias(key: key))]
        default:
            view.filterStatic(text)
        }
        view.select(view.selected)
        afterSelectionChange()
        let t5 = ticks()
        if visible {
            results.needsDisplay = true
            results.displayIfNeeded()
        }
        metrics.recordQuery(queryLength: text.utf8.count, hits: view.rows.count, t4: t4, t5: t5, t6: ticks())
    }

    /// True while a query has no rows yet only because the data thread hasn't answered.
    var waitingForData: Bool { webRowWaiting != nil }

    private func buildRootRows(_ text: String) {
        seq += 1
        webRowWaiting = nil
        let trimmed = text.trimmingCharacters(in: .whitespaces)
        if trimmed == "?" {
            view.rows = helpRows(launcherHint: prettyChord(activeHotkey))
            view.asyncInsertAt = view.rows.count
            return
        }
        let result = orbit_engine_query_root(core.engine, text)
        var rows: [Row] = []
        rows.reserveCapacity(result.count)
        if let pointer = result.rows {
            for index in 0..<result.count { rows.append(Row(pointer[index])) }
        }
        markRunningApps(&rows)
        // The core adds the web row only when nothing else matched, which is the one case with
        // rows but an insert position of 0. Notes and snippets answer a moment later, so the row
        // waits for them. Otherwise Enter could search the web for a saved note.
        if result.wants_data, result.async_insert_at == 0, let web = rows.first {
            webRowWaiting = web
            rows.removeAll()
        }
        view.rows = rows
        view.asyncInsertAt = result.async_insert_at
        if result.wants_data {
            orbit_worker_search(worker, seq, UInt32(ORBIT_SCOPE_ROOT), trimmed, 6)
        }
    }

    private func markRunningApps(_ rows: inout [Row]) {
        guard host.focusRunningApp, rows.contains(where: isAppRow) else { return }
        if runningApps == nil {
            runningApps = Set(NSWorkspace.shared.runningApplications.compactMap { $0.bundleURL?.path })
        }
        for index in rows.indices {
            if case .app(let path) = rows[index].icon, runningApps?.contains(path) == true {
                rows[index].kind = "Switch to"
                rows[index].subtitle = "Running"
            }
        }
    }

    private func requestScopeRows(_ text: String) {
        seq += 1
        let dataScope: Int
        switch view.scope {
        case .notes: dataScope = ORBIT_SCOPE_NOTES
        case .snippets: dataScope = ORBIT_SCOPE_SNIPPETS
        default: dataScope = ORBIT_SCOPE_CLIPBOARD
        }
        if dataScope == ORBIT_SCOPE_CLIPBOARD, !host.clipEnabled {
            view.rows = [
                .op("Turn on clipboard history", "Stored only on this Mac. Concealed copies and excluded apps are skipped.", glyph: ORBIT_GLYPH_CLIPBOARD, .enableClipboard),
                .info("Clipboard history is off", "Nothing you copy is recorded"),
            ]
            return
        }
        orbit_worker_search(worker, seq, UInt32(dataScope), text, rowLimit)
    }

    private func afterSelectionChange() {
        if let row = view.selectedRow {
            view.secondaryLabel = secondaryFor(row, isApp: isAppRow(row))?.0
        } else {
            view.secondaryLabel = nil
        }
    }

    private func isAppRow(_ row: Row) -> Bool {
        if case .app = row.icon { return true }
        return false
    }

    // MARK: - Worker events

    func drainWorker() {
        var event = OrbitEvent()
        while orbit_worker_next_event(worker, &event) {
            handle(event)
        }
    }

    private func handle(_ event: OrbitEvent) {
        switch Int(event.kind) {
        case ORBIT_EVENT_RESULTS:
            if event.seq == seq {
                var rows: [Row] = []
                if let pointer = event.rows {
                    for index in 0..<event.row_count { rows.append(Row(pointer[index])) }
                }
                mergeResults(scope: Int(event.scope), query: borrowed(event.text), rows: rows)
            }
        case ORBIT_EVENT_USAGE:
            orbit_worker_apply_usage(worker, core.engine, nowUnix())
        case ORBIT_EVENT_NOTE_BODY:
            NoteEditor.show(worker: worker, id: event.id, body: borrowed(event.text))
        case ORBIT_EVENT_NOTE_SAVED:
            NoteEditor.saved(token: event.token, id: event.id)
            if metrics.benchMode { metrics.recordLine("note-saved,\(event.id)") }
        case ORBIT_EVENT_NOTE_TEXT:
            copyToClipboard(borrowed(event.text), message: "Note copied")
        case ORBIT_EVENT_NOTE_RESTORED:
            refreshIfDataScope()
            report("Note restored")
        case ORBIT_EVENT_SNIPPET:
            onSnippet(event)
        case ORBIT_EVENT_SNIPPET_SAVED:
            SnippetEditor.saved(token: event.token)
            let other = borrowed(event.text)
            if !other.isEmpty { report("Trigger also used by “\(other)”. The older snippet wins.") }
            refreshIfDataScope()
        case ORBIT_EVENT_TRIGGERS:
            let matcher = orbit_worker_take_triggers(worker)
            if let tap {
                tap.setTriggers(matcher)
            } else {
                orbit_triggers_free(matcher)
            }
        case ORBIT_EVENT_CLIP:
            onClipContent(kind: Int(event.clip_kind), text: borrowed(event.text), blob: borrowed(event.text2), purpose: Int(event.purpose))
        case ORBIT_EVENT_CHANGED:
            refreshIfDataScope()
            if metrics.benchMode { metrics.recordLine("changed,\(ticks())") }
        case ORBIT_EVENT_EXPORTED:
            let text = borrowed(event.text)
            if event.ok {
                report("Exported to \(text)")
                // A test run names its own export folder, and shouldn't open windows on the desktop.
                if options.exportDir == nil { NSWorkspace.shared.open(URL(fileURLWithPath: text)) }
            } else {
                report("Export failed: \(text)")
            }
        case ORBIT_EVENT_IMPORTED:
            let text = borrowed(event.text)
            report(event.ok ? text : "Import failed: \(text)")
            // Imported snippets may bring triggers the running tap hasn't heard of.
            if event.ok, tap != nil, host.expansionEnabled { orbit_worker_snippet_triggers(worker) }
        case ORBIT_EVENT_COUNTS:
            lastCounts = (event.notes, event.snippets, event.clips, event.sqlite_bytes)
            if view.scope == .diagnostics {
                view.staticRows = diagnosticsRows()
                view.filterStatic(query)
                redraw()
            }
            if metrics.benchMode { metrics.recordLine("counts,\(event.notes),\(event.snippets),\(event.clips),\(event.sqlite_bytes)") }
        case ORBIT_EVENT_FAILED:
            report(borrowed(event.text))
        case ORBIT_EVENT_FINISHED:
            finishExit()
        default:
            break
        }
    }

    private func refreshIfDataScope() {
        guard visible, view.scope.isData else { return }
        let selected = view.selected
        refreshResults()
        view.select(selected)
    }

    /// Adds results from the data thread. In the root list they go below what is already shown,
    /// so a selection cannot jump under the user's fingers.
    private func mergeResults(scope: Int, query: String, rows: [Row]) {
        switch scope {
        case ORBIT_SCOPE_ROOT:
            // A menu may have opened since this search started.
            guard view.scope == .root else { return }
            let at = min(view.asyncInsertAt, view.rows.count)
            view.rows.insert(contentsOf: rows, at: at)
            if let web = webRowWaiting {
                webRowWaiting = nil
                if view.rows.isEmpty { view.rows.append(web) }
            }
        case ORBIT_SCOPE_NOTES:
            view.rows = rows
            let q = query.trimmingCharacters(in: .whitespaces)
            let new = Row.op(q.isEmpty ? "New note" : "New note “\(q)”", "", glyph: ORBIT_GLYPH_NOTE, .newNote)
            if rows.isEmpty || q.isEmpty {
                view.rows.insert(new, at: 0)
            } else {
                view.rows.append(new)
            }
            if canUndoTrash, q.isEmpty {
                view.rows.insert(.op("Undo: restore the note you just trashed", "", glyph: ORBIT_GLYPH_DELETE, .noteUndoTrash), at: 0)
            }
        case ORBIT_SCOPE_SNIPPETS:
            view.rows = rows
            view.rows.append(.op("New snippet", "", glyph: ORBIT_GLYPH_SNIPPET, .newSnippet))
        default:
            view.rows = rows
            if clipPaused { status = "Clipboard history is paused" }
        }
        view.select(view.selected)
        afterSelectionChange()
        redraw()
    }

    // MARK: - Running things

    private func activateSelected(secondary: Bool, tEnter: Int64) {
        guard let row = view.selectedRow else { return }
        let op = secondary ? secondaryFor(row, isApp: isAppRow(row))?.1 : row.primary
        if let op {
            execute(op)
            metrics.recordAction(kind: "enter", tEnter: tEnter, tDispatched: ticks())
        }
    }

    private func openActionMenu() {
        switch view.scope {
        case .actions, .confirm, .setAlias: return
        default: break
        }
        guard let row = view.selectedRow, row.source != .plain else { return }
        var pinned = false
        if case .catalog(_, let key) = row.source { pinned = host.pinnedContains(key, core: core) }
        let menu = menuFor(row, isApp: isAppRow(row), isPinned: pinned)
        pushView()
        view.scope = .actions(of: row.title)
        view.staticRows = menu
        view.resetPosition()
        setPlaceholder()
        setQueryText("")
        refreshResults()
    }

    private func pushView() {
        view.stack.append(SavedView(scope: view.scope, query: query, selected: view.selected))
    }

    private func goBackOrHide() {
        guard let saved = view.stack.popLast() else {
            hide(restoreFocus: true)
            return
        }
        view.scope = saved.scope
        view.staticRows.removeAll()
        setPlaceholder()
        buildStaticRows()
        setQueryText(saved.query)
        status = nil
        refreshResults()
        view.select(saved.selected)
        afterSelectionChange()
        redraw()
    }

    private func appPath(forKey key: String) -> String? {
        var app = OrbitApp()
        let id = key.hasPrefix("app:") ? String(key.dropFirst(4)) : key
        guard orbit_engine_find_app(core.engine, id, "", &app) else { return nil }
        return borrowed(app.path)
    }

    private func execute(_ op: Op) {
        switch op {
        case .runItem(let item, let key):
            var action = OrbitAction()
            guard orbit_engine_item_action(core.engine, item, host.focusRunningApp, &action) else { return }
            orbit_worker_record_usage(worker, key)
            let value = borrowed(action.value)
            let name = borrowed(action.name)
            switch Int(action.kind) {
            case ORBIT_ACTION_FOCUS_OR_LAUNCH: run(.focusOrLaunch(id: value, name: name))
            case ORBIT_ACTION_LAUNCH: run(.launch(id: value, name: name))
            case ORBIT_ACTION_URL:
                // A template link with no query asks for one rather than opening a broken URL.
                if action.is_template {
                    if !name.isEmpty {
                        setQueryText(name + " ")
                        refreshResults()
                    }
                } else {
                    run(.url(value))
                }
            case ORBIT_ACTION_BUILTIN:
                if let command = Builtin(rawValue: value) { runBuiltin(command) }
            case ORBIT_ACTION_CUSTOM: run(.custom(value))
            default: break
            }
        case .askConfirm(let command):
            pushView()
            view.scope = .confirm(command)
            view.resetPosition()
            setPlaceholder()
            setQueryText("")
            buildStaticRows()
            refreshResults()
        case .confirmed(let command):
            hide(restoreFocus: true)
            performBuiltin(command)
        case .back:
            goBackOrHide()
        case .copy(let text):
            hide(restoreFocus: true)
            copyToClipboard(text, message: nil)
        case .openUrl(let url):
            hide(restoreFocus: false)
            openURL(url)
        case .appFolder(let key):
            hide(restoreFocus: false)
            if let path = appPath(forKey: key) {
                NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
            }
        case .togglePin(let key):
            _ = orbit_engine_toggle_pin(core.engine, key)
            reloadHost()
            goBackOrHide()
        case .hideItem(let key):
            _ = orbit_engine_hide_item(core.engine, key)
            reloadHost()
            goBackOrHide()
        case .aliasPrompt(let key, let title):
            view.scope = .setAlias(key: key, of: title)
            view.resetPosition()
            setPlaceholder()
            setQueryText("")
            refreshResults()
        case .saveAlias(let key):
            let alias = query.trimmingCharacters(in: .whitespaces)
            if !alias.isEmpty { _ = orbit_engine_set_alias(core.engine, key, alias) }
            goBackOrHide()
        case .noteOpen(let id):
            hide(restoreFocus: false)
            orbit_worker_note_open(worker, id)
        case .noteCopy(let id):
            hide(restoreFocus: true)
            orbit_worker_note_copy(worker, id)
        case .notePin(let id, let pinned):
            orbit_worker_note_set_pinned(worker, id, pinned)
            goBackOrHide()
        case .noteTrash(let id):
            orbit_worker_note_trash(worker, id)
            canUndoTrash = true
            goBackOrHide()
            status = "Note moved to trash. Undo is at the top of Notes."
            redraw()
        case .noteUndoTrash:
            canUndoTrash = false
            orbit_worker_note_restore_last(worker)
        case .newNote:
            let seed = view.scope == .notes ? query.trimmingCharacters(in: .whitespaces) : ""
            hide(restoreFocus: false)
            NoteEditor.show(worker: worker, id: nil, body: seed)
        case .snippetInsert(let id):
            insertTarget = previousApp
            hide(restoreFocus: true)
            orbit_worker_snippet_get(worker, id, UInt32(ORBIT_PURPOSE_INSERT), 0)
        case .snippetCopy(let id):
            hide(restoreFocus: true)
            orbit_worker_snippet_get(worker, id, UInt32(ORBIT_PURPOSE_COPY), 0)
        case .snippetEdit(let id):
            hide(restoreFocus: false)
            orbit_worker_snippet_get(worker, id, UInt32(ORBIT_PURPOSE_EDIT), 0)
        case .snippetDelete(let id):
            orbit_worker_snippet_delete(worker, id)
            goBackOrHide()
        case .newSnippet:
            hide(restoreFocus: false)
            SnippetEditor.show(worker: worker, fields: .init())
        case .clipPaste(let id):
            insertTarget = previousApp
            hide(restoreFocus: true)
            orbit_worker_clip_get(worker, id, UInt32(ORBIT_PURPOSE_INSERT))
        case .clipCopy(let id):
            hide(restoreFocus: true)
            orbit_worker_clip_get(worker, id, UInt32(ORBIT_PURPOSE_COPY))
        case .clipPin(let id, let pinned):
            orbit_worker_clip_set_pinned(worker, id, pinned)
            goBackOrHide()
        case .clipDelete(let id):
            orbit_worker_clip_delete(worker, id)
            goBackOrHide()
        case .enableClipboard:
            _ = orbit_engine_set_flag(core.engine, "clipboard_enabled", true)
            reloadHost()
            applyClipboardWatcher()
            refreshResults()
        case .openSettings:
            performBuiltin(.settings)
        case .insertPending:
            let pending = pendingExpansion
            pendingExpansion = nil
            hide(restoreFocus: true)
            if let pending { startInsert(pending.text, erase: pending.erase, cursorBack: pending.cursorBack, viaClipboard: false) }
        }
    }

    func run(_ action: Action) {
        switch action {
        case .focusOrLaunch(let id, let name), .launch(let id, let name):
            hide(restoreFocus: false)
            var app = OrbitApp()
            guard orbit_engine_find_app(core.engine, id, name, &app) else {
                report("\(name) isn't installed, or the app index is out of date. Try Rebuild App Index.")
                return
            }
            launchApp(path: borrowed(app.path), name: borrowed(app.name))
        case .openPath(let path):
            hide(restoreFocus: false)
            if !NSWorkspace.shared.open(URL(fileURLWithPath: (path as NSString).expandingTildeInPath)) {
                report("Couldn't open \(path).")
            }
        case .url(let url):
            hide(restoreFocus: false)
            openURL(url)
        case .builtin(let command):
            runBuiltin(command)
        case .custom(let id):
            hide(restoreFocus: false)
            var command = OrbitCustom()
            guard orbit_engine_custom(core.engine, id, &command) else {
                report("That command no longer exists.")
                return
            }
            runCustom(name: borrowed(command.name), program: borrowed(command.program), arguments: borrowed(command.arguments), workingDir: borrowed(command.working_dir))
        case .openNote(let id):
            hide(restoreFocus: false)
            orbit_worker_note_open(worker, id)
        case .insertSnippet(let id):
            insertTarget = visible ? previousApp : NSWorkspace.shared.frontmostApplication
            hide(restoreFocus: true)
            orbit_worker_snippet_get(worker, id, UInt32(ORBIT_PURPOSE_INSERT), 0)
        }
    }

    /// LaunchServices switches to a running app or starts it, so one call covers focus-or-launch.
    /// macOS apps are single-instance by convention, so "launch" doesn't force a second copy.
    private func launchApp(path: String, name: String) {
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.activates = true
        NSWorkspace.shared.openApplication(at: URL(fileURLWithPath: path), configuration: configuration) { [weak self] _, error in
            guard let error else { return }
            DispatchQueue.main.async { self?.report("Couldn't open \(name): \(error.localizedDescription)") }
        }
    }

    private func openURL(_ url: String) {
        var scheme: UnsafeMutablePointer<CChar>?
        switch orbit_url_check(url, &scheme) {
        case let check where check == 0 || check == 1:
            let name = takeString(scheme)
            guard let target = URL(string: url) else {
                report("That doesn't look like a valid address.")
                return
            }
            if check == 1 { report("Opening a \(name ?? "custom"): link. macOS chooses the app that handles it.") }
            if options.noOpen { return }
            NSWorkspace.shared.open(target, configuration: .init()) { [weak self] _, error in
                guard let error else { return }
                DispatchQueue.main.async { self?.report("Couldn't open that address: \(error.localizedDescription)") }
            }
        case 2:
            report("Orbit won't open \(takeString(scheme) ?? "these"): links, because they can run code.")
        default:
            report("That doesn't look like a valid address.")
        }
    }

    /// Splits arguments on spaces, honouring double quotes. No shell is involved, so nothing in
    /// the text can expand or chain commands.
    static func splitArguments(_ text: String) -> [String] {
        var out: [String] = []
        var current = ""
        var inQuotes = false
        var started = false
        for character in text {
            if character == "\"" {
                inQuotes.toggle()
                started = true
            } else if character == " ", !inQuotes {
                if started || !current.isEmpty { out.append(current) }
                current = ""
                started = false
            } else {
                current.append(character)
            }
        }
        if started || !current.isEmpty { out.append(current) }
        return out
    }

    private func runCustom(name: String, program: String, arguments: String, workingDir: String) {
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let process = Process()
            process.executableURL = URL(fileURLWithPath: (program as NSString).expandingTildeInPath)
            process.arguments = AppController.splitArguments(arguments)
            if !workingDir.isEmpty { process.currentDirectoryURL = URL(fileURLWithPath: workingDir) }
            do {
                try process.run()
            } catch {
                DispatchQueue.main.async { self?.report("Couldn't run \(name): \(error.localizedDescription)") }
            }
        }
    }

    func runBuiltin(_ command: Builtin) {
        if orbit_builtin_needs_confirmation(command.rawValue) {
            if !visible { show(.root, source: "command", t0: ticks()) }
            execute(.askConfirm(command))
            return
        }
        performBuiltin(command)
    }

    func performBuiltin(_ command: Builtin) {
        let t0 = ticks()
        switch command {
        case .toggleLauncher: toggle(.root, source: "command", t0: t0)
        case .newNote:
            hide(restoreFocus: false)
            NoteEditor.show(worker: worker, id: nil, body: "")
        case .searchNotes: enterScope(.notes, t0: t0)
        case .clipboardHistory: enterScope(.clipboard, t0: t0)
        case .snippetSearch: enterScope(.snippets, t0: t0)
        case .newSnippet:
            hide(restoreFocus: false)
            SnippetEditor.show(worker: worker, fields: .init())
        case .pauseClipboard:
            setClipPaused(true)
            hide(restoreFocus: true)
            report("Clipboard history paused")
        case .resumeClipboard:
            setClipPaused(false)
            hide(restoreFocus: true)
            report("Clipboard history resumed")
        case .skipNextCopy:
            clipWatcher.skipNext = true
            hide(restoreFocus: true)
            report("Your next copy won't be remembered")
        case .clearClipboard:
            orbit_worker_clip_clear(worker)
        case .resetShortcuts:
            _ = orbit_engine_reset_shortcuts(core.engine)
            reloadHost()
            registerHotkeys()
            applyHook()
            report("Shortcuts are back to the defaults.")
        case .deleteLocalData:
            deleteLocalData()
        case .settings:
            hide(restoreFocus: false)
            openSettings()
        case .diagnostics: enterScope(.diagnostics, t0: t0)
        case .showHelp: enterScope(.help, t0: t0)
        case .rebuildAppIndex:
            hide(restoreFocus: true)
            startIndexer()
            report("Rebuilding the app index…")
        case .suspendShortcuts: setSuspended(true)
        case .resumeShortcuts: setSuspended(false)
        case .toggleLowResource:
            let on = !host.lowResourceMode
            _ = orbit_engine_set_flag(core.engine, "low_resource_mode", on)
            reloadHost()
            applyCacheLimits()
            if on {
                orbit_worker_shed(worker)
                malloc_zone_pressure_relief(nil, 0)
            }
            hide(restoreFocus: true)
            report(on ? "Low Resource Mode is on" : "Low Resource Mode is off")
        case .exportData:
            hide(restoreFocus: true)
            exportData()
        case .restartOrbit: restart()
        case .quitOrbit: beginExit()
        case .lockScreen:
            hide(restoreFocus: false)
            SystemCommands.lock()
        case .sleep: SystemCommands.sleep()
        case .shutdown: SystemCommands.shutDown()
        case .restart: SystemCommands.restart()
        case .emptyTrash: SystemCommands.emptyTrash { [weak self] message in self?.report(message) }
        case .openDownloads:
            hide(restoreFocus: false)
            NSWorkspace.shared.open(URL(fileURLWithPath: NSHomeDirectory() + "/Downloads"))
        case .openTerminal:
            hide(restoreFocus: false)
            launchApp(path: "/System/Applications/Utilities/Terminal.app", name: "Terminal")
        }
    }

    private func enterScope(_ scope: Scope, t0: Int64) {
        if visible {
            pushView()
            status = nil
            setScope(scope, keepQuery: false)
            redraw()
        } else {
            show(scope, source: "command", t0: t0)
        }
    }

    func exportData() {
        let base = options.exportDir ?? (NSHomeDirectory() + "/Documents")
        let formatter = DateFormatter()
        formatter.dateFormat = "yyyy-MM-dd HHmm"
        let settingsPath = options.dataDir + "/settings.json"
        let settings = (try? String(contentsOfFile: settingsPath, encoding: .utf8)) ?? "{}"
        orbit_worker_export(worker, settings, base, formatter.string(from: Date()))
    }

    func importData() {
        let dialog = NSOpenPanel()
        dialog.allowedContentTypes = [.json]
        dialog.allowsMultipleSelection = false
        NSApp.activate(ignoringOtherApps: true)
        if dialog.runModal() == .OK, let url = dialog.url {
            orbit_worker_import(worker, url.path)
        }
    }

    // MARK: - Hotkeys and HyperKey

    private func onHotkey(id: UInt32, t0: Int64) {
        if suspended { return }
        if id == hotkeyLauncher {
            toggle(.root, source: "hotkey", t0: t0)
        } else if let action = shortcutActions[id] {
            run(action)
        }
    }

    private func onHyper(code: UInt8, t0: Int64) {
        metrics.recordHookDispatch(tHook: t0, tReceived: ticks())
        guard let action = hyperActions[code] else { return }
        if metrics.benchMode { metrics.recordLine("hyper-dispatch,\(code)") }
        switch action {
        case .builtin(.toggleLauncher): toggle(.root, source: "hyper", t0: t0)
        case .builtin(.clipboardHistory): toggle(.clipboard, source: "hyper", t0: t0)
        case .builtin(.snippetSearch): toggle(.snippets, source: "hyper", t0: t0)
        case .builtin(.searchNotes): toggle(.notes, source: "hyper", t0: t0)
        default: run(action)
        }
    }

    func registerHotkeys() {
        hotkeys.unregisterAll()
        hotkeyProblems.removeAll()
        shortcutActions.removeAll()
        hyperActions.removeAll()
        activeHotkey = ""

        var candidates: [(mods: UInt8, key: String, display: String)] = []
        let overridden = options.hotkeyOverride.flatMap { text in parseChord(text).map { (mods: $0.mods, key: $0.key, display: text) } }
        if let overridden {
            // A test chord must not fall back to a real shortcut the owner may be using.
            candidates.append(overridden)
        } else {
            if let chosen = host.hotkey { candidates.append((chosen.mods, chosen.key, chosen.display)) }
            for text in options.fallbackHotkeys ?? defaultHotkeys {
                guard let parsed = parseChord(text), !candidates.contains(where: { $0.mods == parsed.mods && $0.key == parsed.key }) else { continue }
                candidates.append((parsed.mods, parsed.key, text))
            }
        }
        var reasons: [String] = []
        for candidate in candidates {
            switch hotkeys.register(id: hotkeyLauncher, mods: candidate.mods, key: candidate.key) {
            case .success:
                activeHotkey = candidate.display
            case .failure(let why):
                reasons.append("\(prettyChord(candidate.display)): \(why)")
            }
            if !activeHotkey.isEmpty { break }
        }
        if activeHotkey.isEmpty {
            hotkeyProblems.append("No launcher shortcut could be registered. Use the menu bar icon, or choose one in Settings. \(reasons.joined(separator: ". ")).")
        } else if !reasons.isEmpty {
            hotkeyProblems.append("\(reasons.joined(separator: ". ")). Orbit is using \(prettyChord(activeHotkey)) instead.")
        }
        if let owner = host.reservedHotkeyOwner, let chosen = host.hotkey, activeHotkey == chosen.display {
            // The user chose this and the system let it through, so it stays. It is never picked
            // as a default, and it is never taken without this notice.
            hotkeyProblems.append("\(prettyChord(activeHotkey)) is normally \(owner). Orbit took it because you chose it and macOS has that shortcut turned off.")
        }

        for (index, binding) in host.shortcuts.enumerated() {
            guard let action = Action(text: binding.action) else {
                hotkeyProblems.append("\(prettyChord(binding.chord.display)): can't understand “\(binding.action)”")
                continue
            }
            let id = hotkeyShortcutBase + UInt32(index)
            switch hotkeys.register(id: id, mods: binding.chord.mods, key: binding.chord.key) {
            case .success: shortcutActions[id] = action
            case .failure(let why): hotkeyProblems.append("\(prettyChord(binding.chord.display)): \(why)")
            }
        }

        if host.hyperEnabled {
            for (index, binding) in host.hyperBindings.enumerated() {
                guard let code = keyCode(forKeyName: binding.key), let action = Action(text: binding.action) else {
                    hotkeyProblems.append("Hyper+\(binding.key): can't be bound on this keyboard")
                    continue
                }
                hyperActions[UInt8(code)] = action
                if host.hyperMode == "external" {
                    let id = hotkeyHyperBase + UInt32(index)
                    switch hotkeys.register(id: id, mods: modHyper, key: binding.key) {
                    case .success: shortcutActions[id] = action
                    case .failure(let why): hotkeyProblems.append("Hyper+\(binding.key): \(why)")
                    }
                }
            }
            for key in host.hyperDuplicates {
                hotkeyProblems.append("Hyper+\(key) is bound more than once. The first binding is used.")
            }
        }
        // A problem is announced once, not at every start. Settings, Diagnostics and the menu bar
        // item keep showing it for as long as it lasts.
        if orbit_engine_note_hotkey_problems(core.engine, hotkeyProblems.joined(separator: " ")), !hotkeyProblems.isEmpty {
            if metrics.benchMode { metrics.recordLine("hotkey-notice,\(hotkeyProblems.count)") }
            if statusItem != nil || !metrics.benchMode { toast.show(hotkeyProblems[0]) }
        }
        updateStatusItemHint()
    }

    private var wantsCapsRemap: Bool {
        host.hyperEnabled && !suspended && host.hyperMode != "external" && host.hyperTrigger.caseInsensitiveCompare("CapsLock") == .orderedSame
    }

    private func applyRemap() {
        remap.apply { [weak self] problem in
            guard let self else { return }
            self.remapNotice = problem
            if let problem { self.report(problem) }
        }
    }

    /// Creates or removes the tap to match settings. External mode and plain shortcuts never get
    /// here: they use RegisterEventHotKey and hold no tap at all.
    func applyHook() {
        tap?.stop()
        tap = nil
        tapFailed = false
        remapNotice = nil
        let wantsHyper = host.hyperEnabled && host.hyperMode != "external"
        guard !suspended, wantsHyper || host.expansionEnabled else {
            if remap.hasJournal { remap.remove() }
            return
        }
        var config = TapConfig()
        config.expansion = host.expansionEnabled
        if wantsHyper {
            // With Caps Lock as the key, hidutil turns it into F18 and the tap watches F18, so the
            // tap never has to fight Caps Lock's sticky toggle.
            let isCaps = host.hyperTrigger.caseInsensitiveCompare("CapsLock") == .orderedSame
            config.originalIsCapsLock = isCaps
            config.triggerCode = isCaps ? 79 : keyCode(forKeyName: host.hyperTrigger).map { UInt8($0) }
            config.mode = host.hyperMode == "synthetic" ? UInt32(ORBIT_HYPER_SYNTHETIC) : UInt32(ORBIT_HYPER_INTERNAL)
            switch host.hyperTap {
            case "original": config.tap = UInt32(ORBIT_TAP_ORIGINAL)
            case "none": config.tap = UInt32(ORBIT_TAP_NONE)
            case "leader": config.tap = UInt32(ORBIT_TAP_LEADER)
            default: config.tap = UInt32(ORBIT_TAP_ESCAPE)
            }
            config.tapTimeoutMs = host.hyperTimeoutMs
            config.boundCodes = Array(hyperActions.keys)
        }
        let newTap = HyperTap(config: config)
        newTap.onDispatch = { [weak self] code, t0 in self?.onHyper(code: code, t0: t0) }
        newTap.onExpand = { [weak self] id, chars in self?.onExpand(snippetID: id, triggerChars: chars) }
        newTap.onGaveUp = { [weak self] in
            self?.report("macOS keeps disabling Orbit's keyboard tap, so HyperKey is off. Direct shortcuts still work. Re-enable it from Settings.")
            self?.dropTap()
        }
        guard newTap.start() else {
            tapFailed = true
            if remap.hasJournal { remap.remove() }
            report("macOS refused the keyboard tap, so HyperKey and expansion are off. Direct shortcuts still work. Grant Accessibility and Input Monitoring in Settings → HyperKey.")
            return
        }
        tap = newTap
        if wantsCapsRemap { applyRemap() } else if remap.hasJournal { remap.remove() }
        if host.expansionEnabled { orbit_worker_snippet_triggers(worker) }
        frontmostChanged(NSWorkspace.shared.frontmostApplication)
    }

    private func dropTap() {
        tap?.stop()
        tap = nil
        tapFailed = true
        if remap.hasJournal { remap.remove() }
    }

    func setSuspended(_ value: Bool) {
        suspended = value
        hide(restoreFocus: true)
        if value {
            // Removed entirely, not merely told to pass events along.
            hotkeys.unregisterAll()
            shortcutActions.removeAll()
            applyHook()
            report("Orbit shortcuts are suspended. Resume from the menu bar icon.")
        } else {
            registerHotkeys()
            applyHook()
            report("Orbit shortcuts are back on.")
        }
    }

    // MARK: - Snippets, clipboard and text insertion

    private func dateAndTime() -> (String, String) {
        let formatter = DateFormatter()
        formatter.dateStyle = .short
        formatter.timeStyle = .none
        let date = formatter.string(from: Date())
        formatter.dateStyle = .none
        formatter.timeStyle = .short
        return (date, formatter.string(from: Date()))
    }

    private func expand(_ template: String) -> (text: String, cursorBack: Int?, usedClipboard: Bool) {
        let (date, time) = dateAndTime()
        var back: Int64 = -1
        var used = false
        let clipboard = orbit_snippet_uses_clipboard(template) ? activePasteboard.string(forType: .string) : nil
        let text = takeString(orbit_snippet_expand(template, date, time, clipboard, &back, &used)) ?? ""
        return (text, back >= 0 ? Int(back) : nil, used)
    }

    private func onSnippet(_ event: OrbitEvent) {
        let content = borrowed(event.text3)
        switch Int(event.purpose) {
        case ORBIT_PURPOSE_EDIT:
            SnippetEditor.show(worker: worker, fields: .init(id: event.id, name: borrowed(event.text), trigger: borrowed(event.text2), content: content, description: borrowed(event.text4), category: borrowed(event.text5), favourite: event.favourite))
        case ORBIT_PURPOSE_COPY:
            copyToClipboard(expand(content).text, message: nil)
        case ORBIT_PURPOSE_INSERT:
            let expanded = expand(content)
            startInsert(expanded.text, erase: 0, cursorBack: expanded.cursorBack, viaClipboard: false)
        case ORBIT_PURPOSE_EXPAND:
            let expanded = expand(content)
            if expanded.usedClipboard {
                // Never drop clipboard content into a window without the user seeing it.
                showClipboardPreview(name: borrowed(event.text), expanded: expanded, erase: Int(event.erase_chars))
            } else {
                startInsert(expanded.text, erase: Int(event.erase_chars), cursorBack: expanded.cursorBack, viaClipboard: false)
            }
        default:
            break
        }
    }

    private func showClipboardPreview(name: String, expanded: (text: String, cursorBack: Int?, usedClipboard: Bool), erase: Int) {
        let preview = String(expanded.text.prefix(120).map { $0.isNewline ? " " : $0 })
        show(.root, source: "preview", t0: ticks())
        pushView()
        view.scope = .actions(of: name)
        pendingExpansion = (expanded.text, erase, expanded.cursorBack)
        view.staticRows = [
            .op("Cancel", "", glyph: ORBIT_GLYPH_INFO, .back),
            .op("Insert with clipboard", "Press ↓ then Enter", glyph: ORBIT_GLYPH_CLIPBOARD, .insertPending),
            .info(preview, "This snippet includes your clipboard"),
        ]
        view.resetPosition()
        setPlaceholder()
        setQueryText("")
        refreshResults()
    }

    private func onExpand(snippetID: Int64, triggerChars: UInt64) {
        if suspended { return }
        orbit_worker_snippet_get(worker, snippetID, UInt32(ORBIT_PURPOSE_EXPAND), triggerChars)
    }

    private func onClipContent(kind: Int, text: String, blob: String, purpose: Int) {
        let insert = purpose == ORBIT_PURPOSE_INSERT
        switch kind {
        case ORBIT_CLIP_FILES:
            let pasteboard = activePasteboard
            pasteboard.clearContents()
            pasteboard.writeObjects(text.split(separator: "\n").map { NSURL(fileURLWithPath: String($0)) })
            clipWatcher.ownChangeCount = pasteboard.changeCount
            if insert { startPasteKeystroke() }
        case ORBIT_CLIP_IMAGE:
            guard let data = FileManager.default.contents(atPath: blob) else {
                report("That image is no longer available.")
                return
            }
            let pasteboard = activePasteboard
            pasteboard.clearContents()
            pasteboard.setData(data, forType: .png)
            clipWatcher.ownChangeCount = pasteboard.changeCount
            if insert { startPasteKeystroke() }
        default:
            if insert {
                startInsert(text, erase: 0, cursorBack: nil, viaClipboard: true)
            } else {
                copyToClipboard(text, message: nil)
            }
        }
    }

    private func copyToClipboard(_ text: String, message: String?) {
        clipWatcher.ownChangeCount = Inserter.writeClipboard(text)
        if let message { report(message) }
    }

    /// Gives the target app a moment to take the keyboard back before keys are sent to it.
    private static let focusSettleMs = 60

    private func startPasteKeystroke() {
        guard Inserter.canPostKeys else {
            report("It's on your clipboard. Press ⌘V to paste. Grant Accessibility to paste automatically.")
            return
        }
        DispatchQueue.global(qos: .userInteractive).asyncAfter(deadline: .now() + .milliseconds(AppController.focusSettleMs)) {
            Inserter.pasteKeystroke()
        }
    }

    /// Inserts text by keystrokes when Accessibility allows it. Otherwise the text goes on the
    /// clipboard and the user is told, so it is never silently lost.
    private func startInsert(_ text: String, erase: Int, cursorBack: Int?, viaClipboard: Bool) {
        if text.isEmpty { return }
        if viaClipboard {
            clipWatcher.ownChangeCount = Inserter.writeClipboard(text)
            startPasteKeystroke()
            return
        }
        // Nothing has been erased yet, so nothing is lost if the user doesn't paste.
        if text.utf16.count > Inserter.maxTypedUnits {
            clipWatcher.ownChangeCount = Inserter.writeClipboard(text)
            report("That snippet is too long to type, so it's on your clipboard. Press ⌘V to paste it.")
            return
        }
        guard Inserter.canPostKeys else {
            clipWatcher.ownChangeCount = Inserter.writeClipboard(text)
            let why = IsSecureEventInputEnabled() ? "A secure field is active" : "Orbit doesn't have Accessibility permission"
            report("\(why), so the text is on your clipboard. Press ⌘V to paste it.")
            return
        }
        DispatchQueue.global(qos: .userInteractive).asyncAfter(deadline: .now() + .milliseconds(AppController.focusSettleMs)) {
            Inserter.type(text, erase: erase, cursorBack: cursorBack)
        }
    }

    func setClipPaused(_ value: Bool) {
        clipPaused = value
        applyClipboardWatcher()
    }

    func applyClipboardWatcher() {
        if host.clipEnabled, !clipPaused {
            clipWatcher.start()
        } else {
            clipWatcher.stop()
        }
    }

    private func onCopy(_ capture: ClipCapture, pasteboard: NSPasteboard, source: String) {
        switch capture {
        case .text(let text):
            orbit_worker_clip_add_text(worker, text, source)
        case .files(let paths):
            orbit_worker_clip_add_files(worker, paths.joined(separator: "\n"), source)
        case .image:
            let limit = Int(host.clipMaxItemBytes)
            DispatchQueue.global(qos: .utility).async { [weak self] in
                guard let self, let image = autoreleasepool(invoking: { pasteboardImagePNG(pasteboard, maxBytes: limit) }) else { return }
                image.0.withUnsafeBytes { raw in
                    orbit_worker_clip_add_image(self.worker, raw.bindMemory(to: UInt8.self).baseAddress, image.0.count, UInt32(image.1), UInt32(image.2), source)
                }
            }
        case .skipConcealed, .skipExcluded, .skipTooLarge, .skipEmpty:
            break
        }
        // Outcome only, for the benchmark and self-checks. Never the body.
        if metrics.benchMode { metrics.recordLine("clip-decision,\(ClipCapture.label(capture))") }
    }

    // MARK: - Settings, indexing, and everything else

    func reloadHost() {
        if let fresh = core.host() { host = fresh }
    }

    private func openSettings() {
        if settingsWindow == nil {
            settingsWindow = SettingsWindow(controller: self)
        }
        settingsWindow?.present()
    }

    func settingsClosed() {
        settingsWindow = nil
        yieldFocusIfIdle(closing: NSWindow())
    }

    /// Applies a saved settings form in one place. Returns the problem, if any.
    func applySettings(form: SettingsForm) -> (page: Int, message: String)? {
        let old = host
        if let problem = core.apply(form: form) { return problem }
        reloadHost()
        view.visibleRows = host.maxVisibleRows
        registerHotkeys()
        let hookChanged = old.hyperEnabled != host.hyperEnabled || old.hyperTrigger != host.hyperTrigger || old.hyperMode != host.hyperMode
            || old.hyperTap != host.hyperTap || old.hyperTimeoutMs != host.hyperTimeoutMs || old.form.hyperTable != host.form.hyperTable
            || old.hyperExcluded != host.hyperExcluded || old.hyperPassGames != host.hyperPassGames
            || old.expansionEnabled != host.expansionEnabled || old.expansionExcluded != host.expansionExcluded
        if hookChanged { applyHook() }
        applyClipboardWatcher()
        if old.clipPersist != host.clipPersist { orbit_worker_set_clip_persist(worker, host.clipPersist) }
        if old.clipMaxItems != host.clipMaxItems || old.clipRetentionDays != host.clipRetentionDays {
            orbit_worker_set_clip_policy(worker, host.clipMaxItems, host.clipRetentionDays)
        }
        if old.launchAtLogin != host.launchAtLogin { setLaunchAtLogin(host.launchAtLogin) }
        if old.lowResourceMode != host.lowResourceMode {
            applyCacheLimits()
            if host.lowResourceMode {
                orbit_worker_shed(worker)
                malloc_zone_pressure_relief(nil, 0)
            }
        }
        applyTheme()
        return nil
    }

    private func applyCacheLimits() {
        icons.setCapacity(host.lowResourceMode || shedActive ? 16 : 64)
    }

    private func setLaunchAtLogin(_ enabled: Bool) {
        do {
            if enabled {
                try SMAppService.mainApp.register()
            } else {
                try SMAppService.mainApp.unregister()
            }
        } catch {
            report("macOS didn't change the login item: \(error.localizedDescription)")
        }
    }

    var loginItemStatus: String {
        switch SMAppService.mainApp.status {
        case .enabled: return "macOS has Orbit as a login item."
        case .requiresApproval: return "Waiting for your approval in System Settings → General → Login Items."
        case .notRegistered: return "Not a login item."
        case .notFound: return "macOS can't find this copy of Orbit. Run it from an app bundle."
        @unknown default: return "macOS reports an unknown login item state."
        }
    }

    /// Runs on a utility queue. It reads a few hundred small plist files and holds no result in
    /// memory beyond the short list.
    func startIndexer() {
        if indexing {
            indexDirty = true
            return
        }
        indexing = true
        orbit_engine_set_indexing(core.engine, true)
        let dir = options.dataDir
        indexQueue.async { [weak self] in
            let json: String = autoreleasepool {
                let apps = scanApplications()
                IconCache.pruneStale(directory: dir + "/icons", appPaths: apps.map(\.path))
                return encodeApps(apps)
            }
            _ = orbit_apps_save(dir, json)
            DispatchQueue.main.async { self?.onIndexDone(json) }
        }
    }

    private func onIndexDone(_ json: String) {
        indexing = false
        orbit_engine_set_indexing(core.engine, false)
        if orbit_engine_set_apps_json(core.engine, json) {
            orbit_worker_load_usage(worker)
            if visible { refreshResults() }
        }
        if metrics.benchMode { metrics.recordLine("index-done,\(ticks())") }
        if indexDirty {
            indexDirty = false
            startIndexer()
        }
    }

    /// Shows a message in the launcher footer, or a brief toast when the launcher is closed.
    func report(_ message: String) {
        if visible {
            status = message
            redraw()
        } else {
            toast.show(message)
        }
        if metrics.benchMode {
            // For the driver to confirm a failure was visible. Reports never quote user content.
            metrics.recordLine("report,\(message.replacingOccurrences(of: "\n", with: " "))")
        }
    }

    func diagnosticsRows() -> [Row] {
        let memory = processMemory()
        let (showP50, showP95) = metrics.showSummary
        let (queryP50, queryP95) = metrics.querySummary
        let stats = orbit_engine_index_stats(core.engine)
        let tapState: String
        if let tap {
            let maxUs = Double(tap.maxCallbackTicks) * tickToMs * 1000
            tapState = String(format: "Active. Slowest callback %.0f µs, %d re-enables", maxUs, tap.reEnables)
        } else if tapFailed {
            tapState = "Refused by macOS. See Settings → HyperKey"
        } else {
            tapState = "Not running: no feature needs it"
        }
        let clipState: String
        if !host.clipEnabled {
            clipState = "Off"
        } else if clipPaused {
            clipState = "Paused"
        } else {
            clipState = String(format: "Recording. Checks change count every %.2g s", clipWatcher.currentInterval)
        }
        let granted = { (value: Bool) in value ? "granted" : "not granted" }
        let source = launchedFromParent ? " (started from a terminal: macOS answers for that terminal, not Orbit)" : ""
        var rows: [Row] = [
            .info("Memory footprint", "\(formatBytes(memory.footprint)), task_info phys_footprint. Peak \(formatBytes(memory.peakFootprint))"),
            .info("Resident size", "\(formatBytes(memory.resident)), task_info resident_size. Includes shared system frameworks"),
            .info("Hotkey to focused input", metrics.showsTotal == 0 ? "No samples yet" : String(format: "p50 %.1f ms, p95 %.1f ms (last %d opens)", showP50, showP95, min(64, metrics.showsTotal))),
            .info("Keystroke to results drawn", metrics.queriesTotal == 0 ? "No samples yet" : String(format: "p50 %.2f ms, p95 %.2f ms", queryP50, queryP95)),
            .info("Search index", "\(stats.items) items, \(stats.apps) apps, \(formatBytes(stats.heap_bytes)). \(indexing ? "Indexing now" : "Up to date")"),
            .info("Icon cache", "\(icons.count) of \(icons.capacity) icons, \(formatBytes(UInt64(icons.bytes)))"),
            .info("Keyboard tap", tapState),
            .info("Permissions", "Accessibility \(granted(Permissions.accessibility)), Input Monitoring \(granted(Permissions.inputMonitoring))\(source)"),
            .info("Launcher shortcut", activeHotkey.isEmpty ? "None" : prettyChord(activeHotkey)),
            .info("Clipboard history", clipState),
            .info("Low Resource Mode", host.lowResourceMode ? "On" : shedActive ? "Off, caches shed: system memory is tight" : "Off"),
        ]
        if let counts = lastCounts {
            rows.append(.info("Stored", "\(counts.notes) notes, \(counts.snippets) snippets, \(counts.clips) clips. SQLite is holding \(formatBytes(UInt64(max(0, counts.sqlite))))"))
        }
        if let remapNotice { rows.append(.info("Caps Lock", remapNotice)) }
        for problem in hotkeyProblems { rows.append(.info("Shortcut problem", problem)) }
        return rows
    }

    // MARK: - Menu bar

    private func buildStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        item.button?.image = orbitMenuBarImage()
        item.button?.image?.isTemplate = true
        item.button?.toolTip = "Orbit"
        let menu = NSMenu()
        menu.delegate = self
        menu.autoenablesItems = false
        item.menu = menu
        statusItem = item
        updateStatusItemHint()
    }

    private func updateStatusItemHint() {
        statusItem?.button?.toolTip = activeHotkey.isEmpty ? "Orbit: no shortcut registered" : "Orbit: \(prettyChord(activeHotkey))"
    }

    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        func add(_ title: String, _ selector: Selector?, enabled: Bool = true) {
            let item = NSMenuItem(title: title, action: selector, keyEquivalent: "")
            item.target = self
            item.isEnabled = enabled
            menu.addItem(item)
        }
        if let problem = hotkeyProblems.first ?? remapNotice {
            add(String(problem.prefix(70)), nil, enabled: false)
            menu.addItem(.separator())
        }
        add(activeHotkey.isEmpty ? "Open Orbit" : "Open Orbit  \(prettyChord(activeHotkey))", #selector(menuOpen))
        add("Settings…", #selector(menuSettings))
        menu.addItem(.separator())
        add(clipPaused ? "Resume Clipboard History" : "Pause Clipboard History", host.clipEnabled ? #selector(menuPause) : nil, enabled: host.clipEnabled)
        add(suspended ? "Resume Shortcuts" : "Suspend Shortcuts", #selector(menuSuspend))
        menu.addItem(.separator())
        add("Quit Orbit", #selector(menuQuit))
    }

    @objc private func menuOpen() { show(.root, source: "menu", t0: ticks()) }
    @objc private func menuSettings() { performBuiltin(.settings) }
    @objc private func menuPause() { setClipPaused(!clipPaused) }
    @objc private func menuSuspend() { setSuspended(!suspended) }
    @objc private func menuQuit() { beginExit() }

    // MARK: - Exit

    private func restart() {
        let bundle = Bundle.main.bundlePath
        if bundle.hasSuffix(".app") {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/bin/open")
            process.arguments = ["-n", bundle, "--args", "--wait-pid", String(getpid()), "--data-dir", options.dataDir]
            try? process.run()
        } else if let exe = Bundle.main.executablePath {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: exe)
            process.arguments = ["--wait-pid", String(getpid()), "--data-dir", options.dataDir]
            try? process.run()
        }
        beginExit()
    }

    /// Removes what Orbit stored, then quits. The worker does the removal after closing the
    /// databases, and its reply ends the process, as with any other exit.
    private func deleteLocalData() {
        if SMAppService.mainApp.status == .enabled { setLaunchAtLogin(false) }
        orbit_engine_set_read_only(core.engine)
        deleteOnExit = true
        beginExit()
    }

    /// Clean exit. Editors flush first, the keyboard goes back to normal, then the data thread
    /// drains and checkpoints. A deadline guarantees exit if anything stalls.
    func beginExit() {
        if exiting { return }
        exiting = true
        hide(restoreFocus: false)
        NoteEditor.flushAll()
        tap?.stop()
        tap = nil
        remap.removeAndWait(timeoutSeconds: 1)
        hotkeys.unregisterAll()
        clipWatcher.stop()
        appWatcher?.stop()
        if let statusItem { NSStatusBar.system.removeStatusItem(statusItem) }
        metrics.flush()
        let fallback = DispatchWorkItem { [weak self] in self?.finishExit() }
        exitFallback = fallback
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5, execute: fallback)
        if deleteOnExit {
            orbit_worker_delete_local_data(worker)
        } else {
            orbit_worker_shutdown(worker)
        }
    }

    private func finishExit() {
        metrics.flush()
        exit(0)
    }

    func applicationWillTerminate(_ notification: Notification) {
        // Reached if the system ends the app without going through beginExit.
        if !exiting {
            NoteEditor.flushAll()
            tap?.stop()
            remap.removeAndWait(timeoutSeconds: 1)
        }
    }

    // MARK: - Benchmark entry points

    func recordMemoryLine() {
        let memory = processMemory()
        metrics.recordLine("memory,\(memory.footprint),\(memory.resident),\(memory.peakFootprint)")
    }

    func benchSettingsShot() {
        openSettings()
        settingsWindow?.benchSnapshots(prefix: options.dataDir + "/settings-")
    }

    func benchSettingsSave() {
        settingsWindow?.benchSave()
    }

    func benchToggle(t0: Int64) { toggle(.root, source: "bench", t0: t0) }
    func benchEscape() { if visible { goBackOrHide() } }

    func benchType(_ character: UInt32) {
        guard visible, let scalar = Unicode.Scalar(character) else { return }
        (field.currentEditor() as? NSTextView)?.insertText(String(Character(scalar)), replacementRange: NSRange(location: NSNotFound, length: 0))
    }

    func benchEnter() { if visible { activateSelected(secondary: false, tEnter: ticks()) } }

    func benchDown() {
        if visible {
            view.moveSelection(1)
            afterSelectionChange()
            redraw()
        }
    }

    /// Trashes the selected note through its action menu, as the keyboard path does.
    func benchTrashSelected() {
        openActionMenu()
        for row in view.rows {
            if case .noteTrash? = row.primary {
                execute(row.primary!)
                return
            }
        }
    }

    var isLauncherVisible: Bool { visible }

    /// The panel as the window server composites it, including the field's own layer. A process may
    /// read its own window with no Screen Recording grant. The call is obsolete in the SDK, hence dlsym.
    func benchWindowShot(path: String) {
        typealias Capture = @convention(c) (CGRect, UInt32, UInt32, UInt32) -> Unmanaged<CGImage>?
        guard let symbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "CGWindowListCreateImage") else { return }
        let capture = unsafeBitCast(symbol, to: Capture.self)
        // kCGWindowListOptionIncludingWindow, kCGWindowImageDefault
        guard let image = capture(.null, 1 << 3, UInt32(panel.windowNumber), 0)?.takeRetainedValue() else { return }
        if let data = NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:]) {
            FileManager.default.createFile(atPath: path, contents: data)
        }
    }

    func benchSnapshot(path: String) {
        guard let rep = results.bitmapImageRepForCachingDisplay(in: results.bounds) else { return }
        results.cacheDisplay(in: results.bounds, to: rep)
        if let data = rep.representation(using: .png, properties: [:]) {
            FileManager.default.createFile(atPath: path, contents: data)
        }
    }

    func benchState() -> String {
        let key = panel.isKeyWindow
        let responder = field.currentEditor() != nil
        return "state,visible=\(visible),key=\(key),editor=\(responder),hotkey=\(activeHotkey),query=\(query.utf8.count),rows=\(view.rows.count),selected=\(view.selected),frame=\(Int(panel.frame.minX))x\(Int(panel.frame.minY))+\(Int(panel.frame.width))x\(Int(panel.frame.height)),tap=\(tap != nil),tapFailed=\(tapFailed),problems=\(hotkeyProblems.count),ax=\(Permissions.accessibility),im=\(Permissions.inputMonitoring),clip=\(clipWatcher.isRunning),firstProblem=\(hotkeyProblems.first ?? ""),scope=\(view.scope.label),firstKind=\(view.rows.first?.kind ?? ""),firstSubtitle=\((view.rows.first?.subtitle ?? "").replacingOccurrences(of: ",", with: " ")),firstTitle=\((view.rows.first?.title ?? "").replacingOccurrences(of: ",", with: " ")),boardLength=\(options.pasteboardName == nil ? -1 : activePasteboard.string(forType: .string)?.utf16.count ?? 0)"
    }
}

extension HostSettings {
    /// Pinned state isn't in the host view, because search already carries it on each row. The
    /// menu only needs it for one label, so it asks for the suggestion list's verdict.
    func pinnedContains(_ key: String, core: Core) -> Bool {
        let result = orbit_engine_query_root(core.engine, "")
        guard let rows = result.rows else { return false }
        for index in 0..<result.count where borrowed(rows[index].key) == key {
            return rows[index].pinned
        }
        return false
    }
}

extension ClipCapture {
    static func label(_ capture: ClipCapture) -> String {
        switch capture {
        case .skipConcealed: return "skip-concealed"
        case .skipExcluded: return "skip-excluded"
        case .skipTooLarge: return "skip-too-large"
        case .skipEmpty: return "skip-empty"
        case .text: return "text"
        case .files: return "files"
        case .image: return "image"
        }
    }
}

func orbitMenuBarImage() -> NSImage {
    NSImage(size: NSSize(width: 18, height: 18), flipped: false) { rect in
        NSColor.black.setStroke()
        let ring = NSBezierPath(ovalIn: rect.insetBy(dx: 3.5, dy: 3.5))
        ring.lineWidth = 2
        ring.stroke()
        NSColor.black.setFill()
        NSBezierPath(ovalIn: NSRect(x: 11.5, y: 11.5, width: 5, height: 5)).fill()
        return true
    }
}

/// A brief message when the launcher is closed. Needs no notification permission, never takes
/// focus, and removes itself.
final class Toast {
    private var panel: NSPanel?
    private var label: NSTextField?
    private var dismiss: DispatchWorkItem?

    func show(_ message: String) {
        let panel = self.panel ?? makePanel()
        self.panel = panel
        guard let label, let screen = NSScreen.screens.first(where: { NSMouseInRect(NSEvent.mouseLocation, $0.frame, false) }) ?? NSScreen.main else { return }
        label.stringValue = message
        let width: CGFloat = 460
        let fitted = label.sizeThatFits(NSSize(width: width - 28, height: 400))
        let height = fitted.height + 20
        let area = screen.visibleFrame
        panel.setFrame(NSRect(x: area.midX - width / 2, y: area.maxY - height - 24, width: width, height: height), display: false)
        label.frame = NSRect(x: 14, y: 10, width: width - 28, height: fitted.height)
        panel.orderFrontRegardless()
        dismiss?.cancel()
        let work = DispatchWorkItem { [weak self] in
            self?.panel?.orderOut(nil)
            // Freed rather than kept: most sessions show no toast at all.
            self?.panel = nil
            self?.label = nil
        }
        dismiss = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 5, execute: work)
    }

    private func makePanel() -> NSPanel {
        let panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 460, height: 60), styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
        panel.level = .statusBar
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.ignoresMouseEvents = true
        panel.isReleasedWhenClosed = false
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient]
        let content = NSVisualEffectView()
        content.material = .hudWindow
        content.state = .active
        content.wantsLayer = true
        content.layer?.cornerRadius = 10
        content.layer?.masksToBounds = true
        panel.contentView = content
        let text = NSTextField(wrappingLabelWithString: "")
        text.font = NSFont.systemFont(ofSize: 13)
        content.addSubview(text)
        label = text
        return panel
    }
}

/// System commands. Only ever reached after the launcher's confirmation row for the destructive
/// ones. None of these are exercised by any automated test.
enum SystemCommands {
    private static func run(_ tool: String, _ arguments: [String]) {
        DispatchQueue.global(qos: .userInitiated).async {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: tool)
            process.arguments = arguments
            try? process.run()
        }
    }

    /// Asks loginwindow, which shows Apple's own confirmation as well.
    private static func loginwindow(_ event: String) {
        run("/usr/bin/osascript", ["-e", "tell application \"loginwindow\" to «event aevt\(event)»"])
    }

    static func lock() {
        // There is no public API to lock. login.framework is what the system menu uses, and if it
        // ever goes away the fallback sleeps the display, which locks if the owner requires that.
        typealias Lock = @convention(c) () -> Void
        let path = "/System/Library/PrivateFrameworks/login.framework/Versions/Current/login"
        if let handle = dlopen(path, RTLD_LAZY), let symbol = dlsym(handle, "SACLockScreenImmediate") {
            unsafeBitCast(symbol, to: Lock.self)()
            return
        }
        run("/usr/bin/pmset", ["displaysleepnow"])
    }

    static func sleep() { run("/usr/bin/pmset", ["sleepnow"]) }
    static func shutDown() { loginwindow("shut") }
    static func restart() { loginwindow("rrst") }

    /// Finder does the emptying. macOS asks the owner once for permission to automate Finder.
    static func emptyTrash(report: @escaping (String) -> Void) {
        DispatchQueue.global(qos: .userInitiated).async {
            var error: NSDictionary?
            NSAppleScript(source: "tell application \"Finder\" to empty the trash")?.executeAndReturnError(&error)
            if error != nil {
                DispatchQueue.main.async { report("Couldn't empty the trash. Allow Orbit to control Finder in System Settings → Privacy & Security → Automation.") }
            }
        }
    }
}
