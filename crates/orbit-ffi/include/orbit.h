/*
 * orbit.h: the C ABI between Orbit's Rust core and a native host.
 *
 * Strings: every string is NUL-terminated UTF-8. Input strings are copied before the call
 * returns, and NULL reads as empty. Interior NULs can't cross this boundary and are dropped.
 *
 * Ownership:
 *   - A `char *` return is owned by the caller. Release it with orbit_string_free. NULL means
 *     "no value" or failure, as noted on the function.
 *   - A `const char *` or `const OrbitRow *` is borrowed. Its lifetime is stated on the function
 *     that produced it. Copy what you need to keep.
 *   - `*_new` and `*_start` handles are released with the matching `*_free`, once.
 *
 * Threads:
 *   - OrbitEngine: one thread at a time. The macOS host uses the main thread only.
 *   - OrbitWorker: request functions may be called from any thread. orbit_worker_next_event and
 *     the borrowed data it returns belong to one thread. The worker runs its own data thread,
 *     which owns SQLite and all blocking file work, so requests never block on disk.
 *   - OrbitHyper and OrbitTriggers: one thread at a time. Their key functions allocate nothing
 *     and do no I/O, so they are safe in an event tap callback.
 *   - orbit_wake_fn runs on the data thread. It must only schedule a drain on the host's thread.
 *
 * Errors: no panic crosses this boundary. A panic inside a call yields that call's fallback
 * (NULL, false, 0, or an empty struct), as does passing a NULL handle.
 */
#ifndef ORBIT_H
#define ORBIT_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define ORBIT_ABI_VERSION 1

typedef struct OrbitEngine OrbitEngine;
typedef struct OrbitWorker OrbitWorker;
typedef struct OrbitHyper OrbitHyper;
typedef struct OrbitTriggers OrbitTriggers;

uint32_t orbit_abi_version(void);
void orbit_string_free(char *value);
/* Sends panic locations (never messages, which could quote user data) to <data_dir>/crash.log. */
void orbit_install_panic_log(const char *data_dir);

/* ---- Rows ---- */

enum { ORBIT_GLYPH_SEARCH = 0, ORBIT_GLYPH_NOTE = 1, ORBIT_GLYPH_SNIPPET = 2, ORBIT_GLYPH_CLIPBOARD = 3,
       ORBIT_GLYPH_CALC = 4, ORBIT_GLYPH_LINK = 5, ORBIT_GLYPH_WEB = 6, ORBIT_GLYPH_PIN = 7,
       ORBIT_GLYPH_FOLDER = 8, ORBIT_GLYPH_COMMAND = 9, ORBIT_GLYPH_INFO = 10, ORBIT_GLYPH_IMAGE = 11,
       ORBIT_GLYPH_SETTINGS = 12, ORBIT_GLYPH_DIAGNOSTICS = 13, ORBIT_GLYPH_RELOAD = 14, ORBIT_GLYPH_PAUSE = 15,
       ORBIT_GLYPH_POWER = 16, ORBIT_GLYPH_LOCK = 17, ORBIT_GLYPH_MOON = 18, ORBIT_GLYPH_DELETE = 19,
       ORBIT_GLYPH_HELP = 20, ORBIT_GLYPH_TERMINAL = 21 };

enum { ORBIT_OP_NONE = 0, ORBIT_OP_RUN_ITEM = 1, ORBIT_OP_COPY = 2, ORBIT_OP_OPEN_URL = 3,
       ORBIT_OP_NOTE_OPEN = 4, ORBIT_OP_SNIPPET_INSERT = 5, ORBIT_OP_CLIP_PASTE = 6 };

enum { ORBIT_SOURCE_PLAIN = 0, ORBIT_SOURCE_CATALOG = 1, ORBIT_SOURCE_NOTE = 2, ORBIT_SOURCE_SNIPPET = 3,
       ORBIT_SOURCE_CLIP = 4 };

