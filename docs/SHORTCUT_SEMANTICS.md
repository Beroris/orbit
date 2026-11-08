# Shortcut semantics

How Orbit identifies keys, how the HyperKey state machine decides, and where shortcuts stop working. Sections 1 to 8 describe the Windows host, and the macOS host has its own section at the end. Sources: `crates/orbit-hotkeys/src/{chord,hyper}.rs`, `crates/orbit-core/src/hotkeys.rs`, `platforms/windows/src/{hook,keys,game}.rs`, and the wiring in `platforms/windows/src/app.rs`.

Where this document says "not tested" or "no code", it means exactly that: the behaviour comes from Windows or is unknown, not something Orbit's tests establish. Latency and timing figures are in [PERFORMANCE_BUDGET.md](PERFORMANCE_BUDGET.md).

Related: [ARCHITECTURE.md](ARCHITECTURE.md), [PRIVACY.md](PRIVACY.md).

## 1. Physical versus logical keys

Orbit matches **logical keys by Windows virtual-key (VK) code**. It does not match physical key positions.

- Shortcuts are stored as text with logical names: `C`, `Space`, `F13`, `CapsLock`, `;`. `Chord::parse` canonicalises them (`chord.rs`), and `keys::vk_from_name` maps a name to a VK code (`keys.rs`).
- The keyboard hook reads `KBDLLHOOKSTRUCT.vkCode`. For letters and punctuation, Windows derives that code from the active keyboard layout. So `Hyper+A` means "the key that produces A on the current layout". On a layout where that key sits elsewhere, the binding follows the letter, not the position.
- The scan code is not used to decide anything. It is passed along only when Orbit re-injects a key in synthetic mode, and to `ToUnicodeEx` for snippet expansion.
- There is no setting for physical-position matching.

Details:

| Item | Behaviour |
|---|---|
| Key names accepted | Letters, digits, `, . / ; ' [ ] \ - = \``, `F1`–`F24`, `Space`, `Enter`, `Tab`, `Escape`, `Backspace`, `Delete`, `Insert`, `Home`, `End`, `PageUp`, `PageDown`, arrows, `CapsLock`, `Menu`, `RightAlt`, `RightCtrl`, `RightShift`, `RightSuper`, `ScrollLock`, `Pause`. Anything else is rejected with `unknown key name`. |
| Numpad keys | No names. Numpad digits cannot be bound; the digit names are the top-row keys. |
| Modifiers in a chord | `Ctrl`, `Alt` (`Option`), `Shift`, `Win` (`Cmd`, `Super`, `Meta`), and `Hyper` = all four. Left and right variants are not distinguished in a chord. |
| Printed form | `Ctrl+Alt+Shift+Super+key` order; all four modifiers print as `Hyper+key`. The saved file uses `Super`, the UI shows `Win`. |
| Plus key | `Ctrl++` is not supported. |
| Modifier-only chords | Rejected (`shortcut has only modifiers`). |
| Two main keys | Rejected. |
| Layout at expansion time | The foreground thread's layout (`GetKeyboardLayout`) is used to turn key events into characters for snippet triggers. |

## 2. Three ways a shortcut reaches Orbit

| Mechanism | Used for | Needs a hook |
|---|---|---|
| `RegisterHotKey` with `MOD_NOREPEAT` | Launcher shortcut, every entry in `shortcuts`, and Hyper bindings in `external` mode (registered as `Ctrl+Alt+Shift+Win+key`) | No |
| `WH_KEYBOARD_LL` + `HyperMachine` | HyperKey in `internal` and `synthetic` modes | Yes |
| `WH_KEYBOARD_LL` + `TriggerMatcher` | Snippet expansion | Yes |

The hook does not exist unless HyperKey (non-external) or expansion is enabled (`hook::config_from_settings`, test `no_hook_by_default`). If Windows refuses the hook, direct shortcuts keep working and a notification says so.

