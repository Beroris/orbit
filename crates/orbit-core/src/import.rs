//! Reading what other launchers keep, to bring it into Orbit: quicklinks and web searches,
//! snippets, notes, and the launcher's own hotkey.
//!
//! Only reading happens here. The host finds the files and decides what to keep. Nothing read is
//! ever run or opened, and text stays text: a snippet's placeholders are turned into Orbit's where
//! Orbit has the same one, and left as typed where it doesn't.

use crate::links::{Field, check_link};
use crate::settings::{CustomCommand, QuickLink};
use orbit_hotkeys::{Chord, Mods};
use serde_json::Value;

/// What one launcher had that Orbit can use.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Haul {
    pub hotkey: Option<Chord>,
    pub links: Vec<Link>,
    pub snippets: Vec<Snippet>,
    pub notes: Vec<Note>,
    /// What was there but can't come over, in plain words, such as "4 extensions".
    pub left_out: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub name: String,
    /// In Orbit's form: `{query}` and named blanks such as `{city=London}`.
    pub url: String,
    pub alias: String,
    /// The app it opened with, by the name the other launcher gave it. Empty for the default.
    pub open_with: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Snippet {
    pub name: String,
    pub text: String,
    pub keyword: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub body: String,
}

/// Why a file couldn't be read, in words for the person who chose it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// It's encrypted, and needs the passphrase it was exported with.
    NeedsPassphrase,
    /// That passphrase didn't open it, or the file is damaged.
    WrongPassphrase,
    /// Not a file Orbit knows how to read.
    Unknown,
    Damaged(String),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::NeedsPassphrase => f.write_str("This export is encrypted. Type the passphrase you chose when you exported it."),
            ReadError::WrongPassphrase => f.write_str("That passphrase didn't open it. Check it, or export the file again."),
            ReadError::Unknown => f.write_str("Orbit doesn't know this kind of file. Choose an export from Raycast, or an Orbit export."),
            ReadError::Damaged(why) => write!(f, "The file looks damaged: {why}"),
        }
    }
}

impl Haul {
    pub fn is_empty(&self) -> bool {
        self.hotkey.is_none() && self.links.is_empty() && self.snippets.is_empty() && self.notes.is_empty()
    }

    /// "12 quicklinks, 3 snippets and the hotkey Alt+Space".
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        let mut count = |n: usize, one: &str, many: &str| {
            if n > 0 {
                parts.push(format!("{n} {}", if n == 1 { one } else { many }));
            }
        };
        count(self.links.len(), "quicklink", "quicklinks");
        count(self.snippets.len(), "snippet", "snippets");
        count(self.notes.len(), "note", "notes");
        if let Some(hotkey) = &self.hotkey {
            parts.push(format!("the hotkey {}", hotkey.display("Win")));
        }
        if parts.is_empty() {
            return "Nothing Orbit can use".into();
        }
        join_list(&parts)
    }

    /// Adds another file's findings from the same launcher. The first hotkey found stays.
    pub fn take(&mut self, other: Haul) {
        if self.hotkey.is_none() {
            self.hotkey = other.hotkey;
        }
        for link in other.links {
            if !self.links.iter().any(|known| known.url == link.url && known.name == link.name) {
                self.links.push(link);
            }
        }
        for snippet in other.snippets {
            if !self.snippets.contains(&snippet) {
                self.snippets.push(snippet);
            }
        }
        self.notes.extend(other.notes);
        self.left_out.extend(other.left_out);
    }
}

/// "a, b and c".
pub fn join_list(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn text<'a>(value: &'a Value, keys: &[&str]) -> &'a str {
    keys.iter().find_map(|key| value.get(*key).and_then(Value::as_str)).unwrap_or("")
}