enum { ORBIT_CLIP_TEXT = 0, ORBIT_CLIP_URL = 1, ORBIT_CLIP_FILES = 2, ORBIT_CLIP_IMAGE = 3 };

typedef struct {
    const char *title;
    const char *subtitle;
    const char *kind;      /* Label at the row's right edge, may be empty. */
    const char *icon_path; /* Bundle whose icon to show, or empty to use `glyph`. */
    const char *payload;   /* Text for ORBIT_OP_COPY, URL for ORBIT_OP_OPEN_URL. */
    const char *key;       /* Stable identity of a catalog row. */
    int64_t id;            /* Note, snippet or clip id. */
    uint32_t item;         /* Catalog item, for orbit_engine_item_action. */
    uint16_t glyph;
    uint16_t op;           /* What Enter does. ORBIT_OP_NONE makes the row informational. */
    uint16_t source;
    uint16_t clip_kind;
    bool accent;
    bool pinned;
} OrbitRow;

/* ---- Engine: settings, catalog, synchronous search ---- */

/* Loads settings.json and apps.json from data_dir, creating it if needed. Never blocks on SQLite. */
OrbitEngine *orbit_engine_new(const char *data_dir);
void orbit_engine_free(OrbitEngine *engine);
/* Owned. One-time message about damaged or newer settings, or NULL. */
char *orbit_engine_take_notice(OrbitEngine *engine);
bool orbit_engine_had_cached_apps(OrbitEngine *engine);
/* Owned. Everything the running host needs to read, as JSON. See HostView in src/form.rs. */
char *orbit_engine_host_json(OrbitEngine *engine);
/* Validates and saves a settings form (the `form` object of the host JSON).
 * Returns -1 on success, otherwise the settings page index with the problem, or -2 if saving
 * failed. On anything but -1, *message receives an owned explanation. */
int32_t orbit_engine_apply_form(OrbitEngine *engine, const char *form_json, char **message);
/* name: clipboard_enabled, low_resource_mode, launch_at_login or onboarded. Saves. */
bool orbit_engine_set_flag(OrbitEngine *engine, const char *name, bool value);
/* Puts the launcher shortcut, direct shortcuts and Hyper mappings back to the defaults. */
bool orbit_engine_reset_shortcuts(OrbitEngine *engine);
/* True when this shortcut problem text differs from the one last announced. Saves the new text. */
bool orbit_engine_note_hotkey_problems(OrbitEngine *engine, const char *text);
/* Nothing further is written to settings.json. Used before deleting local data. */
void orbit_engine_set_read_only(OrbitEngine *engine);
bool orbit_engine_toggle_pin(OrbitEngine *engine, const char *key);
bool orbit_engine_hide_item(OrbitEngine *engine, const char *key);
bool orbit_engine_set_alias(OrbitEngine *engine, const char *key, const char *alias);
void orbit_engine_set_indexing(OrbitEngine *engine, bool indexing);
/* json: [{"id","name","path"}]. True if the list changed. Bad JSON changes nothing. */
bool orbit_engine_set_apps_json(OrbitEngine *engine, const char *json);
/* Writes the app cache atomically. Any thread: it touches no engine. */
bool orbit_apps_save(const char *data_dir, const char *json);

typedef struct {
    const OrbitRow *rows;
    size_t count;
    size_t async_insert_at; /* Where results from the data thread go. */
    bool wants_data;        /* The host should also send a SCOPE_ROOT search to the worker. */
} OrbitQuery;

/* The synchronous part of a root query: apps, commands, links, calculator, URLs, web fallback.
 * `rows` is valid until the next orbit_engine_query_root on this engine. */
OrbitQuery orbit_engine_query_root(OrbitEngine *engine, const char *query);

enum { ORBIT_ACTION_NONE = 0, ORBIT_ACTION_FOCUS_OR_LAUNCH = 1, ORBIT_ACTION_LAUNCH = 2, ORBIT_ACTION_URL = 3,
       ORBIT_ACTION_BUILTIN = 4, ORBIT_ACTION_CUSTOM = 5 };

