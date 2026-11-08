//! C ABI over Orbit's shared crates. `include/orbit.h` is the contract: read it for ownership and
//! threading. Every entry point catches panics, so none can unwind into Swift, and tolerates a
//! NULL handle by returning its documented fallback.
#![allow(clippy::missing_safety_doc)]

pub mod engine;
pub mod form;
pub mod hidutil;
pub mod rows;
pub mod worker;

use engine::{AppEntry, Engine};
use form::SettingsForm;
use orbit_core::snippet::{TemplateContext, TriggerMatcher, expand_template, uses_clipboard};
use orbit_core::{Action, Builtin};
use orbit_data::{NewClip, RetentionPolicy, SnippetInput, UsageRecord};
use orbit_hotkeys::{Chord, Effect, HyperConfig, HyperMachine, HyperMode, TapBehavior};
use rows::{OrbitRow, RowList, cstring};
use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::Ordering;
use worker::{Req, Resp};

pub const ABI_VERSION: u32 = 1;

fn guard<T>(fallback: T, body: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(body)).unwrap_or(fallback)
}

unsafe fn text(pointer: *const c_char) -> String {
    if pointer.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(pointer) }.to_string_lossy().into_owned()
}

fn owned(value: &str) -> *mut c_char {
    cstring(value).into_raw()
}

