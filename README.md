<img src="assets/icon.png" width="64" height="64" alt="">

# Orbit

A small, keyboard-first launcher for Windows and macOS. Press a shortcut, type, press Enter. It finds apps, commands, notes, snippets and clipboard history, and it's built to stay out of the way: one native process, no web runtime, no cloud.

## What it does

- **Launcher.** Fast ranked search over apps and commands, with a calculator that knows percentages ("90% of 429", "429 + 10%"), unit and currency conversion ("123 euro in pounds", "123 pound to kilo"), the weather ("weather London"), a speed test by Speedtest.net, and quick links. On Windows it forgives typing mistakes, keeps favourites at the top, and can open on favourites alone.
- **Action menu.** Ctrl+K (Cmd+K on macOS) shows what you can do with the selected item. On Windows it's a small menu above the footer, and the list stays where it was behind it. A running app's menu has End task.
- **Window layouts (Windows).** Left, right, top and bottom half, maximise, almost maximise, centre, and move to the next display, for the window you opened Orbit over. Give any of them a hotkey to use it straight from the keyboard.
- **System toggles (Windows).** Dark mode, mute, Bluetooth and Wi-Fi.
- **Timer and stopwatch (Windows).** Type "timer 5m", or open Timer and type how long. Both keep going with the launcher closed, and Windows says when a timer ends.
- **Pick Colour (Windows).** Pick any pixel on screen, with a magnifier, and copy it as hex, RGB or HSL.
- **Orbit menu (Windows).** The Orbit logo in the footer opens a short menu of Orbit's own: Settings, Check for Updates, Help, Suspend Shortcuts, Restart and Quit.
- **Emoji list (Windows).** Search "emoji" for a grid of 1,609 colour emoji with keyword search, pins and recently used. Enter pastes one where you were typing. There are no skin tone variants or country flags yet.
- **Quicklinks.** Open a site or a search by name or alias. On Windows, make one from the launcher with Create Quicklink, or type an address and choose Save as Quicklink.
- **File search (Windows).** Type "everything" and press Enter to search every file on your PC, through [Everything](https://www.voidtools.com) by voidtools. Give it an alias, and the alias and a space search straight away. Orbit can install Everything for you. Files can also show under other results.
- **Extensions (Windows).** Files, 1Password, Ente Auth, Weather, Speed Test and GitHub come with Orbit, each with a switch: one you don't use is gone from the launcher and never runs. Every command can have its own alias, keyword and hotkey, even while its extension is off, and giving it one turns the extension on. Other programs can add commands too, with a folder holding a small manifest and a program that answers in JSON. See [docs/EXTENSIONS.md](docs/EXTENSIONS.md).
- **GitHub (opt-in, Windows).** Your issues, pull requests, notifications and stars in the launcher: `ghi`, `ghp`, `ghn` and `ghs`. Each pull request says where it stands in coloured pills (conflicts, checks failing, changes requested, review requested, draft, approved), with the ones closed in the last two weeks under Recently closed. A drop-down beside the search narrows the list to open ones only, yours, ones waiting for your review, ones that need attention, drafts, or one repository. It goes through GitHub's own `gh` tool. Orbit can install it, then opens its sign-in for you.
- **Import (Windows).** Bring quicklinks, web searches, snippets and notes over from Raycast, Flow Launcher, Wox, Command Palette and Keypirinha, or from an Orbit export. Raycast's exports open with their passphrase.
- **Hotkeys.** On Windows, choose **Set hotkey** in the action menu and press the keys. Pressing an app's hotkey again minimizes it.
- **Vim keys (opt-in, Windows).** Navigate results with Alt+H J K L, or use full Vim, where the launcher opens ready to navigate and `i` starts typing.
- **HyperKey (opt-in).** Turn Caps Lock or another key into a modifier, so Hyper+C can open your editor. On Windows it steps aside in fullscreen games.
- **Launcher shortcut in games (Windows).** The launcher's shortcut goes to a fullscreen game instead of opening Orbit, and you can list other apps that should keep it. Change both on the General page of Settings.
- **macOS feel (opt-in, Windows).** Alt does what Cmd does on a Mac: Alt+Left and Alt+Right for the ends of the line, Alt+Backspace to delete to the start, Alt+Q to close an app and Alt+H to hide it.
- **1Password and Ente Auth (opt-in, Windows).** Search them from the launcher to copy a password, or to paste an Ente code straight into the box you were typing in (Ctrl+Enter copies it instead). Orbit asks each app's own command-line tool, and can install that tool for you. It keeps no secrets itself.
- **Clipboard history (opt-in).** Text, files and images. Stays on your computer and skips password managers and apps you list.
- **Snippets.** Search and insert them, and on Windows make one from the launcher with Create Snippet. Expanding a keyword as you type is opt-in.
- **Notes.** Plain-text notes with autosave, full-text search, and no limit on how many you keep.

Windows and macOS don't have the same features yet. [docs/ROADMAP.md](docs/ROADMAP.md) has the status of each, and which were tested.

## Supported systems

| System | State |
|---|---|
| Windows 11 x64 | Tested on 10.0.26200. Other Windows versions and Arm are not tested. |
| macOS on Apple Silicon | Tested on macOS 27.2. The package declares macOS 14 as its minimum, but nothing older has been run. Intel is not tested. |

Builds aren't signed, so SmartScreen or Gatekeeper may warn on first run.

## Install on Windows

Download **`Orbit-<version>-Setup.exe`** from the [latest release](https://github.com/Beroris/orbit/releases/latest) and run it. Orbit Setup installs Orbit for your account only, so it doesn't ask for admin rights, and can start it when you sign in. Then Orbit carries on in the same window: it brings over what you had in your old launcher, lets you pick your shortcut, and turns on extensions such as file search, 1Password and Ente Auth. Press Alt+Space to open the launcher. Orbit lives in the tray, and your data is in `%LOCALAPPDATA%\Orbit`.

Orbit keeps itself up to date: once a day it looks for a new release, downloads it in the background, and installs it when you click the notification, or choose it from the tray menu or Settings. Settings → General → "Check for updates" turns the daily look off. Running a newer Setup by hand works too, and keeps your settings and data. Remove Orbit from Settings → Apps; it asks before deleting your data, and keeps it unless you say otherwise. For scripts, `/VERYSILENT` (or `--silent`) installs or updates without a window, and `orbit.exe --uninstall --silent` removes Orbit and keeps your data.

Or take **`Orbit-<version>-Portable.exe`**: the same app without installing. It keeps its settings in an `Orbit settings` folder beside itself, so it can live on a USB stick.

See [THIRD_PARTY_NOTICES.md](docs/THIRD_PARTY_NOTICES.md) for the emoji data's and the Geist font's licences.

## Build

You need Rust 1.85 or newer. Windows builds were tested with 1.96.

```sh
# Windows
cargo build --release -p orbit-windows      # target/release/orbit.exe
pwsh scripts/package-windows.ps1            # dist/Orbit-<version>-Setup.exe and -Portable.exe

# macOS: also needs the Command Line Tools
scripts/macos/build_app.sh                  # platforms/macos/build/Orbit.app
```

See [platforms/macos/README.md](platforms/macos/README.md) for the macOS details.

## Keys

| Key | What it does |
|---|---|
| Alt+Space | Open or close the launcher on Windows. macOS tries Option+Space first. If it's taken, Orbit picks a fallback and tells you. |
| Enter | Run the selected item |
| Ctrl+Enter | Second action on the result, shown in the footer. On a Windows app, that's Run as administrator. |
| Ctrl+K / Cmd+K | Action menu |
| Ctrl+, | Orbit Settings, from the launcher (Windows). Settings save as you change them. |
| Ctrl+F or / | Search Settings, from the Settings window (Windows). Enter goes to the setting. |
| Esc | Clear what's typed, then back, then close |

On Windows, Ctrl+Backspace deletes a word in Orbit's search box and in every text box in Settings and its forms.

## Permissions

**Windows** needs no special permission. HyperKey and snippet expansion install a low-level keyboard hook, only if you turn them on. It keeps no record of what you type: see [docs/PRIVACY.md](docs/PRIVACY.md). Windows doesn't deliver keys from elevated windows to it, so HyperKey doesn't work over those.

**macOS** needs Accessibility and Input Monitoring only for HyperKey and snippet expansion. Typing a snippet also uses Accessibility; without it, snippets go to the clipboard. Plain shortcuts need neither.

Quit or Suspend Orbit Shortcuts, from the tray or menu bar, gives the keyboard back at once.

## Tests and benchmarks

```sh
cargo test --workspace                                   # unit tests
pwsh scripts/test-install-windows.ps1                    # Setup: silent install, update, uninstall (Windows)

# Takes focus and injects keys for several minutes on a private data folder (Windows):
cargo build --release -p orbit-windows -p orbit-bench
target/release/orbit-bench features --orbit target/release/orbit.exe
target/release/orbit-bench core data e2e --orbit target/release/orbit.exe \
    --baseline bench-results/baseline-windows.json       # fails if about 15% slower or larger

scripts/macos/feature_checks.sh                          # macOS
```

Close your own copy of Orbit first, because the feature suite tests Alt+Space and Caps Lock.

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Performance budget and results](docs/PERFORMANCE_BUDGET.md)
- [Shortcut semantics](docs/SHORTCUT_SEMANTICS.md)
- [Privacy](docs/PRIVACY.md)
- [Data portability](docs/DATA_PORTABILITY.md)
- [Extensions](docs/EXTENSIONS.md)
- [Decisions](docs/DECISIONS.md)
- [Roadmap](docs/ROADMAP.md)

## Support

Orbit is free. If it saves you time, you can [support me on Ko-fi](https://ko-fi.com/beroris).

## License

GPL-3.0. See [LICENSE](LICENSE). The Geist font in `assets/fonts` keeps its own SIL Open Font License.
