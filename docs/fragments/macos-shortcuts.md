# macOS shortcuts

Sources: `platforms/macos/Sources/Orbit/Hotkeys.swift`, `HyperTap.swift`, `AppController.swift`, `crates/orbit-hotkeys/src/hyper.rs`. Tested on one Mac: M4 Pro, macOS 27.2, built-in keyboard, F-key chords injected by the test driver.

## Hotkey chain

Direct shortcuts use Carbon's `RegisterEventHotKey` with `kEventHotKeyExclusive`. It needs no permission and holds no event tap.

`registerHotkeys` tries the launcher shortcut in this order, and stops at the first that registers:

1. The shortcut you chose.
2. Option+Space, Ctrl+Option+Space, Ctrl+Shift+Space, Ctrl+Option+O.

Cmd+Space isn't in the chain. `--hotkey` replaces the whole chain with one chord, and `--fallback-hotkeys` replaces the four defaults. Both exist so tests stay away from real shortcuts.

When Orbit falls back it says which shortcut it's using instead. If nothing registers, it tells you to use the menu bar icon. Problems appear in a toast, the menu bar menu, Settings and Diagnostics.

`Settings.hotkey_notice` holds the last announced text, so a problem gets one toast per distinct text. A second start with the same problem stays quiet and Diagnostics still lists it. If the problem goes away and comes back, Orbit announces it again. `feature_checks.sh` verifies this through a `hotkey-notice` line in the bench log. At start-up in bench mode the toast itself is skipped, so the check confirms the announcement decision and not the toast window.

Reset All Shortcuts clears the launcher shortcut, direct shortcuts and Hyper mappings. It asks for confirmation first. Other HyperKey settings stay.

## Collision detection limits

Orbit can detect two kinds of collision:

- `kEventHotKeyExclusive` makes registration fail if another process holds the same chord through Carbon. Verified with two Orbit processes.
- `CopySymbolicHotKeys` lists the enabled system shortcuts, and Orbit refuses a match. Read in the code, not tested with a real system shortcut, because tests never register one.

It can't detect these:

- An app that watches keys with its own event tap instead of registering a hotkey.
- An app's own menu shortcuts, which only apply while that app is active.
- A shortcut the system doesn't list in the symbolic hotkey table.

A collision of those kinds means both programs may react, or one may swallow the key first, with no message from Orbit. Not tested.

## HyperKey modes

| Mode | Mechanism | Needs |
|---|---|---|
| External | Another tool sends the four modifiers. Orbit registers each mapping with Carbon and holds no tap. | Nothing. Not tested end to end with a real external tool. |
| Internal | A `CGEventTap` runs `HyperMachine`. The trigger is swallowed and bound keys dispatch actions. | Accessibility and Input Monitoring |
| Synthetic | The same tap adds Ctrl, Option, Shift and Cmd flags to keys passing through. | Accessibility and Input Monitoring |

The trigger's tap action is one of Escape, Original, None or Leader.

With Leader, a tap of the trigger starts a 2 s wait with nothing held. The next key dispatches if it's mapped, and is swallowed along with its repeats and key-up. An unmapped key types normally and ends the wait, and so does a late key. Modifiers pass through without ending the wait. The e2e driver verifies this through the tap with F19 as the trigger, under the grants the driver inherits from its parent process. Orbit.app's own grant wasn't tested, because each rebuild of an ad-hoc-signed app needs granting again.

Orbit's Caps Lock trigger path has not been run on this Mac. The owner already maps Caps Lock to F20, and tests never write the real mapping.

## hidutil and the journal

Internal and Synthetic modes with Caps Lock as the trigger use `hidutil` to map Caps Lock to F18, and the tap watches F18.

1. Orbit reads the current mapping and asks the core for a plan. The core refuses if another tool has remapped Caps Lock or keyboards hold different lists. Orbit shows that refusal.
2. Orbit writes `hidutil-journal` in the data folder.
3. Orbit runs the `hidutil` write.
4. Orbit removes its own pair on quit, at the lock screen, when the feature is turned off, and at the next start after a crash. A refused removal keeps the journal, so the next start tries again.

Orbit applies the mapping again after wake and unlock. The mapping doesn't survive a reboot.

Verified: plan logic in `cargo test -p orbit-ffi`, a write aimed at a device that doesn't exist changes nothing (`--self-test`), and a leftover journal is cleared at start without changing other tools' mappings (`feature_checks.sh`). Not tested: a real Caps Lock remap.

## Tap re-enable

macOS sends `tapDisabledByTimeout` or `tapDisabledByUserInput` when it disables a tap. Orbit resets the machine, clears the trigger buffer, and re-enables the tap at most 5 times a minute (`TapGuard`). Past that it drops the tap, turns HyperKey off, and tells you to re-enable it from Settings. `--self-test` verifies the limit. A real disable from macOS hasn't been seen or forced.

The tap callback calls only the allocation-free state machines. The tap runs on its own thread. Resets also happen on sleep, on lock, and when the frontmost app changes.

## Secure input

While another app holds secure input, such as in a password field, macOS doesn't deliver keystrokes to event taps. HyperKey and expansion then get no keys. Direct shortcuts through Carbon were not tested in that state.

`Inserter.canPostKeys` is false during secure input, so a snippet goes to the clipboard with a message instead of being typed. Expansion clears its buffer when `IsSecureEventInputEnabled()` is true. Read in the code, not tested with a real secure field.

## Snippet insertion

Inserted text is typed with `CGEvent`. Orbit never saves or restores your clipboard around it. Text over 4096 UTF-16 units goes to the clipboard with a message, and a typed trigger stays in place, as on Windows. `feature_checks.sh` verifies the message and pasteboard contents from the launcher path on a private pasteboard. The path that leaves a trigger in place wasn't run: it's read in the code, where the check comes before any erase.

## IME

While the launcher field has marked text, `control(_:textView:doCommandBy:)` returns false so the input method keeps Enter, Esc and the arrows. The note editor has its own `hasMarkedText()` check. Read in the code. Composition with a real input method is not tested.

Expansion reads characters from key events. How it behaves during composition is not tested.

## Known limit

Keys typed within about 15 ms of the shortcut can reach the previous app, because the key window changes after the hotkey arrives. Earlier builds were clean in 7 and 9 of 10 attempts at 15 ms. Both runs of the final build were clean in 10 of 10, which is too few attempts to call it fixed. At 30 ms every run was clean.
