# ORBIT — ULTRA-LIGHTWEIGHT NATIVE DESKTOP LAUNCHER

## COMPLETE PRODUCT, ENGINEERING, PERFORMANCE & IMPLEMENTATION PROMPT

**Version:** 1.0  
**Prepared:** 5 November 2026  
**Intended recipient:** Claude Code, OpenAI Codex, or another autonomous coding agent  
**Working name:** Orbit  
**Target platforms:** **macOS and Windows** first; Linux may follow  
**Non-negotiable product identity:** keyboard-first, native, extremely fast, tiny background footprint, fully useful without cloud or AI.

---

## 0. YOUR JOB AS THE CODING AGENT

You are the principal systems engineer, macOS engineer, Windows engineer, interaction designer, performance engineer, and test engineer for **Orbit**.

**Build the application, not simply a specification or a mock-up.** If a repository exists, inspect it and preserve sensible working pieces. If the repository is empty, construct a clean native architecture and demonstrate a working minimum end-to-end slice. Implement the most important features first, measure performance with real instrumentation, and do not describe aspirational metrics as achieved benchmarks.

### Priority order — enforce strictly

1. **Reliability of global hotkeys and core launcher.**
2. **Measured speed and low resident memory under system memory pressure.**
3. **Predictable keyboard UX, focus restoration, search relevance and application launching.**
4. **HyperKey / one-key modifier shortcuts on BOTH macOS and Windows.**
5. **Clipboard history, snippets and unlimited notes.**
6. Settings, polish, migration, distribution and optional integrations.

Features that add background CPU, memory, app startup overhead, keyboard lag, crashes, or additional persistent processes are not automatically worth shipping. Quantify their cost and make them opt-in where appropriate.

### Engineering behaviour

- Begin by inspecting OS, build tools, repo, current code and dependency state.
- Identify the minimum working slice and build it immediately.
- Prefer a **single-process, native launcher** with a shared efficient core over a web application running inside a desktop shell.
- Do not add Electron, a resident Chromium runtime, a resident Node.js backend, an embedded web server, or a mandatory cloud service.
- Do not introduce a complex plugin framework or AI agent in the MVP.
- Use platform-native low-overhead APIs for keys, windows, app indexing, pasteboard, focus and launch operations.
- Document all Accessibility/Input Monitoring permissions, OS limitations and compatibility concerns accurately.
- Provide reproducible profiling scripts and performance results on machines that can actually be tested.
- Never silently seize reserved OS shortcuts or interfere with password entry, secure desktop, games, remote sessions or other apps without documented user control.
- Build equivalent experiences on macOS and Windows while allowing different native implementation details.

**Deliverable expectation:** source code, runnable native binaries or instructions for producing them, project files, tests, benchmark harness, technical decisions, help/settings UI, optional unsigned test builds if signing is unavailable, and a truthful implemented/deferred list.

---

## 1. WHY ORBIT EXISTS

Orbit is an alternative to large, feature-heavy command launchers for people who prioritise responsiveness. The intended user may have:

- A work laptop with **RAM almost constantly exhausted**, where every added background process matters.
- A performance-focused gaming/development PC where a resident launcher must not meaningfully increase CPU use, RAM pressure, frame-time spikes or idle wakeups.
- A Mac where they like the convenience of Raycast's keyboard-driven interface, HyperKey, snippets, clipboard history and notes, but would prefer a smaller, purpose-built tool.

A useful launcher should be available in an instant—*not* feel like opening a web application or waking an entire application platform.

### Core promise

> **Press. Type. Act. Instantly.**

### Product positioning

- **Native-feeling:** single compact command bar, no large main window by default.
- **Local-first:** no internet required; default search is local; no mandatory account.
- **Minimal:** no ambient AI, no decorative dashboard, no ads, no always-on network connection.
- **Keyboard-first:** full control without touching the mouse.
- **Customisable:** global actions, HyperKey mappings, fast snippets, searchable unlimited notes.
- **Trustworthy:** respects privacy and OS keyboard security boundaries.

### Competitive technical context

Do not assume Raycast is purely a web application. Raycast's published 2.0 architecture describes native hosts for macOS/Windows, with a React web frontend, a long-lived Node backend and Rust components. Orbit differentiates through **a much smaller default runtime and a constrained product scope**, not by claiming every competing app is inherently slow.

Reference: <https://www.raycast.com/blog/a-technical-deep-dive-into-the-new-raycast> (published 14 May 2026).

---

## 2. PERFORMANCE IS A PRODUCT FEATURE, NOT A POST-LAUNCH CLEANUP

### Design-to performance budgets (targets, not guarantees)

These are aggressive **engineering goals**. Verify on specified reference machines and state any misses honestly. They are not assurances for every hardware/OS combination.

| Measurement | Stretch goal | Acceptable first public beta target | Notes |
|---|---:|---:|---|
| Background resident memory, Windows | ≤ 25–35 MB | ≤ 45 MB | Measure private bytes/working set, label both |
| Background resident memory, macOS | ≤ 30–40 MB | ≤ 55 MB | Record footprint and resident size, distinguish metrics |
| Warm hotkey → visible launcher (p95) | ≤ 30 ms | ≤ 60 ms | Key event to first frame, not merely callback receipt |
| Hotkey → usable focused input (p95) | ≤ 40 ms | ≤ 75 ms | Input ready and foreground window active |
| Search results after each keystroke (p95) | ≤ 15 ms | ≤ 40 ms | For pre-indexed local dataset, excludes user think time |
| Global shortcut dispatch (p95) | ≤ 20 ms | ≤ 50 ms | Until action request is issued; external app open time is separate |
| Idle CPU | ~0% | ≤ 0.2% average | No tight polling loops; report methodology |
| Avoidable idle disk writes | 0 | Rare/batched | No continuous state rewrites |
| Additional background processes | 0 | 1 only if technically justified | Include helper processes in full resource budget |
| Restore after unlock/sleep | near instant | reliable within seconds | Test device reattach, hotkey re-registration |
| First query after launcher was idle | same as warm | under 100 ms p95 on target system | Do not wait on network or lazy heavy services |

**Define how measurements work**: p50/p95/p99; sample count; hardware; OS; battery/AC; baseline RAM usage; memory-pressure level; cold versus warm; number of indexed apps/notes/snippets; whether file search is enabled. Output timings as a bench report in the repository.