typedef struct {
    uint32_t kind;
    const char *value; /* App id, URL, key such as "cmd:settings", or custom command id. */
    const char *name;  /* App name, or a template link's alias. */
    bool is_template;  /* The URL contains {query}, so it needs a query before opening. */
} OrbitAction;

typedef struct { const char *id; const char *name; const char *path; } OrbitApp;
typedef struct { const char *name; const char *program; const char *arguments; const char *working_dir; } OrbitCustom;
typedef struct { uint64_t items; uint64_t apps; uint64_t heap_bytes; } OrbitIndexStats;

/* The next three fill *out with strings valid until the next call of any of them. */
bool orbit_engine_item_action(OrbitEngine *engine, uint32_t item, bool focus_running, OrbitAction *out);
bool orbit_engine_find_app(OrbitEngine *engine, const char *id, const char *name, OrbitApp *out);
bool orbit_engine_custom(OrbitEngine *engine, const char *id, OrbitCustom *out);
OrbitIndexStats orbit_engine_index_stats(OrbitEngine *engine);

/* ---- Pure helpers. Any thread. ---- */

bool orbit_builtin_needs_confirmation(const char *key);
char *orbit_builtin_title(const char *key); /* Owned, NULL for an unknown key. */
/* 0 safe, 1 unusual scheme, 2 blocked scheme, 3 invalid. For 1 and 2, *scheme gets an owned name. */
int32_t orbit_url_check(const char *url, char **scheme);
/* mods: 1 ctrl, 2 alt/option, 4 shift, 8 cmd. *key or *error receives an owned string. */
bool orbit_chord_parse(const char *chord, uint8_t *mods, char **key, char **error);
char *orbit_note_title(const char *body); /* Owned. */
bool orbit_snippet_uses_clipboard(const char *template_text);
/* Owned. *cursor_back is characters to move left afterwards, or -1. clipboard may be NULL. */
char *orbit_snippet_expand(const char *template_text, const char *date, const char *time, const char *clipboard,
                           int64_t *cursor_back, bool *used_clipboard);
/* Owned message if this can't be a text-expansion trigger, or NULL if it can. */
char *orbit_snippet_trigger_problem(const char *trigger);
/* Owned. The hidutil matcher for all keyboards. */
char *orbit_hidutil_keyboards(void);
/* Owned JSON {"conflict":string|null,"commands":[{"matching","set"}]}. `current` is the output of
 * `hidutil property --matching <matching> --get UserKeyMapping`, and `matching` NULL means all
 * keyboards. remove=false plans Caps Lock to F18. remove=true plans taking only that pair out. */
char *orbit_hidutil_plan(const char *current, bool remove, const char *matching);

/* ---- HyperKey state machine ---- */

enum { ORBIT_HYPER_INTERNAL = 0, ORBIT_HYPER_SYNTHETIC = 1, ORBIT_HYPER_EXTERNAL = 2 };
enum { ORBIT_TAP_ESCAPE = 0, ORBIT_TAP_ORIGINAL = 1, ORBIT_TAP_NONE = 2, ORBIT_TAP_LEADER = 3 };
enum { ORBIT_EFFECT_DISPATCH = 1, ORBIT_EFFECT_TAP = 2, ORBIT_EFFECT_SYNTHETIC_DOWN = 3, ORBIT_EFFECT_SYNTHETIC_RELEASE = 4 };

typedef struct { uint8_t kind; uint8_t code; uint8_t tap; } OrbitHyperEffect;
typedef struct { bool swallow; uint8_t count; OrbitHyperEffect effects[2]; } OrbitHyperDecision;

OrbitHyper *orbit_hyper_new(uint8_t trigger, uint32_t mode, uint32_t tap, uint32_t tap_timeout_ms,
                            const uint8_t *modifier_codes, size_t modifier_count);
