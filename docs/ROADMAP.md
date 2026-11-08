# Roadmap

What's built, and what's next by impact. "Tested" below means the automated suites on one machine per platform: see [PERFORMANCE_BUDGET.md](PERFORMANCE_BUDGET.md).

## Status by platform

| Feature | Windows | macOS |
|---|---|---|
| Launcher, search, app launching | tested | tested |
| Ctrl/Cmd+K action menu | written as a small menu over the list, seen in off-screen frames; its automated checks are written and **not run yet** | tested, as the older full list |
| Orbit menu from the footer logo | seen in off-screen frames; automated check written and **not run yet**. Its Updates row only says there's no update channel | not built |
| Emoji list | unit tested, seen in off-screen frames; pasting check written and **not run yet** | not built |
| Quicklink and command forms, and making them from the launcher | unit tested, forms seen in off-screen frames; **not used on screen yet** | not built |
| Launcher shortcut left to games and listed apps | check with a listed app written and **not run yet**; no real game tried | not built |
| Scrolling Settings pages, and the Snippets and Links pages | unit tested, seen in off-screen frames; real wheel, Tab and dragging **not tested** | not built |
| Settings saving as you change them, with no Save or Cancel | checked by driving the hidden window with messages; end-to-end check written and **not run yet** | not built |
| Settings overhaul: key recorder for the launcher shortcut, hotkeys added and changed on the Hotkeys page, lists sized to fit, dependent settings dimmed, drop-downs for numbers, confirm in place, window fits small screens | checked by driving the hidden window with messages and seen in off-screen frames; real keys, focus, a real second monitor and a screen smaller than the window **not tested** | not built |
| Ctrl+Backspace in Settings and forms | unit tested for where a word starts; not pressed in the real windows yet | not built |
| Less glass, rounder corners, soft shadow (layered window over a blur plate) | off-screen frames only. **The blur plate and the corners haven't been seen on a real screen** | not built |
| Set a hotkey from the action menu | tested | not built |
| Keycap badges and redesigned look | checked by screenshots, before the rounder design above | not built |
| An app's hotkey, pressed again, minimizes it | tested | not built |
| HyperKey | tested with injected keys, used by the owner with a physical key | tested with injected F-keys only; real Caps Lock remap not tested |
| Game detection | tested with a test window in a game folder; no real game tried | not built: still the older fullscreen rule |
| Clipboard history | tested | tested |
| Snippets and expansion | tested | tested, with one unexplained flake in typed insertion |
| Notes | tested | tested |
| Settings window | tested | tested for what the checks open; Data page buttons compiled only |
| Highlight colour from the system accent | unit tested, seen on one grey accent; other accents and changing it live not tried | not built |
| Searchable list of apps for exclusions | tested | not built: still typed names |
| Footer hints as buttons | tested with clicks sent to the window, before they moved into rounded pills; the updated check is **not run yet** | not built |
| Vim keys with Alt, and full Vim | tested with injected keys | not built |
| Search that forgives typing mistakes | tested | not built on the app side, though the search is shared |
| Favourites section, reorder, favourites-only opening | tested | not built |
| Ctrl+, for Settings (Cmd+, on Mac) | tested | not built |
| macOS feel keys | tested with injected keys | not applicable: a Mac already has them |
| Earlier, stronger glass look | seen on screen over a bright picture and over black, dark and light; judged by eye. Replaced by the row above | not built |
| Clipboard image thumbnails | hidden frames checked with made-up images | not built |
| 1Password and Ente Auth | automated checks use stand-ins for their tools; the owner confirmed listing and copying with their real ones. Clearing a copied password after 30 s failed its check and was fixed afterwards: **the fix has not been run yet** | not built |
| Installing the Ente tool from Settings | the app's install code ran for real into a scratch folder, and the PATH step on a scratch registry key. The button itself: not tested | not built |
| Installing the 1Password tool from Settings | **not tested**, so a real install doesn't change the owner's PC unasked | not built |
| Signed installer and updates | not built: unsigned zip | not built: unsigned local build |
| Hook and shortcut recovery after real sleep and unlock | not tested | not tested |
| UAC, elevated windows, RDP | not tested | not applicable, secure input not tested |

## Next, by impact

### For people using it every day

1. **Signed installer and updates.** The largest step from a working copy to something people can install safely. The Orbit menu has an Updates row waiting for this.
2. **macOS parity:** Set hotkey in Cmd+K, game detection (`LSApplicationCategoryType` games plus fullscreen), press-again to hide, the new look with real Liquid Glass or `NSVisualEffectView`, favourites and favourites-only opening, Cmd+, for Settings, clipboard thumbnails, the clipboard preview beside its list, quicklink boxes for `{name}` and website icons, and 1Password and Ente Auth (their tools work the same on a Mac, with Touch ID for 1Password).
3. **Onboarding.** A short first run that explains the launcher shortcut and offers HyperKey.
4. **Hotkeys list:** real note titles instead of "Note 12".
5. **Media controls**, clipboard as a root result, a chosen browser per link, quicklink icons for sites that keep theirs elsewhere or only as SVG, a live Ente code that counts down in the list before you copy it, and emoji skin tones and country flags.
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
18. CI: `.github/workflows/ci.yml` exists and has never run. `scripts/uninstall-windows.ps1` also hasn't been run.
