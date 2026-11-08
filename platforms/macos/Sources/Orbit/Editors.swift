// The notes editor and the snippet editor. Ordinary windows, created when opened and freed when
// closed, so they cost nothing while no one is using them.

import AppKit
import COrbit

/// Accessory apps have no visible menu bar, but ⌘C, ⌘V and ⌘Z only work through a main menu.
func installMainMenu() {
    let main = NSMenu()
    let app = NSMenuItem()
    main.addItem(app)
    let appMenu = NSMenu()
    appMenu.addItem(withTitle: "Close Window", action: #selector(NSWindow.performClose(_:)), keyEquivalent: "w")
    app.submenu = appMenu
    let edit = NSMenuItem()
    main.addItem(edit)
    let editMenu = NSMenu(title: "Edit")
    editMenu.addItem(withTitle: "Undo", action: Selector(("undo:")), keyEquivalent: "z")
    let redo = editMenu.addItem(withTitle: "Redo", action: Selector(("redo:")), keyEquivalent: "z")
    redo.keyEquivalentModifierMask = [.command, .shift]
    editMenu.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
    editMenu.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
    editMenu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
    editMenu.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
    edit.submenu = editMenu
    NSApp.mainMenu = main
}

/// Hands focus back to whatever was in front, once Orbit has no more windows open.
func yieldFocusIfIdle(closing: NSWindow) {
    let others = NSApp.windows.contains { $0 !== closing && $0.isVisible && !($0 is NSPanel) && $0.canBecomeMain }
    if !others { NSApp.hide(nil) }
}

private final class NoteTextView: NSTextView {
    /// Esc dismisses, as in the launcher: "open launcher, type note, dismiss".
    override func cancelOperation(_ sender: Any?) {
        if hasMarkedText() {
            super.cancelOperation(sender)
        } else {
            window?.performClose(sender)
        }
    }
}

final class NoteEditor: NSObject, NSWindowDelegate, NSTextViewDelegate {
    private static var open: [UInt64: NoteEditor] = [:]
    private static var nextToken: UInt64 = 1

    private let window: NSWindow
    private let textView: NSTextView
    private let worker: OpaquePointer
    let token: UInt64
    private var noteID: Int64
    private var saveTimer: Timer?
    private var dirty = false

    /// Autosave waits for a pause in typing, so a burst of keys is one write.
    private static let debounce: TimeInterval = 0.5

    private init(worker: OpaquePointer, id: Int64?, body: String) {
        self.worker = worker
        noteID = id ?? -1
        token = NoteEditor.nextToken
        NoteEditor.nextToken += 1
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 620, height: 460), styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: true)
        window.isReleasedWhenClosed = false
        let scroll = NSScrollView(frame: window.contentView!.bounds)
        scroll.autoresizingMask = [.width, .height]
        scroll.hasVerticalScroller = true
        let text = NoteTextView(frame: scroll.bounds)
        text.autoresizingMask = [.width]
        text.isVerticallyResizable = true
        text.textContainer?.widthTracksTextView = true
        text.allowsUndo = true
        text.isRichText = false
        text.usesFindBar = true
        text.isAutomaticQuoteSubstitutionEnabled = false
        text.isAutomaticDashSubstitutionEnabled = false
        text.font = NSFont.systemFont(ofSize: 14)
        text.textContainerInset = NSSize(width: 12, height: 12)
        text.string = body
        scroll.documentView = text
        window.contentView?.addSubview(scroll)
        textView = text
        super.init()
        text.delegate = self
        window.delegate = self
        updateTitle()
    }

    static func show(worker: OpaquePointer, id: Int64?, body: String) {
        // One editor per note: opening it again brings the first one forward.
        if let id, let existing = open.values.first(where: { $0.noteID == id }) {
            NSApp.activate(ignoringOtherApps: true)
            existing.window.makeKeyAndOrderFront(nil)
            return
        }
        let editor = NoteEditor(worker: worker, id: id, body: body)
        open[editor.token] = editor
        editor.window.center()
        NSApp.unhide(nil)
        NSApp.activate(ignoringOtherApps: true)
        editor.window.makeKeyAndOrderFront(nil)
        editor.window.makeFirstResponder(editor.textView)
        editor.textView.setSelectedRange(NSRange(location: (body as NSString).length, length: 0))
        // A new note seeded from the search box is worth keeping even if nothing else is typed.
        if id == nil, !body.isEmpty { editor.scheduleSave() }
    }

    static func saved(token: UInt64, id: Int64) {
        guard let editor = open[token] else { return }
        editor.noteID = id
    }

    /// Writes every editor's text now. Called on quit, before the data thread shuts down.
    static func flushAll() {
        for editor in open.values { editor.flush() }
    }

    static var openCount: Int { open.count }

    private func updateTitle() {
        let title = takeString(orbit_note_title(textView.string)) ?? ""
        window.title = title.isEmpty ? "New note" : title
    }

    func textDidChange(_ notification: Notification) {
        updateTitle()
        scheduleSave()
    }

    private func scheduleSave() {
        dirty = true
        saveTimer?.invalidate()
        saveTimer = Timer.scheduledTimer(withTimeInterval: NoteEditor.debounce, repeats: false) { [weak self] _ in self?.flush() }
    }

    private func flush() {
        saveTimer?.invalidate()
        saveTimer = nil
        guard dirty else { return }
        dirty = false
        orbit_worker_note_save(worker, token, noteID, textView.string)
    }

    func windowWillClose(_ notification: Notification) {
        flush()
        if noteID >= 0 { orbit_worker_note_delete_if_empty(worker, noteID) }
        NoteEditor.open[token] = nil
        yieldFocusIfIdle(closing: window)
    }
}

