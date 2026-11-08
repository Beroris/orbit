// Global shortcuts through Carbon's RegisterEventHotKey, which needs no permission.

import AppKit
import Carbon

// Orbit's modifier bits, from orbit_hotkeys::Mods.
let modCtrl: UInt8 = 1
let modAlt: UInt8 = 2
let modShift: UInt8 = 4
let modCmd: UInt8 = 8
let modHyper: UInt8 = 15

/// Physical positions on an ANSI keyboard. Used for keys that are not characters, and as the
/// fallback when the current layout has no key for a character.
private let namedKeys: [String: UInt32] = [
    "Space": 49, "Enter": 36, "Tab": 48, "Escape": 53, "Backspace": 51, "Delete": 117, "Home": 115, "End": 119,
    "PageUp": 116, "PageDown": 121, "Up": 126, "Down": 125, "Left": 123, "Right": 124,
    "F1": 122, "F2": 120, "F3": 99, "F4": 118, "F5": 96, "F6": 97, "F7": 98, "F8": 100, "F9": 101, "F10": 109,
    "F11": 103, "F12": 111, "F13": 105, "F14": 107, "F15": 113, "F16": 106, "F17": 64, "F18": 79, "F19": 80, "F20": 90,
    "CapsLock": 57, "RightCtrl": 62, "RightAlt": 61, "RightShift": 60, "RightSuper": 54,
]

private let ansiCharacters: [String: UInt32] = [
    "A": 0, "S": 1, "D": 2, "F": 3, "H": 4, "G": 5, "Z": 6, "X": 7, "C": 8, "V": 9, "B": 11, "Q": 12, "W": 13, "E": 14,
    "R": 15, "Y": 16, "T": 17, "1": 18, "2": 19, "3": 20, "4": 21, "6": 22, "5": 23, "=": 24, "9": 25, "7": 26, "-": 27,
    "8": 28, "0": 29, "]": 30, "O": 31, "U": 32, "[": 33, "I": 34, "P": 35, "L": 37, "J": 38, "'": 39, "K": 40, ";": 41,
    "\\": 42, ",": 43, "/": 44, "N": 45, "M": 46, ".": 47, "`": 50,
]

/// Which physical key types `character` in the current layout. Asking the layout makes Hyper+C
/// mean the key labelled C on Dvorak and similar layouts.
private func layoutKeyCode(for character: String) -> UInt32? {
    guard let source = TISCopyCurrentKeyboardLayoutInputSource()?.takeRetainedValue(),
          let raw = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData) else { return nil }
    let data = Unmanaged<CFData>.fromOpaque(raw).takeUnretainedValue()
    guard let bytes = CFDataGetBytePtr(data) else { return nil }
    let layout = UnsafeRawPointer(bytes).assumingMemoryBound(to: UCKeyboardLayout.self)
    let want = character.lowercased()
    for code in UInt16(0)..<128 {
        var dead: UInt32 = 0
        var length = 0
        var chars = [UniChar](repeating: 0, count: 4)
        let status = UCKeyTranslate(layout, code, UInt16(kUCKeyActionDown), 0, UInt32(LMGetKbdType()), OptionBits(kUCKeyTranslateNoDeadKeysBit), &dead, 4, &length, &chars)
        if status == noErr, length > 0, String(utf16CodeUnits: chars, count: length) == want { return UInt32(code) }
    }
    return nil
}

func keyCode(forKeyName name: String) -> UInt32? {
    if let code = namedKeys[name] { return code }
    return layoutKeyCode(for: name) ?? ansiCharacters[name]
}

private func carbonModifiers(_ mods: UInt8) -> UInt32 {
    var out: UInt32 = 0
    if mods & modCtrl != 0 { out |= UInt32(controlKey) }
    if mods & modAlt != 0 { out |= UInt32(optionKey) }
    if mods & modShift != 0 { out |= UInt32(shiftKey) }
    if mods & modCmd != 0 { out |= UInt32(cmdKey) }
    return out
}

/// A chord in macOS notation, for messages and help.
func prettyChord(_ text: String) -> String {
    text.replacingOccurrences(of: "Ctrl+", with: "⌃")
        .replacingOccurrences(of: "Alt+", with: "⌥")
        .replacingOccurrences(of: "Shift+", with: "⇧")
        .replacingOccurrences(of: "Cmd+", with: "⌘")
        .replacingOccurrences(of: "Hyper+", with: "⌃⌥⇧⌘")
}

enum RegisterFailure: Error, CustomStringConvertible {
    case unknownKey(String)
    case system(String)
    case taken

    var description: String {
        switch self {
        case .unknownKey(let key): return "\(key) isn't a key this Mac can register"
        case .system(let owner): return "macOS uses it for \(owner)"
        case .taken: return "another app already registered it"
        }
    }
}

/// True if the system has this chord in its enabled shortcut table, such as Spotlight.
func isEnabledSystemShortcut(keyCode: UInt32, modifiers: UInt32) -> Bool {
    var list: Unmanaged<CFArray>?
    guard CopySymbolicHotKeys(&list) == noErr, let items = list?.takeRetainedValue() as? [[String: Any]] else { return false }
    return items.contains { item in
        (item[kHISymbolicHotKeyEnabled as String] as? Bool ?? false)
            && (item[kHISymbolicHotKeyCode as String] as? Int) == Int(keyCode)
            && (item[kHISymbolicHotKeyModifiers as String] as? Int) == Int(modifiers)
    }
}

final class HotkeyCenter {
    private static let signature: OSType = 0x4F52_4254 // 'ORBT'
    /// Set before installing. Called on the main thread with the id and the moment the event arrived.
    var onHotkey: ((UInt32, Int64) -> Void)?
    private var refs: [UInt32: EventHotKeyRef] = [:]
    private var handler: EventHandlerRef?
    fileprivate static weak var current: HotkeyCenter?

    func install() -> Bool {
        HotkeyCenter.current = self
        var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        let status = InstallEventHandler(GetEventDispatcherTarget(), { _, event, _ -> OSStatus in
            let t0 = ticks()
            var id = EventHotKeyID()
            let status = GetEventParameter(event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID), nil, MemoryLayout<EventHotKeyID>.size, nil, &id)
            guard status == noErr, id.signature == HotkeyCenter.signature else { return OSStatus(eventNotHandledErr) }
            HotkeyCenter.current?.onHotkey?(id.id, t0)
            return noErr
        }, 1, &spec, nil, &handler)
        return status == noErr
    }

    /// `exclusive` asks Carbon to fail if another process holds the chord, which is what makes
    /// a collision visible rather than two apps silently racing.
    func register(id: UInt32, mods: UInt8, key: String) -> Result<Void, RegisterFailure> {
        guard let code = keyCode(forKeyName: key) else { return .failure(.unknownKey(key)) }
        let modifiers = carbonModifiers(mods)
        if isEnabledSystemShortcut(keyCode: code, modifiers: modifiers) {
            return .failure(.system("a system shortcut"))
        }
        var ref: EventHotKeyRef?
        let hotkeyID = EventHotKeyID(signature: HotkeyCenter.signature, id: id)
        let status = RegisterEventHotKey(code, modifiers, hotkeyID, GetEventDispatcherTarget(), OptionBits(kEventHotKeyExclusive), &ref)
        guard status == noErr, let ref else { return .failure(.taken) }
        refs[id] = ref
        return .success(())
    }

    func unregisterAll() {
        for ref in refs.values { UnregisterEventHotKey(ref) }
        refs.removeAll()
    }
}
