# Orbit architecture

This document describes the code in this checkout: three platform-neutral Rust crates, a benchmark crate, and the Windows host. Every statement was checked against the source; file paths are given so you can check it again.

Measured numbers (latency, memory, idle cost) are not in this document. See [PERFORMANCE_BUDGET.md](PERFORMANCE_BUDGET.md).

Related documents: [SHORTCUT_SEMANTICS.md](SHORTCUT_SEMANTICS.md), [PRIVACY.md](PRIVACY.md), [DATA_PORTABILITY.md](DATA_PORTABILITY.md).

## Layout

| Path | Kind | Role |
|---|---|---|
| `crates/orbit-hotkeys` | library | Shortcut text form (`Chord`, `Mods`) and the HyperKey state machine (`HyperMachine`). No OS calls. Depends only on `serde`. |
| `crates/orbit-core` | library | Search index and ranking, calculator, actions and built-in command list, settings schema, snippet templates and the trigger matcher, plain-text binding tables. No OS calls, no background threads. Depends on `orbit-hotkeys`. |
| `crates/orbit-data` | library | SQLite store: notes, snippets, clipboard history, usage counts, export and import. Depends on `orbit-core` and `rusqlite` (bundled SQLite). |
| `crates/orbit-bench` | binaries | Benchmark harness (`orbit-bench`, `orbit-bench-probe`). Not part of the shipped app. Its end-to-end suite is Windows-only and drives a real `orbit` process. |
| `platforms/windows` | host (binary `orbit`) | The Windows app. Pure Rust, the `windows` crate (0.62), Win32, GDI and DirectWrite. No web runtime. |

Dependency direction is one way: `orbit-hotkeys` ← `orbit-core` ← `orbit-data` ← host. A host owns every OS call. The shared crates never create threads and never touch the OS.

Build settings that affect the architecture (`Cargo.toml`, `.cargo/config.toml`):

- `panic = "unwind"` in release, so a panic on an optional thread does not abort the process. There is no code that restarts a thread that has panicked.
- The C runtime is linked statically on the Windows targets.

## What the shared crates are responsible for

