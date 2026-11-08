// What the launcher shows, and what each row can do. The port of rows.rs and view.rs.

import COrbit
import Foundation

enum Builtin: String {
    case toggleLauncher = "cmd:launcher"
    case newNote = "cmd:new-note"
    case searchNotes = "cmd:notes"
    case clipboardHistory = "cmd:clipboard"
    case snippetSearch = "cmd:snippets"
    case newSnippet = "cmd:new-snippet"
    case pauseClipboard = "cmd:clipboard-pause"
    case resumeClipboard = "cmd:clipboard-resume"
    case skipNextCopy = "cmd:clipboard-skip"
    case clearClipboard = "cmd:clipboard-clear"
    case resetShortcuts = "cmd:reset-shortcuts"
    case deleteLocalData = "cmd:delete-data"
    case settings = "cmd:settings"
    case diagnostics = "cmd:diagnostics"
    case rebuildAppIndex = "cmd:reindex"
    case suspendShortcuts = "cmd:suspend"
    case resumeShortcuts = "cmd:resume"
    case toggleLowResource = "cmd:low-resource"
    case exportData = "cmd:export"
    case showHelp = "cmd:help"
    case restartOrbit = "cmd:restart-orbit"
    case quitOrbit = "cmd:quit"
    case lockScreen = "cmd:lock"
    case sleep = "cmd:sleep"
    case shutdown = "cmd:shutdown"
    case restart = "cmd:restart"
    case emptyTrash = "cmd:empty-trash"
    case openDownloads = "cmd:downloads"
    case openTerminal = "cmd:terminal"
}

/// An action in its `bindings_text` form, such as `app:Safari` or `cmd:clipboard`.
enum Action {
    case focusOrLaunch(id: String, name: String)
    case launch(id: String, name: String)
    case openPath(String)
    case url(String)
    case builtin(Builtin)
    case custom(String)
    case openNote(Int64)
    case insertSnippet(Int64)

    init?(text: String) {
        guard let colon = text.firstIndex(of: ":") else { return nil }
        let kind = text[..<colon]
        let value = String(text[text.index(after: colon)...])
        switch kind {
        case "app": self = .focusOrLaunch(id: "", name: value)
        case "start": self = .launch(id: "", name: value)
        case "path": self = .openPath(value)
        case "url": self = .url(value)
        case "cmd":
            guard let command = Builtin(rawValue: text) else { return nil }
            self = .builtin(command)
        case "custom": self = .custom(value)
        case "note":
            guard let id = Int64(value) else { return nil }
            self = .openNote(id)
        case "snippet":
            guard let id = Int64(value) else { return nil }
            self = .insertSnippet(id)
        default: return nil
        }
    }
}

enum Scope: Equatable {
    case root, notes, snippets, clipboard, help, diagnostics
    case actions(of: String)
    case confirm(Builtin)
    case setAlias(key: String, of: String)

    var inputGlyph: Int {
        switch self {
        case .root: return Int(ORBIT_GLYPH_SEARCH)
        case .notes: return Int(ORBIT_GLYPH_NOTE)
        case .snippets: return Int(ORBIT_GLYPH_SNIPPET)
        case .clipboard: return Int(ORBIT_GLYPH_CLIPBOARD)
        case .help, .diagnostics: return Int(ORBIT_GLYPH_INFO)
        case .actions: return Int(ORBIT_GLYPH_COMMAND)
        case .confirm: return Int(ORBIT_GLYPH_POWER)
        case .setAlias: return Int(ORBIT_GLYPH_LINK)
        }
    }

    var placeholder: String {
        switch self {
        case .root: return "Search apps, commands, notes…"
        case .notes: return "Search notes…"
        case .snippets: return "Search snippets…"
        case .clipboard: return "Search clipboard history…"
        case .help: return "Filter help…"
        case .diagnostics: return "Filter diagnostics…"
        case .actions(let of): return "Actions for \(of)"
        case .confirm: return "Are you sure?"
        case .setAlias(_, let of): return "Type an alias for \(of)"
        }
    }