### Reference test matrix

1. **Constrained Windows laptop:** 8 GB RAM, mainstream low-power CPU, memory pressure caused by ordinary browser + office workload; no swapping caused by Orbit alone if possible.
2. **Windows gaming desktop:** modern 6+ core CPU, gaming/benchmark session with Orbit idle. Measure frame times and background CPU/wakeups, not just average FPS.
3. **Apple Silicon Mac:** 8 GB/16 GB as available, Safari/Chrome, editor, multiple apps; memory pressure tested.
4. Optional Intel Mac if hardware available.

Where devices are unavailable, build the benchmark harness and label those cells **not tested**. Never fabricate benchmark outcomes. “Fast” must mean instrumented results on actual hardware.

### Resource rules

- No periodic full app rescans; use OS notifications/events and debounced targeted refresh.
- No per-keystroke filesystem scans, disk database opens, full UI tree rebuilds or network calls.
- No permanently resident screenshot capture or window content recording.
- No blanket process enumeration at high frequency.
- No unbounded note, clipboard thumbnail, icon or query caches.
- No high-frequency timers just to keep the tray/menu bar alive.
- Initialise only essential services at startup; open optional feature stores lazily or in low-priority stages.
- Precompute app-name/alias search tokens, keep small hot indexes in memory, and request extra detail only when needed.
- Cache app icons in compact disk cache with memory LRU cap; avoid decoding large icons on search every time.
- Avoid running animations that lengthen perceived interaction; subtle transitions must be optional and respect reduced motion.
- Avoid memory-mapped mega-indexes unless measurement justifies them.
- Always profile resource cost before adopting any major library or persistent helper.

### Under-memory-pressure behaviour

Orbit should not simply degrade with the computer. It should:

- Keep the hotkey event loop and core query index responsive.
- Shed optional caches first, not critical key mapping state.
- Avoid expensive foreground garbage collection / heap churn (particularly relevant when choosing runtimes).
- Defer thumbnail and attachment decoding while launching.
- Suspend non-essential indexing/maintenance under pressure or when on battery, where OS signals permit.
- Provide a **Low Resource Mode** switch that disables optional history images, live previews, non-essential indexing and animations; also support auto-trigger if reliable OS metrics are available.
- Report a concise diagnostic panel showing resource usage, cache size, indexing state and measurement method, without continuous heavyweight monitoring.

---

## 3. ARCHITECTURE — SHARED RUST CORE + TWO THIN NATIVE HOSTS

### Recommended stack

- **Shared core:** Rust (stable), providing command registry, fuzzy matching, indexing logic, SQLite access, shortcut configuration/validation, snippets engine, notes repository, clipboard dedupe and benchmark utilities.
- **macOS shell/UI:** Swift + AppKit. Use an `NSPanel`/appropriate nonstandard floating launcher surface, AppKit keyboard and focus handling, native text field/list, menubar extra. SwiftUI is acceptable for non-critical settings screens **only if measured overhead is acceptable**; avoid loading a large UI runtime on the critical launcher path if it increases latency.
- **Windows shell/UI:** native Win32 windowing with Direct2D/DirectWrite or another genuinely lightweight native rendering path; Rust-based Windows API bindings are an alternative if they simplify interop **without adding a web view**. Document why the chosen UI approach is appropriate. A simple native list/search field beats an over-engineered custom GPU compositor.
- **Storage:** SQLite, with prepared statements, proper transactions, WAL where measured appropriate; SQLite FTS5 for notes/snippets/commands if advantageous. Most applications/shortcuts can be searched by a tiny in-memory index.
- **FFI:** narrow stable C ABI or similarly sound native interface to Rust core; well-defined ownership, UTF-8 strings, errors, thread-affinity, event dispatch. Avoid expensive serialization roundtrips on every keystroke.
- **Distribution:** macOS app bundle + DMG/PKG if appropriate, Windows MSIX or signed MSI/installer depending on update model, plus portable build if feasible.
- **No network:** no backend required; optional integrations use explicit on-demand clients.

### Single-process preference

Prefer each platform's host linking the Rust core **into one process**. A separate small launcher helper is allowed only when proven necessary (e.g. process isolation for optional plugins) and counts toward the performance budget. Startup items, keyboard hooks, UI and search should remain robust if optional worker tasks fail.

### Suggested repository layout

```text
orbit/
  crates/
    orbit-core/             commands, search, ranking, settings types
    orbit-data/             sqlite, migrations, repositories
    orbit-hotkeys/          shared shortcut mapping model, tests
    orbit-notes/            notes and index abstractions
    orbit-snippets/         templates, expansion engine
    orbit-bench/            bench harness and metric schemas
    orbit-ffi/              versioned API / bindings
  platforms/
    macos/                  Xcode project, Swift/AppKit, macOS adapters
    windows/                MSVC/CMake or Rust/Win32 host, platform adapters
  docs/
    ARCHITECTURE.md
    PERFORMANCE_BUDGET.md
    SHORTCUT_SEMANTICS.md
    PRIVACY.md
    DECISIONS.md
  fixtures/
  scripts/
  README.md
```

If separate crates do not improve clarity yet, start with fewer and split later. Small executable size and minimal dependencies are desirable, but **resident memory and latency matter more than the installer byte count**.

### Event architecture

```text
OS global shortcut registered
       |
       v
Native hotkey callback (non-blocking)
       |
       v
Show/focus existing native launcher window
       |
       +--> Ready search input, cached suggestions
       |
User types query
       |
       v
Tiny in-memory query index + ranking
       |
       v
Native list diff update; icons loaded asynchronously if missing
       |
User presses Enter/shortcut
       |
       v
Command dispatcher -> platform adapter / local feature
       |
       v
Hide launcher + preserve/restore sensible focus behaviour
```

The event loop must **never block on database migration, disk enumeration, thumbnail conversion, network activity, or a plugin process**.

### Platform adapter interface (illustrative)