unsafe fn put(out: *mut *mut c_char, value: &str) {
    if !out.is_null() {
        unsafe { *out = owned(value) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn orbit_abi_version() -> u32 {
    ABI_VERSION
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_string_free(value: *mut c_char) {
    if !value.is_null() {
        guard((), || drop(unsafe { CString::from_raw(value) }));
    }
}

/// Writes panic locations to `<data_dir>/crash.log`. The location only, since a panic message
/// could quote user data.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_install_panic_log(data_dir: *const c_char) {
    let dir = PathBuf::from(unsafe { text(data_dir) });
    guard((), move || {
        std::panic::set_hook(Box::new(move |info| {
            use std::io::Write;
            let location = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
            let thread = std::thread::current().name().unwrap_or("?").to_string();
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("crash.log")) {
                let _ = writeln!(file, "panic on thread {thread} at {location}");
            }
        }));
    });
}

// --- Engine -------------------------------------------------------------------------------

pub struct OrbitEngine(Engine);

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_engine_new(data_dir: *const c_char) -> *mut OrbitEngine {
    let dir = PathBuf::from(unsafe { text(data_dir) });
    guard(ptr::null_mut(), || Box::into_raw(Box::new(OrbitEngine(Engine::new(dir)))))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_engine_free(engine: *mut OrbitEngine) {
    if !engine.is_null() {
        guard((), || drop(unsafe { Box::from_raw(engine) }));
    }
}

/// Runs `body` with the engine, or returns `fallback` for NULL or a panic.
unsafe fn with_engine<T>(engine: *mut OrbitEngine, fallback: T, body: impl FnOnce(&mut Engine) -> T) -> T {
    let Some(engine) = (unsafe { engine.as_mut() }) else { return fallback };
    guard(fallback, || body(&mut engine.0))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_engine_take_notice(engine: *mut OrbitEngine) -> *mut c_char {
    unsafe {
        with_engine(engine, ptr::null_mut(), |e| match e.notice.take() {
            Some(notice) => owned(&notice),
            None => ptr::null_mut(),
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_engine_had_cached_apps(engine: *mut OrbitEngine) -> bool {
    unsafe { with_engine(engine, false, |e| e.had_cached_apps) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_engine_host_json(engine: *mut OrbitEngine) -> *mut c_char {
    unsafe {
        with_engine(engine, ptr::null_mut(), |e| match serde_json::to_string(&e.host_view()) {
            Ok(json) => owned(&json),
            Err(_) => ptr::null_mut(),
        })
    }
}

/// -1 on success. Otherwise the settings page with the problem, or -2 if saving failed, and
/// `*message` receives an owned explanation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_engine_apply_form(engine: *mut OrbitEngine, form_json: *const c_char, message: *mut *mut c_char) -> i32 {
    let json = unsafe { text(form_json) };
    unsafe {
        with_engine(engine, -2, |e| {
            let form: SettingsForm = match serde_json::from_str(&json) {
                Ok(form) => form,
                Err(error) => {
                    put(message, &format!("The form could not be read: {error}"));
                    return -2;
                }
            };
            match e.apply_form(&form) {
                Ok(()) => -1,
                Err((page, why)) => {
                    put(message, &why);
                    page
                }
            }
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_engine_set_flag(engine: *mut OrbitEngine, name: *const c_char, value: bool) -> bool {
    let name = unsafe { text(name) };
    unsafe {
        with_engine(engine, false, |e| {
            match name.as_str() {
                "clipboard_enabled" => e.settings.clipboard.enabled = value,
                "low_resource_mode" => e.settings.low_resource_mode = value,
                "launch_at_login" => e.settings.launch_at_login = value,
                "onboarded" => e.settings.onboarded = value,
                _ => return false,
            }
            e.save().is_ok()
        })
    }
}

/// Puts the launcher shortcut, direct shortcuts and Hyper mappings back to the defaults.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_engine_reset_shortcuts(engine: *mut OrbitEngine) -> bool {
    unsafe {
        with_engine(engine, false, |e| {
            e.settings.launcher.hotkey = None;
            e.settings.shortcuts.clear();
            e.settings.hyper.bindings.clear();
            e.save().is_ok()
        })
    }
}

/// Remembers the shortcut problems last shown, so each is announced once and not at every start.
/// True when the text differs from what was last announced.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_engine_note_hotkey_problems(engine: *mut OrbitEngine, text_in: *const c_char) -> bool {
    let problems = unsafe { text(text_in) };
    unsafe {
        with_engine(engine, false, |e| {
            if e.settings.hotkey_notice == problems {
                return false;
            }
            e.settings.hotkey_notice = problems;
            let _ = e.save();
            true
        })
    }
}

/// Stops anything further being written to settings.json. Used before deleting local data.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_engine_set_read_only(engine: *mut OrbitEngine) {
    unsafe {
        with_engine(engine, (), |e| e.read_only = true);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_engine_toggle_pin(engine: *mut OrbitEngine, key: *const c_char) -> bool {
    let key = unsafe { text(key) };
    unsafe {
        with_engine(engine, false, |e| {
            if let Some(at) = e.settings.pinned.iter().position(|p| *p == key) {
                e.settings.pinned.remove(at);
            } else {
                e.settings.pinned.push(key);
            }
            e.save_and_rebuild()
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_engine_hide_item(engine: *mut OrbitEngine, key: *const c_char) -> bool {
    let key = unsafe { text(key) };
    unsafe {
        with_engine(engine, false, |e| {
            if !e.settings.hidden.contains(&key) {
                e.settings.hidden.push(key);
            }
            e.save_and_rebuild()
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_engine_set_alias(engine: *mut OrbitEngine, key: *const c_char, alias: *const c_char) -> bool {
    let (key, alias) = unsafe { (text(key), text(alias)) };
    unsafe {
        with_engine(engine, false, |e| {
            e.settings.aliases.insert(key, alias);
            e.save_and_rebuild()
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_engine_set_indexing(engine: *mut OrbitEngine, indexing: bool) {
    unsafe { with_engine(engine, (), |e| e.indexing = indexing) }
}

/// Replaces the app list from JSON, `[{"id","name","path"}]`. True if it changed. Bad JSON leaves
/// the list as it was and returns false.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_engine_set_apps_json(engine: *mut OrbitEngine, json: *const c_char) -> bool {
    let json = unsafe { text(json) };
    unsafe {
        with_engine(engine, false, |e| match serde_json::from_str::<Vec<AppEntry>>(&json) {
            Ok(apps) => e.set_apps(apps),
            Err(_) => false,
        })
    }
}

/// Writes the app cache. Safe on any thread: it does not touch an engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_apps_save(data_dir: *const c_char, json: *const c_char) -> bool {
    let (dir, json) = unsafe { (text(data_dir), text(json)) };
    guard(false, || engine::write_atomic(&PathBuf::from(dir).join(engine::INDEX_FILE), json.as_bytes()).is_ok())
}

#[repr(C)]
pub struct OrbitQuery {
    pub rows: *const OrbitRow,
    pub count: usize,
    pub async_insert_at: usize,
    pub wants_data: bool,
}

/// Runs the synchronous part of a root query. The rows stay valid until the next call of this
/// function on the same engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_engine_query_root(engine: *mut OrbitEngine, query: *const c_char) -> OrbitQuery {
    let empty = OrbitQuery { rows: ptr::null(), count: 0, async_insert_at: 0, wants_data: false };
    let query = unsafe { text(query) };
    unsafe {
        with_engine(engine, empty, |e| {
            let result = e.query_root(&query);
            e.rows.set(&result.rows);
            OrbitQuery { rows: e.rows.as_ptr(), count: e.rows.len(), async_insert_at: result.async_insert_at, wants_data: result.wants_data }
        })
    }
}

pub const ACTION_NONE: u32 = 0;
pub const ACTION_FOCUS_OR_LAUNCH: u32 = 1;
pub const ACTION_LAUNCH: u32 = 2;
pub const ACTION_URL: u32 = 3;
pub const ACTION_BUILTIN: u32 = 4;
pub const ACTION_CUSTOM: u32 = 5;

#[repr(C)]
pub struct OrbitAction {
    pub kind: u32,
    /// App id, URL, built-in key such as "cmd:settings", or custom command id.
    pub value: *const c_char,
    /// App name, or a template link's alias.
    pub name: *const c_char,
    /// The URL still contains `{query}`, so it needs a query rather than opening.
    pub is_template: bool,
}

fn hold(e: &mut Engine, value: &str) -> *const c_char {
    let string = cstring(value);
    let pointer = string.as_ptr();
    e.held.push(string);
    pointer
}

/// The action for a catalog row. Strings stay valid until the next accessor call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_engine_item_action(engine: *mut OrbitEngine, item: u32, focus_running: bool, out: *mut OrbitAction) -> bool {
    if out.is_null() {
        return false;
    }
    unsafe {
        with_engine(engine, false, |e| {
            let Some(action) = e.catalog.entry_action(item, focus_running) else { return false };
            e.held.clear();
            let (kind, value, name, is_template) = match &action {
                Action::FocusOrLaunchApp { id, name } => (ACTION_FOCUS_OR_LAUNCH, id.clone(), name.clone(), false),
                Action::LaunchApp { id, name } => (ACTION_LAUNCH, id.clone(), name.clone(), false),
                Action::Url { url } => {
                    let alias = e.catalog.link_alias(item).unwrap_or_default();
                    (ACTION_URL, url.clone(), alias, url.contains("{query}"))
                }
                Action::Builtin { command } => (ACTION_BUILTIN, builtin_key(*command).to_string(), String::new(), false),
                Action::Custom { id } => (ACTION_CUSTOM, id.clone(), String::new(), false),
                _ => (ACTION_NONE, String::new(), String::new(), false),
            };
            let value = hold(e, &value);
            let name = hold(e, &name);
            *out = OrbitAction { kind, value, name, is_template };
            true
        })
    }
}

fn builtin_key(command: Builtin) -> &'static str {
    if command == Builtin::ToggleLauncher {
        return orbit_core::bindings_text::LAUNCHER_KEY;
    }
    Builtin::ALL.iter().find(|entry| entry.0 == command).map_or(orbit_core::bindings_text::LAUNCHER_KEY, |entry| entry.1)
}

#[repr(C)]
pub struct OrbitApp {
    pub id: *const c_char,
    pub name: *const c_char,
    pub path: *const c_char,
}

/// Finds an app by id, then by case-insensitive name. Strings stay valid until the next accessor.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_engine_find_app(engine: *mut OrbitEngine, id: *const c_char, name: *const c_char, out: *mut OrbitApp) -> bool {
    if out.is_null() {
        return false;
    }
    let (id, name) = unsafe { (text(id), text(name)) };
    unsafe {
        with_engine(engine, false, |e| {
            let Some(app) = e.catalog.find_app(&id, &name).cloned() else { return false };
            e.held.clear();
            *out = OrbitApp { id: hold(e, &app.id), name: hold(e, &app.name), path: hold(e, &app.path) };
            true
        })
    }
}

#[repr(C)]
pub struct OrbitCustom {
    pub name: *const c_char,
    pub program: *const c_char,
    pub arguments: *const c_char,
    pub working_dir: *const c_char,
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_engine_custom(engine: *mut OrbitEngine, id: *const c_char, out: *mut OrbitCustom) -> bool {
    if out.is_null() {
        return false;
    }
    let id = unsafe { text(id) };
    unsafe {
        with_engine(engine, false, |e| {
            let Some(command) = e.catalog.custom(&id).cloned() else { return false };
            e.held.clear();
            *out = OrbitCustom {
                name: hold(e, &command.name),
                program: hold(e, &command.program),
                arguments: hold(e, &command.arguments),
                working_dir: hold(e, &command.working_dir),
            };
            true
        })
    }
}

#[repr(C)]
#[derive(Default)]
pub struct OrbitIndexStats {
    pub items: u64,
    pub apps: u64,
    pub heap_bytes: u64,
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_engine_index_stats(engine: *mut OrbitEngine) -> OrbitIndexStats {
    unsafe {
        with_engine(engine, OrbitIndexStats::default(), |e| OrbitIndexStats {
            items: e.catalog.index.len() as u64,
            apps: e.catalog.apps.len() as u64,
            heap_bytes: e.catalog.index.heap_bytes() as u64,
        })
    }
}

// --- Pure helpers -------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_builtin_needs_confirmation(key: *const c_char) -> bool {
    let key = unsafe { text(key) };
    guard(false, || Builtin::from_key(&key).is_some_and(Builtin::needs_confirmation))
}

/// Owned. The command's title, in macOS wording. NULL for an unknown key.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_builtin_title(key: *const c_char) -> *mut c_char {
    let key = unsafe { text(key) };
    guard(ptr::null_mut(), || match Builtin::ALL.iter().find(|entry| entry.1 == key) {
        Some(entry) if entry.0 == Builtin::EmptyTrash => owned("Empty Trash"),
        Some(entry) => owned(entry.2),
        None => ptr::null_mut(),
    })
}

/// 0 safe, 1 unusual scheme, 2 blocked scheme (`*scheme` receives an owned name for 1 and 2),
/// 3 invalid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_url_check(url: *const c_char, scheme: *mut *mut c_char) -> i32 {
    use orbit_core::action::{UrlCheck, check_url};
    let url = unsafe { text(url) };
    guard(3, || match check_url(&url) {
        UrlCheck::Safe => 0,
        UrlCheck::Unusual(name) => {
            unsafe { put(scheme, &name) };
            1
        }
        UrlCheck::Blocked(name) => {
            unsafe { put(scheme, &name) };
            2
        }
        UrlCheck::Invalid => 3,
    })
}

/// Parses `Ctrl+Alt+Space`-style text. On success returns true with `*mods` as Orbit's modifier
/// bits and `*key` an owned canonical key name. On failure `*error` receives an owned message.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_chord_parse(chord: *const c_char, mods: *mut u8, key: *mut *mut c_char, error: *mut *mut c_char) -> bool {
    let chord = unsafe { text(chord) };
    guard(false, || match Chord::parse(&chord) {
        Ok(parsed) => {
            unsafe {
                if !mods.is_null() {
                    *mods = parsed.mods.0;
                }
                put(key, &parsed.key);
            }
            true
        }
        Err(why) => {
            unsafe { put(error, &why.to_string()) };
            false
        }
    })
}

/// Owned. The title Orbit derives from a note's first line.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_note_title(body: *const c_char) -> *mut c_char {
    let body = unsafe { text(body) };
    guard(ptr::null_mut(), || owned(&orbit_data::title_from_body(&body)))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_snippet_uses_clipboard(template: *const c_char) -> bool {
    let template = unsafe { text(template) };
    guard(false, || uses_clipboard(&template))
}

/// Owned. Expands a snippet template. `*cursor_back` is the number of characters to move left
/// after inserting, or -1 if there is no `{cursor}`. `clipboard` may be NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_snippet_expand(
    template: *const c_char,
    date: *const c_char,
    time: *const c_char,
    clipboard: *const c_char,
    cursor_back: *mut i64,
    used_clipboard: *mut bool,
) -> *mut c_char {
    let (template, date, time) = unsafe { (text(template), text(date), text(time)) };
    let clip = if clipboard.is_null() { None } else { Some(unsafe { text(clipboard) }) };
    guard(ptr::null_mut(), || {
        let expanded = expand_template(&template, &TemplateContext { date: &date, time: &time, clipboard: clip.as_deref() });
        unsafe {
            if !cursor_back.is_null() {
                *cursor_back = expanded.cursor_back.map_or(-1, |n| n as i64);
            }
            if !used_clipboard.is_null() {
                *used_clipboard = expanded.used_clipboard;
            }
        }
        owned(&expanded.text)
    })
}

/// Owned message if this can't be a text-expansion trigger, or NULL if it can.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_snippet_trigger_problem(trigger: *const c_char) -> *mut c_char {
    use orbit_core::snippet::{MAX_TRIGGER_CHARS, TriggerProblem, validate_trigger};
    let trigger = unsafe { text(trigger) };
    guard(ptr::null_mut(), || match validate_trigger(&trigger) {
        Ok(()) => ptr::null_mut(),
        Err(TriggerProblem::Empty) => ptr::null_mut(), // a snippet need not have a trigger
        Err(TriggerProblem::TooShort) => owned("A trigger needs at least two characters, or it would fire during ordinary typing."),
        Err(TriggerProblem::TooLong) => owned(&format!("A trigger can be at most {MAX_TRIGGER_CHARS} characters.")),
        Err(TriggerProblem::ContainsWhitespace) => owned("A trigger can't contain spaces."),
    })
}

/// Owned JSON `{"conflict": string|null, "commands":[{"matching","set"}]}`. `current` is
/// `hidutil property --matching <matching> --get UserKeyMapping` output, and `matching` NULL means
/// all keyboards. `remove` false plans the Caps Lock to F18 remap; true plans taking only that
/// pair back out.
/// Owned. The hidutil matcher for all keyboards.
#[unsafe(no_mangle)]
pub extern "C" fn orbit_hidutil_keyboards() -> *mut c_char {
    owned(hidutil::KEYBOARD_MATCHING)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_hidutil_plan(current: *const c_char, remove: bool, matching: *const c_char) -> *mut c_char {
    let current = unsafe { text(current) };
    let matching = if matching.is_null() { hidutil::KEYBOARD_MATCHING.to_string() } else { unsafe { text(matching) } };
    guard(ptr::null_mut(), || {
        let plan = if remove {
            hidutil::plan_remove(&current, hidutil::USAGE_CAPS_LOCK, hidutil::USAGE_F18, &matching)
        } else {
            hidutil::plan_apply(&current, hidutil::USAGE_CAPS_LOCK, hidutil::USAGE_F18, &matching)
        };
        serde_json::to_string(&plan).map_or(ptr::null_mut(), |json| owned(&json))
    })
}

// --- HyperKey -----------------------------------------------------------------------------

pub struct OrbitHyper(HyperMachine);

pub const HYPER_MODE_INTERNAL: u32 = 0;
pub const HYPER_MODE_SYNTHETIC: u32 = 1;
pub const HYPER_MODE_EXTERNAL: u32 = 2;
pub const HYPER_TAP_ESCAPE: u32 = 0;
pub const HYPER_TAP_ORIGINAL: u32 = 1;
pub const HYPER_TAP_NONE: u32 = 2;
pub const HYPER_TAP_LEADER: u32 = 3;

pub const HYPER_EFFECT_DISPATCH: u8 = 1;
pub const HYPER_EFFECT_TAP: u8 = 2;
pub const HYPER_EFFECT_SYNTHETIC_DOWN: u8 = 3;
pub const HYPER_EFFECT_SYNTHETIC_RELEASE: u8 = 4;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct OrbitHyperEffect {
    pub kind: u8,
    pub code: u8,
    pub tap: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct OrbitHyperDecision {
    pub swallow: bool,
    pub count: u8,
    pub effects: [OrbitHyperEffect; 2],
}

fn tap_code(tap: TapBehavior) -> u8 {
    match tap {
        TapBehavior::Escape => HYPER_TAP_ESCAPE as u8,
        TapBehavior::Original => HYPER_TAP_ORIGINAL as u8,
        TapBehavior::None => HYPER_TAP_NONE as u8,
        TapBehavior::Leader => HYPER_TAP_LEADER as u8,
    }
}

fn convert(decision: orbit_hotkeys::Decision) -> OrbitHyperDecision {
    let mut out = OrbitHyperDecision { swallow: decision.swallow, ..Default::default() };
    for effect in decision.effects() {
        let converted = match effect {
            Effect::Dispatch { code } => OrbitHyperEffect { kind: HYPER_EFFECT_DISPATCH, code, tap: 0 },
            Effect::Tap(tap) => OrbitHyperEffect { kind: HYPER_EFFECT_TAP, code: 0, tap: tap_code(tap) },
            Effect::SyntheticChordDown { code } => OrbitHyperEffect { kind: HYPER_EFFECT_SYNTHETIC_DOWN, code, tap: 0 },
            Effect::SyntheticRelease => OrbitHyperEffect { kind: HYPER_EFFECT_SYNTHETIC_RELEASE, code: 0, tap: 0 },
        };
        out.effects[out.count as usize] = converted;
        out.count += 1;
    }
    out
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_hyper_new(trigger: u8, mode: u32, tap: u32, tap_timeout_ms: u32, modifier_codes: *const u8, modifier_count: usize) -> *mut OrbitHyper {
    guard(ptr::null_mut(), || {
        let mode = match mode {
            HYPER_MODE_SYNTHETIC => HyperMode::Synthetic,
            HYPER_MODE_EXTERNAL => HyperMode::External,
            _ => HyperMode::Internal,
        };
        let tap = match tap {
            HYPER_TAP_ORIGINAL => TapBehavior::Original,
            HYPER_TAP_NONE => TapBehavior::None,
            HYPER_TAP_LEADER => TapBehavior::Leader,
            _ => TapBehavior::Escape,
        };
        let modifiers = if modifier_codes.is_null() { &[][..] } else { unsafe { std::slice::from_raw_parts(modifier_codes, modifier_count) } };
        Box::into_raw(Box::new(OrbitHyper(HyperMachine::new(HyperConfig { trigger, mode, tap, tap_timeout_ms }, modifiers))))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_hyper_free(hyper: *mut OrbitHyper) {
    if !hyper.is_null() {
        guard((), || drop(unsafe { Box::from_raw(hyper) }));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_hyper_set_bound(hyper: *mut OrbitHyper, codes: *const u8, count: usize) {
    let Some(hyper) = (unsafe { hyper.as_mut() }) else { return };
    guard((), || {
        let codes = if codes.is_null() { &[][..] } else { unsafe { std::slice::from_raw_parts(codes, count) } };
        hyper.0.set_bound_keys(codes.iter().copied());
    })
}

/// Allocation-free and does no I/O. Safe inside an event tap callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_hyper_on_key(hyper: *mut OrbitHyper, code: u8, is_down: bool, time_ms: u32) -> OrbitHyperDecision {
    let Some(hyper) = (unsafe { hyper.as_mut() }) else { return OrbitHyperDecision::default() };
    guard(OrbitHyperDecision::default(), || convert(hyper.0.on_key(code, is_down, time_ms)))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_hyper_reset(hyper: *mut OrbitHyper) -> OrbitHyperDecision {
    let Some(hyper) = (unsafe { hyper.as_mut() }) else { return OrbitHyperDecision::default() };
    guard(OrbitHyperDecision::default(), || convert(hyper.0.reset()))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_hyper_set_suspended(hyper: *mut OrbitHyper, suspended: bool) -> OrbitHyperDecision {
    let Some(hyper) = (unsafe { hyper.as_mut() }) else { return OrbitHyperDecision::default() };
    guard(OrbitHyperDecision::default(), || convert(hyper.0.set_suspended(suspended)))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_hyper_is_idle(hyper: *mut OrbitHyper) -> bool {
    let Some(hyper) = (unsafe { hyper.as_ref() }) else { return true };
    guard(true, || hyper.0.is_idle())
}

// --- Trigger expansion --------------------------------------------------------------------

pub struct OrbitTriggers(TriggerMatcher);

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_triggers_free(triggers: *mut OrbitTriggers) {
    if !triggers.is_null() {
        guard((), || drop(unsafe { Box::from_raw(triggers) }));
    }
}

/// Feeds one typed character. Returns the snippet id on a match and sets `*trigger_chars`, or 0.
/// Allocation-free. The rolling buffer is bounded and never leaves memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_triggers_push(triggers: *mut OrbitTriggers, character: u32, trigger_chars: *mut u64) -> i64 {
    let Some(triggers) = (unsafe { triggers.as_mut() }) else { return 0 };
    let Some(c) = char::from_u32(character) else { return 0 };
    guard(0, || match triggers.0.push(c) {
        Some(hit) => {
            unsafe {
                if !trigger_chars.is_null() {
                    *trigger_chars = hit.trigger_chars as u64;
                }
            }
            hit.snippet_id
        }
        None => 0,
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_triggers_backspace(triggers: *mut OrbitTriggers) {
    let Some(triggers) = (unsafe { triggers.as_mut() }) else { return };
    guard((), || triggers.0.backspace());
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_triggers_clear(triggers: *mut OrbitTriggers) {
    let Some(triggers) = (unsafe { triggers.as_mut() }) else { return };
    guard((), || triggers.0.clear());
}

// --- Worker -------------------------------------------------------------------------------

pub type OrbitWake = Option<unsafe extern "C" fn(*mut c_void)>;

pub struct OrbitWorker {
    handles: worker::WorkerHandles,
    /// What the event returned by the last `orbit_worker_next_event` points into.
    strings: Vec<CString>,
    rows: RowList,
    usage: Option<Vec<UsageRecord>>,
    triggers: Option<Vec<(String, i64)>>,
}

struct SendPtr(*mut c_void);
// The host promises `wake` and `context` may be used from the data thread: see orbit.h.
unsafe impl Send for SendPtr {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_worker_start(data_dir: *const c_char, persist_clipboard: bool, max_items: u32, retention_days: u32, wake: OrbitWake, context: *mut c_void) -> *mut OrbitWorker {
    let dir = PathBuf::from(unsafe { text(data_dir) });
    let context = SendPtr(context);
    guard(ptr::null_mut(), move || {
        let wake_fn = move || {
            let context = &context;
            if let Some(wake) = wake {
                unsafe { wake(context.0) };
            }
        };
        match worker::start(dir, persist_clipboard, RetentionPolicy { max_items, retention_days }, wake_fn) {
            Some(handles) => Box::into_raw(Box::new(OrbitWorker { handles, strings: Vec::new(), rows: RowList::default(), usage: None, triggers: None })),
            None => ptr::null_mut(),
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_worker_free(worker: *mut OrbitWorker) {
    if !worker.is_null() {
        guard((), || drop(unsafe { Box::from_raw(worker) }));
    }
}

unsafe fn send(worker: *mut OrbitWorker, request: Req) {
    if let Some(worker) = unsafe { worker.as_ref() } {
        // A closed channel means the data thread is gone. Nothing useful can be done about it here.
        let _ = worker.handles.tx.send(request);
    }
}

pub const EVENT_NONE: u32 = 0;
pub const EVENT_RESULTS: u32 = 1;
pub const EVENT_USAGE: u32 = 2;
pub const EVENT_NOTE_BODY: u32 = 3;
pub const EVENT_NOTE_SAVED: u32 = 4;
pub const EVENT_NOTE_TEXT: u32 = 5;
pub const EVENT_NOTE_RESTORED: u32 = 6;
pub const EVENT_SNIPPET: u32 = 7;
pub const EVENT_SNIPPET_SAVED: u32 = 8;
pub const EVENT_TRIGGERS: u32 = 9;
pub const EVENT_CLIP: u32 = 10;
pub const EVENT_CHANGED: u32 = 11;
pub const EVENT_EXPORTED: u32 = 12;
pub const EVENT_IMPORTED: u32 = 13;
pub const EVENT_COUNTS: u32 = 14;
pub const EVENT_FAILED: u32 = 15;
pub const EVENT_FINISHED: u32 = 16;

#[repr(C)]
pub struct OrbitEvent {
    pub kind: u32,
    pub scope: u32,
    pub purpose: u32,
    pub clip_kind: u32,
    pub seq: u64,
    pub token: u64,
    pub erase_chars: u64,
    pub id: i64,
    pub notes: i64,
    pub snippets: i64,
    pub clips: i64,
    pub sqlite_bytes: i64,
    pub ok: bool,
    pub favourite: bool,
    pub enabled: bool,
    pub text: *const c_char,
    pub text2: *const c_char,
    pub text3: *const c_char,
    pub text4: *const c_char,
    pub text5: *const c_char,
    pub rows: *const OrbitRow,
    pub row_count: usize,
}

impl OrbitEvent {
    fn empty() -> OrbitEvent {
        let blank = c"".as_ptr();
        OrbitEvent {
            kind: EVENT_NONE,
            scope: 0,
            purpose: 0,
            clip_kind: 0,
            seq: 0,
            token: 0,
            erase_chars: 0,
            id: 0,
            notes: 0,
            snippets: 0,
            clips: 0,
            sqlite_bytes: 0,
            ok: false,
            favourite: false,
            enabled: false,
            text: blank,
            text2: blank,
            text3: blank,
            text4: blank,
            text5: blank,
            rows: ptr::null(),
            row_count: 0,
        }
    }
}

fn fill(worker: &mut OrbitWorker, response: Resp) -> OrbitEvent {
    worker.strings.clear();
    let strings = &mut worker.strings;
    let mut hold = |value: &str| -> *const c_char {
        let string = cstring(value);
        let pointer = string.as_ptr();
        strings.push(string);
        pointer
    };
    let mut event = OrbitEvent::empty();
    match response {
        Resp::Results { seq, scope, query, rows } => {
            worker.rows.set(&rows);
            event.kind = EVENT_RESULTS;
            event.seq = seq;
            event.scope = scope;
            event.text = hold(&query);
            event.rows = worker.rows.as_ptr();
            event.row_count = worker.rows.len();
        }
        Resp::Usage(records) => {
            worker.usage = Some(records);
            event.kind = EVENT_USAGE;
        }
        Resp::NoteBody { id, body } => {
            event.kind = EVENT_NOTE_BODY;
            event.id = id;
            event.text = hold(&body);
        }
        Resp::NoteSaved { token, id } => {
            event.kind = EVENT_NOTE_SAVED;
            event.token = token;
            event.id = id;
        }
        Resp::NoteText { text } => {
            event.kind = EVENT_NOTE_TEXT;
            event.text = hold(&text);
        }
        Resp::NoteRestored => event.kind = EVENT_NOTE_RESTORED,
        Resp::Snippet { snippet, purpose, erase } => {
            event.kind = EVENT_SNIPPET;
            event.purpose = purpose;
            event.erase_chars = erase;
            event.id = snippet.id;
            event.favourite = snippet.favourite;
            event.enabled = snippet.enabled;
            event.text = hold(&snippet.name);
            event.text2 = hold(&snippet.trigger);
            event.text3 = hold(&snippet.content);
            event.text4 = hold(&snippet.description);
            event.text5 = hold(&snippet.category);
        }
        Resp::SnippetSaved { token, id, conflict } => {
            event.kind = EVENT_SNIPPET_SAVED;
            event.token = token;
            event.id = id;
            event.text = hold(conflict.as_deref().unwrap_or(""));
        }
        Resp::Triggers(list) => {
            worker.triggers = Some(list);
            event.kind = EVENT_TRIGGERS;
        }
        Resp::Clip { kind, text, blob, purpose } => {
            event.kind = EVENT_CLIP;
            event.clip_kind = rows::clip_kind_code(kind) as u32;
            event.purpose = purpose;
            event.text = hold(&text);
            event.text2 = hold(&blob);
        }
        Resp::Changed => event.kind = EVENT_CHANGED,
        Resp::Exported(result) => {
            event.kind = EVENT_EXPORTED;
            event.ok = result.is_ok();
            event.text = match &result {
                Ok(path) => hold(&path.to_string_lossy()),
                Err(why) => hold(why),
            };
        }
        Resp::Imported(result) => {
            event.kind = EVENT_IMPORTED;
            event.ok = result.is_ok();
            event.text = match &result {
                Ok(summary) => hold(summary),
                Err(why) => hold(why),
            };
        }
        Resp::Counts { notes, snippets, clips, sqlite_bytes } => {
            event.kind = EVENT_COUNTS;
            event.notes = notes;
            event.snippets = snippets;
            event.clips = clips;
            event.sqlite_bytes = sqlite_bytes;
        }
        Resp::Failed(message) => {
            event.kind = EVENT_FAILED;
            event.text = hold(&message);
        }
        Resp::Finished => event.kind = EVENT_FINISHED,
    }
    event
}

/// Non-blocking. True if `*out` was filled. Its pointers stay valid until the next call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_worker_next_event(worker: *mut OrbitWorker, out: *mut OrbitEvent) -> bool {
    let Some(worker) = (unsafe { worker.as_mut() }) else { return false };
    if out.is_null() {
        return false;
    }
    guard(false, || match worker.handles.rx.try_recv() {
        Ok(response) => {
            let event = fill(worker, response);
            unsafe { *out = event };
            true
        }
        Err(_) => false,
    })
}

/// Moves the usage records from the latest `EVENT_USAGE` into the engine's ranking.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_worker_apply_usage(worker: *mut OrbitWorker, engine: *mut OrbitEngine, now_unix: i64) {
    let Some(worker) = (unsafe { worker.as_mut() }) else { return };
    let Some(records) = worker.usage.take() else { return };
    unsafe { with_engine(engine, (), |e| e.catalog.apply_usage(&records, now_unix)) }
}

/// Owned matcher built from the latest `EVENT_TRIGGERS`, or NULL if there is none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_worker_take_triggers(worker: *mut OrbitWorker) -> *mut OrbitTriggers {
    let Some(worker) = (unsafe { worker.as_mut() }) else { return ptr::null_mut() };
    guard(ptr::null_mut(), || match worker.triggers.take() {
        Some(list) => Box::into_raw(Box::new(OrbitTriggers(TriggerMatcher::new(list.iter().map(|(t, id)| (t.as_str(), *id)))))),
        None => ptr::null_mut(),
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn orbit_worker_search(worker: *mut OrbitWorker, seq: u64, scope: u32, query: *const c_char, limit: u32) {
    let Some(w) = (unsafe { worker.as_ref() }) else { return };
    w.handles.latest_seq.store(seq, Ordering::Relaxed);
    let query = unsafe { text(query) };
    unsafe { send(worker, Req::Search { seq, scope, query, limit: limit as usize }) };
}

macro_rules! request {
    ($name:ident ( $($arg:ident : $ty:ty),* ) => |$($bind:ident),*| $req:expr) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(worker: *mut OrbitWorker, $($arg: $ty),*) {
            $(let $bind = $arg;)*
            let request = guard(None, || Some($req));
            if let Some(request) = request {
                unsafe { send(worker, request) };
            }
        }
    };
}

request!(orbit_worker_load_usage() => | | Req::LoadUsage);
request!(orbit_worker_record_usage(key: *const c_char) => |key| Req::RecordUsage(unsafe { text(key) }));
request!(orbit_worker_note_open(id: i64) => |id| Req::NoteOpen { id });
request!(orbit_worker_note_delete_if_empty(id: i64) => |id| Req::NoteDeleteIfEmpty { id });
request!(orbit_worker_note_set_pinned(id: i64, pinned: bool) => |id, pinned| Req::NoteSetPinned { id, pinned });
request!(orbit_worker_note_trash(id: i64) => |id| Req::NoteTrash { id });
request!(orbit_worker_note_restore_last() => | | Req::NoteRestoreLast);
request!(orbit_worker_note_copy(id: i64) => |id| Req::NoteCopy { id });
request!(orbit_worker_snippet_get(id: i64, purpose: u32, erase_chars: u64) => |id, purpose, erase_chars| Req::SnippetGet { id, purpose, erase: erase_chars });
request!(orbit_worker_snippet_delete(id: i64) => |id| Req::SnippetDelete { id });
request!(orbit_worker_snippet_triggers() => | | Req::SnippetTriggers);
request!(orbit_worker_clip_get(id: i64, purpose: u32) => |id, purpose| Req::ClipGet { id, purpose });
request!(orbit_worker_clip_set_pinned(id: i64, pinned: bool) => |id, pinned| Req::ClipSetPinned { id, pinned });
request!(orbit_worker_clip_delete(id: i64) => |id| Req::ClipDelete { id });
request!(orbit_worker_clip_clear() => | | Req::ClipClear);
request!(orbit_worker_set_clip_policy(max_items: u32, retention_days: u32) => |max_items, retention_days| Req::SetClipPolicy(RetentionPolicy { max_items, retention_days }));
request!(orbit_worker_set_clip_persist(persist: bool) => |persist| Req::SetClipPersist(persist));
request!(orbit_worker_shed() => | | Req::Shed);
request!(orbit_worker_counts() => | | Req::Counts);
request!(orbit_worker_shutdown() => | | Req::Shutdown);
request!(orbit_worker_delete_local_data() => | | Req::DeleteLocalData);
request!(orbit_worker_import(path: *const c_char) => |path| Req::Import { path: PathBuf::from(unsafe { text(path) }) });
request!(orbit_worker_export(settings_json: *const c_char, base_dir: *const c_char, stamp: *const c_char) => |settings_json, base_dir, stamp| Req::Export {
    settings_json: unsafe { text(settings_json) },
    base_dir: PathBuf::from(unsafe { text(base_dir) }),
    stamp: unsafe { text(stamp) },
});
// `id` < 0 means the editor has no note yet.
request!(orbit_worker_note_save(token: u64, id: i64, body: *const c_char) => |token, id, body| Req::NoteSave {
    token,
    id: (id >= 0).then_some(id),
    body: unsafe { text(body) },
});
request!(orbit_worker_snippet_save(token: u64, id: i64, name: *const c_char, trigger: *const c_char, content: *const c_char, description: *const c_char, category: *const c_char, favourite: bool) =>
    |token, id, name, trigger, content, description, category, favourite| Req::SnippetSave {
        token,
        id: (id >= 0).then_some(id),
        input: SnippetInput {
            name: unsafe { text(name) },
            trigger: unsafe { text(trigger) },
            content: unsafe { text(content) },
            description: unsafe { text(description) },
            category: unsafe { text(category) },
            favourite,
            enabled: true,
        },
    });
request!(orbit_worker_clip_add_text(text_value: *const c_char, source_app: *const c_char) => |text_value, source_app| Req::ClipAdd {
    clip: NewClip::text(unsafe { text(text_value) }, &unsafe { text(source_app) }),
    image: None,
});
// `paths` is newline-separated.
request!(orbit_worker_clip_add_files(paths: *const c_char, source_app: *const c_char) => |paths, source_app| Req::ClipAdd {
    clip: {
        let list: Vec<String> = unsafe { text(paths) }.lines().map(str::to_string).collect();
        NewClip::files(&list, &unsafe { text(source_app) })
    },
    image: None,
});
// Copies `len` bytes of PNG.
request!(orbit_worker_clip_add_image(bytes: *const u8, len: usize, width: u32, height: u32, source_app: *const c_char) => |bytes, len, width, height, source_app| {
    let data = if bytes.is_null() { Vec::new() } else { unsafe { std::slice::from_raw_parts(bytes, len) }.to_vec() };
    let hash = worker::image_hash(&data);
    Req::ClipAdd {
        clip: NewClip::image(String::new(), width, height, len as u64, hash, &unsafe { text(source_app) }),
        image: Some(data),
    }
});

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    fn c(value: &str) -> CString {
        CString::new(value).unwrap()
    }

    fn temp(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbit-ffi-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn string(pointer: *const c_char) -> String {
        unsafe { CStr::from_ptr(pointer) }.to_string_lossy().into_owned()
    }

    fn take(pointer: *mut c_char) -> String {
        let value = string(pointer);
        unsafe { orbit_string_free(pointer) };
        value
    }

    static WAKES: Mutex<u32> = Mutex::new(0);
    unsafe extern "C" fn count_wake(_: *mut c_void) {
        *WAKES.lock().unwrap() += 1;
    }

    /// Polls the way the host does after a wake, until `want` arrives.
    fn wait_for(worker: *mut OrbitWorker, want: u32, mut each: impl FnMut(&OrbitEvent)) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            let mut event = OrbitEvent::empty();
            if unsafe { orbit_worker_next_event(worker, &mut event) } {
                each(&event);
                if event.kind == want {
                    return true;
                }
            } else {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        false
    }

    #[test]
    fn null_handles_are_harmless() {
        unsafe {
            orbit_engine_free(ptr::null_mut());
            orbit_worker_free(ptr::null_mut());
            orbit_hyper_free(ptr::null_mut());
            orbit_triggers_free(ptr::null_mut());
            orbit_string_free(ptr::null_mut());
            assert_eq!(orbit_engine_query_root(ptr::null_mut(), c"x".as_ptr()).count, 0);
            assert!(orbit_engine_host_json(ptr::null_mut()).is_null());
            orbit_worker_search(ptr::null_mut(), 1, 0, c"x".as_ptr(), 5);
            orbit_worker_shutdown(ptr::null_mut());
            let mut event = OrbitEvent::empty();
            assert!(!orbit_worker_next_event(ptr::null_mut(), &mut event));
            assert!(!orbit_hyper_on_key(ptr::null_mut(), 1, true, 0).swallow);
            assert_eq!(orbit_triggers_push(ptr::null_mut(), 65, ptr::null_mut()), 0);
        }
    }

    #[test]
    fn panics_become_the_fallback() {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let value = guard(7, || -> i32 { panic!("boom") });
        std::panic::set_hook(previous);
        assert_eq!(value, 7);
    }

    #[test]
    fn engine_round_trip_through_the_abi() {
        let dir = temp("engine");
        let path = c(dir.to_str().unwrap());
        unsafe {
            let engine = orbit_engine_new(path.as_ptr());
            assert!(!engine.is_null());
            assert_eq!(orbit_abi_version(), ABI_VERSION);
            let apps = c(r#"[{"id":"com.apple.calculator","name":"Calculator","path":"/System/Applications/Calculator.app"}]"#);
            assert!(orbit_engine_set_apps_json(engine, apps.as_ptr()));
            assert!(!orbit_engine_set_apps_json(engine, apps.as_ptr()), "unchanged");
            assert!(!orbit_engine_set_apps_json(engine, c"not json".as_ptr()));

            let query = orbit_engine_query_root(engine, c"calculator".as_ptr());
            assert!(query.count >= 1);
            let first = *query.rows;
            assert_eq!(string(first.title), "Calculator");
            assert_eq!(string(first.icon_path), "/System/Applications/Calculator.app");

            let mut action = std::mem::zeroed::<OrbitAction>();
            assert!(orbit_engine_item_action(engine, first.item, true, &mut action));
            assert_eq!(action.kind, ACTION_FOCUS_OR_LAUNCH);
            assert_eq!(string(action.value), "com.apple.calculator");

            let mut app = std::mem::zeroed::<OrbitApp>();
            assert!(orbit_engine_find_app(engine, c"".as_ptr(), c"calculator".as_ptr(), &mut app));
            assert_eq!(string(app.path), "/System/Applications/Calculator.app");
            assert!(!orbit_engine_find_app(engine, c"".as_ptr(), c"nope".as_ptr(), &mut app));

            assert!(orbit_engine_toggle_pin(engine, c"app:com.apple.calculator".as_ptr()));
            let suggestions = orbit_engine_query_root(engine, c"".as_ptr());
            assert_eq!(string((*suggestions.rows).title), "Calculator");

            // Form: an unchanged round trip succeeds, a bad value names its page.
            let host = take(orbit_engine_host_json(engine));
            let view: form::HostView = serde_json::from_str(&host).unwrap();
            let good = c(&serde_json::to_string(&view.form).unwrap());
            let mut message = ptr::null_mut();
            assert_eq!(orbit_engine_apply_form(engine, good.as_ptr(), &mut message), -1);
            assert!(message.is_null());
            let mut bad_form = view.form.clone();
            bad_form.clip_max = "0".into();
            let bad = c(&serde_json::to_string(&bad_form).unwrap());
            assert_eq!(orbit_engine_apply_form(engine, bad.as_ptr(), &mut message), 3);
            assert!(take(message).contains("clipboard items"));
            orbit_engine_free(engine);

            // Settings persisted.
            let engine = orbit_engine_new(path.as_ptr());
            let host = take(orbit_engine_host_json(engine));
            assert!(!host.is_empty());
            let suggestions = orbit_engine_query_root(engine, c"".as_ptr());
            assert!(suggestions.count > 0);
            orbit_engine_free(engine);
        }
    }

    #[test]
    fn worker_round_trip_with_stale_search_skipping() {
        let dir = temp("worker");
        let path = c(dir.to_str().unwrap());
        unsafe {
            let worker = orbit_worker_start(path.as_ptr(), true, 100, 0, Some(count_wake), ptr::null_mut());
            assert!(!worker.is_null());

            orbit_worker_note_save(worker, 9, -1, c"Ask about turbo fitment\nand compare costs".as_ptr());
            let mut saved = 0;
            assert!(wait_for(worker, EVENT_NOTE_SAVED, |e| if e.kind == EVENT_NOTE_SAVED {
                saved = e.id;
                assert_eq!(e.token, 9);
            }));
            assert!(saved > 0);
            assert!(*WAKES.lock().unwrap() > 0, "the wake callback ran");

            // Second save with the same token updates the same note.
            orbit_worker_note_save(worker, 9, -1, c"Ask about turbo fitment\nedited".as_ptr());
            assert!(wait_for(worker, EVENT_NOTE_SAVED, |e| if e.kind == EVENT_NOTE_SAVED {
                assert_eq!(e.id, saved);
            }));

            // Only the newest of a burst of searches may answer: older ones are either skipped by
            // the worker or, if already running, ignored by the host's own seq check.
            for seq in 1..=50u64 {
                orbit_worker_search(worker, seq, worker::SCOPE_ROOT, c"turbo".as_ptr(), 6);
            }
            let mut newest = 0;
            let mut titles = Vec::new();
            assert!(wait_for(worker, EVENT_RESULTS, |e| if e.kind == EVENT_RESULTS {
                newest = e.seq;
                for i in 0..e.row_count {
                    titles.push(string((*e.rows.add(i)).title));
                }
            }));
            assert!(newest <= 50);
            assert_eq!(titles.first().map(String::as_str), Some("Ask about turbo fitment"));

            orbit_worker_note_open(worker, saved);
            assert!(wait_for(worker, EVENT_NOTE_BODY, |e| if e.kind == EVENT_NOTE_BODY {
                assert!(string(e.text).contains("edited"));
            }));

            // Clip kinds and pinned rows.
            orbit_worker_clip_add_text(worker, c"https://example.com".as_ptr(), c"Safari".as_ptr());
            assert!(wait_for(worker, EVENT_CHANGED, |_| {}));
            orbit_worker_search(worker, 100, worker::SCOPE_CLIPBOARD, c"".as_ptr(), 10);
            let mut clip_id = 0;
            assert!(wait_for(worker, EVENT_RESULTS, |e| if e.kind == EVENT_RESULTS && e.seq == 100 {
                assert_eq!(e.row_count, 1);
                let row = *e.rows;
                assert_eq!(row.clip_kind, rows::CLIP_URL);
                assert_eq!(string(row.kind), "Link");
                clip_id = row.id;
            }));
            orbit_worker_clip_get(worker, clip_id, worker::PURPOSE_COPY);
            assert!(wait_for(worker, EVENT_CLIP, |e| if e.kind == EVENT_CLIP {
                assert_eq!(string(e.text), "https://example.com");
            }));

            // A PNG blob lands in the images folder and is removed with the clip.
            orbit_worker_clip_add_image(worker, [1u8, 2, 3].as_ptr(), 3, 1, 1, c"Preview".as_ptr());
            assert!(wait_for(worker, EVENT_CHANGED, |_| {}));
            assert_eq!(std::fs::read_dir(dir.join("clipboard-images")).unwrap().count(), 1);
            orbit_worker_clip_clear(worker);
            assert!(wait_for(worker, EVENT_CHANGED, |_| {}));
            assert_eq!(std::fs::read_dir(dir.join("clipboard-images")).unwrap().count(), 0);

            // Snippets feed the trigger matcher.
            orbit_worker_snippet_save(worker, 1, -1, c"Sig".as_ptr(), c";sig".as_ptr(), c"Regards".as_ptr(), c"".as_ptr(), c"".as_ptr(), false);
            assert!(wait_for(worker, EVENT_TRIGGERS, |_| {}));
            let triggers = orbit_worker_take_triggers(worker);
            assert!(!triggers.is_null());
            assert!(orbit_worker_take_triggers(worker).is_null(), "taken once");
            let mut chars = 0u64;
            let mut hit = 0;
            for ch in ";sig".chars() {
                hit = orbit_triggers_push(triggers, ch as u32, &mut chars);
            }
            assert!(hit > 0);
            assert_eq!(chars, 4);
            orbit_triggers_free(triggers);

            // Export then import: everything is already there, so it is all skipped.
            let export_base = temp("export");
            orbit_worker_export(worker, c"{}".as_ptr(), c(export_base.to_str().unwrap()).as_ptr(), c"2026-11-05 1200".as_ptr());
            let mut folder = String::new();
            assert!(wait_for(worker, EVENT_EXPORTED, |e| if e.kind == EVENT_EXPORTED {
                assert!(e.ok);
                folder = string(e.text);
            }));
            assert!(std::path::Path::new(&folder).join("orbit-export.json").exists());
            orbit_worker_import(worker, c(&format!("{folder}/orbit-export.json")).as_ptr());
            assert!(wait_for(worker, EVENT_IMPORTED, |e| if e.kind == EVENT_IMPORTED {
                assert!(e.ok);
                assert!(string(e.text).contains("Imported 0 notes"), "{}", string(e.text));
            }));

            orbit_worker_counts(worker);
            assert!(wait_for(worker, EVENT_COUNTS, |e| if e.kind == EVENT_COUNTS {
                assert_eq!(e.notes, 1);
                assert_eq!(e.snippets, 1);
            }));

            orbit_worker_shutdown(worker);
            assert!(wait_for(worker, EVENT_FINISHED, |_| {}));
            orbit_worker_free(worker);
        }
    }

    #[test]
    fn hyper_machine_through_the_abi() {
        const F18: u8 = 0x4F;
        const C: u8 = 8;
        const SHIFT: u8 = 0x38;
        unsafe {
            let hyper = orbit_hyper_new(F18, HYPER_MODE_INTERNAL, HYPER_TAP_ESCAPE, 250, [SHIFT].as_ptr(), 1);
            orbit_hyper_set_bound(hyper, [C].as_ptr(), 1);

            assert!(orbit_hyper_on_key(hyper, F18, true, 0).swallow);
            let down = orbit_hyper_on_key(hyper, C, true, 10);
            assert!(down.swallow);
            assert_eq!(down.count, 1);
            assert_eq!(down.effects[0].kind, HYPER_EFFECT_DISPATCH);
            assert_eq!(down.effects[0].code, C);
            assert!(orbit_hyper_on_key(hyper, C, false, 20).swallow);
            assert_eq!(orbit_hyper_on_key(hyper, F18, false, 30).count, 0, "chord release is no tap");
            assert!(orbit_hyper_is_idle(hyper));

            assert!(orbit_hyper_on_key(hyper, F18, true, 100).swallow);
            let tap = orbit_hyper_on_key(hyper, F18, false, 110);
            assert_eq!(tap.effects[0].kind, HYPER_EFFECT_TAP);
            assert_eq!(tap.effects[0].tap, HYPER_TAP_ESCAPE as u8);

            assert!(!orbit_hyper_on_key(hyper, C, true, 200).swallow, "plain typing passes");
            assert!(!orbit_hyper_on_key(hyper, C, false, 210).swallow);

            orbit_hyper_on_key(hyper, F18, true, 300);
            orbit_hyper_reset(hyper);
            assert!(orbit_hyper_is_idle(hyper));
            orbit_hyper_set_suspended(hyper, true);
            assert!(!orbit_hyper_on_key(hyper, F18, true, 400).swallow, "suspended passes all");
            orbit_hyper_free(hyper);
        }
    }

    #[test]
    fn helpers() {
        unsafe {
            assert!(orbit_builtin_needs_confirmation(c"cmd:shutdown".as_ptr()));
            assert!(orbit_builtin_needs_confirmation(c"cmd:empty-trash".as_ptr()));
            assert!(!orbit_builtin_needs_confirmation(c"cmd:settings".as_ptr()));
            assert_eq!(take(orbit_builtin_title(c"cmd:empty-trash".as_ptr())), "Empty Trash");
            assert!(orbit_builtin_title(c"cmd:nope".as_ptr()).is_null());

            let mut scheme = ptr::null_mut();
            assert_eq!(orbit_url_check(c"https://a.b".as_ptr(), &mut scheme), 0);
            assert_eq!(orbit_url_check(c"javascript:alert(1)".as_ptr(), &mut scheme), 2);
            assert_eq!(take(scheme), "javascript");
            assert_eq!(orbit_url_check(c"not a url".as_ptr(), ptr::null_mut()), 3);

            let mut mods = 0u8;
            let mut key = ptr::null_mut();
            let mut error = ptr::null_mut();
            assert!(orbit_chord_parse(c"Ctrl+Option+Shift+F13".as_ptr(), &mut mods, &mut key, &mut error));
            assert_eq!(mods, 1 | 2 | 4);
            assert_eq!(take(key), "F13");
            assert!(!orbit_chord_parse(c"Ctrl+Nope".as_ptr(), &mut mods, ptr::null_mut(), &mut error));
            assert!(take(error).contains("unknown key"));

            assert_eq!(take(orbit_note_title(c"Hello\nworld".as_ptr())), "Hello");

            let (mut back, mut used) = (0i64, false);
            let out = orbit_snippet_expand(c"{date} {clipboard}{cursor}!".as_ptr(), c"D".as_ptr(), c"T".as_ptr(), c"C".as_ptr(), &mut back, &mut used);
            assert_eq!(take(out), "D C!");
            assert_eq!(back, 1);
            assert!(used);
            let out = orbit_snippet_expand(c"plain".as_ptr(), c"".as_ptr(), c"".as_ptr(), ptr::null(), &mut back, &mut used);
            assert_eq!(take(out), "plain");
            assert_eq!(back, -1);
            assert!(!used);

            assert!(orbit_snippet_trigger_problem(c";sig".as_ptr()).is_null());
            assert!(orbit_snippet_trigger_problem(c"".as_ptr()).is_null());
            assert!(take(orbit_snippet_trigger_problem(c"x".as_ptr())).contains("two characters"));

            let plan = take(orbit_hidutil_plan(c"".as_ptr(), false, c"{\"ProductID\":65534}".as_ptr()));
            assert!(plan.contains("65534"), "{plan}");
        }
    }
}