    var label: String {
        switch self {
        case .root: return "Orbit"
        case .notes: return "Notes"
        case .snippets: return "Snippets"
        case .clipboard: return "Clipboard history"
        case .help: return "Help"
        case .diagnostics: return "Diagnostics"
        case .actions: return "Actions"
        case .confirm: return "Confirm"
        case .setAlias: return "Alias"
        }
    }

    var emptyMessage: String? {
        switch self {
        case .root: return "No results"
        case .notes: return "No notes yet"
        case .snippets: return "No snippets yet"
        case .clipboard: return "Nothing here yet. Copy something and it will appear."
        default: return nil
        }
    }

    /// Scopes whose rows come from the data thread.
    var isData: Bool { self == .notes || self == .snippets || self == .clipboard }
}

enum RowIcon: Equatable {
    case app(path: String)
    case glyph(Int)
}

enum RowSource: Equatable {
    case plain
    case catalog(item: UInt32, key: String)
    case note(id: Int64, pinned: Bool)
    case snippet(id: Int64)
    case clip(id: Int64, kind: Int, pinned: Bool)
}

enum Op: Equatable {
    case runItem(item: UInt32, key: String)
    case askConfirm(Builtin)
    case confirmed(Builtin)
    case back
    case copy(String)
    case openUrl(String)
    case appFolder(String)
    case togglePin(String)
    case hideItem(String)
    case aliasPrompt(key: String, title: String)
    case saveAlias(key: String)
    case noteOpen(Int64)
    case noteCopy(Int64)
    case notePin(Int64, Bool)
    case noteTrash(Int64)
    case noteUndoTrash
    case newNote
    case snippetInsert(Int64)
    case snippetCopy(Int64)
    case snippetEdit(Int64)
    case snippetDelete(Int64)
    case newSnippet
    case clipPaste(Int64)
    case clipCopy(Int64)
    case clipPin(Int64, Bool)
    case clipDelete(Int64)
    case enableClipboard
    case openSettings
    case insertPending
}

struct Row {
    var title: String
    var subtitle: String
    var kind: String = ""
    var icon: RowIcon
    var accent = false
    var source: RowSource = .plain
    var primary: Op?

    static func info(_ title: String, _ subtitle: String, glyph: Int = ORBIT_GLYPH_INFO) -> Row {
        Row(title: title, subtitle: subtitle, icon: .glyph(Int(glyph)))
    }

    static func op(_ title: String, _ subtitle: String, glyph: Int, _ op: Op) -> Row {
        Row(title: title, subtitle: subtitle, icon: .glyph(Int(glyph)), primary: op)
    }

    init(title: String, subtitle: String, kind: String = "", icon: RowIcon, accent: Bool = false, source: RowSource = .plain, primary: Op? = nil) {
        self.title = title
        self.subtitle = subtitle
        self.kind = kind
        self.icon = icon
        self.accent = accent
        self.source = source
        self.primary = primary
    }

    /// Copies a row out of the core's borrowed memory.
    init(_ c: OrbitRow) {
        title = borrowed(c.title)
        subtitle = borrowed(c.subtitle)
        kind = borrowed(c.kind)
        let iconPath = borrowed(c.icon_path)
        icon = iconPath.isEmpty ? .glyph(Int(c.glyph)) : .app(path: iconPath)
        accent = c.accent
        switch Int(c.source) {
        case ORBIT_SOURCE_CATALOG: source = .catalog(item: c.item, key: borrowed(c.key))
        case ORBIT_SOURCE_NOTE: source = .note(id: c.id, pinned: c.pinned)
        case ORBIT_SOURCE_SNIPPET: source = .snippet(id: c.id)
        case ORBIT_SOURCE_CLIP: source = .clip(id: c.id, kind: Int(c.clip_kind), pinned: c.pinned)
        default: source = .plain
        }
        switch Int(c.op) {
        case ORBIT_OP_RUN_ITEM: primary = .runItem(item: c.item, key: borrowed(c.key))
        case ORBIT_OP_COPY: primary = .copy(borrowed(c.payload))
        case ORBIT_OP_OPEN_URL: primary = .openUrl(borrowed(c.payload))
        case ORBIT_OP_NOTE_OPEN: primary = .noteOpen(c.id)
        case ORBIT_OP_SNIPPET_INSERT: primary = .snippetInsert(c.id)
        case ORBIT_OP_CLIP_PASTE: primary = .clipPaste(c.id)
        default: primary = nil
        }
    }
}