```text
PlatformAdapter
  registerGlobalShortcut(binding) -> RegistrationResult
  unregisterGlobalShortcut(id)
  registerHyperKey(config) -> PermissionOrCompatibilityResult
  showLauncher(monitor, focusContext)
  hideLauncher(restoreFocus)
  enumerateInstalledApplications() -> AppDescriptor[]
  subscribeApplicationChanges(callback)
  launchApplication(appId, arguments?, files?) -> ActionResult
  activateApplication(appId) -> ActionResult
  openFile(path) -> ActionResult
  openUrl(url) -> ActionResult
  copyToClipboard(content) -> ActionResult
  subscribeClipboardChanges(callback)
  insertTextIntoFocusedApp(text, mode) -> ActionResult
  queryAccessibilityPermissions() -> PermissionStatus
  setLaunchAtLogin(enabled) -> Result
```

These methods are **conceptual**. Map them to supported platform APIs; document any missing capability or permission distinction.

---

## 4. LAUNCHER CORE — THE MOST IMPORTANT PRODUCT EXPERIENCE (MVP P0)

### Opening the launcher

- Default macOS shortcut: configurable; suggest `Option + Space` **only if available**. Do not fight macOS Spotlight or an existing Raycast shortcut. Offer a shortcut recorder and collision feedback.
- Default Windows shortcut: configurable; suggest `Alt + Space` **only if free**, with fallback e.g. `Ctrl + Space` or another user-selected combo.
- Opens centered near top/upper-middle of active display, not centered over the whole screen unless desired.
- Use active monitor, account for DPI scaling, notch, menu bar, taskbar, multi-monitor arrangements.
- Input focused as soon as visible; selected item actionable by Enter.
- `Esc` hides; subsequent activation behaves predictably and can optionally preserve last query.
- Preload a small “recent/favourites” list, not a blank expensive UI.
- Mouse support is secondary but smooth: click item, scroll, right-click for actions.

### Supported searchable result types

1. Installed applications.
2. Built-in commands: sleep/lock/shutdown confirmation, empty trash/recycle bin confirmation, settings, diagnostics, restart Orbit.
3. User-defined custom commands / aliases.
4. Quick links / websites.
5. Notes (title/body).
6. Snippets (name/trigger/text), with explicit insertion actions.
7. Clipboard history (with type and preview).
8. Calculator result and unit conversions (simple pure local math; optional phase).
9. Optional local files and folders, in approved indexed locations.
10. Optional recent/frequent window switching.

**No mandatory web search or AI in the launcher.** Keep remote features explicitly opt-in and out of the critical query path.

### Key interactions

| Input | Behaviour |
|---|---|
| Configured global shortcut | Toggle launcher, focus input |
| Type text | Filter/rank results immediately |
| Up / Down | Change selected result |
| Enter | Execute selected item |
| Command/Control + Enter | Secondary action if defined, shown in UI |
| Tab | Reveal contextual actions or scope, according to chosen UX |
| Esc | Close sub-menu, otherwise close launcher |
| Backspace in empty query | Optionally go back a scope |
| `?` / help command | Shortcut help / discoverability |
| Command/Control + K | Commands/action menu (if consistent with OS expectations) |
| `:` or `>` prefix | Explicit command mode, optional |

Exact platform differences should be documented in settings/tooltips; do not pretend Command and Control are interchangeable for system shortcuts.

### Search ranking

Implement deterministic local fuzzy matching:

1. Exact alias/shortcut match.
2. Exact app or command name.
3. Prefix match.
4. Word-boundary match.
5. Fuzzy subsequence and typo tolerance.
6. User-pinned favourites.
7. Recent/frequent use signal with bounded decay.
8. Secondary metadata/category matches.

Keep scoring explainable and predictable. An obscure frequently used app should not always outrank an exact user query. Support ASCII and Unicode names, case/diacritic handling, and tokenisation. Never let slow full-text note body queries block app-name results; run them asynchronously and merge rankings without jumpy selection changes.

### App indexing

**macOS:** investigate LaunchServices / NSWorkspace and allowed application locations, bundle IDs, installed and registered app change notifications; do not recursively traverse the entire disk every time the launcher opens. Cache bundle IDs, names, paths, icon references and aliases.

**Windows:** index Start Menu entries, installed/UWP/app package registrations via supported APIs, user-configurable executables/shortcuts, and a limited set of common app locations. Use shell APIs where appropriate and update incrementally; do not enumerate all process modules or crawl all disks.

Support installed-but-not-yet-indexed app discovery via an explicit “Rebuild App Index” command and diagnostics. Launching should preserve the app's native shell semantics and handle not-found/uninstalled apps cleanly.

### App actions

- Launch app; optionally activate existing instance by preference.
- Open file/folder/URL.
- Run user-defined script or shell command **only with explicit setup** and safety warnings; no arbitrary remote command execution.
- Built-in commands for common OS functions, with confirmations for destructive ones.
- Assign personal alias and direct hotkey to any command.
- Pin, hide or reorder favourites.

---

## 5. GLOBAL SHORTCUTS & HYPERKEY — FEATURE PARITY ON MAC AND WINDOWS (MVP P0)

This is a **headline Orbit feature**, not a later plugin.

### Desired usage

A user holds a dedicated HyperKey and presses a letter:

- `Hyper + C` → open/focus coding app (e.g. VS Code or configurable editor).
- `Hyper + B` → preferred browser.
- `Hyper + T` → terminal.
- `Hyper + N` → open Orbit Notes.
- `Hyper + V` → open clipboard history.
- `Hyper + S` → open snippets menu, or a configurable action.
- `Hyper + M` → send an explicit command to Mia **if an integration has been configured** (later).

These are **example presets, not hardcoded ownership of all keys**. Users may assign Hyper + any permitted key to app launch, focus, URL, command, note, snippet, etc.

### HyperKey semantics

The common Hyper chord is `Control + Alt/Option + Shift + Command/Windows`, often mapped to a single physical key such as Caps Lock. Orbit should offer two modes:

1. **Hyper as synthetic modifier chord**, where technically supported; intended to interoperate with global shortcuts in other apps.
2. **Orbit-only leader/shortcut mode**, where holding a chosen physical key lets Orbit interpret `Hyper + key` internally, without depending on other programs seeing a synthetic chord. This can be faster, safer and less invasive, and is a practical fallback.

Make the modes and compatibility consequences explicit. Do not claim a single implementation reliably intercepts every application or OS security context.

### Configurable trigger behaviour