## 3. The HyperKey state machine

`HyperMachine` has no OS dependency. The host feeds it every physical key event and applies the result. Defaults: trigger `CapsLock`, mode `internal`, tap behaviour `escape`, tap timeout 250 ms (the settings window accepts 50–2000), HyperKey off.

### Modes

| Mode | Unbound key pressed while Hyper is held | Other apps see a Hyper chord |
|---|---|---|
| `internal` (default) | Swallowed and ignored | No |
| `synthetic` | Re-injected with Ctrl+Alt+Shift+Win held | Yes |
| `external` | No hook runs at all. Orbit registers `Hyper+key` hotkeys for its own bindings and expects another tool to produce the chord | Whatever the other tool sends |

Bound keys (enabled bindings) are always handled by Orbit in `internal` and `synthetic` modes. The key is swallowed and `Dispatch` is posted to the UI thread. Unbound keys in `internal` mode are swallowed so a letter is not typed by accident.

### State

| State | Meaning |
|---|---|
| `held` | The trigger is down and was pressed while Orbit was not suspended |
| `chord_used` | Some non-modifier key was pressed during this hold, so the release is not a tap |
| `synthetic_active` | Virtual Ctrl+Alt+Shift+Win are currently down in the OS |
| `down` | Every key seen down and not yet up, whether passed or swallowed |
| `swallowed` | Keys whose key-down was swallowed. Their key-up and autorepeat are swallowed too |
| `suspended` | Everything passes untouched |

### Transitions (per event, `on_key`)

Checked in this order:

1. If the key is in `swallowed`: swallow it. On key-up, remove it from `swallowed`. This runs before anything else, so a key held past the trigger's release never types.
2. If suspended: pass.
3. If the key is the trigger:
   - Key-down: swallow. First press (not already held) starts the hold and records the time. Autorepeat is swallowed with no effect.
   - Key-up with no hold in progress: pass. This covers a key-up from before Orbit started or after a reset.
   - Key-up ending a hold: it is a **tap** if no chord was used and the hold lasted at most `tap_timeout_ms` (a wrapping 32-bit millisecond clock, tested across wraparound). The key-up is swallowed. If `synthetic_active`, the effect is `SyntheticRelease` and it is never a tap. Otherwise a tap fires the configured tap effect once, unless the tap behaviour is `none` or `leader`. With `leader`, a tap starts a 2 s window (`LEADER_WINDOW_MS`) in which the next key, pressed with nothing held, is treated as if it were pressed during a hold.
4. If not held, or the key is a modifier: pass. Modifiers pressed during a hold pass and do not turn a tap into a chord.
5. Key-up of a key that was not swallowed: pass. The app saw its key-down, so it must see the key-up.
6. Key-down repeat of a key that went down before the trigger: swallow, with no effect and no chord. Its key-up still passes.
7. Any other key-down during a hold marks `chord_used`, then:
   - bound: add to `swallowed`, swallow, `Dispatch { code }`;
   - unbound, `synthetic` mode, virtual modifiers not yet down: swallow and `SyntheticChordDown`. The real key-up then passes through unchanged. While `synthetic_active`, later keys simply pass under the held virtual modifiers;
   - unbound, `internal` or `external` mode: add to `swallowed`, swallow.

Multiple chords in one hold are allowed; each fires once.

### Tap effects (performed by the hook thread)

| Tap setting | What the host injects |
|---|---|
| `escape` (default) | Escape press and release |
| `original` | The trigger key's own press and release, so Caps Lock toggles on tap |
| `none` | Nothing |
| `leader` | Nothing. The tap opens a 2 s window for one mapped key, pressed with nothing held |

The trigger key itself is always swallowed while the machine is active, so Caps Lock does not toggle when used as Hyper and typing does not turn upper case. Orbit does not read or change the existing Caps Lock state except through the `original` tap. Keys Orbit re-injects in synthetic mode have their autorepeat swallowed, and their key-up passes.