fn list<'a>(value: &'a Value, path: &[&str]) -> &'a [Value] {
    let mut at = value;
    for key in path {
        match at.get(*key) {
            Some(next) => at = next,
            None => return &[],
        }
    }
    at.as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn parse_json(text: &str) -> Option<Value> {
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

/// An alias is one word. Anything else is dropped rather than half kept.
fn clean_alias(alias: &str) -> String {
    let alias = alias.trim();
    if alias.is_empty() || alias == "*" || alias.chars().any(char::is_whitespace) || alias.chars().count() > 32 {
        return String::new();
    }
    alias.to_string()
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

// --- Hotkeys -------------------------------------------------------------------------------

/// A Windows virtual-key code's name, as Orbit writes keys.
pub fn vk_name(code: u32) -> Option<String> {
    let name = match code {
        0x41..=0x5A | 0x30..=0x39 => return Some(char::from(code as u8).to_string()),
        0x70..=0x87 => return Some(format!("F{}", code - 0x6F)),
        0x20 => "Space",
        0x0D => "Enter",
        0x09 => "Tab",
        0x1B => "Escape",
        0x08 => "Backspace",
        0x2E => "Delete",
        0x2D => "Insert",
        0x24 => "Home",
        0x23 => "End",
        0x21 => "PageUp",
        0x22 => "PageDown",
        0x25 => "Left",
        0x26 => "Up",
        0x27 => "Right",
        0x28 => "Down",
        0xBA => ";",
        0xBB => "=",
        0xBC => ",",
        0xBD => "-",
        0xBE => ".",
        0xBF => "/",
        0xC0 => "`",
        0xDB => "[",
        0xDC => "\\",
        0xDD => "]",
        0xDE => "'",
        _ => return None,
    };
    Some(name.to_string())
}

/// A PC keyboard's scan code (set 1, with 0xE0 in front for the extended keys), as Raycast for
/// Windows stores keys.
fn scan_code_name(code: u64) -> Option<String> {
    const ROWS: [(u64, &str); 4] = [(0x10, "QWERTYUIOP"), (0x1E, "ASDFGHJKL"), (0x2C, "ZXCVBNM"), (0x02, "1234567890")];
    for (start, keys) in ROWS {
        if code >= start && code < start + keys.len() as u64 {
            return Some(keys.chars().nth((code - start) as usize)?.to_string());
        }
    }
    let name = match code {
        0x3B..=0x44 => return Some(format!("F{}", code - 0x3A)),
        0x57 => "F11",
        0x58 => "F12",
        0x39 => "Space",
        0x1C => "Enter",
        0x0F => "Tab",
        0x01 => "Escape",
        0x0E => "Backspace",
        0x0C => "-",
        0x0D => "=",
        0x1A => "[",
        0x1B => "]",
        0x27 => ";",
        0x28 => "'",
        0x29 => "`",
        0x2B => "\\",
        0x33 => ",",
        0x34 => ".",
        0x35 => "/",
        0xE048 => "Up",
        0xE050 => "Down",
        0xE04B => "Left",
        0xE04D => "Right",
        0xE047 => "Home",
        0xE04F => "End",
        0xE049 => "PageUp",
        0xE051 => "PageDown",
        0xE052 => "Insert",
        0xE053 => "Delete",
        _ => return None,
    };
    Some(name.to_string())
}

/// A Mac's key code (Carbon's kVK), as Raycast for Mac stores keys.
fn mac_key_name(code: u64) -> Option<String> {
    const LETTERS: &[(u64, &str)] = &[
        (0, "A"), (1, "S"), (2, "D"), (3, "F"), (4, "H"), (5, "G"), (6, "Z"), (7, "X"), (8, "C"), (9, "V"), (11, "B"), (12, "Q"), (13, "W"), (14, "E"), (15, "R"),
        (16, "Y"), (17, "T"), (18, "1"), (19, "2"), (20, "3"), (21, "4"), (22, "6"), (23, "5"), (24, "="), (25, "9"), (26, "7"), (27, "-"), (28, "8"), (29, "0"),
        (30, "]"), (31, "O"), (32, "U"), (33, "["), (34, "I"), (35, "P"), (36, "Enter"), (37, "L"), (38, "J"), (39, "'"), (40, "K"), (41, ";"), (42, "\\"),
        (43, ","), (44, "/"), (45, "N"), (46, "M"), (47, "."), (48, "Tab"), (49, "Space"), (50, "`"), (51, "Backspace"), (53, "Escape"), (96, "F5"), (97, "F6"),
        (98, "F7"), (99, "F3"), (100, "F8"), (101, "F9"), (103, "F11"), (109, "F10"), (111, "F12"), (115, "Home"), (116, "PageUp"), (117, "Delete"), (118, "F4"),
        (119, "End"), (120, "F2"), (121, "PageDown"), (122, "F1"), (123, "Left"), (124, "Right"), (125, "Down"), (126, "Up"),
    ];
    LETTERS.iter().find(|(at, _)| *at == code).map(|(_, name)| name.to_string())
}

/// A hotkey from separate flags and a virtual-key code, as PowerToys and Command Palette keep it.
fn flags_hotkey(value: &Value) -> Option<Chord> {
    let flag = |key: &str| value.get(key).and_then(Value::as_bool).unwrap_or(false);
    let code = value.get("code").and_then(Value::as_u64)? as u32;
    let mut mods = Mods::NONE;
    for (on, modifier) in [(flag("ctrl"), Mods::CTRL), (flag("alt"), Mods::ALT), (flag("shift"), Mods::SHIFT), (flag("win"), Mods::SUPER)] {
        if on {
            mods = mods.union(modifier);
        }
    }
    Chord::new(mods, &vk_name(code)?).ok().filter(|chord| !chord.mods.is_empty())
}

/// "Alt + Space", "alt+space", "Ctrl+Win+K".
fn text_hotkey(text: &str) -> Option<Chord> {
    let cleaned = text.replace("LWin", "Win").replace("RWin", "Win");
    Chord::parse(&cleaned).ok().filter(|chord| !chord.mods.is_empty())
}

// --- Raycast -------------------------------------------------------------------------------

pub mod raycast {
    use super::*;
    use aes_gcm::aead::consts::U16;
    use aes_gcm::aead::{Aead, KeyInit};
    use std::io::Read;

    const RAYCFG3: &[u8] = b"RAYCFG3\n";
    const GZIP: &[u8] = &[0x1f, 0x8b];
    /// What Raycast for Mac encrypted its exports with unless the user changed it.
    const CLASSIC_DEFAULT_PASSWORD: &str = "12345678";
    /// No real export is this big, and anything larger isn't one.
    const MAX_UNPACKED: u64 = 256 * 1024 * 1024;

    type Gcm16 = aes_gcm::AesGcm<aes::Aes256, U16>;

    fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, ReadError> {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(bytes).take(MAX_UNPACKED).read_to_end(&mut out).map_err(|e| ReadError::Damaged(e.to_string()))?;
        Ok(out)
    }

    fn hex(text: &str) -> Option<Vec<u8>> {
        if text.len() % 2 != 0 {
            return None;
        }
        (0..text.len()).step_by(2).map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok()).collect()
    }

    fn scrypt_key(passphrase: &str, salt: &[u8]) -> Result<[u8; 32], ReadError> {
        let params = scrypt::Params::new(14, 8, 1, 32).map_err(|e| ReadError::Damaged(e.to_string()))?;
        let mut key = [0u8; 32];
        scrypt::scrypt(passphrase.as_bytes(), salt, &params, &mut key).map_err(|e| ReadError::Damaged(e.to_string()))?;
        Ok(key)
    }

    /// AES-256-GCM with Raycast's 16-byte nonce. `sealed` is the ciphertext with its tag at the end.
    fn open_gcm(passphrase: &str, salt: &[u8], iv: &[u8], sealed: &[u8]) -> Result<Vec<u8>, ReadError> {
        if iv.len() != 16 || salt.is_empty() {
            return Err(ReadError::Damaged("its encryption details are missing".into()));
        }
        let key = scrypt_key(passphrase, salt)?;
        let cipher = Gcm16::new(&key.into());
        cipher.decrypt(aes_gcm::aead::generic_array::GenericArray::from_slice(iv), sealed).map_err(|_| ReadError::WrongPassphrase)
    }

    /// Whether the file needs a passphrase before it can be read. A Mac export with Raycast's
    /// default password doesn't.
    pub fn needs_passphrase(bytes: &[u8]) -> bool {
        matches!(read(bytes, None), Err(ReadError::NeedsPassphrase))
    }

    /// Reads any of Raycast's exports: Settings & Data (`.rayconfig`, from Raycast 2 for Windows
    /// and Mac, or from Raycast for Mac before it), or the snippets or quicklinks it exports as JSON.
    pub fn read(bytes: &[u8], passphrase: Option<&str>) -> Result<Haul, ReadError> {
        if bytes.starts_with(RAYCFG3) {
            return read_raycfg3(bytes, passphrase);
        }
        if bytes.starts_with(GZIP) {
            let inner = gunzip(bytes)?;
            let value: Value = serde_json::from_slice(&inner).map_err(|_| ReadError::Unknown)?;
            if value.get("data").is_some() && value.get("schemaVersion").is_some() {
                return read_envelope(&value, passphrase);
            }
            return from_any_json(&value);
        }
        if let Some(value) = std::str::from_utf8(bytes).ok().and_then(parse_json) {
            return from_any_json(&value);
        }
        // Raycast for Mac: a random IV, then AES-256-CBC of the gzipped JSON, keyed by the
        // password's SHA-256.
        if bytes.len() < 32 || bytes.len() % 16 != 0 {
            return Err(ReadError::Unknown);
        }
        let tried = passphrase.unwrap_or(CLASSIC_DEFAULT_PASSWORD);
        match open_classic(bytes, tried) {
            Some(value) => from_any_json(&value),
            None if passphrase.is_none() => Err(ReadError::NeedsPassphrase),
            None => Err(ReadError::WrongPassphrase),
        }
    }

    fn open_classic(bytes: &[u8], password: &str) -> Option<Value> {
        use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
        use sha2::Digest;
        let key = sha2::Sha256::digest(password.as_bytes());
        let (iv, sealed) = bytes.split_at(16);
        let plain = cbc::Decryptor::<aes::Aes256>::new_from_slices(&key, iv).ok()?.decrypt_padded_vec_mut::<Pkcs7>(sealed).ok()?;
        if !plain.starts_with(GZIP) {
            return None;
        }
        serde_json::from_slice(&gunzip(&plain).ok()?).ok()
    }

    fn read_raycfg3(bytes: &[u8], passphrase: Option<&str>) -> Result<Haul, ReadError> {
        let damaged = || ReadError::Damaged("it ends too soon".into());
        let length = u32::from_le_bytes(bytes.get(8..12).ok_or_else(damaged)?.try_into().map_err(|_| damaged())?) as usize;
        let header_end = 12usize.checked_add(length).filter(|end| *end <= bytes.len()).ok_or_else(damaged)?;
        let header: Value = serde_json::from_slice(&gunzip(&bytes[12..header_end])?).map_err(|e| ReadError::Damaged(e.to_string()))?;
        let rest = &bytes[header_end..];
        let encryption = header.get("encryption").unwrap_or(&header);
        let (iv, salt) = (text(encryption, &["iv"]), text(encryption, &["salt"]));
        let packed = if iv.is_empty() && header.get("encryption").is_none() {
            rest.to_vec()
        } else {
            let Some(passphrase) = passphrase.filter(|p| !p.is_empty()) else { return Err(ReadError::NeedsPassphrase) };
            let (iv, salt) = (hex(iv).ok_or_else(damaged)?, hex(salt).ok_or_else(damaged)?);
            open_gcm(passphrase, &salt, &iv, rest)?
        };
        let payload: Value = serde_json::from_slice(&gunzip(&packed)?).map_err(|e| ReadError::Damaged(e.to_string()))?;
        let mac = text(&header, &["osName"]).to_ascii_lowercase().contains("mac");
        Ok(from_raycast2(&payload, mac))
    }

    /// The format early versions of Raycast 2 wrote: gzipped JSON holding the encrypted payload.
    fn read_envelope(value: &Value, passphrase: Option<&str>) -> Result<Haul, ReadError> {
        let damaged = || ReadError::Damaged("its contents are missing".into());
        let data = hex(text(value, &["data"])).ok_or_else(damaged)?;
        let mac = text(value, &["osName"]).to_ascii_lowercase().contains("mac");
        let plain = match value.get("encryption") {
            Some(encryption) => {
                let Some(passphrase) = passphrase.filter(|p| !p.is_empty()) else { return Err(ReadError::NeedsPassphrase) };
                let iv = hex(text(encryption, &["iv"])).ok_or_else(damaged)?;
                let salt = hex(text(encryption, &["salt"])).ok_or_else(damaged)?;
                let mut sealed = data;
                sealed.extend(hex(text(encryption, &["authTag", "tag"])).unwrap_or_default());
                open_gcm(passphrase, &salt, &iv, &sealed)?
            }
            None => data,
        };
        let json = if plain.starts_with(GZIP) { gunzip(&plain)? } else { plain };
        let payload: Value = serde_json::from_slice(&json).map_err(|e| ReadError::Damaged(e.to_string()))?;
        Ok(from_raycast2(&payload, mac))
    }

    /// A decrypted export, or one of the JSON files: whichever this is.
    fn from_any_json(value: &Value) -> Result<Haul, ReadError> {
        if let Some(items) = value.as_array() {
            return Ok(from_list(items));
        }
        if value.as_object().is_some_and(|object| object.keys().any(|key| key.starts_with("builtin_package_"))) {
            return Ok(from_classic(value));
        }
        if ["quicklinks", "snippets", "notes", "settings"].iter().any(|key| value.get(*key).is_some()) {
            return Ok(from_raycast2(value, false));
        }
        Err(ReadError::Unknown)
    }

    /// Export Snippets or Export Quicklinks: a list of one or the other.
    fn from_list(items: &[Value]) -> Haul {
        let mut haul = Haul::default();
        for item in items {
            if item.get("link").is_some() || item.get("url").is_some() {
                push_link(&mut haul, item, "");
            } else if item.get("text").is_some() {
                push_snippet(&mut haul, item);
            }
        }
        note_placeholders(&mut haul);
        haul
    }

    fn push_link(haul: &mut Haul, item: &Value, alias: &str) {
        let name = text(item, &["name", "title"]).trim();
        let target = text(item, &["link", "url"]).trim();
        if name.is_empty() || target.is_empty() {
            return;
        }
        haul.links.push(Link { name: name.to_string(), url: link_template(target), alias: clean_alias(alias), open_with: text(item, &["openWith"]).trim().to_string() });
    }

    fn push_snippet(haul: &mut Haul, item: &Value) {
        let body = text(item, &["text"]);
        let name = text(item, &["name", "title"]).trim();
        if body.is_empty() {
            return;
        }
        let keyword = text(item, &["keyword", "alias"]).trim();
        let name = if name.is_empty() { keyword } else { name };
        haul.snippets.push(Snippet { name: name.to_string(), text: snippet_template(body), keyword: clean_alias(keyword) });
    }

    fn push_note(haul: &mut Haul, item: &Value) {
        let title = text(item, &["title"]).trim();
        let body = text(item, &["markdown", "text", "content", "body"]);
        if title.is_empty() && body.trim().is_empty() {
            return;
        }
        // Orbit names a note by its first line, so the title goes there if it isn't already.
        let first = body.lines().next().unwrap_or("").trim_start_matches('#').trim();
        let body = if title.is_empty() || first == title { body.to_string() } else { format!("# {title}\n\n{body}") };
        haul.notes.push(Note { body });
    }

    /// One line about placeholders Orbit doesn't have, if any were found.
    fn note_placeholders(haul: &mut Haul) {
        let snippets = haul.snippets.iter().filter(|snippet| has_foreign_placeholder(&snippet.text, SNIPPET_PLACEHOLDERS)).count();
        if snippets > 0 {
            haul.left_out.push(format!("{} that Orbit can't fill in, such as {{selection}}, which stay as typed", plural(snippets, "snippet uses placeholders", "snippets use placeholders")));
        }
    }

    /// Raycast 2's export: each kind of thing under its own name.
    fn from_raycast2(payload: &Value, mac: bool) -> Haul {
        let mut haul = Haul::default();
        // A quicklink's alias is kept with the commands, by the quicklink's id.
        let commands = list(payload, &["settings", "commands"]);
        let alias_of = |id: &str| -> String {
            if id.is_empty() {
                return String::new();
            }
            commands
                .iter()
                .find(|command| text(command, &["id"]).rsplit("::=::").next() == Some(id) && text(command, &["id"]).contains("quicklink"))
                .map(|command| text(command, &["alias"]).to_string())
                .unwrap_or_default()
        };
        for item in list(payload, &["quicklinks", "quicklinks"]).iter().chain(list(payload, &["quicklinks"])) {
            push_link(&mut haul, item, &alias_of(text(item, &["id"])));
        }
        for item in list(payload, &["snippets", "snippets"]).iter().chain(list(payload, &["snippets"])) {
            push_snippet(&mut haul, item);
        }
        for item in list(payload, &["notes", "notes"]).iter().chain(list(payload, &["notes"])) {
            push_note(&mut haul, item);
        }
        let general = payload.get("settings").and_then(|settings| settings.get("general"));
        haul.hotkey = general.and_then(|general| general.get("globalHotkey")).and_then(|hotkey| raycast_hotkey(hotkey, mac));
        let extensions = list(payload, &["settings", "nodeExtensions"]).len().max(list(payload, &["nodeExtensions"]).len());
        if extensions > 0 {
            haul.left_out.push(plural(extensions, "extension from Raycast's store", "extensions from Raycast's store"));
        }
        let shortcuts = commands.iter().filter(|command| !text(command, &["id"]).contains("quicklink") && (command.get("windowsHotkey").is_some() || !text(command, &["alias"]).is_empty())).count();
        if shortcuts > 0 {
            haul.left_out.push(format!("aliases and hotkeys on {}", plural(shortcuts, "command", "commands")));
        }
        note_placeholders(&mut haul);
        haul
    }

    /// Raycast for Mac's export, before Raycast 2.
    fn from_classic(value: &Value) -> Haul {
        let mut haul = Haul::default();
        for item in list(value, &["builtin_package_quicklinks", "quicklinks"]) {
            push_link(&mut haul, item, "");
        }
        for item in list(value, &["builtin_package_snippets", "snippets"]) {
            push_snippet(&mut haul, item);
        }
        for item in list(value, &["builtin_package_raycastNotes", "notes"]) {
            push_note(&mut haul, item);
        }
        let hotkey = value.get("builtin_package_raycastPreferences").and_then(|p| p.get("preferencesGeneral")).map(|p| text(p, &["raycastGlobalHotkey"])).unwrap_or("");
        haul.hotkey = classic_hotkey(hotkey);
        let extensions = value.as_object().map_or(0, |object| object.keys().filter(|key| key.starts_with("remote_package_") || key.starts_with("extension_")).count());
        if extensions > 0 {
            haul.left_out.push(plural(extensions, "extension from Raycast's store", "extensions from Raycast's store"));
        }
        note_placeholders(&mut haul);
        haul
    }

    /// "Option-49": Mac modifiers and a Mac key code. Cmd and Option become Alt, as Orbit's
    /// macOS feel has it.
    pub(super) fn classic_hotkey(text: &str) -> Option<Chord> {
        let (mods_text, code) = text.rsplit_once('-')?;
        let key = mac_key_name(code.parse().ok()?)?;
        let mut mods = Mods::NONE;
        for part in mods_text.split('-') {
            mods = mods.union(match part {
                "Command" | "Option" => Mods::ALT,
                "Control" => Mods::CTRL,
                "Shift" => Mods::SHIFT,
                _ => Mods::NONE,
            });
        }
        Chord::new(mods, &key).ok().filter(|chord| !chord.mods.is_empty())
    }

    /// Raycast 2's hotkey: modifiers by name, and a key by its code or its name.
    pub(super) fn raycast_hotkey(value: &Value, mac: bool) -> Option<Chord> {
        let shortcut = value.get("kind").filter(|kind| text(kind, &["type"]) == "SingleStep")?.get("shortcut")?;
        let mut mods = Mods::NONE;
        for modifier in shortcut.get("modifiers").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
            mods = mods.union(match text(modifier, &["modifier"]) {
                "Ctrl" | "Control" => Mods::CTRL,
                "Alt" | "Option" => Mods::ALT,
                "Shift" => Mods::SHIFT,
                // The Mac's Cmd is Alt in Orbit's macOS feel, and the Windows key otherwise.
                "Meta" if mac => Mods::ALT,
                "Meta" | "Win" | "Super" => Mods::SUPER,
                "Hyper" => Mods::HYPER,
                _ => Mods::NONE,
            });
        }
        let key = shortcut.get("key")?;
        let name = match text(key, &["type"]) {
            "LayoutDependent" => {
                let name = key.get("keyType").map(|kind| text(kind, &["key"])).unwrap_or("");
                name.strip_prefix("Arrow").unwrap_or(name).to_string()
            }
            _ => {
                let code = key.get("code").and_then(Value::as_u64)?;
                if mac { mac_key_name(code)? } else { scan_code_name(code)? }
            }
        };
        Chord::new(mods, &name).ok().filter(|chord| !chord.mods.is_empty())
    }
}