void orbit_hyper_free(OrbitHyper *hyper);
void orbit_hyper_set_bound(OrbitHyper *hyper, const uint8_t *codes, size_t count);
/* Key codes are macOS virtual key codes. time_ms is a wrapping millisecond clock. */
OrbitHyperDecision orbit_hyper_on_key(OrbitHyper *hyper, uint8_t code, bool is_down, uint32_t time_ms);
OrbitHyperDecision orbit_hyper_reset(OrbitHyper *hyper);
OrbitHyperDecision orbit_hyper_set_suspended(OrbitHyper *hyper, bool suspended);
bool orbit_hyper_is_idle(OrbitHyper *hyper);

/* ---- Trigger matcher. Bounded rolling buffer, memory only. ---- */

void orbit_triggers_free(OrbitTriggers *triggers);
/* The snippet id on a match, with *trigger_chars set. Otherwise 0. */
int64_t orbit_triggers_push(OrbitTriggers *triggers, uint32_t character, uint64_t *trigger_chars);
void orbit_triggers_backspace(OrbitTriggers *triggers);
void orbit_triggers_clear(OrbitTriggers *triggers);

/* ---- Worker: the data thread ---- */

enum { ORBIT_SCOPE_ROOT = 0, ORBIT_SCOPE_NOTES = 1, ORBIT_SCOPE_SNIPPETS = 2, ORBIT_SCOPE_CLIPBOARD = 3 };
enum { ORBIT_PURPOSE_INSERT = 0, ORBIT_PURPOSE_COPY = 1, ORBIT_PURPOSE_EXPAND = 2, ORBIT_PURPOSE_EDIT = 3 };
enum { ORBIT_EVENT_NONE = 0, ORBIT_EVENT_RESULTS = 1, ORBIT_EVENT_USAGE = 2, ORBIT_EVENT_NOTE_BODY = 3,
       ORBIT_EVENT_NOTE_SAVED = 4, ORBIT_EVENT_NOTE_TEXT = 5, ORBIT_EVENT_NOTE_RESTORED = 6, ORBIT_EVENT_SNIPPET = 7,
       ORBIT_EVENT_SNIPPET_SAVED = 8, ORBIT_EVENT_TRIGGERS = 9, ORBIT_EVENT_CLIP = 10, ORBIT_EVENT_CHANGED = 11,
       ORBIT_EVENT_EXPORTED = 12, ORBIT_EVENT_IMPORTED = 13, ORBIT_EVENT_COUNTS = 14, ORBIT_EVENT_FAILED = 15,
       ORBIT_EVENT_FINISHED = 16 };

typedef void (*orbit_wake_fn)(void *context);

/*
 * Field use by kind. Strings not listed are empty, never NULL.
 *   RESULTS       seq scope text=query rows row_count
 *   USAGE         then call orbit_worker_apply_usage
 *   NOTE_BODY     id text=body
 *   NOTE_SAVED    token id
 *   NOTE_TEXT     text=body
 *   SNIPPET       id purpose erase_chars favourite enabled
 *                 text=name text2=trigger text3=content text4=description text5=category
 *   SNIPPET_SAVED token id text=name of the older snippet with this trigger, or empty
 *   TRIGGERS      then call orbit_worker_take_triggers
 *   CLIP          clip_kind purpose text=text or paths text2=image path
 *   EXPORTED      ok text=folder or error
 *   IMPORTED      ok text=summary or error
 *   COUNTS        notes snippets clips sqlite_bytes
 *   FAILED        text=message, safe to show. Never contains clipboard or note text.
 */
typedef struct {
    uint32_t kind;
    uint32_t scope;
    uint32_t purpose;
    uint32_t clip_kind;
    uint64_t seq;
    uint64_t token;
    uint64_t erase_chars;
    int64_t id;
    int64_t notes;
    int64_t snippets;
    int64_t clips;
    int64_t sqlite_bytes;
    bool ok;
    bool favourite;
    bool enabled;
    const char *text;
    const char *text2;
    const char *text3;
    const char *text4;
    const char *text5;
    const OrbitRow *rows;
    size_t row_count;
} OrbitEvent;