- Default physical key candidate: **Caps Lock**, user-confirmed.
- Optional candidates: right Option/Alt, right Command/Win, right Control, Menu/Application key, F-key, etc., according to platform support.
- Tap Caps Lock → configurable `Escape`, original Caps Lock toggle, no-op, or other action.
- Hold Caps Lock + letter → mapped action, without unintentionally enabling uppercase typing.
- Optionally press-and-release leader mode to avoid complex modifier injection if user prefers.
- Per-app exceptions: ignore shortcuts while playing games or using certain tools.
- Temporary suspend shortcut or tray/menu-bar toggle.
- Disable HyperKey globally when user turns it off; **restore normal Caps Lock behaviour**.
- Detect conflicts with OS and other registered shortcuts, and warn clearly.
- Persist key mappings across restart/update with schema migration.
- Show a searchable shortcut list and keyboard recording UI.
- Never leave virtual modifier keys stuck after key-up, interrupted events, sleep, crash, or focus change.

### macOS implementation notes

- Use native hotkey APIs for normal global shortcuts, e.g. a supported system registration mechanism.
- For remapping physical Caps Lock/Hyper, investigate `CGEventTapCreate` and related event APIs, keyboard layouts, hardware key events and required Accessibility/Input Monitoring consent. This is not ordinary global-hotkey registration.
- Event taps may require user permissions; report current status, open the correct System Settings pane where possible, and provide a no-remap fallback.
- Event taps can be disabled by timeout or secure OS conditions. Detect loss/re-enable safely; do not loop uncontrollably.
- Be careful with Caps Lock's toggle/`flagsChanged` behaviour and different keyboard layouts. Test external keyboards and laptop keyboards.
- Do **not** install a kernel extension or a permanent privileged daemon for routine shortcuts. If true system-wide synthetic Hyper cannot be reliably implemented without inappropriate privileges, use Orbit-only leader mode and document it.
- Don't break secure input, FileVault login, the lock screen, password managers or accessibility tools. Restricted contexts may simply not support Orbit shortcuts.
- If users already use Karabiner-Elements or another remapper, allow **external Hyper chord mode** so Orbit recognises existing mapped shortcuts without taking over Caps Lock.

API reference: <https://developer.apple.com/documentation/coregraphics/cgevent/tapcreate(tap:place:options:eventsofinterest:callback:userinfo:)>

### Windows implementation notes

- For conventional global shortcuts, prefer `RegisterHotKey`/`WM_HOTKEY` where it meets requirements; collision failures should be shown, not silently swallowed.
- For a configurable physical Hyper trigger, investigate `WH_KEYBOARD_LL` hooks with a minimal callback and thread message loop. **A hook callback must be extremely cheap and must never run database operations or user commands.** Dispatch recognised events to the app's worker/UI queue.
- Where synthetic modifiers are necessary, investigate `SendInput` with carefully tracked key state and event tagging, accounting for keyboard layout, user keyboard modifiers, elevation/integrity boundaries and anti-cheat/remote desktop behaviour.
- Avoid a driver or privileged service. Do not promise operation over the UAC secure desktop or elevated apps when privilege boundaries block events.
- Allow a safe fallback to registered chord shortcuts if input hooks are blocked or undesirable.
- Test Windows key shortcuts, Caps Lock toggle state, IME composition, multiple keyboards, RDP, gaming fullscreen and secure prompts.
- Do not create or market this as a stealth keylogger. Process only the minimal shortcut events required. Never record all typed keys.

API references:  
<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-registerhotkey>  
<https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelkeyboardproc>

### Direct hotkeys (without opening search)

Any command can have a shortcut: open application, focus application, create note, paste snippet, open folder/URL, show clipboard, run script with explicit configuration. Detect duplicates, support disabling/reassignment, and minimise conflicts with system shortcuts.

### Required testing for keyboard correctness

1. Caps Lock tapped: selected tap behaviour occurs exactly once.
2. Caps Lock held + C: configured editor launched/focused exactly once.
3. Caps Lock released quickly after key chord: no stray `C`, no stuck modifiers, normal typing resumes.
4. Repeated hold/tap and modifier chord sequences; autorepeat correctness.
5. Left/right Ctrl/Alt/Shift and keyboard layout differences.
6. Two physical keyboards connected; test sleep/wake and reconnect.
7. Locked screen and secure text input: safely do nothing where not allowed.
8. Conflict with Raycast, Spotlight, PowerToys or other shortcut programs: user sees clear error or precedence explanation.
9. Toggle HyperKey off, quit Orbit or uninstall Orbit: keyboard returns to normal.
10. Fullscreen game session: Orbit stays idle/suspended per setting, no input latency or key interference.

---

## 6. CLIPBOARD HISTORY — USEFUL BUT NEVER SPYWARE (MVP P1)

A fast, keyboard-searchable history similar to the most useful parts of clipboard managers, with local persistence and privacy controls.

### Essential capabilities

- Capture text, URLs, short rich-text, file references and images if enabled.
- Show timestamp, type, compact preview, source app **only if reliably and permissibly available**, and pin/favourite state.
- Search text history locally, fuzzy/prefix with sensible ranking.
- Paste selected entry back into original app, or copy to active clipboard with a clear fallback if automated paste is unavailable.
- One-key action shortcut (e.g. Hyper + V).
- Pin important items, delete one, bulk delete, clear all and pause capturing.
- Per-app exclusion list, including user-defined password manager apps and private apps.
- Retention controls: item count, retention days, maximum content size; compact thumbnails; disable image recording by default on very low-memory systems or enable via setting.
- Dedupe consecutive duplicate copies with predictable rules.
- Do not fetch remote URLs or preview external content just to show history.

### Privacy defaults

- Clipboard capture is **opt-in during onboarding** with transparent explanation.
- Exclude known secret types where identifiable, but **never promise foolproof password or OTP detection**.
- Default to local-only encrypted-at-rest where feasible with platform-managed key material. If the DB cannot be fully encrypted without adding an unacceptable dependency, document the tradeoff; at minimum provide secure OS file permissions and an option to use nonpersistent history.
- Do not log clipboard body contents or transmit them to telemetry.
- Respect app exclusion and manual pause instantly.
- Offer automatic clearing after a period and “Do not remember next copy” control.
- Support private ephemeral session mode.

### Platform strategy

**Windows:** use `AddClipboardFormatListener` and `WM_CLIPBOARDUPDATE`; no 10 ms polling loop. Avoid disruptive clipboard ownership behaviour and be careful with clipboard contention.