// --- Placeholders --------------------------------------------------------------------------

/// The snippet placeholders Orbit has.
const SNIPPET_PLACEHOLDERS: &[&str] = &["date", "time", "clipboard", "cursor"];

/// Splits `{name key="value" key=value | modifier}` into its name and attributes.
fn parse_placeholder(inner: &str) -> (String, Vec<(String, String)>) {
    let body = inner.split('|').next().unwrap_or("").trim();
    let (name, mut rest) = match body.find(char::is_whitespace) {
        Some(at) => (&body[..at], body[at..].trim_start()),
        None => (body, ""),
    };
    let mut attributes = Vec::new();
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq].trim().to_string();
        let after = rest[eq + 1..].trim_start();
        let (value, next) = if let Some(quoted) = after.strip_prefix('"').or_else(|| after.strip_prefix('\u{201C}')) {
            let end = quoted.find(['"', '\u{201D}']).unwrap_or(quoted.len());
            (quoted[..end].to_string(), quoted.get(end + 1..).unwrap_or(""))
        } else {
            let end = after.find(char::is_whitespace).unwrap_or(after.len());
            (after[..end].to_string(), &after[end..])
        };
        attributes.push((key, value));
        rest = next.trim_start();
    }
    (name.to_string(), attributes)
}

/// Walks the text, handing each `{...}` to `swap`, which gives what goes in its place.
fn replace_placeholders(text: &str, mut swap: impl FnMut(&str) -> String) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find(['{', '}']) {
            Some(close) if after.as_bytes()[close] == b'}' => {
                out.push_str(&swap(&after[..close]));
                rest = &after[close + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// A quicklink's address in Orbit's form. `{argument name="City" default="London"}` becomes
/// `{City=London}`, an unnamed argument or `{query}` becomes `{query}`, and Flow's and Wox's
/// `{q}` does too.
pub fn link_template(url: &str) -> String {
    let mut unnamed = 0;
    replace_placeholders(url, |inner| {
        let (name, attributes) = parse_placeholder(inner);
        let attribute = |key: &str| attributes.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        match name.as_str() {
            "argument" => {
                let named = attribute("name").map(|n| n.chars().filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.')).collect::<String>()).filter(|n| !n.trim().is_empty());
                let name = match named {
                    Some(name) => name.trim().to_string(),
                    None => {
                        unnamed += 1;
                        if unnamed == 1 { "query".into() } else { format!("Text {unnamed}") }
                    }
                };
                match attribute("default").filter(|d| !d.is_empty()) {
                    Some(default) => format!("{{{name}={default}}}"),
                    None => format!("{{{name}}}"),
                }
            }
            "query" | "Query" | "q" => "{query}".into(),
            _ => format!("{{{inner}}}"),
        }
    })
}

/// A snippet's text in Orbit's form. Orbit's own placeholders keep their meaning, a date with a
/// format becomes Orbit's date, and the rest stay as typed. A literal `{{` stays literal.
pub fn snippet_template(text: &str) -> String {
    let escaped = text.replace("{{", "\u{0}");
    let converted = replace_placeholders(&escaped, |inner| {
        let (name, _) = parse_placeholder(inner);
        match name.as_str() {
            "date" | "time" | "clipboard" | "cursor" => format!("{{{name}}}"),
            "datetime" => "{date} {time}".into(),
            _ => format!("{{{inner}}}"),
        }
    });
    converted.replace('\u{0}', "{{{{")
}

fn has_foreign_placeholder(text: &str, known: &[&str]) -> bool {
    let mut found = false;
    let without_literals = text.replace("{{", "");
    replace_placeholders(&without_literals, |inner| {
        let (name, _) = parse_placeholder(inner);
        if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && !known.contains(&name.as_str()) {
            found = true;
        }
        String::new()
    });
    found
}

// --- Flow Launcher and Wox -----------------------------------------------------------------

/// Flow Launcher's or Wox's `Settings.json`: the hotkey. Their own query shortcuts are counted.
pub fn flow_settings(text: &str) -> Haul {
    let Some(value) = parse_json(text) else { return Haul::default() };
    let mut haul = Haul { hotkey: text_hotkey(super::import::text(&value, &["Hotkey"])), ..Haul::default() };
    let shortcuts = list(&value, &["CustomShortcuts"]).len();
    if shortcuts > 0 {
        haul.left_out.push(plural(shortcuts, "query shortcut", "query shortcuts"));
    }
    let plugin_keys = list(&value, &["CustomPluginHotkeys"]).len();
    if plugin_keys > 0 {
        haul.left_out.push(plural(plugin_keys, "plugin hotkey", "plugin hotkeys"));
    }
    haul
}

/// The Web Searches plugin's settings, the same in Flow Launcher and Wox: each search, with its
/// keyword as the alias. One that's switched off stays behind.
pub fn web_searches(text: &str) -> Haul {
    let Some(value) = parse_json(text) else { return Haul::default() };
    let mut haul = Haul::default();
    for source in list(&value, &["SearchSources"]) {
        if source.get("Enabled").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let (name, url) = (super::import::text(source, &["Title"]).trim(), super::import::text(source, &["Url"]).trim());
        if name.is_empty() || url.is_empty() {
            continue;
        }
        haul.links.push(Link { name: name.to_string(), url: link_template(url), alias: clean_alias(super::import::text(source, &["ActionKeyword"])), open_with: String::new() });
    }
    haul
}

/// Folders kept for quick access: Flow's Explorer plugin (`QuickAccessLinks`) and Wox's Folder
/// plugin (`FolderLinks`). Each becomes a quicklink to the folder.
pub fn folder_links(text: &str) -> Haul {
    let Some(value) = parse_json(text) else { return Haul::default() };
    let mut haul = Haul::default();
    for link in list(&value, &["QuickAccessLinks"]).iter().chain(list(&value, &["FolderLinks"])) {
        let path = super::import::text(link, &["Path"]).trim().trim_end_matches('\\');
        if path.is_empty() {
            continue;
        }
        let named = super::import::text(link, &["Name", "Nickname"]).trim();
        let name = if named.is_empty() { path.rsplit('\\').next().unwrap_or(path) } else { named };
        haul.links.push(Link { name: name.to_string(), url: path.to_string(), alias: String::new(), open_with: String::new() });
    }
    haul
}

/// Wox 2, from its database: settings as (key, value), and plugins' settings as
/// (plugin id, key, value).
pub fn wox2(settings: &[(String, String)], plugins: &[(String, String, String)]) -> Haul {
    const WEB_SEARCH: &str = "c1e350a7-c521-4dc3-b4ff-509f720fde86";
    let mut haul = Haul::default();
    if let Some((_, value)) = settings.iter().find(|(key, _)| key == "MainHotkey" || key == "MainHotkey@windows") {
        let hotkey = parse_json(value).map(|json| match &json {
            Value::String(text) => text.clone(),
            other => super::import::text(other, &["windows", "Windows"]).to_string(),
        });
        haul.hotkey = text_hotkey(hotkey.as_deref().unwrap_or(value));
    }
    if let Some((_, value)) = settings.iter().find(|(key, _)| key == "QueryShortcuts") {
        let count = parse_json(value).and_then(|json| json.as_array().map(Vec::len)).unwrap_or(0);
        if count > 0 {
            haul.left_out.push(plural(count, "query shortcut", "query shortcuts"));
        }
    }
    for (plugin, key, value) in plugins {
        if plugin != WEB_SEARCH || key != "webSearches" {
            continue;
        }
        for search in parse_json(value).and_then(|json| json.as_array().cloned()).unwrap_or_default() {
            let off = search.get("Disabled").and_then(Value::as_bool) == Some(true) || search.get("Enabled").and_then(Value::as_bool) == Some(false);
            let url = search.get("Urls").and_then(Value::as_array).and_then(|urls| urls.first()).and_then(Value::as_str).unwrap_or_else(|| super::import::text(&search, &["Url"]));
            let name = super::import::text(&search, &["Title"]).trim();
            if off || name.is_empty() || url.is_empty() {
                continue;
            }
            let url = url.replace("{wox:parameter?name=query}", "{query}");
            haul.links.push(Link { name: name.to_string(), url: link_template(&url), alias: clean_alias(super::import::text(&search, &["Keyword"])), open_with: String::new() });
        }
    }
    haul
}

// --- PowerToys, Command Palette, Keypirinha ------------------------------------------------

/// PowerToys Run's settings: only its hotkey, as it keeps nothing else Orbit has.
pub fn powertoys_run(text: &str) -> Haul {
    let hotkey = parse_json(text).and_then(|value| value.get("properties")?.get("open_powerlauncher").and_then(flags_hotkey));
    Haul { hotkey, ..Haul::default() }
}

/// Command Palette's settings: its hotkey.
pub fn command_palette_settings(text: &str) -> Haul {
    let hotkey = parse_json(text).and_then(|value| value.get("Hotkey").and_then(flags_hotkey));
    Haul { hotkey, ..Haul::default() }
}

/// Command Palette's bookmarks. Their `{name}` blanks are already Orbit's.
pub fn command_palette_bookmarks(text: &str) -> Haul {
    let Some(value) = parse_json(text) else { return Haul::default() };
    let mut haul = Haul::default();
    for bookmark in list(&value, &["Data"]) {
        let (name, target) = (super::import::text(bookmark, &["Name"]).trim(), super::import::text(bookmark, &["Bookmark"]).trim());
        if !name.is_empty() && !target.is_empty() {
            haul.links.push(Link { name: name.to_string(), url: target.to_string(), alias: String::new(), open_with: String::new() });
        }
    }
    haul
}

/// Keypirinha's main settings (`hotkey_run` under `[app]`) and its WebSearch package's custom
/// sites (`[site/Name]` with `url = ...%s`).
pub fn keypirinha(main: &str, web_search: &str) -> Haul {
    let mut haul = Haul::default();
    let mut section = String::new();
    for line in main.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_ascii_lowercase();
        } else if section == "app" {
            if let Some((key, value)) = line.split_once('=') {
                if key.trim() == "hotkey_run" {
                    haul.hotkey = text_hotkey(value.trim());
                }
            }
        }
    }
    let mut site: Option<(String, String)> = None;
    let finish = |site: &mut Option<(String, String)>, haul: &mut Haul| {
        if let Some((name, url)) = site.take() {
            if !url.is_empty() {
                haul.links.push(Link { name, url: url.replace("%s", "{query}"), alias: String::new(), open_with: String::new() });
            }
        }
    };
    for line in web_search.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            finish(&mut site, &mut haul);
            site = name.trim().strip_prefix("site/").map(|name| (name.trim().to_string(), String::new()));
        } else if let (Some((_, url)), Some((key, value))) = (&mut site, line.split_once('=')) {
            if key.trim() == "url" {
                *url = value.trim().to_string();
            }
        }
    }
    finish(&mut site, &mut haul);
    haul
}

