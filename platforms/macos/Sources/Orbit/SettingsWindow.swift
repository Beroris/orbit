// The settings window, with the same pages as platforms/windows/src/settings_win.rs. Created when
// opened and freed when closed. On Save it hands one complete form to the controller, and the core
// validates it, so this file holds layout and no rules.

import AppKit
import COrbit

private let triggers = ["CapsLock", "F13", "F14", "F15", "F16", "F17", "F18", "F19", "F20", "RightCtrl", "RightAlt"]
private let modes: [(String, String)] = [
    ("Orbit handles it (recommended)", "internal"),
    ("Send ⌃⌥⇧⌘ to other apps too", "synthetic"),
    ("Another remapper produces the chord (no permission needed)", "external"),
]
private let taps: [(String, String)] = [
    ("Tap sends Escape", "escape"),
    ("Tap keeps the key's normal behaviour", "original"),
    ("Tap does nothing", "none"),
    ("Leader: tap it, then press a mapped key, with nothing held", "leader"),
]
private let appearances: [(String, String)] = [("Match macOS", "system"), ("Light", "light"), ("Dark", "dark")]

private final class PageBuilder {
    let stack = NSStackView()
    let width: CGFloat = 620

    init() {
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 8
        stack.edgeInsets = NSEdgeInsets(top: 12, left: 12, bottom: 12, right: 12)
    }

    @discardableResult
    func label(_ text: String, secondary: Bool = false) -> NSTextField {
        let field = NSTextField(wrappingLabelWithString: text)
        field.preferredMaxLayoutWidth = width
        if secondary { field.textColor = .secondaryLabelColor }
        stack.addArrangedSubview(field)
        return field
    }

    func check(_ title: String, _ value: Bool) -> NSButton {
        let button = NSButton(checkboxWithTitle: title, target: nil, action: nil)
        button.state = value ? .on : .off
        stack.addArrangedSubview(button)
        return button
    }

    func edit(_ value: String, width fieldWidth: CGFloat = 220) -> NSTextField {
        let field = NSTextField(string: value)
        field.widthAnchor.constraint(equalToConstant: fieldWidth).isActive = true
        stack.addArrangedSubview(field)
        return field
    }

    func popup(_ titles: [String], selected: Int) -> NSPopUpButton {
        let button = NSPopUpButton()
        button.addItems(withTitles: titles)
        button.selectItem(at: max(0, selected))
        stack.addArrangedSubview(button)
        return button
    }

    func area(_ value: String, height: CGFloat) -> NSTextView {
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder
        let text = NSTextView()
        text.isRichText = false
        text.allowsUndo = true
        text.isAutomaticQuoteSubstitutionEnabled = false
        text.isAutomaticDashSubstitutionEnabled = false
        text.font = NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)
        text.string = value
        text.isVerticallyResizable = true
        text.textContainer?.widthTracksTextView = true
        text.autoresizingMask = [.width]
        scroll.documentView = text
        scroll.widthAnchor.constraint(equalToConstant: width).isActive = true
        scroll.heightAnchor.constraint(equalToConstant: height).isActive = true
        stack.addArrangedSubview(scroll)
        return text
    }

    @discardableResult
    func button(_ title: String, _ target: AnyObject, _ action: Selector) -> NSButton {
        let button = NSButton(title: title, target: target, action: action)
        stack.addArrangedSubview(button)
        return button
    }
}

final class SettingsWindow: NSObject, NSWindowDelegate {
    private unowned let controller: AppController
    private let window: NSWindow
    private let tabs = NSTabView()
    private let statusLabel = NSTextField(wrappingLabelWithString: "")
    private var keyMonitor: Any?

    private var hotkey: NSTextField!
    private var login, focusRunning, preserve, lowResource: NSButton!
    private var appearance: NSPopUpButton!
    private var shortcuts: NSTextView!
    private var hyperOn, hyperFullscreen: NSButton!
    private var hyperTrigger, hyperMode, hyperTap: NSPopUpButton!
    private var hyperTimeout: NSTextField!
    private var hyperTable, hyperExcluded: NSTextView!
    private var permissionLabel: NSTextField!
    private var clipOn, clipPersist, clipImages: NSButton!
    private var clipMax, clipDays: NSTextField!
    private var clipExcluded: NSTextView!
    private var expansion: NSButton!
    private var snippetExcluded: NSTextView!
    private var links, commands: NSTextView!