func primaryLabel(_ op: Op) -> String {
    switch op {
    case .runItem, .openUrl: return "Open"
    case .askConfirm: return "Continue"
    case .confirmed: return "Confirm"
    case .back: return "Back"
    case .copy: return "Copy"
    case .noteOpen: return "Open note"
    case .snippetInsert, .insertPending: return "Insert"
    case .clipPaste: return "Paste"
    case .saveAlias: return "Save"
    default: return "Run"
    }
}

/// The ⌘↵ action, if the row has one. Windows offers "Run as admin" for apps. macOS has no
/// equivalent, so apps offer Show in Finder.
func secondaryFor(_ row: Row, isApp: Bool) -> (String, Op)? {
    switch row.source {
    case .catalog(_, let key): return isApp ? ("Show in Finder", .appFolder(key)) : nil
    case .note(let id, _): return ("Copy", .noteCopy(id))
    case .snippet(let id): return ("Copy", .snippetCopy(id))
    case .clip(let id, _, _): return ("Copy", .clipCopy(id))
    case .plain: return nil
    }
}

/// Entries for the ⌘K action menu.
func menuFor(_ row: Row, isApp: Bool, isPinned: Bool) -> [Row] {
    var out: [Row] = []
    if let primary = row.primary { out.append(.op(primaryLabel(primary), "Enter", glyph: ORBIT_GLYPH_COMMAND, primary)) }
    switch row.source {
    case .plain: break
    case .catalog(_, let key):
        if isApp { out.append(.op("Show in Finder", "⌘Enter", glyph: ORBIT_GLYPH_FOLDER, .appFolder(key))) }
        out.append(.op(isPinned ? "Unpin from suggestions" : "Pin to suggestions", "", glyph: ORBIT_GLYPH_PIN, .togglePin(key)))
        out.append(.op("Set alias…", "", glyph: ORBIT_GLYPH_LINK, .aliasPrompt(key: key, title: row.title)))
        out.append(.op("Hide from search", "", glyph: ORBIT_GLYPH_DELETE, .hideItem(key)))
    case .note(let id, let pinned):
        out.append(.op("Copy contents", "⌘Enter", glyph: ORBIT_GLYPH_CLIPBOARD, .noteCopy(id)))
        out.append(.op(pinned ? "Unpin note" : "Pin note", "", glyph: ORBIT_GLYPH_PIN, .notePin(id, !pinned)))
        out.append(.op("Move to trash", "Kept for 30 days", glyph: ORBIT_GLYPH_DELETE, .noteTrash(id)))
    case .snippet(let id):
        out.append(.op("Copy text", "⌘Enter", glyph: ORBIT_GLYPH_CLIPBOARD, .snippetCopy(id)))
        out.append(.op("Edit snippet", "", glyph: ORBIT_GLYPH_SNIPPET, .snippetEdit(id)))
        out.append(.op("Delete snippet", "", glyph: ORBIT_GLYPH_DELETE, .snippetDelete(id)))
    case .clip(let id, _, let pinned):
        out.append(.op("Copy only", "⌘Enter", glyph: ORBIT_GLYPH_CLIPBOARD, .clipCopy(id)))
        out.append(.op(pinned ? "Unpin" : "Pin (keeps it past retention)", "", glyph: ORBIT_GLYPH_PIN, .clipPin(id, !pinned)))
        out.append(.op("Delete from history", "", glyph: ORBIT_GLYPH_DELETE, .clipDelete(id)))
    }
    return out
}