Reference: <https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-addclipboardformatlistener>

**macOS:** `NSPasteboard.general.changeCount` may be used to detect changes, but it is not an event subscription; measure a **low-frequency adaptive interval** if polling is required, pause when appropriate, and read content only after a change and if enabled. Do not create wasteful high-rate timers. Document permission/privacy behaviour.

Reference: <https://developer.apple.com/documentation/appkit/nspasteboard/changecount>

### Performance limits

- Set a reasonable initial history cap, user-adjustable; **never load all retained images or entire clipboard history into RAM**.
- Batch or debounce metadata writes; SQLite indexes on timestamps and searchable text.
- Thumbnails generated in background, with size caps.
- Avoid retaining 200 MB copied images as raw in-memory buffers.
- Pausing clipboard capture should release optional resources.

---

## 7. SNIPPETS & TEXT EXPANSION (MVP P1)

This should be a **first-class product feature**—not a fragile optional extension.

### User features

- Unlimited locally stored snippets.
- Title, expansion trigger, optional description, folders/tags, favourite status.
- Plain text, multiline text, optionally rich text later.
- Search and insert from launcher.
- Text expansion with user-defined triggers, e.g. `;email`, `;addr` or `;sig`; **default disabled until explicitly enabled** because it requires input monitoring/interception and can conflict with other apps.
- Snippet variables: `{date}`, `{time}`, `{clipboard}` (only when explicitly used), selection/cursor markers; keep implementation safe and deterministic.
- Optional prompted placeholders, e.g. customer name, for later iteration.
- Custom hotkeys for frequently used snippets.
- A preview before inserting variable-containing text if it could expose clipboard content.
- Import/export JSON and plain text for portability; no proprietary lock-in.

### Engine rules

- Snippet matching should not keep an unbounded global keystroke log. Use a tiny rolling trigger buffer **only when expansion is enabled and allowed in the active app**; discard it promptly and avoid persistence.
- Use OS accessibility APIs only as needed and with permission.
- Provide trigger conflict resolution (longest trigger, explicit priority or deterministic rule).
- Avoid expansion in password fields and excluded apps, and suspend appropriately in secure entry modes.
- Be careful with IMEs, dead keys, Unicode, text editors with custom input methods, clipboard restore race conditions and rich-text paste.
- Do not paste through the clipboard unless the UI provides acceptable fallback behaviour and restores prior clipboard only when technically safe; otherwise typed injection may be preferable for short content.
- If automatic expansion is blocked by platform or target app, user can still search and copy snippet manually.

### Suggested default categories

Personal, Work, Code, Links, Templates. Categories are optional and user-editable.

---

## 8. UNLIMITED NOTES — FAST AND LOCAL, NOT A FULL NOTION CLONE (MVP P1)

The user particularly likes lightweight quick notes but dislikes arbitrary small limits. Orbit should provide **unlimited notes without an artificial five-note cap**.

### Essential

- Create a note instantly from launcher or `Hyper + N`.
- Editable title and body, Markdown-friendly plain-text editing, simple formatting shortcuts.
- Automatic local saving with sensible debounce, plus forced save on close/app shutdown.
- Search full title and body from Orbit.
- Pinned notes and favourites.
- Optional tags/collections and archive/trash.
- Open note in a small movable native window or a panel near launcher.
- Multiple notes can exist; opening/editing one must not require the whole note database in RAM.
- Show last-edited time, expose copy content, copy link/ID, export text/Markdown.
- Autosave recovery and no loss on app/window crash; avoid half-written content.
- Undo/redo support and standard native text behaviours.
- Light/dark mode and keyboard shortcuts consistent with the launcher.

### Optional later

- Floating always-on-top scratchpad.
- Pinned notes on separate desktops/spaces.
- Checklists, code blocks and syntax highlighting (only if tiny overhead).
- Encrypted private notes, protected by system credentials.
- Local sync or optional end-to-end encrypted sync, only once conflict resolution is sound.
- Links to files, URLs and application commands.

### Why the scope matters

The winning experience is “open launcher → type note → dismiss” in seconds. Heavy rich-text editors, collaboration stacks, remote databases and AI summarisation are intentionally excluded from the MVP.

---

## 9. ESSENTIAL ADDITIONAL COMMANDS (MVP P0/P1)

### Quick links and web search

- Save named URLs with alias/hotkey; validate protocols and warn for unusual schemes.
- Launch default browser or chosen browser if configured.
- Optional web search: a lightweight URL template, only when user selects the action. No default network lookup on every query.
- Example: `g rust async` opens a configured browser search; the search itself stays off the hot path.

### Calculator

- Local arithmetic (e.g. `16*3.5+4`) with instant output and copy action.
- Unit conversion may be added using fixed tables; do not rely on remote APIs or claim live currency exchange rates without fetching an explicit source.
- Clearly distinguish numerical error from empty input.

### System commands

- Lock screen, sleep, settings panel, open downloads, open terminal, switch audio device where supported, empty trash, shutdown/restart with confirmations.
- Only expose actions implementable reliably without unnecessary privileges.
- Allow custom applications and shell scripts, but clearly display what will execute and consider shell quoting and working directory.
- No command from untrusted notes/clipboard should execute automatically.

### Basic window/app switching

- Search currently running applications and activate windows where platform APIs allow.
- Keep an optional `Switch to app` result distinct from `Launch app`.
- Full window management/tiling is **not required** for MVP; optional commands may come later.

### File and folder access

- Quick access to common folders and user-pinned paths.
- Optional indexed file search with **explicit, bounded directories** and filters for extensions/types.
- Prefer OS native indexes/search APIs where feasible; do not recursively crawl all files on the hotkey or every keystroke.
- Provide visible per-directory indexing controls; omit cloud-only files by default when risky.

---

## 10. OPTIONAL ORBIT ECOSYSTEM INTEGRATIONS (AFTER CORE IS PROVEN)

The developer also has two other projects: **Mia**, a personal assistant, and **Spotlight**, a separate lightweight desktop Spotify client. These are optional integrations, not dependencies.