### Synthetic chord and release

`SyntheticChordDown` sends Left Ctrl, Left Alt, Left Shift, Left Win down, then the key. `SyntheticRelease` sends a masking key (VK `0xFF`) press and release, so the Win key-up does not open the Start menu, then releases Win, Shift, Alt, Ctrl.

Everything Orbit injects carries `dwExtraInfo = 0x4F524249` (`"ORBI"`). The hook passes tagged events straight through, so Orbit never processes its own input.

Events injected by other software (`LLKHF_INJECTED`) pass through untouched: they don't drive HyperKey and don't feed the trigger buffer. The `--accept-injected-input` flag turns this off for Orbit's own test driver.

### Guarantees the code and tests enforce

Tests in `hyper.rs` cover: a tap fires exactly once; slow hold is not a tap; a chord fires once and its release does not fire the tap; the trigger released before the letter still swallows the letter's key-up; autorepeat after trigger release does not type; modifiers pass and do not cancel a tap; keys held before the trigger keep their key-up; reset mid-chord releases virtual modifiers; suspend finishes held keys safely; clock wraparound; plain typing is untouched; and a randomised test that every swallowed key-down gets a swallowed key-up and the machine ends idle. `cargo test -p orbit-hotkeys` runs them. They test the state machine, not real hardware.

### Hook lifecycle

- The hook runs on its own thread at the highest priority. The callback does not touch a database or start a process. It updates small in-memory state, posts messages, and for tap and synthetic effects calls `SendInput`.
- The thread reinstalls the hook every five minutes if the machine is idle (not mid-chord), to recover from Windows dropping a slow hook, and immediately after unlock or resume. A reinstall counter is shown in Diagnostics.
- While a hold is in progress a one-second watchdog timer runs, and only then. If no key event arrives for 5 s during a hold, or a hold lasts 30 s, it reinstalls the hook and resets, because the key-up it's waiting for may never come. It also retries releasing virtual modifiers.
- Moving to an elevated foreground window resets a hold, since key events from that window never reach the hook.
- On quit, panic or an unhandled exception, Orbit releases any virtual modifiers it holds (`release_synthetic_modifiers`, called from the panic hook, the exception filter, `reset` and shutdown). Shutdown resets the machine before unhooking.
- Suspending shortcuts (tray menu or "Suspend Orbit Shortcuts") removes the hook and unregisters every `RegisterHotKey` shortcut, so chords reach the focused app.
- An exclusion list (`hyper.excluded_apps`) and the game setting suspend the machine. The decision is made each time the trigger goes down while not held; it is not re-evaluated mid-hold.
- The launcher's hotkey has its own list (`launcher.excluded_apps`) and game setting (`launcher.pass_through_in_games`, on by default), with the same rule for a game: it must fill the screen, and either be recognised as a game or be in exclusive fullscreen. A registered hotkey never reaches another program, so Orbit unregisters it while such a program is in front and registers it again afterwards. It checks when the front window changes, through a `WinEvent` hook that exists only while the list has entries or the game setting is on. A game can fill the screen after it came to the front, which raises no event. The first press then only opens nothing and releases the key, and later presses reach the game.

## 4. Conflicts