final class SnippetEditor: NSObject, NSWindowDelegate {
    private static var open: [UInt64: SnippetEditor] = [:]
    private static var nextToken: UInt64 = 1

    private let window: NSWindow
    private let worker: OpaquePointer
    private let snippetID: Int64
    private let token: UInt64
    private let name = NSTextField()
    private let trigger = NSTextField()
    private let category = NSTextField()
    private let summary = NSTextField()
    private let favourite = NSButton(checkboxWithTitle: "Favourite", target: nil, action: nil)
    private let content = NSTextView()
    private let status = NSTextField(wrappingLabelWithString: "")

    struct Fields {
        var id: Int64 = -1
        var name = ""
        var trigger = ""
        var content = ""
        var description = ""
        var category = ""
        var favourite = false
    }

    private init(worker: OpaquePointer, fields: Fields) {
        self.worker = worker
        snippetID = fields.id
        token = SnippetEditor.nextToken
        SnippetEditor.nextToken += 1
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 560, height: 460), styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: true)
        window.isReleasedWhenClosed = false
        window.title = fields.id < 0 ? "New snippet" : fields.name
        super.init()
        window.delegate = self

        name.stringValue = fields.name
        trigger.stringValue = fields.trigger
        trigger.placeholderString = "Such as ;sig. Optional"
        category.stringValue = fields.category
        category.placeholderString = "Personal, Work, Code, Links, Templates. Optional"
        summary.stringValue = fields.description
        favourite.state = fields.favourite ? .on : .off
        content.string = fields.content
        content.isRichText = false
        content.allowsUndo = true
        content.font = NSFont.systemFont(ofSize: 13)
        content.isAutomaticQuoteSubstitutionEnabled = false
        content.isVerticallyResizable = true
        content.textContainer?.widthTracksTextView = true
        content.autoresizingMask = [.width]

        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder
        scroll.documentView = content
        scroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 180).isActive = true

        let save = NSButton(title: "Save", target: self, action: #selector(saveClicked))
        save.keyEquivalent = "\r"
        save.keyEquivalentModifierMask = .command
        let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancelClicked))
        cancel.keyEquivalent = "\u{1b}"
        let buttons = NSStackView(views: [status, NSView(), cancel, save])
        status.textColor = .systemOrange

        let help = NSTextField(wrappingLabelWithString: "{date}, {time} and {cursor} fill in on insert. {clipboard} uses your clipboard and asks first when typed as a trigger. ⌘Enter saves.")
        help.textColor = .secondaryLabelColor
        help.font = NSFont.systemFont(ofSize: 11)

        let stack = NSStackView(views: [
            labeled("Name", name), labeled("Trigger", trigger), labeled("Category", category), labeled("Description", summary),
            favourite, scroll, help, buttons,
        ])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 8
        stack.edgeInsets = NSEdgeInsets(top: 14, left: 14, bottom: 14, right: 14)
        stack.translatesAutoresizingMaskIntoConstraints = false
        window.contentView?.addSubview(stack)
        let host = window.contentView!
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: host.topAnchor),
            stack.bottomAnchor.constraint(equalTo: host.bottomAnchor),
            stack.leadingAnchor.constraint(equalTo: host.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: host.trailingAnchor),
            scroll.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -28),
            buttons.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -28),
        ])
    }

    private func labeled(_ title: String, _ field: NSTextField) -> NSView {
        let label = NSTextField(labelWithString: title)
        label.widthAnchor.constraint(equalToConstant: 80).isActive = true
        field.widthAnchor.constraint(greaterThanOrEqualToConstant: 420).isActive = true
        return NSStackView(views: [label, field])
    }

    static func show(worker: OpaquePointer, fields: Fields) {
        let editor = SnippetEditor(worker: worker, fields: fields)
        open[editor.token] = editor
        editor.window.center()
        NSApp.unhide(nil)
        NSApp.activate(ignoringOtherApps: true)
        editor.window.makeKeyAndOrderFront(nil)
        editor.window.makeFirstResponder(editor.name)
    }

    static func saved(token: UInt64) {
        open[token]?.window.close()
    }

    @objc private func saveClicked() {
        let triggerText = trigger.stringValue.trimmingCharacters(in: .whitespaces)
        if name.stringValue.trimmingCharacters(in: .whitespaces).isEmpty {
            status.stringValue = "A snippet needs a name."
            return
        }
        if let problem = takeString(orbit_snippet_trigger_problem(triggerText)) {
            status.stringValue = problem
            return
        }
        orbit_worker_snippet_save(worker, token, snippetID, name.stringValue, triggerText, content.string, summary.stringValue, category.stringValue, favourite.state == .on)
    }

    @objc private func cancelClicked() {
        window.close()
    }

    func windowWillClose(_ notification: Notification) {
        SnippetEditor.open[token] = nil
        yieldFocusIfIdle(closing: window)
    }
}
