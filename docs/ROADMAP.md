# Roadmap

What's built, and what's next by impact. "Tested" below means the automated suites on one machine per platform: see [PERFORMANCE_BUDGET.md](PERFORMANCE_BUDGET.md).

## Status by platform

| Feature | Windows | macOS |
|---|---|---|
| Launcher, search, app launching | tested | tested |
| Ctrl/Cmd+K action menu | a small menu over the list; tested with keys and with clicks sent to the window, and seen on a real screen | tested, as the older full list |
| Orbit menu from the footer logo | tested with a click sent to the window | not built |
| Updates from GitHub releases: the daily look, the download, the notification, the tray item and Settings' Updates row | unit tested; the look and download tested against the real release on GitHub; the Settings row seen in off-screen frames. Installing an update ran end to end once on the owner's PC, from 1.2.0 to 1.2.1 | not built |
| Emoji list | tested, including pasting where you were typing | not built |
| Quicklink and command forms, and making them from the launcher | unit tested, forms seen in off-screen frames; **not used on screen yet** | not built |
| Launcher shortcut left to games and listed apps | tested with a listed app; no real game tried | not built |
| Scrolling Settings pages, and the Snippets and Links pages | unit tested, seen in off-screen frames; real wheel, Tab and dragging **not tested** | not built |
| Settings saving as you change them, with no Save or Cancel | tested: changes made with keys and clicks save with Settings still open, and closing it unchanged changes nothing | not built |
| Settings overhaul: key recorder for the launcher shortcut, hotkeys added and changed on the Hotkeys page, lists sized to fit, dependent settings dimmed, drop-downs for numbers, confirm in place, window fits small screens | checked by driving the hidden window with messages and seen in off-screen frames; real keys, focus, a real second monitor and a screen smaller than the window **not tested** | not built |
| Ctrl+Backspace in Settings and forms | unit tested for where a word starts; not pressed in the real windows yet | not built |
| Searching Settings from the box at the top left, with Ctrl+F or / to get there | unit tested, checked by driving the hidden window with messages, and seen in off-screen frames. **Real typing, clicks and focus not tried on screen yet** | not built |
| Less glass, rounder corners, soft shadow (layered window over a blur plate) | seen on a real screen, recorded frame by frame | not built |
| Motion: fades, a gliding selection, menus rising, pages cross-fading, the height easing, and the Animations switch | seen on a real screen, recorded frame by frame, with the switch on and off; keys typed at once and clicks during the fade-out checked there too | not built |
| Set a hotkey from the action menu | tested | not built |
| Keycap badges and redesigned look | checked by screenshots, before the rounder design above | not built |
| An app's hotkey, pressed again, minimizes it | tested | not built |
| Opening the launcher on the main screen, or on a specific one picked from a picture of the screens | unit tested, read this PC's one real screen, checked by driving the hidden window with messages over three stand-in screens, and seen in off-screen frames. **Not tried with a real second monitor, a real click or plugging one in** | not built |
| HyperKey | tested with injected keys, used by the owner with a physical key | tested with injected F-keys only; real Caps Lock remap not tested |
| Game detection | tested with a test window in a game folder; no real game tried | not built: still the older fullscreen rule |
| Clipboard history | tested | tested |
| Snippets and expansion | tested | tested, with one unexplained flake in typed insertion |
| Notes | tested | tested |
| Settings window | tested | tested for what the checks open; Data page buttons compiled only |
| Highlight colour from the system accent | unit tested, seen on one grey accent; other accents and changing it live not tried | not built |
| Searchable list of apps for exclusions | tested | not built: still typed names |
| Footer hints as buttons | tested with clicks sent to the window | not built |
| Vim keys with Alt, and full Vim | tested with injected keys | not built |
| Search that forgives typing mistakes | tested | not built on the app side, though the search is shared |
| Favourites section, reorder, favourites-only opening | tested | not built |
| Ctrl+, for Settings (Cmd+, on Mac) | tested | not built |
| macOS feel keys | tested with injected keys | not applicable: a Mac already has them |
| Earlier, stronger glass look | seen on screen over a bright picture and over black, dark and light; judged by eye. Replaced by the row above | not built |
| Clipboard image thumbnails | hidden frames checked with made-up images | not built |
| 1Password and Ente Auth | automated checks use stand-ins for their tools; the owner confirmed listing and copying with their real ones. Typing an Ente code on Enter, and copying it on Ctrl+Enter, tested with the stand-in; **not tried with the real tool yet**, nor the clipboard when it can't be typed. Clearing a copied password after 30 s tested | not built |
| Installing the Ente tool from Settings | the app's install code ran for real into a scratch folder, and the PATH step on a scratch registry key. The button itself: not tested | not built |
| Installing the 1Password tool from Settings | **not tested**, so a real install doesn't change the owner's PC unasked | not built |
| Orbit Setup: install, update and uninstall, then the welcome in the installed Orbit | unit tested; every page seen in off-screen frames; a silent install, an update over 1.0.0, uninstall and reinstall tested in a sandbox by `scripts/test-install-windows.ps1`. **Clicking through the Setup window, and the handover from Setup to the installed Orbit, have not been tried yet** | not built |
| Import from Raycast, Flow Launcher, Wox, PowerToys Run, Command Palette and Keypirinha | parsers unit tested; real Raycast exports read; the Import page seen in off-screen frames. **Not run on this PC's launchers in a running Orbit yet** | not built |
| Extensions | manifests, messages and the catalog unit tested; the example extension runs in PowerShell through Orbit's own runner in a test. **Adding one from Settings and using it in the launcher not tried on screen yet** | not built |
| Built-in extensions with a switch each (Files, 1Password, Ente Auth, Weather, Speed Test, GitHub), and aliases, keywords and hotkeys for every command, settable while an extension is off | unit tested, including that one turned off takes its commands with it; the Extensions and Search pages seen in off-screen frames. **Not clicked through on screen yet** | not built |
| GitHub, with pull requests' and issues' state as tags, Recently closed and filters | parsers and filters unit tested; the lists, tags and filter drop-down seen in off-screen frames with made-up answers; both GraphQL queries run against a real account, which had only closed pull requests. **Installing `gh` and its sign-in from Settings, and Mark as Read, not tried yet** | not built |
| A click in a box puts the caret where it lands, dragging selects, a double click selects a word | hit testing unit tested against DirectWrite. **Not clicked through on screen yet** | not built |
| File search with Everything | the messages to and from Everything unit tested, against voidtools' layouts. **Not tried with a running Everything**, which isn't installed on this PC | not built |
| Signed builds and update checks | not built: unsigned. Updating means running a newer Setup | not built: unsigned local build |
| Hook and shortcut recovery after real sleep and unlock | not tested | not tested |
| UAC, elevated windows, RDP | not tested | not applicable, secure input not tested |