| Situation | What Orbit does |
|---|---|
| Launcher shortcut already registered by another program | `RegisterHotKey` fails. Orbit tries, in order: the user's choice, then `Alt+Space`, `Ctrl+Alt+Space`, `Ctrl+Shift+Space`, `Ctrl+Alt+O`. The first that registers wins. Any fallback is reported, and the message names free alternatives. It is announced once per distinct text and stays visible in Settings and Diagnostics. If none registers, the notice says to use the tray icon or Settings. |
| Direct shortcut already taken | `RegisterHotKey` fails; the problem is listed in Diagnostics and Settings, and announced once per distinct problem text. |
| Chord Orbit considers reserved | `Ctrl+Alt+Delete`, `Win+L`, `Alt+Tab`, `Alt+F4`, `Alt+Esc` are refused before registration (`is_reserved_on_windows`). The list is advisory; the OS registration result is the real answer. |
| Launcher chord equals a direct shortcut | The settings window refuses to save. |
| Two Hyper bindings on one key | Allowed. The first enabled binding wins, and a problem line says so (`duplicate_hyper_keys`). |
| Two snippets with the same trigger | Allowed. The older snippet (lower id) wins when both match with the same length; saving shows which one wins. Longer triggers beat shorter ones. |
| Hyper trigger also used by another remapper | Orbit does not detect other hooks. Not tested. |
| After unlock or resume | All hotkeys are unregistered and registered again, so a combination another program took in the meantime is listed as a problem in Diagnostics. |

## 5. Secure input boundaries

Windows has no single "secure input" flag like macOS. This is what Orbit does, and does not do.

**Detection Orbit has**

- Snippet expansion: before erasing a typed trigger, the insertion thread asks UI Automation whether the focused element is a password field (`IsPassword`, with 700 ms timeouts), which also recognises browser and XAML fields. It also checks classic edit controls for `ES_PASSWORD`. If either says yes, or if the check can't answer, the expansion is dropped and nothing is erased. A custom-drawn box that doesn't report itself to UI Automation isn't recognised.
- Per-app exclusion lists by executable name: `hyper.excluded_apps` (empty by default), `snippets.excluded_apps` and `clipboard.excluded_apps` (both default to a list of password-manager executables). Matching is a case-insensitive comparison of the executable file name. If the process image path cannot be read, the name is empty and nothing matches. In Settings these lists are chosen by searching a list of apps, or with Browse, rather than typed. A UWP app's windows belong to `ApplicationFrameHost.exe`, so one can't be excluded on its own, and Settings doesn't offer that host.
- Clipboard opt-out formats are honoured before content is read. See [PRIVACY.md](PRIVACY.md).

**What Orbit does not do**

- HyperKey has no password-field check. It only has the exclusion list.
- The expansion hook receives key events while a password field has focus. The check runs after a trigger has matched, not before keys enter the buffer. The buffer is at most 32 characters, in memory only, never on disk, and is cleared by the events in section 6. Text typed into a web password field can therefore sit in that buffer until one of them happens.
- There is no IME handling. Composition is not detected; behaviour while composing is not tested.
- There is no code that reacts to the UAC secure desktop or the sign-in screen. Orbit runs on the user's desktop. Keyboard hooks installed from the user desktop are not given events from the secure desktop by Windows; this is Windows behaviour, not tested here.
- Orbit does not try to read or replay password or one-time-code entry.

## 6. Snippet expansion boundaries

Opt-in (`snippets.expansion_enabled`, default off). Off means no hook and no buffer.