/* Starts the data thread. The store opens on that thread, so this returns immediately. */
OrbitWorker *orbit_worker_start(const char *data_dir, bool persist_clipboard, uint32_t max_items,
                                uint32_t retention_days, orbit_wake_fn wake, void *context);
/* After FINISHED, or at exit. Closes the request channel, which ends the data thread. */
void orbit_worker_free(OrbitWorker *worker);
/* Non-blocking. True if *out was filled. Its pointers are valid until the next call. */
bool orbit_worker_next_event(OrbitWorker *worker, OrbitEvent *out);
void orbit_worker_apply_usage(OrbitWorker *worker, OrbitEngine *engine, int64_t now_unix);
/* Owned matcher from the latest TRIGGERS event, or NULL. */
OrbitTriggers *orbit_worker_take_triggers(OrbitWorker *worker);

/* Requests. Each returns at once. A search older than the latest seq is skipped. */
void orbit_worker_search(OrbitWorker *worker, uint64_t seq, uint32_t scope, const char *query, uint32_t limit);
void orbit_worker_load_usage(OrbitWorker *worker);
void orbit_worker_record_usage(OrbitWorker *worker, const char *key);
void orbit_worker_note_open(OrbitWorker *worker, int64_t id);
/* id < 0: the editor has no note yet. Its first non-empty save for `token` creates one. */
void orbit_worker_note_save(OrbitWorker *worker, uint64_t token, int64_t id, const char *body);
void orbit_worker_note_delete_if_empty(OrbitWorker *worker, int64_t id);
void orbit_worker_note_set_pinned(OrbitWorker *worker, int64_t id, bool pinned);
void orbit_worker_note_trash(OrbitWorker *worker, int64_t id);
void orbit_worker_note_restore_last(OrbitWorker *worker);
void orbit_worker_note_copy(OrbitWorker *worker, int64_t id);
void orbit_worker_snippet_get(OrbitWorker *worker, int64_t id, uint32_t purpose, uint64_t erase_chars);
void orbit_worker_snippet_save(OrbitWorker *worker, uint64_t token, int64_t id, const char *name, const char *trigger,
                               const char *content, const char *description, const char *category, bool favourite);
void orbit_worker_snippet_delete(OrbitWorker *worker, int64_t id);
void orbit_worker_snippet_triggers(OrbitWorker *worker);
void orbit_worker_clip_add_text(OrbitWorker *worker, const char *text, const char *source_app);
void orbit_worker_clip_add_files(OrbitWorker *worker, const char *newline_separated_paths, const char *source_app);
/* Copies len bytes of PNG. */
void orbit_worker_clip_add_image(OrbitWorker *worker, const uint8_t *bytes, size_t len, uint32_t width, uint32_t height,
                                 const char *source_app);
void orbit_worker_clip_get(OrbitWorker *worker, int64_t id, uint32_t purpose);
void orbit_worker_clip_set_pinned(OrbitWorker *worker, int64_t id, bool pinned);
void orbit_worker_clip_delete(OrbitWorker *worker, int64_t id);
void orbit_worker_clip_clear(OrbitWorker *worker);
void orbit_worker_set_clip_policy(OrbitWorker *worker, uint32_t max_items, uint32_t retention_days);
void orbit_worker_set_clip_persist(OrbitWorker *worker, bool persist);
void orbit_worker_export(OrbitWorker *worker, const char *settings_json, const char *base_dir, const char *stamp);
void orbit_worker_import(OrbitWorker *worker, const char *path);
void orbit_worker_shed(OrbitWorker *worker);
void orbit_worker_counts(OrbitWorker *worker);
/* Finishes earlier requests, checkpoints, then replies with FINISHED. */
void orbit_worker_shutdown(OrbitWorker *worker);
/* Removes what Orbit stored, by name, then replies with FINISHED. Send only after confirmation. */
void orbit_worker_delete_local_data(OrbitWorker *worker);

#ifdef __cplusplus
}
#endif
#endif /* ORBIT_H */