## Next, by impact

### For people using it every day

1. **Signed builds and update checks.** Orbit Setup installs and updates, but SmartScreen warns about an unsigned exe, and nothing tells you a newer version is out. The Orbit menu has an Updates row waiting for this.
2. **macOS parity:** Set hotkey in Cmd+K, game detection (`LSApplicationCategoryType` games plus fullscreen), press-again to hide, the new look with real Liquid Glass or `NSVisualEffectView`, favourites and favourites-only opening, Cmd+, for Settings, clipboard thumbnails, the clipboard preview beside its list, quicklink boxes for `{name}` and website icons, and 1Password and Ente Auth (their tools work the same on a Mac, with Touch ID for 1Password).
3. **More in the welcome.** It brings things over from other launchers, picks the shortcut, and turns on Everything, 1Password and Ente Auth; it could offer HyperKey and snippet expansion too.
4. **Hotkeys list:** real note titles instead of "Note 12".
5. **Media controls**, clipboard as a root result, a chosen browser per link, a live Ente code that counts down in the list before you copy it, and emoji skin tones and country flags.
6. **Note editing:** note kill loses up to 400 ms of typing, so shorten or remove that window.

### For staying light under pressure

7. **Targeted app index refresh** and a watcher for packaged apps, replacing "rebuild when asked or stale".
8. **Lazy clipboard database**, so it isn't opened until needed.
9. **`CreateMemoryResourceNotification`** to trim sooner under real memory pressure.
10. **A true memory-pressure run.** The one so far reached 92% load and isn't a RAM-exhausted or hard-fault test.
11. **Game frame-time harness** from the brief's reference matrix.

### Filling gaps

12. Direct shortcuts ignore per-app exclusions. Make them respect those, or say so at the point of adding one.
13. More game signals: sibling files or loaded modules, if the current ones prove short.
14. `{selection}`, plain-text snippet import and export, rich-text clipboard capture, bulk delete.
15. Encryption at rest, and clipboard export as an opt-in.
16. Density setting, and handling DPI changes in the secondary windows.
17. Windows on Arm: untested.
18. CI: `.github/workflows/ci.yml` is written but not in the repo yet, because pushing a workflow needs a GitHub sign-in with the `workflow` scope, and it has never run. `scripts/uninstall-windows.ps1`, for a copy built from source, also hasn't been run.
