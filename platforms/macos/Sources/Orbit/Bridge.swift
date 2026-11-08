// Small helpers over the C ABI. See crates/orbit-ffi/include/orbit.h for the ownership rules.

import COrbit
import Foundation

/// Takes ownership of a string the core returned.
func takeString(_ pointer: UnsafeMutablePointer<CChar>?) -> String? {
    guard let pointer else { return nil }
    defer { orbit_string_free(pointer) }
    return String(cString: pointer)
}

func borrowed(_ pointer: UnsafePointer<CChar>?) -> String {
    guard let pointer else { return "" }
    return String(cString: pointer)
}

func nowUnix() -> Int64 { Int64(Date().timeIntervalSince1970) }

/// Mirrors `HostView` in crates/orbit-ffi/src/form.rs.
struct HostSettings: Decodable {
    struct ChordView: Decodable { var mods: UInt8; var key: String; var display: String }
    struct Binding: Decodable { var key: String; var action: String }
    struct Shortcut: Decodable { var chord: ChordView; var action: String }

    var hotkey: ChordView?
    var reservedHotkeyOwner: String?
    var preserveQuery: Bool
    var maxVisibleRows: Int
    var focusRunningApp: Bool
    var hyperEnabled: Bool
    var hyperTrigger: String
    var hyperMode: String
    var hyperTap: String
    var hyperTimeoutMs: UInt32
    var hyperBindings: [Binding]
    var hyperExcluded: [String]
    var hyperPassGames: Bool
    var hyperDuplicates: [String]
    var shortcuts: [Shortcut]
    var clipEnabled: Bool
    var clipPersist: Bool
    var clipImages: Bool
    var clipMaxItems: UInt32
    var clipRetentionDays: UInt32
    var clipMaxItemBytes: UInt32
    var clipExcluded: [String]
    var expansionEnabled: Bool
    var expansionExcluded: [String]
    var appearance: String
    var lowResourceMode: Bool
    var launchAtLogin: Bool
    var onboarded: Bool
    var readOnly: Bool
    var form: SettingsForm
}

/// Mirrors `SettingsForm`. Every rule about these values lives in the core.
struct SettingsForm: Codable {
    var hotkey = ""
    var launchAtLogin = false
    var focusRunningApp = true
    var preserveQuery = false
    var lowResourceMode = false
    var appearance = "system"
    var shortcuts = ""
    var hyperEnabled = false
    var hyperTrigger = "CapsLock"
    var hyperMode = "internal"
    var hyperTap = "escape"
    var hyperTimeout = "250"
    var hyperPassGames = true
    var hyperTable = ""
    var hyperExcluded = ""
    var clipEnabled = false
    var clipPersist = true
    var clipImages = false
    var clipMax = "500"
    var clipDays = "30"
    var clipExcluded = ""
    var expansion = false
    var snippetExcluded = ""
    var links = ""
    var commands = ""
}

final class Core {
    let engine: OpaquePointer
    let dataDir: String

    init?(dataDir: String) {
        guard let engine = orbit_engine_new(dataDir) else { return nil }
        self.engine = engine
        self.dataDir = dataDir
    }

    func host() -> HostSettings? {
        guard let json = takeString(orbit_engine_host_json(engine)), let data = json.data(using: .utf8) else { return nil }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try? decoder.decode(HostSettings.self, from: data)
    }

    /// Returns nil on success, otherwise the page and message of the problem.
    func apply(form: SettingsForm) -> (page: Int, message: String)? {
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        guard let data = try? encoder.encode(form), let json = String(data: data, encoding: .utf8) else {
            return (0, "The form could not be read.")
        }
        var message: UnsafeMutablePointer<CChar>?
        let page = orbit_engine_apply_form(engine, json, &message)
        if page == -1 { return nil }
        return (max(0, Int(page)), takeString(message) ?? "Couldn't save your settings.")
    }
}

/// Reads chord text into modifier bits and a key name.
func parseChord(_ text: String) -> (mods: UInt8, key: String)? {
    var mods: UInt8 = 0
    var key: UnsafeMutablePointer<CChar>?
    var error: UnsafeMutablePointer<CChar>?
    guard orbit_chord_parse(text, &mods, &key, &error) else {
        _ = takeString(error)
        return nil
    }
    return (mods, takeString(key) ?? "")
}