- **Mia:** user-configured named commands that call a local authenticated endpoint or bounded CLI with explicit permission. Example: `Hyper + M` opens a text command entry or voice action. No always-on microphone or background connection in Orbit.
- **Spotlight client:** register actions such as Play/Pause, Next, Previous, Search Song or Focus Player via the client's documented IPC/CLI/protocol if such an interface exists. Do not invent an API or assume an integration already works. Generic system media controls can be supported independently.
- **Proxmox/OPNsense:** user-added quick links or optional authenticated commands, not continuous status polling.
- **Developer tooling:** open workspace in selected IDE, open project terminal, run an explicitly configured script. Do not turn Orbit into a resource-hungry agent manager.

An extensibility model can later use explicit, **on-demand** scripts or subprocesses with timeouts, output size limits, allowlists and clear permission boundaries. Do not embed a resident JavaScript/Node plugin runtime in the MVP.

---

## 11. VISUAL DESIGN — MINIMAL, PREMIUM, SPEED-FIRST

### Launcher design

- Compact floating native panel, roughly 600–750 logical pixels wide depending on platform/display scale; responsive, with user setting for compact/comfortable density.
- Search field at the top with icon, clear focused state and correct text selection/IME handling.
- Results list, ~5–8 visible items. App icon, title, short subtitle and optional key hint.
- A small footer with contextual key tips, not decorative clutter.
- Search input and result row font sizing legible at common display scales.
- Consistent keyboard selection state, including after async results arrive.
- No immersive home/dashboard window by default.
- Optional acrylic/vibrancy/blur **only if not measurably slowing show time or creating GPU pressure**; allow solid opaque mode.
- Respect platform dark/light, high contrast and reduced motion.
- Support arbitrary display DPI/scaling and multi-monitor focus.

### Notes / clipboard / snippets

Reuse the same design primitives and native window behaviours. Use simple list → detail interactions, efficient virtualised lists for many records, useful empty states and small context menus. Keep images lazy. No unbounded animated backgrounds or UI frameworks loaded purely for appearance.

### Settings layout

Categories: General, Search, Hotkeys, HyperKey, Clipboard, Snippets, Notes, Integrations, Appearance, Data & Privacy, Performance/Diagnostics, About.

Important detail: settings can be a separate native window and may be more elaborate than the launcher; **settings performance must not penalise the background launcher**.

---

## 12. STORAGE, DATA MODEL & SEARCH INDEX

Suggested SQLite tables (logical design):

| Table | Essential fields |
|---|---|
| `settings` | key, type, value, updated_at |
| `commands` | id, kind, title, subtitle, alias, payload, enabled |
| `applications` | app_id, platform_id, name, path, icon_cache_key, last_seen_at |
| `shortcut_bindings` | id, action_ref, physical_key?, modifiers, mode, enabled, conflict_state |
| `quick_links` | id, name, url, aliases, created_at |
| `command_usage` | action_id, last_used_at, count, recency_score |
| `notes` | id, title, body, created_at, updated_at, pinned, archived, deleted_at? |
| `snippets` | id, name, trigger, content, tags, enabled, created_at |
| `clipboard_items` | id, kind, content_ref/hash, preview, source_app?, created_at, pinned, expires_at? |
| `attachments` | id, mime, blob_path, content_hash, size, thumbnail_ref |
| `index_meta` | id, source, version, last_indexed, dirty_state |

SQLite FTS5 can support note/snippet search; use cautious quoting and prepared statements so special user input does not break queries. App/command search should preferentially use a tiny native in-memory token index. Keep schema migration versioned and tested.

Reference: <https://www.sqlite.org/fts5.html>

### Suggested query protocol

```text
SearchRequest {
  query: string,
  scopes: [apps, commands, links, notes, snippets, clipboard, files],
  max_results: 10,
  request_seq: u64
}

SearchResult {
  id: string,
  kind: enum,
  title: string,
  subtitle?: string,
  icon_key?: string,
  score: i32,
  primary_action: ActionId,
  secondary_actions: ActionId[]
}
```

Return app and command hits synchronously from cached data. Merge notes/clipboard/files asynchronously if necessary; attach sequence IDs and **discard stale responses** so results from an earlier query cannot overwrite newer results. Keep selection stable when new hits appear.

### Startup strategy

1. Native process starts and creates message loop / native app lifecycle.
2. Load tiny settings and hotkey map.
3. Register shortcuts and initialise a prebuilt minimal app index.
4. Create launcher window hidden and prepare initial UI/search input **if benchmarks show this reduces hotkey latency without unacceptable RAM cost**; otherwise create lazily and retain the window after first launch.
5. Initialise optional clipboard/notes indexes in low-priority background steps.
6. On app changes, update only affected entries.

No network or heavy DB migration on key callback. If migration is necessary, do it safely and allow key listener to report a graceful temporary not-ready state rather than freezing.

### Data portability

- Export/import settings, shortcut maps, notes, snippets and quick links in documented JSON/Markdown/ZIP formats.
- Clipboard export must be opt-in and warn about potentially sensitive content.
- Protect imports from arbitrary file-path traversal and oversized archives.
- Include versioned schema and rollback; preserve user data across updates.
- Provide “Reset All Shortcuts”, “Reset Index”, “Clear Clipboard History”, “Export Notes”, “Delete Local Data”.

---

## 13. SECURITY, PERMISSIONS, RELIABILITY & PLATFORM REALITY

### Principle: no unnecessary privileges

- Orbit must run as the normal user, not administrator/root.
- Access privileges only when features genuinely need them.
- Permission onboarding should explain the exact feature and show an independently usable fallback.
- Users should be able to disable input monitoring-dependent features while keeping normal app launching usable.
- Secure keyboard environments can block hooks and text insertion; degrade gracefully rather than bypassing protections.
- No OS-level driver, system-wide Accessibility hijacking or insecure autostart installer.

### Explicit safety constraints

- Never store raw keystroke streams in disk logs or analytics.
- Snippet trigger buffers are short, ephemeral, and never used as general typing history.
- Clipboard capture is transparent and user-controlled; excluded applications are honoured.
- Do not intercept or replay password/OTP entry as a feature.
- Do not execute notes or copied text as shell commands without explicit user invocation.
- Prevent malicious URL protocols, command injection, path traversal and import-based code execution.
- Signing/notarisation and installer trust must be handled properly for release builds.

### macOS platform-specific