// --- Bringing links in ---------------------------------------------------------------------

/// How adding found links to Orbit's went.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Merged {
    pub added: usize,
    /// Orbit already had a link to the same address.
    pub already: usize,
    /// Links Orbit can't open, such as a Mac's `~/Downloads`.
    pub unusable: usize,
    /// Added, but without their alias, because Orbit uses that word already.
    pub aliases_dropped: usize,
}

/// Adds the links Orbit doesn't have yet, checked as the quicklink form checks them. `app_id`
/// finds the app a link opened with, by the name the other launcher gave it.
pub fn merge_links(found: &[Link], links: &mut Vec<QuickLink>, commands: &[CustomCommand], app_id: impl Fn(&str) -> Option<String>) -> Merged {
    let mut merged = Merged::default();
    for link in found {
        let open_with = if link.open_with.is_empty() { String::new() } else { app_id(&link.open_with).unwrap_or_default() };
        let draft = QuickLink { id: String::new(), name: link.name.clone(), url: link.url.clone(), alias: link.alias.clone(), open_with };
        let (checked, dropped) = match check_link(draft.clone(), links, commands) {
            Ok(checked) => (checked, false),
            Err((Field::Alias, _)) => match check_link(QuickLink { alias: String::new(), ..draft }, links, commands) {
                Ok(checked) => (checked, true),
                Err(_) => {
                    merged.unusable += 1;
                    continue;
                }
            },
            Err(_) => {
                merged.unusable += 1;
                continue;
            }
        };
        if links.iter().any(|known| known.url.eq_ignore_ascii_case(&checked.url)) {
            merged.already += 1;
            continue;
        }
        merged.aliases_dropped += dropped as usize;
        merged.added += 1;
        links.push(checked);
    }
    merged
}