| Rule | Detail (`snippet.rs`, `hook.rs`) |
|---|---|
| What is kept | A fixed array of at most 32 characters (`MAX_TRIGGER_CHARS`), in memory. Never written to disk or logged. |
| Trigger validity | 2–32 characters, no whitespace. One-character triggers are refused because they would fire during ordinary typing. Invalid triggers are ignored by the matcher. |
| Matching | Exact, case-sensitive suffix match of the recent characters. No word-boundary or delimiter is needed: the expansion fires when the last trigger character is typed. Longest trigger wins; ties go to the lower snippet id. |
| Which keys feed it | Key-downs only. Characters come from `ToUnicodeEx` with the "do not change keyboard state" flag, so the focused app's dead-key state is not disturbed. Events the Hyper machine swallowed are not fed. Events Orbit injected are ignored. |
| Backspace | Removes the last character from the buffer. |
| Cleared by | Tab, Enter, Escape, Page Up/Down, Home, End, arrows, Insert, Delete; Ctrl, Alt or Win combinations (Ctrl+Alt together counts as AltGr and types text); a key that produces no character; a chord Orbit swallowed; a foreground-window change; a focus change or mouse capture reported by Windows; a pause of 5 s between keys; a match; lock, suspend and reload of the trigger list. |
| Not observed | Mouse clicks as such. There is no mouse hook. A click inside a browser page or other custom-drawn surface may raise no focus or capture event, so text typed either side of it within 5 s can still join into a match. |
| Exclusions | `snippets.excluded_apps` is checked on each foreground change (exe name). Expansion is off in Orbit's own windows. |
| Games | A match is dropped if the window in front is a fullscreen game (section 7), whatever `pass_through_in_games` says. That setting applies to HyperKey. |
| On a match | The final trigger character has already reached the app. The UI thread checks for a password control, fetches the snippet, expands `{date}`, `{time}`, `{clipboard}`, `{cursor}`, then an insertion thread erases the trigger with Backspace presses and inserts. |
| `{clipboard}` | Never inserted silently on expansion: a preview row is shown and the user must approve. A snippet picked manually from the launcher is inserted without that extra step. |
| Insertion | Waits up to a short fixed time for the target window to be foreground and for held modifiers to be released, then types the text as Unicode key events in chunks of 128 UTF-16 units with a 10 ms pause, rechecking focus and modifiers between chunks. Surrogate pairs stay together. The limit is 4096 units. Longer text goes on the clipboard with a message saying to press Ctrl+V, and the typed trigger stays in place. Orbit never saves and restores your clipboard. `{cursor}` is honoured by pressing Left. Orbit's own clipboard writes are not recorded in history. Long text can take seconds in slow targets (about 7 s for 3,099 units in a plain Win32 edit box). |
| Elevated target | If the target process is elevated and Orbit is not, injection is skipped, the text is put on the clipboard, and a notification says so. If the target cannot be inspected it is treated as elevated. |
| Triggers after import | The matcher's trigger list is refreshed when a snippet is saved or deleted, at startup, and after an import. |

## 7. Environments