struct SavedView {
    var scope: Scope
    var query: String
    var selected: Int
}

struct ViewState {
    var scope: Scope = .root
    var rows: [Row] = []
    /// The full list behind a static scope. `rows` holds what passes the filter.
    var staticRows: [Row] = []
    var selected = 0
    var scroll = 0
    /// Where results from the data thread go in `rows`, so they never shift rows above them.
    var asyncInsertAt = 0
    var secondaryLabel: String?
    var visibleRows = 8
    var stack: [SavedView] = []
    var lastQuery: String?

    var selectedRow: Row? { selected < rows.count ? rows[selected] : nil }

    mutating func select(_ index: Int) {
        if rows.isEmpty {
            selected = 0
            scroll = 0
            return
        }
        selected = min(index, rows.count - 1)
        ensureVisible()
    }

    mutating func moveSelection(_ delta: Int) {
        if rows.isEmpty { return }
        // One-step moves wrap. Page moves stop at the ends.
        let next: Int
        if abs(delta) == 1 {
            next = ((selected + delta) % rows.count + rows.count) % rows.count
        } else {
            next = max(0, min(rows.count - 1, selected + delta))
        }
        select(next)
    }

    mutating func scrollBy(_ delta: Int) {
        let maxScroll = max(0, rows.count - visibleRows)
        scroll = max(0, min(maxScroll, scroll + delta))
        if selected < scroll {
            selected = scroll
        } else if selected >= scroll + visibleRows {
            selected = scroll + visibleRows - 1
        }
    }

    private mutating func ensureVisible() {
        if selected < scroll {
            scroll = selected
        } else if selected >= scroll + visibleRows {
            scroll = selected + 1 - visibleRows
        }
    }

    mutating func resetPosition() {
        selected = 0
        scroll = 0
    }

    mutating func filterStatic(_ query: String) {
        let needle = query.trimmingCharacters(in: .whitespaces).folding(options: [.caseInsensitive, .diacriticInsensitive], locale: nil)
        rows = staticRows.filter { row in
            if needle.isEmpty { return true }
            let folded = { (text: String) in text.folding(options: [.caseInsensitive, .diacriticInsensitive], locale: nil) }
            return folded(row.title).contains(needle) || folded(row.subtitle).contains(needle)
        }
        select(selected)
    }

    func footerHints() -> String {
        var parts: [String] = []
        if let primary = selectedRow?.primary { parts.append("↵ \(primaryLabel(primary))") }
        if let secondaryLabel { parts.append("⌘↵ \(secondaryLabel)") }
        var isActions = false
        if case .actions = scope { isActions = true }
        if let row = selectedRow, row.source != .plain, !isActions { parts.append("⌘K Actions") }
        parts.append(stack.isEmpty ? "Esc Close" : "Esc Back")
        return parts.joined(separator: "    ")
    }
}

func helpRows(launcherHint: String) -> [Row] {
    [
        .info(launcherHint, "Open or close Orbit"),
        .info("↑ ↓, Page Up/Down", "Move through results"),
        .info("Enter", "Run the selected result"),
        .info("⌘Enter", "Second action on the result, shown in the footer"),
        .info("⌘K or Tab", "All actions for the result"),
        .info("Esc", "Go back, or close Orbit"),
        .info("Delete", "In an empty box, go back"),
        .info("> or :", "Start your search with this to see commands only"),
        .info("g something", "Search with a quick link template. Alias \"g\" is Google"),
        .info("2*21 or 10 km to mi", "Calculate and convert. Enter copies the result"),
        .info("github.com", "Open an address directly"),
        .info("?", "Show this help"),
    ]
}