/// What an import did, in a sentence or two: "Added 12 quicklinks and 3 snippets. 2 were already
/// in Orbit."
pub fn result_text(links: Merged, snippets: (usize, usize), notes: (usize, usize)) -> String {
    let mut added = Vec::new();
    for (n, one, many) in [(links.added, "quicklink", "quicklinks"), (snippets.0, "snippet", "snippets"), (notes.0, "note", "notes")] {
        if n > 0 {
            added.push(plural(n, one, many));
        }
    }
    let mut text = if added.is_empty() { "Nothing new to add.".to_string() } else { format!("Added {}.", join_list(&added)) };
    let already = links.already + snippets.1 + notes.1;
    if already > 0 {
        text.push_str(&format!(" {} already in Orbit.", if already == 1 { "1 was".to_string() } else { format!("{already} were") }));
    }
    if links.unusable > 0 {
        text.push_str(&format!(" {} can't open on Windows.", plural(links.unusable, "link", "links")));
    }
    if links.aliases_dropped > 0 {
        text.push_str(&format!(" {} came without {}, as Orbit uses {} already.", plural(links.aliases_dropped, "link", "links"), if links.aliases_dropped == 1 { "its alias" } else { "their aliases" }, if links.aliases_dropped == 1 { "that word" } else { "those words" }));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes_gcm::aead::consts::U16;
    use aes_gcm::aead::{Aead, KeyInit};
    use std::io::Write;

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    const PAYLOAD: &str = r#"{
        "quicklinks": {"quicklinks": [
            {"id": "q1", "name": "Search GitHub", "link": "https://github.com/search?q={argument name=\"Query\"}"},
            {"id": "q2", "name": "Weather", "link": "https://weather.example/{argument name=\"City\" default=\"London\"}", "openWith": "Google Chrome"}
        ]},
        "snippets": {"snippets": [
            {"title": "Sign-off", "text": "Thanks,\n{cursor}\nSent {datetime}", "keyword": ";sig"},
            {"title": "Today", "text": "{date format=\"yyyy-MM-dd\"} {selection}", "keyword": ";td"}
        ]},
        "notes": {"notes": [{"title": "Groceries", "markdown": "- milk\n- eggs"}]},
        "settings": {
            "general": {"globalHotkey": {"kind": {"type": "SingleStep", "shortcut": {"modifiers": [{"modifier": "Alt"}], "key": {"type": "LayoutIndependent", "code": 57}}}, "locality": "Global"}},
            "commands": [
                {"extensionId": "e:r:quicklinks", "id": "c:r:quicklinks::*::quicklink::=::q1", "alias": "gh"},
                {"extensionId": "e:r:clipboard-history", "id": "c:r:clipboard-history::-::clipboardHistory", "alias": "clip"}
            ],
            "nodeExtensions": [{"name": "spotify"}]
        }
    }"#;

    /// An export made the way Raycast 2 makes them, to read back.
    fn raycfg3(passphrase: &str) -> Vec<u8> {
        let (salt, iv) = ([7u8; 16], [9u8; 16]);
        let header = format!(r#"{{"appVersion":"2.0.5.0","osName":"Windows","schemaVersion":3,"encryption":{{"iv":"{}","salt":"{}"}}}}"#, hex(&iv), hex(&salt));
        let header = gzip(header.as_bytes());
        let mut key = [0u8; 32];
        scrypt::scrypt(passphrase.as_bytes(), &salt, &scrypt::Params::new(14, 8, 1, 32).unwrap(), &mut key).unwrap();
        let cipher = aes_gcm::AesGcm::<aes::Aes256, U16>::new(&key.into());
        let sealed = cipher.encrypt(aes_gcm::aead::generic_array::GenericArray::from_slice(&iv), gzip(PAYLOAD.as_bytes()).as_slice()).unwrap();
        let mut file = b"RAYCFG3\n".to_vec();
        file.extend((header.len() as u32).to_le_bytes());
        file.extend(header);
        file.extend(sealed);
        file
    }

    #[test]
    fn a_raycast_2_export_opens_with_its_passphrase() {
        let file = raycfg3("correct horse");
        assert!(raycast::needs_passphrase(&file));
        assert_eq!(raycast::read(&file, Some("wrong one")), Err(ReadError::WrongPassphrase));
        let haul = raycast::read(&file, Some("correct horse")).unwrap();
        assert_eq!(haul.links.len(), 2);
        assert_eq!(haul.links[0], Link { name: "Search GitHub".into(), url: "https://github.com/search?q={Query}".into(), alias: "gh".into(), open_with: String::new() });
        assert_eq!(haul.links[1].url, "https://weather.example/{City=London}");
        assert_eq!(haul.links[1].open_with, "Google Chrome");
        assert_eq!(haul.snippets[0], Snippet { name: "Sign-off".into(), text: "Thanks,\n{cursor}\nSent {date} {time}".into(), keyword: ";sig".into() });
        assert_eq!(haul.snippets[1].text, "{date} {selection}");
        assert_eq!(haul.notes, vec![Note { body: "# Groceries\n\n- milk\n- eggs".into() }]);
        assert_eq!(haul.hotkey, Some(Chord::parse("Alt+Space").unwrap()));
        assert!(haul.left_out.iter().any(|line| line.contains("1 extension")));
        assert!(haul.left_out.iter().any(|line| line.contains("1 command")));
        assert!(haul.left_out.iter().any(|line| line.contains("{selection}")));
        assert_eq!(haul.summary(), "2 quicklinks, 2 snippets, 1 note and the hotkey Alt+Space");
    }

    #[test]
    fn a_raycast_for_mac_export_opens_with_the_default_password() {
        use cbc::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
        use sha2::Digest;
        let json = r#"{"raycast_version":"1.102.4","builtin_package_quicklinks":{"quicklinks":[{"name":"Search Google","url":"https://google.com/search?q={argument}"}]},
            "builtin_package_snippets":{"snippets":[{"name":"Email","text":"me@example.com","alias":";em"}]},
            "builtin_package_raycastPreferences":{"preferencesGeneral":{"raycastGlobalHotkey":"Option-49"}}}"#;
        let key = sha2::Sha256::digest(b"12345678");
        let iv = [3u8; 16];
        let sealed = cbc::Encryptor::<aes::Aes256>::new_from_slices(&key, &iv).unwrap().encrypt_padded_vec_mut::<Pkcs7>(&gzip(json.as_bytes()));
        let mut file = iv.to_vec();
        file.extend(sealed);
        assert!(!raycast::needs_passphrase(&file));
        let haul = raycast::read(&file, None).unwrap();
        assert_eq!(haul.links[0].url, "https://google.com/search?q={query}");
        assert_eq!(haul.snippets[0].keyword, ";em");
        assert_eq!(haul.hotkey, Some(Chord::parse("Alt+Space").unwrap()));
        // The same, gzipped without a password.
        assert_eq!(raycast::read(&gzip(json.as_bytes()), None).unwrap(), haul);
    }

    #[test]
    fn raycasts_snippet_and_quicklink_files_read() {
        let links = r#"[{"link":"https://duckduckgo.com/?q={argument}","name":"Search DuckDuckGo"},{"openWith":"Finder","iconName":"folder-16","link":"~/Downloads","name":"Downloads"}]"#;
        let haul = raycast::read(links.as_bytes(), None).unwrap();
        assert_eq!(haul.links.len(), 2);
        assert_eq!(haul.links[0].url, "https://duckduckgo.com/?q={query}");
        let snippets = "\u{feff}[{\"name\":\";date\",\"text\":\"{date format=\\\"yyyy-MM-dd\\\"}\",\"keyword\":\";date\"}]";
        let haul = raycast::read(snippets.as_bytes(), None).unwrap();
        assert_eq!(haul.snippets, vec![Snippet { name: ";date".into(), text: "{date}".into(), keyword: ";date".into() }]);
        assert_eq!(raycast::read(b"not an export", None), Err(ReadError::Unknown));
    }

    #[test]
    fn placeholders_become_orbits() {
        assert_eq!(link_template("https://x.example/{argument}/{argument}"), "https://x.example/{query}/{Text 2}");
        assert_eq!(link_template("https://x.example/?q={q}&{clipboard}"), "https://x.example/?q={query}&{clipboard}");
        assert_eq!(link_template("https://x.example/{argument name=\"tone\" options=\"a, b\"}"), "https://x.example/{tone}");
        assert_eq!(snippet_template("fn main() {{ }} {clipboard offset=1}"), "fn main() {{{{ }} {clipboard}");
        assert_eq!(snippet_template("{time format=\"HH:mm\"} {uuid}"), "{time} {uuid}");
        assert_eq!(snippet_template("a { b"), "a { b");
    }

    #[test]
    fn raycast_hotkeys_read_from_either_platform() {
        let windows: Value = serde_json::from_str(r#"{"kind":{"type":"SingleStep","shortcut":{"modifiers":[{"modifier":"Ctrl"},{"modifier":"Shift"}],"key":{"type":"LayoutIndependent","code":24}}}}"#).unwrap();
        assert_eq!(raycast::raycast_hotkey(&windows, false), Chord::parse("Ctrl+Shift+O").ok());
        let named: Value = serde_json::from_str(r#"{"kind":{"type":"SingleStep","shortcut":{"modifiers":[{"modifier":"Meta"}],"key":{"type":"LayoutDependent","keyType":{"key":"ArrowRight"}}}}}"#).unwrap();
        assert_eq!(raycast::raycast_hotkey(&named, false), Chord::parse("Win+Right").ok());
        let double: Value = serde_json::from_str(r#"{"kind":{"type":"DoubleTap","shortcut":{"modifier":{"modifier":"Ctrl"}}}}"#).unwrap();
        assert_eq!(raycast::raycast_hotkey(&double, false), None);
        assert_eq!(raycast::classic_hotkey("Shift-Command-9"), Chord::parse("Alt+Shift+V").ok());
    }

    #[test]
    fn flow_launcher_and_wox_read() {
        let settings = r#"{"Hotkey":"Alt + Space","CustomShortcuts":[{"Key":"g","Value":"google"}]}"#;
        let haul = flow_settings(settings);
        assert_eq!(haul.hotkey, Chord::parse("Alt+Space").ok());
        assert_eq!(haul.left_out, vec!["1 query shortcut".to_string()]);
        let searches = r#"{"SearchSources":[
            {"Title":"Google","ActionKeyword":"g","Url":"https://www.google.com/search?q={q}","Enabled":true},
            {"Title":"Off","ActionKeyword":"o","Url":"https://off.example/{q}","Enabled":false},
            {"Title":"Everywhere","ActionKeyword":"*","Url":"https://wiki.example/{q}","Enabled":true}]}"#;
        let haul = web_searches(searches);
        assert_eq!(haul.links.len(), 2);
        assert_eq!(haul.links[0], Link { name: "Google".into(), url: "https://www.google.com/search?q={query}".into(), alias: "g".into(), open_with: String::new() });
        assert_eq!(haul.links[1].alias, "");
        let folders = r#"{"QuickAccessLinks":[{"Path":"C:\\Work\\","Type":1}],"FolderLinks":[{"Path":"D:\\Photos"}]}"#;
        let names: Vec<_> = folder_links(folders).links.into_iter().map(|link| (link.name, link.url)).collect();
        assert_eq!(names, vec![("Work".into(), "C:\\Work".into()), ("Photos".into(), "D:\\Photos".into())]);
        let wox = wox2(
            &[("MainHotkey".into(), "\"alt+space\"".into())],
            &[("c1e350a7-c521-4dc3-b4ff-509f720fde86".into(), "webSearches".into(), r#"[{"Urls":["https://www.google.com/search?q={wox:parameter?name=query}"],"Title":"Google","Keyword":"g","Disabled":false}]"#.into())],
        );
        assert_eq!(wox.hotkey, Chord::parse("Alt+Space").ok());
        assert_eq!(wox.links[0].url, "https://www.google.com/search?q={query}");
    }

    #[test]
    fn powertoys_command_palette_and_keypirinha_read() {
        let run = r#"{"properties":{"open_powerlauncher":{"win":false,"ctrl":false,"alt":true,"shift":false,"code":32,"key":"Space"}}}"#;
        assert_eq!(powertoys_run(run).hotkey, Chord::parse("Alt+Space").ok());
        let palette = r#"{"Hotkey":{"win":true,"ctrl":false,"alt":true,"shift":false,"code":32}}"#;
        assert_eq!(command_palette_settings(palette).hotkey, Chord::parse("Win+Alt+Space").ok());
        let bookmarks = r#"{"Data":[{"Id":"1","Name":"Issues","Bookmark":"https://github.com/{repo}/issues"}]}"#;
        assert_eq!(command_palette_bookmarks(bookmarks).links[0].url, "https://github.com/{repo}/issues");
        let main = "[app]\nhotkey_run = Ctrl+Win+K\n";
        let sites = "[main]\nenable = yes\n[site/Crates]\nurl = https://crates.io/search?q=%s\n[site/Empty]\n";
        let haul = keypirinha(main, sites);
        assert_eq!(haul.hotkey, Chord::parse("Ctrl+Win+K").ok());
        assert_eq!(haul.links, vec![Link { name: "Crates".into(), url: "https://crates.io/search?q={query}".into(), alias: String::new(), open_with: String::new() }]);
    }

    #[test]
    fn hauls_add_up_without_repeats() {
        let mut haul = web_searches(r#"{"SearchSources":[{"Title":"Google","Url":"https://g.example/{q}"}]}"#);
        haul.take(web_searches(r#"{"SearchSources":[{"Title":"Google","Url":"https://g.example/{q}"},{"Title":"Bing","Url":"https://b.example/{q}"}]}"#));
        assert_eq!(haul.links.len(), 2);
        assert_eq!(Haul::default().summary(), "Nothing Orbit can use");
    }

    #[test]
    fn links_merge_without_repeats_or_taken_aliases() {
        let mut links = vec![QuickLink { id: "link-a".into(), name: "Google".into(), url: "https://google.com/search?q={query}".into(), alias: "g".into(), open_with: String::new() }];
        let found = vec![
            Link { name: "Google again".into(), url: "https://google.com/search?q={query}".into(), alias: String::new(), open_with: String::new() },
            Link { name: "GitHub".into(), url: "https://github.com/search?q={query}".into(), alias: "g".into(), open_with: "Firefox".into() },
            Link { name: "Downloads".into(), url: "~/Downloads".into(), alias: String::new(), open_with: String::new() },
        ];
        let merged = merge_links(&found, &mut links, &[], |name| (name == "Firefox").then(|| "firefox-id".to_string()));
        assert_eq!(merged, Merged { added: 1, already: 1, unusable: 1, aliases_dropped: 1 });
        assert_eq!(links[1].alias, "");
        assert_eq!(links[1].open_with, "firefox-id");
        assert_eq!(
            result_text(merged, (2, 1), (0, 0)),
            "Added 1 quicklink and 2 snippets. 2 were already in Orbit. 1 link can't open on Windows. 1 link came without its alias, as Orbit uses that word already."
        );
        assert_eq!(result_text(Merged::default(), (0, 0), (0, 0)), "Nothing new to add.");
    }

    /// Reads real exports, when they're on this PC. Not part of the repository.
    #[test]
    #[ignore]
    fn real_raycast_exports() {
        let Ok(folder) = std::env::var("ORBIT_RAYCAST_SAMPLES") else { return };
        for entry in std::fs::read_dir(folder).unwrap().flatten() {
            let bytes = std::fs::read(entry.path()).unwrap();
            let passphrase = std::env::var("ORBIT_RAYCAST_PASSPHRASE").ok();
            match raycast::read(&bytes, passphrase.as_deref()) {
                Ok(haul) => {
                    println!("{}: {} ({:?})", entry.path().display(), haul.summary(), haul.left_out);
                    for link in haul.links.iter().take(3) {
                        println!("    {link:?}");
                    }
                    for snippet in haul.snippets.iter().take(2) {
                        println!("    {snippet:?}");
                    }
                }
                Err(why) => println!("{}: {why}", entry.path().display()),
            }
        }
    }
}