| Concern | Where |
|---|---|
| Chord parsing and printing (`Alt+Space`, `Hyper+C`) | `orbit-hotkeys/src/chord.rs` |
| HyperKey decisions (swallow or pass, dispatch, tap, synthetic chord) | `orbit-hotkeys/src/hyper.rs` |
| App, command and link search, ranking | `orbit-core/src/search.rs`, `text.rs` |
| Calculator | `orbit-core/src/calc.rs` |
| Actions, built-in command list, URL scheme checks | `orbit-core/src/action.rs` |
| Settings schema, defaults, load outcome | `orbit-core/src/settings.rs` |
| Text tables for bindings (Settings' "Edit as text" view); link and command parsers no window uses now | `orbit-core/src/bindings_text.rs` |
| Snippet placeholders and the trigger matcher | `orbit-core/src/snippet.rs` |
| Notes, snippets, clipboard rows, usage, FTS5 search, export and import | `orbit-data/src/*.rs` |

The host does everything else: windows, drawing, hotkey registration, the keyboard hook, clipboard access, text injection, launching, file watching, the app indexer, tray, autostart.

## Boundary contracts (ABI)

`crates/orbit-ffi` is a C ABI over the shared crates for the macOS host (`include/orbit.h`). The Windows host links the Rust crates directly, so on Windows the boundary is the Rust API plus the contracts below. These are the rules any host has to follow.

**`HyperMachine`** (`orbit-hotkeys/src/hyper.rs`, module comment)

- Key codes are host-defined `u8` values. The Windows host uses virtual-key codes.
- The host calls `on_key` for every physical key event except events it injected itself. The call does no I/O and no allocation.
- The host applies the returned `Decision` (swallow or pass) and performs its `Effect`s in order.
- The host calls `reset` on lock and suspend, and performs the effects it returns.

**`Store`** (`orbit-data/src/lib.rs`, module comment)

- One `Store` is owned by one thread and is never touched from the UI or an input thread. In the Windows host that is the `orbit-data` thread.
- Queries return small summaries with bounded previews. Full note, snippet and clipboard text is fetched only when needed.

**Settings** (`orbit-core/src/settings.rs`)

- One JSON file, `schema_version` 1. Unknown fields are ignored and missing sections take defaults. A file from a newer schema loads read-only. See [DATA_PORTABILITY.md](DATA_PORTABILITY.md).

**Cross-thread handles** (`platforms/windows/src/util.rs`): `SendHwnd` wraps a window handle as an `isize` so other threads can `PostMessage` to the UI thread. Posting is the only thing done through it.

## Windows host: processes and threads

One resident process, `orbit.exe`. A second launch with the same data directory posts a toggle message to the first window and exits (named mutex `Local\OrbitLauncher-<hash of data dir>`, `main.rs`).

| Thread or process | Exists when | Priority | Does |
|---|---|---|---|
| **UI** (main thread) | Always | Above normal | Owns the launcher window, the search index (`Catalog`), settings, the tray icon, the note, snippet and settings windows, and the message loop. All state lives in one `App` behind a `RefCell`. Handles hotkeys, session and power events, clipboard-change events and replies from other threads. |
| **`orbit-data`** | Always, from startup | Below normal | Owns the SQLite `Store`. Does all database work, plus file reads and writes for icons, clipboard image files, export and import. Takes requests over an `mpsc` channel, answers over another, and posts `WM_ORBIT_DATA` to wake the UI. A search whose sequence number is already stale is skipped. Runs migrations when it opens the store. |
| **`orbit-hook`** | Only while a feature needs a hook: HyperKey in `internal` or `synthetic` mode, or snippet expansion. Not running by default. Not running while shortcuts are suspended. | Highest | Installs `WH_KEYBOARD_LL` and a foreground-change `WinEvent` hook, runs the state machines, and posts recognised events to the UI thread. Has its own message loop for control messages. |
| **`orbit-watch`** | Always (exits at once if it cannot watch any folder) | Default | Sleeps in `WaitForMultipleObjects` on change notifications for the two Start Menu folders. After changes stop for a quiet period it posts `WM_ORBIT_INDEX_DIRTY`. No timers. |
| `orbit-launch` | Per launch | Default | Calls `ShellExecuteExW`, so a slow launch never blocks the UI. Posts an error message back on failure. |
| `orbit-insert` | Per text insertion | Default | Waits for the target window to be foreground, then types or pastes the text. Posts the outcome back. |
| `orbit-clip` | Per clipboard change | Default | Reads the clipboard after `WM_CLIPBOARDUPDATE`, applies the capture rules, sends the result to the data thread. |
| `orbit-index-wait` | While an index run is active | Default | Waits for the indexer child process and posts `WM_ORBIT_INDEX_DONE`. |
| **Indexer child process** | While indexing | Idle priority class, EcoQoS | `orbit --index-apps --data-dir <dir>`. Short-lived. Enumerates apps, writes `apps.json` and `icons/*.bgra` atomically, exits with a status code. Started at launch if `apps.json` is missing or older than a day, when the Start Menu watcher reports changes, and on "Rebuild App Index". Only one runs at a time; a request during a run is remembered and run afterwards (`index_dirty`). |

Other files in the host: `launch.rs` finds running windows and force-foregrounds; `catalog.rs` builds the search index from apps, built-ins, links, custom commands, aliases and usage; `editor.rs`, `link_form.rs` and `settings_win.rs` are the note, snippet, quicklink, command and settings windows (created on the UI thread), built from the controls in `forms.rs`; `ui.rs`, `gfx.rs`, `text.rs`, `view.rs`, `rows.rs`, `grid.rs` draw into a CPU back buffer, with the launcher's text from DirectWrite.

The launcher is two windows. The main one is layered and presented with `UpdateLayeredWindow`, so it can have antialiased corners and a soft shadow: panel plus a shadow margin on every side, with mouse positions converted back to the panel. Below it sits a click-through "plate" window that holds the acrylic blur, clipped to a rounded region and moved with the main window. With glass off, or where Windows can't blur, the plate isn't used and the panel is opaque.

The small menu (`view.menu`) keeps the list behind it (`Behind`: scope, query, rows, scroll, selection), so closing it restores what was there without searching again. The emoji list is a grid laid out by `grid.rs`. Its table is parsed when the list opens and dropped with its glyph cache when the launcher hides.

### Re-entrancy on the UI thread

`main_wndproc` borrows `App` for each message. A modal loop (the tray menu) cannot hold that borrow, so the tray menu is handled before the borrow. If a message arrives while the borrow is held, a defined set of messages (`is_deferrable`: commands, hotkeys, timers, clipboard, power, session, app messages) is queued and replayed afterwards instead of being lost.

## Message flow

All cross-thread notifications are window messages posted to the main window, or `mpsc` channels plus a wake-up message.

**Launcher hotkey and direct shortcuts.** `RegisterHotKey` on the main window → `WM_HOTKEY` → `on_hotkey` → show/hide the launcher or run the bound action. No hook involved.

**HyperKey (internal and synthetic modes).** Keyboard → `orbit-hook` callback → `HyperMachine::on_key` → for a bound key, `PostMessage(WM_ORBIT_HYPER, key code, event time)` → UI thread looks up the action and runs it. The callback performs no database access, allocates nothing and starts no process. Tap and synthetic-chord effects are carried out in the callback with `SendInput`, to keep their order against the key that caused them, which is the one place the hook thread does work other than deciding. Pointers posted between threads go through an allowlist (`util::box_for_post`, `take_posted`), so a stray message can't be taken for one.

**HyperKey (external mode).** No hook. Each binding is registered as `Ctrl+Alt+Shift+Win+<key>` with `RegisterHotKey`; an outside remapper is expected to produce that chord.

**Text expansion.** Hook callback feeds the `TriggerMatcher` → on a match, posts `WM_ORBIT_EXPAND (snippet id, trigger length)` → UI thread (skips if suspended) → `Req::SnippetGet` to the data thread → `Resp::Snippet` → UI expands placeholders → if the template uses `{clipboard}`, the UI shows a preview and waits for approval; otherwise → `orbit-insert` thread, which checks for a password field through UI Automation and the classic edit style before erasing the trigger, then types the text in chunks.

**Typing a query.** The edit control's change notification → the UI thread searches the in-memory index synchronously for apps, commands and links. For notes and snippets it sends `Req::Search` with a sequence number to the data thread, which answers with `Resp::Results`; the UI merges them only if the sequence number is still current, so late replies do not reorder rows.

**Clipboard history.** `AddClipboardFormatListener` → `WM_CLIPBOARDUPDATE` → spawn `orbit-clip` → capture rules → `Req::ClipAdd` → data thread writes the image file if any, then the row, then applies retention.

**Session and power.** `WTSRegisterSessionNotification` (this session only) and `WM_POWERBROADCAST`. On lock or suspend the UI hides the launcher and tells the hook thread to reset. On unlock and resume it asks the hook thread to reinstall the hook and re-registers all hotkeys. Details in [SHORTCUT_SEMANTICS.md](SHORTCUT_SEMANTICS.md).

### Work that still happens on the UI thread

The brief asks that the event loop never block on disk. These disk or blocking operations do run on the UI thread in this code:

- Reading `settings.json` and `apps.json` during startup.
- Writing `settings.json` (`save_settings`, atomic write).
- Reading a clipboard image file when the user pastes one (`on_clip_content`).
- Waiting for the hook thread to report whether the hook installed (`HookHandle::start` blocks on a channel until the hook thread answers).
- Reading and writing the clipboard when expanding a template that includes `{clipboard}`, and when copying a note or snippet.

Database migration, searches, saves, export and import do run off the UI thread.

## Memory reclaim

There is no background polling for memory. Each mechanism is triggered by a user action or a settings change.

| Mechanism | Trigger | Effect |
|---|---|---|
| Back buffer dropped | Launcher hides | The GDI frame buffer is released and recreated on the next show. |
| Working-set trim | A timer started when the launcher hides; cancelled on show | `SetProcessWorkingSetSizeEx` with the "empty" values. Delay comes from `--trim-ms`, disabled by `--no-trim`; default is set in `main.rs`. |
| SQLite page-cache shed | Memory pressure seen at show time, entering low resource mode, closing an editor | `PRAGMA shrink_memory` on both connections. |
| Small icon cache | Low Resource Mode, or memory pressure | Icon cache capacity drops to a smaller fixed size. |
| Memory-pressure check | Each time the launcher is shown | `GlobalMemoryStatusEx` load percentage against a threshold in `check_memory_pressure`. Restored when pressure ends. |
| Rich-edit and heap release | A short delay after a note editor closes | Unloads `msftedit.dll` if no note editor is open and calls `HeapSetInformation(HeapOptimizeResources)`. |
| Image capture off | Low Resource Mode | Clipboard images are not captured. |

Bounded by design, not by reclaim: `orbit-data` sets a small SQLite page cache per connection and disables memory mapping (`tune` in `orbit-data/src/lib.rs`); queries return previews instead of full records; the search index is one string arena (`orbit-core/src/search.rs`). The test `ten_thousand_records_stay_on_disk` in `orbit-data/src/lib.rs` checks that opening a store with 10,000 records of each kind does not load them.

## Startup and shutdown

Order, from `main.rs` and `app::startup`:

1. Parse arguments. `--index-apps` runs the indexer and exits without creating a window.
2. Single-instance check. A second instance toggles the first and exits.
3. Create the data directory, install the panic hook and the unhandled-exception filter. Both release any virtual modifiers. The panic hook appends the thread name and `file:line` to `crash.log`, never the panic message.
4. Load `settings.json` (damaged file moved to `settings.damaged.json`, newer-schema file loaded read-only) and `apps.json`.
5. Create the hidden launcher window and its edit control.
6. Start the `orbit-data` thread (opens the store and migrates there).
7. Build the in-memory search index.
8. Add the tray icon, register hotkeys, start the keyboard hook if a feature needs it, start the clipboard listener if history is on.
9. Register for session notifications, start the Start Menu watcher, ask the data thread for usage counts.
10. Start the indexer child if `apps.json` is missing, or stale and `--no-reindex` was not given.
11. On the first run, show a "Welcome to Orbit" notification and save `onboarded = true`.

Clean exit (`begin_exit`): close editors, drop the hook (which resets the state machine and releases virtual modifiers), unregister hotkeys, remove the clipboard listener, remove the tray icon, flush the bench log if any, then send `Shutdown` to the data thread, which runs `PRAGMA optimize` and `wal_checkpoint(TRUNCATE)` on the main database. A timer forces exit if anything stalls. `WM_ENDSESSION` also takes this path, and then blocks the UI thread for a bounded time until the data thread reports it has finished its last write (`worker::wait_for_finish`), because Windows may end the process as soon as the handler returns.

Command-line options (`main.rs`): `--data-dir`, `--index-apps`, `--refresh-icons`, `--bench-log`, `--hotkey`, `--trim-ms`, `--no-trim`, `--no-reindex`, `--wait-pid`, `--accept-injected-input`. `--bench-log`, `--accept-injected-input` and the benchmark harness exist for measurement and testing; see [PERFORMANCE_BUDGET.md](PERFORMANCE_BUDGET.md).

For checking how Settings and the forms look, `orbit --data-dir <folder> --preview-settings <page, link-form, command-form or snippet-form> <out.png>` opens one off-screen in a window that can't take focus, saves a picture and exits, before the single-instance check and before any hook is installed. It needs `--data-dir`, and adds `--preview-theme dark|light`, `--preview-scroll <pixels>`, `--preview-empty` and `--preview-arm-delete`. Details are in `settings_preview.rs`.

## Data layout

Default data directory: `%LOCALAPPDATA%\Orbit` (falls back to `%USERPROFILE%\AppData\Local\Orbit`, then the temp directory). `--data-dir` overrides it, and each data directory gets its own instance mutex.

| Path in the data directory | Written by | Contents |
|---|---|---|
| `settings.json` | UI thread | Settings, schema version 1. |
| `settings.damaged.json` | UI thread | A settings file that failed to parse. |
| `orbit.db`, `-wal`, `-shm` | data thread | Notes, snippets, usage counts, FTS5 indexes. |
| `clipboard.db`, `-wal`, `-shm` | data thread | Clipboard history. Absent when "keep history after Orbit exits" is off; history is then in memory. |
| `clipboard-images/<hash>.dib` | data thread | Raw bitmap data for image clips. |
| `link-icons/<hash>.bgra`, `.none` | icon thread, data thread | Quicklink website icons, 64 by 64, or a note that a site had none. See PRIVACY.md. |
| `exchange-rates.xml` | exchange-rate thread, UI thread | The European Central Bank's daily reference rates, for currency conversion. Fetched when a conversion is typed and the copy is over 12 hours old. See PRIVACY.md. |
| `apps.json` | indexer child | Installed-app list. A cache; rebuilt if missing. |
| `icons/<hash>.bgra` | indexer child | App icons. A cache. |
| `crash.log` | panic hook | Thread name and source location of panics. |
| `*.tmp` | atomic writers | Transient; a write goes to a `.tmp` file and is renamed. |

Outside the data directory: an optional `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` value named `Orbit` when "launch at login" is on, and export folders under `Documents`. Formats are in [DATA_PORTABILITY.md](DATA_PORTABILITY.md); what is stored and how to delete it is in [PRIVACY.md](PRIVACY.md).

## Failure behaviour

| Failure | Result |
|---|---|
| Database cannot be opened (including a database from a newer Orbit) | The data thread reports it and keeps answering icon requests and shutdown. Launching apps still works; notes, snippets and clipboard history do not. |
| Windows refuses the keyboard hook | Notification; HyperKey and expansion are off, direct shortcuts still work. Diagnostics shows "Refused by Windows". |
| Launcher hotkey is taken | Falls back through a fixed candidate list; if none registers, the tray icon is the way in. See [SHORTCUT_SEMANTICS.md](SHORTCUT_SEMANTICS.md). |
| `apps.json` missing or unreadable | Indexer runs; until it finishes the app list is empty. |
| Settings file damaged | Moved aside, defaults used, notification shown. |
| Settings file from a newer schema | Loaded best-effort; changes are not saved. |

## Known gaps against the brief

- The brief's layout lists a separate `orbit-snippets` crate. Snippet templates and the trigger matcher are in `orbit-core/src/snippet.rs`; snippet storage is in `orbit-data/src/snippets.rs`.
- The brief asks for a C ABI between core and hosts. macOS uses one (`crates/orbit-ffi`). The Windows host links Rust directly, since it's Rust too.
- Some file reads and writes run on the UI thread (list above).
- The data thread is not restarted if it dies.

## macOS host

Sources: `platforms/macos/Sources/Orbit`, `crates/orbit-ffi`. Tested on one Mac: Mac16,8 (M4 Pro, 24 GB), macOS 27.2, built-in Retina display, AC power. The package declares macOS 14 as its minimum, and nothing older than 27.2 has been run.

### Host structure

Orbit for macOS is one process with no helper and no web runtime. Swift and AppKit link the Rust crates statically through `crates/orbit-ffi`.

| File | What it does |
|---|---|
| `main.swift` | Parses flags, takes the per-folder instance lock, starts AppKit |
| `AppController.swift` | Launcher state and behaviour, the port of `platforms/windows/src/app.rs` |
| `LauncherPanel.swift` | The non-activating panel, and `ResultsView`, which draws rows and footer |
| `Rows.swift` | Scopes, rows and actions, the port of `rows.rs` and `view.rs` |
| `Hotkeys.swift` | Carbon hotkeys and key names |
| `HyperTap.swift` | The event tap, permissions, `hidutil` |
| `Clipboard.swift` | Pasteboard watcher, skip rules, typed insertion |
| `AppIndex.swift` | App scan, FSEvents watcher, icon cache |
| `Editors.swift`, `SettingsWindow.swift` | Note and snippet windows, Settings |
| `Metrics.swift`, `Bench.swift` | Timing lines, the benchmark's Darwin notifications, `--self-test` |

### Threads

| Thread | Runs |
|---|---|
| Main | All AppKit, all launcher state, and `OrbitEngine` |
| Data thread, started by `orbit_worker_start` | SQLite and blocking file work. Requests from the host never wait on disk. |
| `orbit-tap` | The event tap callback, only while HyperKey Internal or Synthetic or expansion is on |
| `orbit.index`, `orbit.icons`, `orbit.hidutil` dispatch queues | App scans and icon pruning, icon loading, `hidutil` runs |

The e2e driver counted 15 threads just after start and 4 after 60 s idle with the tap off.

### FFI ownership

`crates/orbit-ffi/include/orbit.h` is the contract.

- A returned `char *` belongs to the caller, who frees it with `orbit_string_free`.
- A returned `const char *` or `const OrbitRow *` is borrowed, for the lifetime stated on that function. `Row(_:)` in Swift copies what it keeps.
- `*_new` and `*_start` handles are freed once with the matching `*_free`.
- `OrbitEngine` is used from one thread, the main thread here. `OrbitHyper` and `OrbitTriggers` are used from the tap thread. Their key functions allocate nothing and do no I/O.
- Worker requests may come from any thread. `orbit_wake_fn` runs on the data thread and only schedules a drain on the main thread.
- No panic crosses the boundary. A panic yields that call's documented fallback and writes its location to `crash.log`.

### How the launcher draws

`ResultsView` draws each frame on the CPU into a new IOSurface and sets it as the layer's `contents`. The layer rounds the corners. On show, the frame is drawn before the panel is ordered front. On hide, the layer lets go of the surface, about 5 MB at 2x. The text field keeps its own AppKit layer.

The reason is memory. When `ResultsView` drew through AppKit's backing store, a frame that replaced visible rows made the process allocate graphics memory, charged to `phys_footprint`. A `footprint -p` capture during that spike showed 102 MB of dirty `Owned physical footprint (unmapped) (graphics)` in 47 regions, 19 MB of IOSurface and 9 MB of IOAccelerator. About 1.2 s after a frame, it became reclaimable and left the footprint. Typing kept drawing, so it stayed.

#### What was measured

Each row is one process with a private data folder and stub rows, with no text input, driven by notification. Peak is the highest `proc_pid_rusage` `ri_phys_footprint` sampled every 50 ms after the rows were set. These are from a throwaway experiment build, not from the shipped code.

| Configuration | Peak MB |
|---|---:|
| Everything on, 20 glyph rows | 159 |
| Everything on, 20 app rows | 168 |
| One row, or 20 short rows | 155, 160 |
| App icons off | 157 |
| Glyph images off | 153 |
| Row and footer text off | 157 |
| Glyph images and text off | 150 |
| Regular instead of semibold title | 158 |
| CoreText `CTLineDraw` instead of `NSString.draw` | 160 |
| Selection fill off | 158 |
| Scrollbar off | 160 |
| Rounded clip off | 153 |
| `drawsAsynchronously` set off, or set on | 160, 160 |
| Field removed from the view | 157 |
| Only text on | 151 |
| Only glyph images on | 153 |
| Only the scrollbar on | 149 |
| Only the selection fill on | 32 |
| Only the rounded clip on | 32 |
| Glyphs, text, selection, scrollbar and clip all off | 33 |
| Private `CGBitmapContext`, image handed to `layer.contents` | 37 |

Two more results on the backing store path: a redraw with no content change stayed at 33 MB, and a first frame that already had rows peaked at 41 MB.

So rows were the trigger, typing and the text input stack were not, and no one drawing part caused it. A CPU-drawn frame avoids it. Which part of AppKit, Core Animation or the graphics driver makes the allocation was not found, and why the selection fill alone doesn't trigger it was not found either.

A `CGImage` in `contents` added about 9 ms between T1 and T2. An IOSurface removed most of that: over 30 opens, T0 to window server visible p95 was 31 to 32 ms with a `CGImage` and 26 ms with an IOSurface, and peak footprint was 55 MB against 46 MB.

Opening Settings or a note window through the normal AppKit path peaked at 39 MB and 34 MB. Typing in those windows was not measured.

Not tested: a display with another scale factor, moving between displays, and the light theme with this draw path.

### Memory measures

`footprint` is `phys_footprint`, what Activity Monitor calls Memory. It's read with `task_info(TASK_VM_INFO)` inside the process and `proc_pid_rusage` from outside. `resident` is `resident_size`, which includes shared framework pages, so it reads far higher than what Orbit alone costs. Diagnostics shows both.

Results from `orbit-bench e2e --opens 30 --idle-secs 60` on the final build, with an empty data set and no tap. A second run of the same build agreed within 1 MB on every line except as shown.

| Point | Footprint MB | Resident MB |
|---|---:|---:|
| Idle after start, before any open | 14.5 | 79.1 |
| After 30 opens | 30.1 | 100.8 |
| After typing | 33.5 | 113.4 |
| After 500 searches | 35.2 | 115.7 |
| After 60 s idle, hidden | 29.8 (29.5 in the second run) | 118.1 |
| Peak in the run | 54.5 | |

The earlier build, before the draw change, had 48.5 MB footprint after idle, 176.2 MB after typing and a 197.1 MB peak.

The brief's budget for background memory is 30 to 40 MB stretch and 55 MB beta. Footprint after idle is within stretch. Resident size is not within either figure, at 118 MB, and most of it is shared framework pages. Under memory pressure, not tested on this machine.