- Design for contemporary macOS versions and document minimum OS support; do not claim support for old systems without testing.
- AppKit app activation, `NSPanel` focus and multi-Space behaviour require real-device tests.
- Login item using `SMAppService` where supported; honour user choice.
- Permissions such as Accessibility and Input Monitoring may require app restart/regrant after signing changes. Show actionable UI guidance.
- Do not assume Mac App Store sandboxing allows all global keyboard remapping behaviours; investigate distribution requirements before deciding to ship via the store.

Reference: <https://developer.apple.com/documentation/servicemanagement/smappservice>

### Windows platform-specific

- Win32 message loop, keyboard shortcut thread, clipboard listener and UI thread ownership need deterministic shutdown.
- Handle session lock/unlock, sleep/wake, Windows Explorer restarts, high DPI and different input locales.
- Windows may reserve or already register a shortcut. Failure must appear in settings with alternatives.
- User Account Control secure desktop and integrity levels impose real limitations; do not claim universal injection.
- Fullscreen applications and anti-cheat: allow suspend mode and avoid injecting synthetic keyboard events when unsafe.

### Update policy

- Signed updates; verify provenance and integrity.
- No background network activity unless user has explicitly enabled update checks; a lightweight periodic check can be user-configured after MVP.
- Include a manual “Check for Updates” action.
- Rollback/recovery from failed update should leave shortcuts and notes intact.

---

## 14. PHASED ENGINEERING EXECUTION — DO THIS ORDER

### Phase 0 — Architecture spike & baseline profiling

- Detect OS/build capabilities; inspect repository.
- Scaffold shared Rust core and ONE native host in available OS environment.
- Instrument process memory, application start, hotkey-to-first-frame and query latency.
- Prototype blank native launcher panel, register one working global shortcut, focus input, execute sample command.
- Capture baseline before adopting any heavyweight UI stack.
- Write `docs/DECISIONS.md` with evidence comparing a small set of approaches.

**Gate:** working hotkey → visible/focused native panel → command action, with timestamped metrics.

### Phase 1 — Launcher MVP, highest urgency

- Native launcher UI for available host.
- App discovery/index/cache and launch/focus actions.
- Fuzzy search, ranking, aliases, favourites and recent usage.
- Global shortcut configurator, collision errors, settings persistence.
- Basic quick links and core system actions.
- Keyboard navigation, multi-monitor and focus correctness.
- Minimal tray/menu-bar item, optional start at login.

**Gate:** launcher remains responsive with 1,000+ indexed app/command/quick-link entries and no internet.

### Phase 2 — HyperKey on macOS and Windows

- Generic shortcut mapping model first, OS-specific adapters second.
- Reliable Caps Lock / selectable HyperKey with configurable tap and hold behaviours.
- Both internal Orbit mode and, where dependable, system synthetic modifier mode.
- Mapping action selector, per-app exclusions, suspend toggle and conflict diagnostics.
- Test no stuck modifiers and normal keyboard restoration across crashes/sleep.
- Document permission flow and limitations.

**Gate:** `Hyper + C` reliably focuses chosen coding application on both OS implementations (when OS permissions permit). Shortcuts don't break normal typing.

### Phase 3 — Clipboard, snippets, unlimited notes

- Local SQLite migrations/repositories and search.
- Clipboard history toggle/list/search/copy/paste, retention and exclusions.
- Snippets create/edit/search/insert, optional expansion engine.
- Quick notes with unlimited count, autosave, pin, full-text search, trash/export.
- Data portability and security tests.

**Gate:** ten thousand notes/snippets/clipboard metadata records must not be loaded into RAM at startup; normal launcher remains responsive.

### Phase 4 — Cross-platform parity and resource optimisation

- Build/port remaining native host; share the same core domain logic.
- Implement cross-platform golden behaviour tests.
- Memory pressure benchmarks; CPU/wakeups and game background impact tests.
- Remove any dependency failing the resource budget without strong justification.
- Accessibility and theme polish; low-resource mode.

**Gate:** benchmark report explicitly lists achieved p50/p95/p99, resource measurements, environmental limits and any failures.

### Phase 5 — Release

- Signed installers if signing credentials are available, otherwise test builds with documented limitations.
- CI for macOS and Windows where feasible.
- Migration test from clean and older versions.
- Diagnostics/support screen, safe backup/export, uninstall behaviour.
- Privacy statement based on actual feature operation, including clipboard/keyboard handling.
- User docs and example shortcuts.

### Later, only if the product is demonstrably light

- File search, advanced window switching, optional plugin SDK, optional settings sync, custom actions, deeper Mia/Spotlight integration, calculator enhancements, optional update channel.

**Do not let later features regress startup latency, hotkey dispatch or idle RAM.**

---

## 15. BENCHMARK & TEST PLAN (MUST IMPLEMENT)

### Timing measurement points

```text
T0 = OS event received at hotkey handler
T1 = show requested
T2 = window visible / first native render commit
T3 = launcher actually focused and accepting input
T4 = query received in core
T5 = ranked result set ready
T6 = native results rendered
T7 = Enter/action dispatched
```

Derived metrics:

- `hotkey_to_visible = T2 - T0`
- `hotkey_to_ready = T3 - T0`
- `query_compute = T5 - T4`
- `query_to_render = T6 - T4`
- `command_dispatch_latency = T7 - EnterKeyEventReceived`

Avoid self-inflating claims from `T1-T0` as if that measured actual show time. Where frame timestamps are approximate, name the approximation and verify with OS tooling or external capture if possible.

### Memory metrics

On Windows record: private bytes, working set, peak working set, process count, relevant GPU memory where available. On macOS record resident size and memory footprint (and tools/method). Measure launcher closed/idle, open, after 500 searches, after 100 note opens, and under memory pressure. Distinguish persistent allocations from filesystem cache.

### Tests

- Core search score unit tests with Unicode, exact, fuzzy, aliases, tie-breaks.
- Command dispatch tests; registered hotkey conflicts and missing executable paths.
- Search with 1,000 / 10,000 / 100,000 metadata records, separating UI and index costs.
- Typing during live result updates: no dropped characters or selection jumps.
- Clipboard content types: text, HTML, image, file list, huge content, secrets, duplicate copies.
- Clipboard excluded app never persists content.
- Snippet expands valid trigger exactly once; secure fields are ignored where identifiable.
- Note autosave after sudden process termination and quick reopen.
- HyperKey key-down/key-up ordering, tap vs hold, autorepeat, physical keyboard variety, crashes.
- Shortcut re-register after sleep/resume.
- Crash/restart data integrity and OS permission denial paths.
- User disables all optional features and sees reduced background footprint.
- Installer/uninstaller leaves normal keyboard behaviour and restores login item state appropriately.
- Minimum supported Windows/macOS versions, DPI, accessibility, locale/IME and multi-monitor.