| Situation | What happens |
|---|---|
| **Lock screen** (`WTS_SESSION_LOCK`) | The launcher hides. The hook thread resets: transient key state and the trigger buffer are cleared and any virtual modifiers are released. The hook stays installed. Orbit does nothing at the lock screen itself; no code runs there. |
| **Unlock** | The hook is replaced with a fresh one and every hotkey is registered again. |
| **Sleep and resume** | Suspend resets the hook state like lock. Resume reinstalls the hook and re-registers hotkeys. |
| **Elevated (administrator) window focused** | Direct shortcuts registered with `RegisterHotKey` are delivered by Windows regardless of the focused window. Windows gives the keyboard hook no events from an elevated window, so HyperKey stops there; when one takes the foreground during a hold, Orbit resets it. Text insertion into an elevated window is refused and falls back to the clipboard (section 6). If Orbit itself runs elevated, it does not apply that check. None of this has been exercised on a real elevated window. |
| **UAC prompt (secure desktop)** | Not handled and cannot be; see §5. |
| **Fullscreen app that isn't a game** | Nothing changes. HyperKey and snippets keep working in a browser in F11 or an editor filling the monitor. |
| **Games** | With `pass_through_in_games` (default on), when the trigger goes down in a game, the machine is suspended for that hold: the trigger and every key pass through untouched, so Caps Lock behaves normally. A window counts as a game only if it covers its whole monitor and is not the desktop, and one of these holds: its program is in Windows' own list (`HKCU\System\GameConfigStore\Children\*\MatchedExeFullPath`), it lives in a game store's library folder such as `\steamapps\common\`, it has an Unreal shipping build's name, or Windows reports exclusive Direct3D fullscreen. A game in a window keeps HyperKey, because filling the monitor is required: Windows' list can hold general programs, such as `javaw.exe` on the test machine. A borderless game that matches none of these isn't recognised, so add its program to `hyper.excluded_apps`. The registry list is read on a short-lived thread at hook start, and again at most once every 60 s when an unknown program is fullscreen at a focus change. The exclusive-fullscreen query (about 60 µs, measured over 2,000 calls via .NET P/Invoke) runs only when a window already fills the monitor as the trigger goes down. Direct `RegisterHotKey` shortcuts are not affected by any of this. The feature suite tests this with a small test window placed in a `steamapps\common` folder; no real game has been tried. |
| **Anti-cheat** | How a given anti-cheat treats a low-level hook is not tested. "Suspend Orbit Shortcuts" removes the keyboard hook and unregisters the `RegisterHotKey` shortcuts, so chords reach the game. Direct shortcuts ignore per-app exclusions, so a game that needs one of them needs suspending. |
| **Remote sessions (RDP)** | Lock, unlock, and console and remote connect and disconnect are all handled for this session: disconnect resets the hook state, connect reinstalls the hook and re-registers shortcuts. There is no check for being inside a remote session. The hook applies to whichever window is focused, including a remote-desktop client window. To keep the trigger key going to the remote machine, add the client's executable name (for example `mstsc.exe`) to `hyper.excluded_apps`. Behaviour inside remote sessions is not tested. |
| **Explorer restart** | The tray icon is re-added when `TaskbarCreated` arrives. |
| **Multiple keyboards** | Not distinguished. Events are not filtered by device. |
| **Quit, crash** | Hook removed, virtual modifiers released, hotkeys unregistered. A crash that skips the handlers cannot be cleaned up by Orbit; Windows removes the hook when the process dies. |

## 7a. Setting a hotkey, and pressing it again

Source: `crates/orbit-core/src/hotkeys.rs` for the rules, `platforms/windows/src/app.rs` for recording.

| Item | Behaviour |
|---|---|
| Where | Ctrl+K on an item, then **Set hotkey**. An item with a hotkey shows **Change hotkey** and **Remove hotkey**, and its keys appear on its row. Settings has a Hotkeys page listing them all, with Remove (or Delete) and an "Edit as text" view. |
| One to one | An item has at most one hotkey, and a hotkey runs one item, across HyperKey mappings and direct shortcuts. Assigning replaces the item's old one. If the key already runs something else, a row asks first. |
| While recording | Registered shortcuts are unregistered and the hook reports every key held with the trigger, so a key already in use can still be pressed. A key with no modifier does nothing unless it is a function key. Esc cancels. Windows' reserved shortcuts are refused. Nothing typed during recording reaches the search box. |
| Pressed again | With `launcher.hotkey_toggles_app` (default on), an app's hotkey minimizes it when one of its windows is in front. If it isn't in front, the hotkey focuses it, or restores or launches it. Commands don't toggle this way, except the launcher views, which already close on a second press. |

## 7b. macOS feel

Off by default. All of this is decided by `hook::mac_chord`, which has its own tests.

| Key | Does | Sent to the app |
|---|---|---|
| Alt+Left, Alt+Right | Start or end of the line | Home, End |
| Alt+Shift+Left, Alt+Shift+Right | Select to the start or end | Shift+Home, Shift+End |
| Alt+Backspace | Delete back to the start | Shift+Home, then Backspace |
| Alt+Q | Close the front app | `WM_CLOSE` to its windows |
| Alt+H | Hide the front app | `SW_MINIMIZE` to its windows |

For a shared host such as File Explorer or a packaged app, only the front window is affected, so Alt+Q doesn't close every Explorer window or Store app. The desktop and taskbar are skipped.

Alt is lifted for the length of the sent keys and put back, with a masking key so that releasing it doesn't open a menu. Alt+Q and Alt+H act once per press, not on key repeat. All of them are left alone in full-screen games, in remote-session apps, for input injected by other programs, and in apps on the list. Alt+Q and Alt+H also leave Orbit's own windows alone, because Alt+H is a Vim key there. Text keys replace Back and Forward in browsers and File Explorer, unless those apps are listed.

## 8. Known gaps against the brief

- No physical-key (scan code) mode; matching is by layout-dependent VK code.
- No IME composition handling; no test of IME, multiple keyboards, RDP or real games on real hardware in this repository.
- Password-field detection depends on what a field reports to UI Automation, and the expansion buffer sees keys typed in password fields before the check.
- `{selection}` and rich-text snippets aren't implemented.
- Direct `RegisterHotKey` shortcuts ignore per-app exclusions.
- A key typed at the same instant as the hotkey can race the focus change and reach the previous app. PERFORMANCE_BUDGET.md has the measured window.
- HyperKey has no password-field check, only the exclusion list.
- The HyperKey trigger list in Settings is `CapsLock`, `RightCtrl`, `RightAlt`, `RightSuper`, `Menu` and `F1`-`F24`. Other names such as `ScrollLock`, `Pause` and `RightShift` map to VK codes and work if set in `settings.json`, and the settings window keeps them. The feature suite injects its key presses, so no trigger has been tested with a physical key press.
- The Settings page says HyperKey "stops at the lock screen and over elevated windows". The second part relies on Windows and has not been exercised on a real elevated window.

## macOS

Set hotkey in the menu, game detection and press-again-to-minimize exist on Windows only so far. The macOS host still reads the mappings as text, and its setting key was renamed without changing what it detects.

Sources: `platforms/macos/Sources/Orbit/Hotkeys.swift`, `HyperTap.swift`, `AppController.swift`, `crates/orbit-hotkeys/src/hyper.rs`. Tested on one Mac: M4 Pro, macOS 27.2, built-in keyboard, F-key chords injected by the test driver.

### Hotkey chain

Direct shortcuts use Carbon's `RegisterEventHotKey` with `kEventHotKeyExclusive`. It needs no permission and holds no event tap.

`registerHotkeys` tries the launcher shortcut in this order, and stops at the first that registers:

1. The shortcut you chose.
2. Option+Space, Ctrl+Option+Space, Ctrl+Shift+Space, Ctrl+Option+O.

Cmd+Space isn't in the chain. `--hotkey` replaces the whole chain with one chord, and `--fallback-hotkeys` replaces the four defaults. Both exist so tests stay away from real shortcuts.

When Orbit falls back it says which shortcut it's using instead. If nothing registers, it tells you to use the menu bar icon. Problems appear in a toast, the menu bar menu, Settings and Diagnostics.

`Settings.hotkey_notice` holds the last announced text, so a problem gets one toast per distinct text. A second start with the same problem stays quiet and Diagnostics still lists it. If the problem goes away and comes back, Orbit announces it again. `feature_checks.sh` verifies this through a `hotkey-notice` line in the bench log. At start-up in bench mode the toast itself is skipped, so the check confirms the announcement decision and not the toast window.

Reset All Shortcuts clears the launcher shortcut, direct shortcuts and Hyper mappings. It asks for confirmation first. Other HyperKey settings stay.

### Collision detection limits

Orbit can detect two kinds of collision:

- `kEventHotKeyExclusive` makes registration fail if another process holds the same chord through Carbon. Verified with two Orbit processes.
- `CopySymbolicHotKeys` lists the enabled system shortcuts, and Orbit refuses a match. Read in the code, not tested with a real system shortcut, because tests never register one.

It can't detect these:

- An app that watches keys with its own event tap instead of registering a hotkey.
- An app's own menu shortcuts, which only apply while that app is active.
- A shortcut the system doesn't list in the symbolic hotkey table.

A collision of those kinds means both programs may react, or one may swallow the key first, with no message from Orbit. Not tested.

### HyperKey modes

| Mode | Mechanism | Needs |
|---|---|---|
| External | Another tool sends the four modifiers. Orbit registers each mapping with Carbon and holds no tap. | Nothing. Not tested end to end with a real external tool. |
| Internal | A `CGEventTap` runs `HyperMachine`. The trigger is swallowed and bound keys dispatch actions. | Accessibility and Input Monitoring |
| Synthetic | The same tap adds Ctrl, Option, Shift and Cmd flags to keys passing through. | Accessibility and Input Monitoring |

The trigger's tap action is one of Escape, Original, None or Leader.

With Leader, a tap of the trigger starts a 2 s wait with nothing held. The next key dispatches if it's mapped, and is swallowed along with its repeats and key-up. An unmapped key types normally and ends the wait, and so does a late key. Modifiers pass through without ending the wait. The e2e driver verifies this through the tap with F19 as the trigger, under the grants the driver inherits from its parent process. Orbit.app's own grant wasn't tested, because each rebuild of an ad-hoc-signed app needs granting again.

Orbit's Caps Lock trigger path has not been run on this Mac. The owner already maps Caps Lock to F20, and tests never write the real mapping.

### hidutil and the journal

Internal and Synthetic modes with Caps Lock as the trigger use `hidutil` to map Caps Lock to F18, and the tap watches F18.

1. Orbit reads the current mapping and asks the core for a plan. The core refuses if another tool has remapped Caps Lock or keyboards hold different lists. Orbit shows that refusal.
2. Orbit writes `hidutil-journal` in the data folder.
3. Orbit runs the `hidutil` write.
4. Orbit removes its own pair on quit, at the lock screen, when the feature is turned off, and at the next start after a crash. A refused removal keeps the journal, so the next start tries again.

Orbit applies the mapping again after wake and unlock. The mapping doesn't survive a reboot.

Verified: plan logic in `cargo test -p orbit-ffi`, a write aimed at a device that doesn't exist changes nothing (`--self-test`), and a leftover journal is cleared at start without changing other tools' mappings (`feature_checks.sh`). Not tested: a real Caps Lock remap.

### Tap re-enable

macOS sends `tapDisabledByTimeout` or `tapDisabledByUserInput` when it disables a tap. Orbit resets the machine, clears the trigger buffer, and re-enables the tap at most 5 times a minute (`TapGuard`). Past that it drops the tap, turns HyperKey off, and tells you to re-enable it from Settings. `--self-test` verifies the limit. A real disable from macOS hasn't been seen or forced.

The tap callback calls only the allocation-free state machines. The tap runs on its own thread. Resets also happen on sleep, on lock, and when the frontmost app changes.

### Secure input

While another app holds secure input, such as in a password field, macOS doesn't deliver keystrokes to event taps. HyperKey and expansion then get no keys. Direct shortcuts through Carbon were not tested in that state.

`Inserter.canPostKeys` is false during secure input, so a snippet goes to the clipboard with a message instead of being typed. Expansion clears its buffer when `IsSecureEventInputEnabled()` is true. Read in the code, not tested with a real secure field.

### Snippet insertion

Inserted text is typed with `CGEvent`. Orbit never saves or restores your clipboard around it. Text over 4096 UTF-16 units goes to the clipboard with a message, and a typed trigger stays in place, as on Windows. `feature_checks.sh` verifies the message and pasteboard contents from the launcher path on a private pasteboard. The path that leaves a trigger in place wasn't run: it's read in the code, where the check comes before any erase.

### IME

While the launcher field has marked text, `control(_:textView:doCommandBy:)` returns false so the input method keeps Enter, Esc and the arrows. The note editor has its own `hasMarkedText()` check. Read in the code. Composition with a real input method is not tested.

Expansion reads characters from key events. How it behaves during composition is not tested.

### Known limit

Keys typed within about 15 ms of the shortcut can reach the previous app, because the key window changes after the hotkey arrives. Earlier builds were clean in 7 and 9 of 10 attempts at 15 ms. Both runs of the final build were clean in 10 of 10, which is too few attempts to call it fixed. At 30 ms every run was clean.