    init(controller: AppController) {
        self.controller = controller
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 700, height: 660), styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: true)
        window.title = "Orbit Settings"
        window.isReleasedWhenClosed = false
        super.init()
        window.delegate = self
        build()
    }

    private func build() {
        let form = controller.host.form
        let add = { (title: String, page: PageBuilder) in
            let item = NSTabViewItem()
            item.label = title
            item.view = page.stack
            self.tabs.addTabViewItem(item)
        }

        let general = PageBuilder()
        if !controller.hotkeyProblems.isEmpty {
            let problem = general.label(controller.hotkeyProblems.joined(separator: " "))
            problem.textColor = .systemOrange
        }
        general.label("Launcher shortcut. Click the box and press a shortcut, or type one. Leave it empty for the default.")
        hotkey = general.edit(form.hotkey)
        general.label("In use now: \(controller.activeHotkey.isEmpty ? "none" : prettyChord(controller.activeHotkey))", secondary: true)
        login = general.check("Start Orbit when I log in", form.launchAtLogin)
        general.label(controller.loginItemStatus, secondary: true)
        focusRunning = general.check("Enter on a running app switches to it", form.focusRunningApp)
        preserve = general.check("Keep my last search when reopening", form.preserveQuery)
        lowResource = general.check("Low Resource Mode: smaller caches, no clipboard images, memory returned to macOS sooner", form.lowResourceMode)
        general.label("Appearance")
        appearance = general.popup(appearances.map(\.0), selected: appearances.firstIndex { $0.1 == form.appearance } ?? 0)
        add("General", general)

        let shortcutPage = PageBuilder()
        shortcutPage.label("Direct global shortcuts. These use RegisterEventHotKey, so they need no permission.")
        shortcutPage.label("One per line, as shortcut = action. For example: Ctrl+Alt+K = cmd:clipboard")
        shortcuts = shortcutPage.area(form.shortcuts, height: 340)
        shortcutPage.label("Actions: app:Safari, url:https://example.com, path:~/file.txt, cmd:clipboard, cmd:new-note, cmd:snippets, cmd:notes, cmd:launcher, custom:id. Start a line with # to turn it off.", secondary: true)
        add("Shortcuts", shortcutPage)

        let hyper = PageBuilder()
        hyperOn = hyper.check("Turn on HyperKey", form.hyperEnabled)
        hyper.label("Holding the key below plus another key runs a mapping. Unless another remapper makes the chord, this uses a keyboard tap, which sees key events only to detect these chords. It stores and logs nothing, and it stops at the lock screen and in secure fields. Quit or Suspend restores the keyboard at once.", secondary: true)
        permissionLabel = hyper.label("")
        let permissionButtons = NSStackView(views: [
            NSButton(title: "Open Accessibility Settings", target: self, action: #selector(openAccessibility)),
            NSButton(title: "Open Input Monitoring Settings", target: self, action: #selector(openInputMonitoring)),
        ])
        hyper.stack.addArrangedSubview(permissionButtons)
        hyper.label("Key to use. Caps Lock is remapped to F18 with hidutil while Orbit runs, and restored on quit. If another tool already remaps it, Orbit leaves it alone and says so.", secondary: true)
        hyperTrigger = hyper.popup(triggers, selected: triggers.firstIndex { $0.caseInsensitiveCompare(form.hyperTrigger) == .orderedSame } ?? 0)
        hyper.label("How it works")
        hyperMode = hyper.popup(modes.map(\.0), selected: modes.firstIndex { $0.1 == form.hyperMode } ?? 0)
        hyper.label("When tapped on its own")
        hyperTap = hyper.popup(taps.map(\.0), selected: taps.firstIndex { $0.1 == form.hyperTap } ?? 0)
        hyper.label("Tap timeout, in milliseconds")
        hyperTimeout = hyper.edit(form.hyperTimeout, width: 80)
        hyperFullscreen = hyper.check("Pass the key through while a fullscreen app is in front", form.hyperPassGames)
        hyper.label("Mappings, one per line, as key = action")
        hyperTable = hyper.area(form.hyperTable, height: 80)
        hyper.button("Add suggested mappings", self, #selector(addSuggested))
        hyper.label("Apps where the key keeps its normal behaviour. One app name or bundle ID per line")
        hyperExcluded = hyper.area(form.hyperExcluded, height: 36)
        add("HyperKey", hyper)

        let clipboard = PageBuilder()
        clipOn = clipboard.check("Keep clipboard history", form.clipEnabled)
        clipboard.label("History stays on this Mac. Copies marked concealed or transient and the apps listed below are skipped. This is a best effort: an app that doesn't mark its copies can't be recognised, so add it below. macOS has no copy notification, so Orbit checks a counter about once a second while this is on, and not at all when it's off or paused.", secondary: true)
        clipPersist = clipboard.check("Keep history after Orbit exits (off keeps it only in memory)", form.clipPersist)
        clipImages = clipboard.check("Include images", form.clipImages)
        clipboard.label("Items to keep")
        clipMax = clipboard.edit(form.clipMax, width: 100)
        clipboard.label("Days to keep, or 0 to keep items regardless of age")
        clipDays = clipboard.edit(form.clipDays, width: 100)
        clipboard.label("Never record copies from these apps. One app name or bundle ID per line")
        clipExcluded = clipboard.area(form.clipExcluded, height: 170)
        clipboard.button("Clear history", self, #selector(clearClipboard))
        add("Clipboard", clipboard)

        let snippets = PageBuilder()
        expansion = snippets.check("Expand snippet triggers as I type", form.expansion)
        snippets.label("This uses a keyboard tap that keeps only the last few characters in memory, to match triggers. Nothing is stored or logged. Secure fields and the apps below are skipped. Snippets with {clipboard} always ask before inserting.", secondary: true)
        snippets.label("Don't expand in these apps. One app name or bundle ID per line")
        snippetExcluded = snippets.area(form.snippetExcluded, height: 340)
        add("Snippets", snippets)

        let linksPage = PageBuilder()
        linksPage.label("Quick links, as name | alias | url. Use {query} for a search template: with alias g, typing “g cats” searches for cats.")
        links = linksPage.area(form.links, height: 210)
        linksPage.label("Commands, as name | alias | program | arguments. They run only when you choose them, without a shell. Use the program's full path.")
        commands = linksPage.area(form.commands, height: 210)
        add("Links & commands", linksPage)

        let data = PageBuilder()
        data.label("Everything Orbit stores lives in this folder:")
        data.label(controller.options.dataDir, secondary: true)
        data.button("Open data folder", self, #selector(openDataFolder))
        data.label("Export writes notes, snippets and settings to one JSON file. Clipboard history isn't exported.")
        data.button("Export now", self, #selector(exportNow))
        data.label("Import adds notes and snippets from an export. Your settings aren't changed, and duplicates are skipped.")
        data.button("Import a file…", self, #selector(importFile))
        data.button("Rebuild app index", self, #selector(rebuildIndex))
        data.button("Show diagnostics", self, #selector(showDiagnostics))
        data.button("Reset all shortcuts…", self, #selector(resetShortcuts))
        data.button("Delete all Orbit data…", self, #selector(deleteData))
        add("Data", data)

        let save = NSButton(title: "Save", target: self, action: #selector(saveClicked))
        save.keyEquivalent = "\r"
        let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancelClicked))
        cancel.keyEquivalent = "\u{1b}"
        statusLabel.textColor = .systemOrange
        statusLabel.preferredMaxLayoutWidth = 460
        let bottom = NSStackView(views: [statusLabel, NSView(), cancel, save])
        bottom.translatesAutoresizingMaskIntoConstraints = false
        tabs.translatesAutoresizingMaskIntoConstraints = false
        let host = window.contentView!
        host.addSubview(tabs)
        host.addSubview(bottom)
        NSLayoutConstraint.activate([
            tabs.topAnchor.constraint(equalTo: host.topAnchor, constant: 8),
            tabs.leadingAnchor.constraint(equalTo: host.leadingAnchor, constant: 8),
            tabs.trailingAnchor.constraint(equalTo: host.trailingAnchor, constant: -8),
            tabs.bottomAnchor.constraint(equalTo: bottom.topAnchor, constant: -8),
            bottom.leadingAnchor.constraint(equalTo: host.leadingAnchor, constant: 14),
            bottom.trailingAnchor.constraint(equalTo: host.trailingAnchor, constant: -14),
            bottom.bottomAnchor.constraint(equalTo: host.bottomAnchor, constant: -12),
        ])
        refreshPermissions()
    }

    private func refreshPermissions() {
        let ax = Permissions.accessibility ? "granted" : "not granted"
        let im = Permissions.inputMonitoring ? "granted" : "not granted"
        var text = "Accessibility: \(ax). Input Monitoring: \(im)."
        if launchedFromParent { text += " Orbit was started from a terminal, so macOS is answering for that terminal rather than Orbit." }
        if !Permissions.accessibility || !Permissions.inputMonitoring {
            text += " After granting, quit and reopen Orbit. Ad-hoc signed builds need granting again after each rebuild. HyperKey's “Another remapper” mode and all direct shortcuts work without either."
        }
        permissionLabel.stringValue = text
    }

    func present() {
        window.center()
        NSApp.unhide(nil)
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
        // Pressing a chord in the shortcut box fills it in. Plain typing also works. Installed only
        // while this window exists, and it sees only keys sent to this app.
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            self?.recordChord(event) == true ? nil : event
        }
    }

    func windowDidBecomeKey(_ notification: Notification) {
        refreshPermissions()
    }

    private func recordChord(_ event: NSEvent) -> Bool {
        guard window.firstResponder === hotkey.currentEditor(), hotkey.currentEditor() != nil else { return false }
        let flags = event.modifierFlags
        var mods = ""
        if flags.contains(.control) { mods += "Ctrl+" }
        if flags.contains(.option) { mods += "Alt+" }
        if flags.contains(.shift) { mods += "Shift+" }
        if flags.contains(.command) { mods += "Cmd+" }
        let name = keyName(event)
        let isFunction = name?.hasPrefix("F") == true && (name?.count ?? 0) > 1
        let hasRealModifier = flags.contains(.control) || flags.contains(.option) || flags.contains(.command)
        guard hasRealModifier || isFunction, let name else { return false }
        hotkey.stringValue = mods + name
        hotkey.currentEditor()?.selectedRange = NSRange(location: hotkey.stringValue.count, length: 0)
        return true
    }

    private func keyName(_ event: NSEvent) -> String? {
        let named: [UInt16: String] = [49: "Space", 36: "Enter", 48: "Tab", 53: "Escape", 51: "Backspace", 117: "Delete", 115: "Home", 119: "End", 116: "PageUp", 121: "PageDown", 126: "Up", 125: "Down", 123: "Left", 124: "Right", 122: "F1", 120: "F2", 99: "F3", 118: "F4", 96: "F5", 97: "F6", 98: "F7", 100: "F8", 101: "F9", 109: "F10", 103: "F11", 111: "F12", 105: "F13", 107: "F14", 113: "F15", 106: "F16", 64: "F17", 79: "F18", 80: "F19", 90: "F20"]
        if let name = named[event.keyCode] { return name }
        guard let text = event.charactersIgnoringModifiers?.uppercased(), text.count == 1 else { return nil }
        return text
    }

    private func collect() -> SettingsForm {
        var form = SettingsForm()
        form.hotkey = hotkey.stringValue
        form.launchAtLogin = login.state == .on
        form.focusRunningApp = focusRunning.state == .on
        form.preserveQuery = preserve.state == .on
        form.lowResourceMode = lowResource.state == .on
        form.appearance = appearances[appearance.indexOfSelectedItem].1
        form.shortcuts = shortcuts.string
        form.hyperEnabled = hyperOn.state == .on
        form.hyperTrigger = triggers[hyperTrigger.indexOfSelectedItem]
        form.hyperMode = modes[hyperMode.indexOfSelectedItem].1
        form.hyperTap = taps[hyperTap.indexOfSelectedItem].1
        form.hyperTimeout = hyperTimeout.stringValue
        form.hyperPassGames = hyperFullscreen.state == .on
        form.hyperTable = hyperTable.string
        form.hyperExcluded = hyperExcluded.string
        form.clipEnabled = clipOn.state == .on
        form.clipPersist = clipPersist.state == .on
        form.clipImages = clipImages.state == .on
        form.clipMax = clipMax.stringValue
        form.clipDays = clipDays.stringValue
        form.clipExcluded = clipExcluded.string
        form.expansion = expansion.state == .on
        form.snippetExcluded = snippetExcluded.string
        form.links = links.string
        form.commands = commands.string
        return form
    }

    @objc private func saveClicked() {
        if let problem = controller.applySettings(form: collect()) {
            tabs.selectTabViewItem(at: min(problem.page, tabs.numberOfTabViewItems - 1))
            statusLabel.stringValue = problem.message
            return
        }
        window.close()
    }

    @objc private func cancelClicked() { window.close() }
    @objc private func openAccessibility() { Permissions.open(pane: Permissions.accessibilityPane) }
    @objc private func openInputMonitoring() { Permissions.open(pane: Permissions.inputMonitoringPane) }

    @objc private func addSuggested() {
        let suggested = "Space = cmd:launcher\nC = cmd:clipboard\nN = cmd:new-note\nS = cmd:snippets\nF = cmd:notes\n"
        let existing = hyperTable.string.trimmingCharacters(in: .whitespacesAndNewlines)
        hyperTable.string = existing.isEmpty ? suggested : existing + "\n" + suggested
    }

    @objc private func clearClipboard() {
        window.close()
        controller.runBuiltin(.clearClipboard)
    }

    @objc private func openDataFolder() { NSWorkspace.shared.open(URL(fileURLWithPath: controller.options.dataDir)) }

    @objc private func exportNow() {
        controller.exportData()
        statusLabel.stringValue = "Exporting. Orbit will open the folder when it's ready."
    }

    @objc private func importFile() {
        controller.importData()
        statusLabel.stringValue = "Importing. Orbit will report the result."
    }

    @objc private func rebuildIndex() { controller.performBuiltin(.rebuildAppIndex) }

    @objc private func resetShortcuts() {
        window.close()
        controller.runBuiltin(.resetShortcuts)
    }

    @objc private func deleteData() {
        window.close()
        controller.runBuiltin(.deleteLocalData)
    }

    @objc private func showDiagnostics() {
        window.close()
        controller.performBuiltin(.diagnostics)
    }

    /// Renders every page to a PNG, for checking layout without Screen Recording.
    func benchSnapshots(prefix: String) {
        for index in 0..<tabs.numberOfTabViewItems {
            tabs.selectTabViewItem(at: index)
            window.contentView?.layoutSubtreeIfNeeded()
            guard let content = window.contentView, let rep = content.bitmapImageRepForCachingDisplay(in: content.bounds) else { continue }
            NSGraphicsContext.saveGraphicsState()
            NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
            NSColor.windowBackgroundColor.setFill()
            content.bounds.fill()
            NSGraphicsContext.restoreGraphicsState()
            // The window's background isn't part of the view, and light text on nothing can't be read.
            content.displayIgnoringOpacity(content.bounds, in: NSGraphicsContext(bitmapImageRep: rep)!)
            if let data = rep.representation(using: .png, properties: [:]) {
                FileManager.default.createFile(atPath: "\(prefix)\(index).png", contents: data)
            }
        }
    }

    func benchSave() {
        saveClicked()
    }

    func windowWillClose(_ notification: Notification) {
        if let keyMonitor { NSEvent.removeMonitor(keyMonitor) }
        keyMonitor = nil
        DispatchQueue.main.async { [controller] in controller.settingsClosed() }
    }
}