### Performance regression gate

Add a benchmark runner that emits JSON and Markdown reports. Compare p95 latency and idle footprint against a checked-in baseline. Treat a regression of **more than ~15%** in a core performance metric as a review blocker unless there is a documented and accepted tradeoff. Avoid noisy automated thresholds on shared CI runners; use repeatable local reference-machine runs for final release decisions.

---

## 16. PRODUCT DECISIONS & TRADEOFF RULES

Use this decision matrix if a tempting feature conflicts with performance:

| Question | Preferred decision |
|---|---|
| Can it work fully offline? | Yes by default. |
| Does it need a permanent helper? | Avoid unless essential; budget it. |
| Does it increase idle RAM significantly? | Make optional or reject. |
| Can app-name search avoid disk/network work? | Yes: pre-index. |
| Can normal global hotkeys avoid raw keyboard hooks? | Yes: use native registration. |
| Are raw hooks needed for Caps Lock Hyper? | Use only with explicit permission/configuration and cheap callbacks. |
| Does UI need embedded HTML? | No for MVP. |
| Does every note load into memory? | No; paginate and index. |
| Must clipboard logging be on by default? | No. |
| Must a user have an account? | No. |
| Could notes/snippets be arbitrarily capped? | No. |
| Should Linux be built simultaneously at expense of Mac/Win quality? | No, only after core parity is stable. |

### Monetisation concept, later

A launcher with basic hotkeys, HyperKey, snippets and unlimited local notes should have a generous free base. If a Pro tier exists, charge for optional extras such as encrypted device sync, multi-device profiles, advanced automation, custom extensions and team workflows. Never cripple local keyboard shortcuts or impose a five-note limit just to upsell. This is a product hypothesis, not a required payment integration.

### Naming note

“Orbit” is the working name; research trademarks and app-store naming collisions before public release. Avoid using logos or copyrighted assets from Raycast/PowerToys/Alfred. The user's separate Spotify client is also named Spotlight, which collides semantically with Apple Spotlight; keep integrations and naming clear.

---

## 17. RECOMMENDED MVP INTERACTION EXAMPLES

### Example A: open coding application in one chord

1. User opens Orbit settings → HyperKey.
2. Enables remap with the permission flow, chooses Caps Lock.
3. Adds mapping `Hyper + C` → “Launch/focus [chosen editor]”.
4. Presses Caps Lock + C from any ordinary application.
5. Editor becomes foreground; user can type normally, with Caps Lock state unmodified.

### Example B: launch through search

1. Press configured launcher shortcut.
2. Type `vsc`.
3. VS Code appears due to prefix/alias/fuzzy ranking.
4. Press Enter. Native app opens/activates.
5. Orbit hides cleanly with focus returning appropriately when reopening.

### Example C: store and use a snippet

1. Open snippet manager with `Hyper + S`.
2. Create trigger `;sig` containing a signature template.
3. Optionally enable text expansion and consent to input monitoring.
4. Type `;sig` in an allowed editor; text expands once.
5. In a password field or excluded app, no expansion and no text capture.

### Example D: quick note

1. Press `Hyper + N`.
2. Type “Ask about turbo fitment and compare costs” and close.
3. Note is saved locally without an account.
4. Next day, search `turbo` in the launcher; note appears and opens.
5. Creating note #6, #50 or #500 is not restricted.

### Example E: work laptop under severe memory pressure

1. Laptop has multiple browsers, office apps and a nearly full RAM budget.
2. Orbit is resident but most nonessential caches have been pruned.
3. Launcher hotkey opens promptly; cached app/command index works without disk scan.
4. If clipboard image previews or file search are disabled in Low Resource Mode, core launching and HyperKey remain fully functional.
5. Diagnostics confirms measured idle resident memory and index state.

---

## 18. EXPECTED DOCUMENTATION & FINAL REPORT

Required project docs:

- `README.md` — install/build/run, supported systems, permissions and common shortcuts.
- `docs/ARCHITECTURE.md` — core/hosts architecture and ABI/threading.
- `docs/PERFORMANCE_BUDGET.md` — targets, measurement methodology, actual benchmark results.
- `docs/SHORTCUT_SEMANTICS.md` — physical vs logical keys, HyperKey state machine, conflicts, secure input boundaries.
- `docs/PRIVACY.md` — clipboard, hotkey and snippet capture, storage and exclusion policies.
- `docs/DATA_PORTABILITY.md` — import/export formats, schema version and rollback.
- `docs/DECISIONS.md` — architecture decisions and measured tradeoffs.
- `docs/ROADMAP.md` — complete vs future features.

The coding agent's final response must state:

1. Actual project files created/changed.
2. Features working on macOS and Windows **separately**; do not imply untested parity.
3. Run/test commands, results and any missing signing/hardware prerequisites.
4. Measured runtime footprint and hotkey/search latency **only if actually measured**, including hardware/environment.
5. Known permission/OS limitations and recovery paths.
6. Remaining items prioritised by impact on user value and performance.

---

## 19. THE EXACT FIRST TASK — START CODING

**Begin immediately.** Inspect the current repository and available OS toolchain. Create a small Rust core library with a command registry and fast ranked search, connect it to a single **native** host on the available platform, register a global launcher shortcut, show/focus a compact input panel, find an installed app and launch it. Add a simple benchmark log for hotkey-to-visible, hotkey-to-ready, search latency and resident memory. Keep optional features absent from the process at this stage. After that works, implement the shared hotkey mapping model and the platform-native HyperKey adapter, then clipboard/snippets/unlimited notes, followed by cross-platform parity and resource-pressure profiling.

If you only have one OS available, complete that native host and create a clearly documented plan/scaffold for the other. **Do not claim that Windows and macOS both work without testing both.**

### Final product standard

When Orbit is not actively in use, a user should barely know it is running. When they press a shortcut, it should feel immediate. A small, dependable launcher that survives a RAM-starved workday is a greater success than a massive extension catalogue that adds noticeable overhead.
