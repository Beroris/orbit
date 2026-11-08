# Decisions

Why Orbit is built the way it is. Figures come from the Windows reference machine unless stated, and [PERFORMANCE_BUDGET.md](PERFORMANCE_BUDGET.md) has the method and full results.

## Native hosts over a shared Rust core

**Decision.** Search, settings, hotkey logic and storage are shared Rust crates. Windows is a pure-Rust Win32 and GDI host. macOS is an AppKit host in Swift that calls Rust through `orbit-ffi`, a C ABI.

**Why.** The brief rules out Electron, resident Chromium and Node. Using each platform's own window system also keeps the things users notice native: focus, text input, tabbing and screen readers.

**Cost.** Each UI is written twice. Features can land on one platform first, and some have: see [ROADMAP.md](ROADMAP.md).

## GDI drawing on Windows, not Direct2D or a UI toolkit

**Decision.** The launcher draws into a CPU backbuffer and copies it to the window.

**Why.** It adds no GPU process memory or heavy dependency, and it draws fast enough. Key to visible is p95 1.75 ms in the last full benchmark.

**Cost.** Rounded corners, icons and keycaps are drawn by our own helpers in `gfx.rs`.

## CPU-drawn surface on macOS

**Decision.** The macOS launcher draws with the CPU into an IOSurface.

**Why.** The first version's GPU-backed path showed up in memory use. This change is the "GPU memory fix" in the macOS results.

**Cost.** Resident size is still 118 MB after use, which is over the budget's figure, though most of it is shared system frameworks. Footprint, which excludes those, is about 30 MB.

## Real Win32 controls in Settings, painted in Orbit's colours

**Decision.** Settings and snippet windows use real `BUTTON`, `EDIT`, `LISTBOX` and `COMBOBOX` controls. Their faces are painted through `NM_CUSTOMDRAW`, owner draw, a subclass for the closed drop-down, and rounded fields the parent paints behind borderless edits.

**Why.** The first version looked like a classic light dialog, which the owner rightly flagged. Drawing custom controls from scratch would have lost tabbing, accessibility and text editing. Painting real ones keeps them.

**Cost.** Each control type needs its own painting path, and high-contrast colours go through the theme rather than the controls' defaults.

## Game detection needs a fullscreen window plus a game signal

**Decision.** HyperKey steps aside only when the window in front fills its monitor and either its program is recognised as a game (Windows' game list, a store library folder, an Unreal shipping name) or Windows reports exclusive Direct3D fullscreen.

**Why.** The first rule, any fullscreen window, broke Hyper+C for the owner in a non-game window that filled the monitor. No single signal is enough on its own: Windows' list on this machine includes `javaw.exe`, and borderless games don't report exclusive fullscreen. Hence fullscreen is required for every signal.

**Cost.** A borderless game outside known folders and missing from Windows' list isn't recognised. The exclusion list is the way out, and Settings says so. Reading loaded modules (DirectX, Vulkan) was rejected as heavier and more intrusive.

**Measured.** `SHQueryUserNotificationState`: p50 57 µs, p99 88 µs, max 300 µs over 2,000 calls via .NET P/Invoke. Called only when a window already fills the monitor, as the trigger goes down, so ordinary typing never pays for it.

## One key, one item

**Decision.** An item has at most one hotkey and a hotkey runs one item, across HyperKey mappings and direct shortcuts. Assigning replaces, and a key already in use asks first.

**Why.** With several possible hotkeys per item, the row badge and the Set, Change and Remove menu items would be ambiguous.

**Cost.** Setting a hotkey from the menu can't give one app two keys. The "Edit as text" view in Settings can still produce that, and the next assign from the menu replaces them.

## Press-again minimizes only apps

**Decision.** An app's hotkey minimizes it when it's already in front. Commands aren't toggled, and launcher views keep their existing close-on-second-press.

**Why.** It's what the owner asked for, and "minimize" has no meaning for a command. On by default, with a switch in General.

## Hook callback does no real work

**Decision.** The low-level keyboard hook runs on its own highest-priority thread, updates small in-memory state and posts messages. No database, no process start. The game list is read on a short-lived thread, never in the callback.

**Why.** Windows drops slow hooks, and a slow hook delays every key press on the machine. The brief requires it.

## SQLite for notes, snippets and clipboard

**Decision.** One SQLite database with FTS for search, used from a data thread. Windows trims the working set 15 s after hide.

**Why.** It's dependable and can be read with common tools, which suits the brief's data-portability goals. Notes search stayed at p95 22.8 ms over 10,000 notes after adding an index on `id DESC` for recent-note listing.

**Cost.** A 5 MB note leaves about 16.9 MB of private memory until restart.

## Orbit's highlight colour follows Windows

**Decision.** On Windows, the highlight colour comes from the system accent (`AccentPalette` in the registry), using the shade Windows uses for each theme: one step lighter on dark, one step darker on light. Text on it is black or white, whichever reads better. If the accent would be too faint against the background, it moves towards the text colour until it reaches 3:1 contrast. If it can't be read, Orbit uses Windows' default blue. The logo stays orange.

**Why.** A fixed orange made the app look unlike the rest of the desktop, and the owner disliked it. A pale accent is a real case: the first machine this ran on has a light grey accent, which is close to invisible on a light background without the contrast rule.

**Cost.** One registry read each time the theme is resolved, which only happens at start and when a system setting changes.

## App lists are a search box, not typed program names

**Decision.** The three lists of excluded apps in Settings are a search box over a list of apps, with Browse for anything not listed. A typed name that matches nothing can still be added as a program name. `settings.json` still stores program names such as `game.exe`, because that's what's compared with the app in front.

**Why.** Nobody should need to know that a game's program is called `FortniteClient-Win64-Shipping.exe` to turn a key off in it.

**Cost.** The list comes from the app index plus programs with a window open when Settings opens. Apps in the index with no program path, which are most Microsoft Store apps, aren't offered unless they're running as their own program. A UWP app such as Calculator runs inside `ApplicationFrameHost.exe`, so it can't be excluded by name at all, which was already true of the typed lists.

## Vim keys in the launcher

**Decision.** Three settings, in `launcher.keyboard`: Standard, Vim keys with Alt, and Full Vim. Standard stays the default. In Full Vim the launcher opens in normal mode with a block caret and an indicator in the footer. `i`, `a`, `I`, `A` and `/` start typing, and Esc stops. `j` and `k` move through results.

`h` goes back and `l` runs the selected result, as in file managers with Vim keys, rather than moving the caret. The query is one short line and the results are what you're navigating, so that's where the keys earn their place. `0`, `$`, `w`, `b` and the arrow keys move the caret.

**Why.** The owner asked for both. Commands are read from typed characters rather than key codes, so `:`, `?` and `$` work on any keyboard layout.

**Cost.** `l` runs something where a Vim user might expect the caret to move. It applies only to the launcher: notes are still a plain text box.

## Benchmark gate allowances

**Decision.** The regression gate fails at about 15% over a checked-in baseline, with absolute allowances for very small values.

**Why.** Without allowances, a metric of 0 or a few microseconds fails from noise alone, as the first version did. The gate was checked both ways: it passes on a rerun and fails when a baseline is tightened.

**Cost.** A regression below the allowance goes unnoticed, and search over 100,000 items can fail once on a busy machine, which is why one rerun is the documented response.

## Injected keys are ignored by default

**Decision.** The running app ignores input injected by other programs unless `--accept-injected-input` is passed. Only the benchmark and feature suite pass it.

**Why.** Another program shouldn't be able to drive HyperKey by synthesising keys. A side effect, reported honestly below, is that no automated test exercises a physical key press; the owner's own use on 2026-11-06 is the first physical check.

## Password managers go through their own command-line tools

**Decision.** 1Password and Ente Auth are reached through `op` and `ente`. Orbit runs them with arguments passed directly and no shell, reads their answers, and shows them. Item names stay in memory for up to ten minutes and are dropped on lock and under memory pressure. A value is fetched only when chosen, wiped from memory when it's been copied, and never logged.

**Why.** Each of them already has the hard parts: the vault, the login, and for 1Password the approval prompt. Ente's newer Rust CLI (`ente auth code list` and `view`) reads codes from its own encrypted store, so Orbit needs neither a plain-text export, as Raycast's Ente extension does, nor its own storage. Any Orbit copy of a secret would be a new thing to get wrong.

**Cost.** Each tool has to be installed. The Install button does that, and only when pressed. It's the one thing in Orbit that reaches the network: it starts Windows Package Manager for 1Password, or a PowerShell script that downloads Ente's release from GitHub and checks it against the release's `SHA256SUMS`. Ente's winget package is the old Go tool, which lacks `auth code`, so winget isn't used for it. Orbit itself still has no network code.

## Copied secrets are marked, and clear themselves

**Decision.** A copied password or code carries `ExcludeClipboardContentFromMonitorProcessing`, `CanIncludeInClipboardHistory` = 0 and `CanUploadToCloudClipboard` = 0, and clears after 30 seconds unless something else has been copied since. It also clears on lock and on quit. A username carries the marks but doesn't clear.

**Why.** Those are the formats Windows documents for this. Clearing only if the clipboard sequence number hasn't moved means Orbit never throws away something the user copied later.

**Cost.** A clipboard tool that ignores the marks can still see the value. Nothing can prevent that from the outside.

## The launcher stays open while a tool is asked

**Decision.** While `op` or `ente` is being asked, losing focus doesn't close the launcher. It also stops being topmost, so an approval prompt can appear above it, and takes focus back when the answer comes, unless that took over 20 seconds. Esc still cancels, and a value that arrives after that is thrown away.

**Why.** The launcher normally closes the moment it loses focus, and 1Password's approval prompt takes focus. Closing there would look like the feature had simply stopped.

## Glass with a dark tint

**Decision.** The launcher draws premultiplied alpha over the acrylic blur from `SetWindowCompositionAttribute`, with a real tint passed to it, because Windows draws a nearly clear tint as solid black. The documented system backdrop looked flat grey with no colour from behind it, so it isn't the default. It's on by default with a Settings switch, and falls back to a solid colour when Windows has transparency effects off, in high contrast, or in a remote session.

**Why.** The owner asked for the feel of Raycast's Windows and macOS looks, which depend on seeing the desktop through the window. Only the look was taken: Orbit's name, logo and layout details are its own.

**Cost.** See [PERFORMANCE_BUDGET.md](PERFORMANCE_BUDGET.md).

## The launcher's text comes from DirectWrite

**Decision.** The launcher's text is laid out by DirectWrite, which returns how much of each pixel a run of letters covers, and that's blended into the same CPU-drawn buffer as everything else. Direct2D and a GPU surface aren't involved. Segoe UI Variable is asked for its exact weight and optical size where Windows allows that. The Settings, note and snippet windows still use GDI.

**Why.** The owner judged the look lacking once they saw it on their own screen. GDI puts letters on whole pixels and hints them to fit, so it couldn't match the type in the Raycast reference images, however the layout changed. DirectWrite also brings font fallback and right-to-left layout that GDI's single call didn't.

**Cost.** One more system DLL in the process and a font lookup on first use. See [PERFORMANCE_BUDGET.md](PERFORMANCE_BUDGET.md).

## The typeface is Geist, built in

**Decision.** Orbit's text is Geist Sans, the Bero Suite's typeface, shared with Spotlight and Prism. It is the same file they ship, with its `head.unitsPerEm` lowered from 1000 to 971 so it draws 3% larger. It is built into `orbit.exe` and never installed on the system. GDI (Settings, forms, notes) gets it from `AddFontMemResourceEx`, private to the process, and checks with `GetTextFaceW` that "Geist" really came back. DirectWrite (the launcher) reads the same bytes as an in-memory font collection and asks it for exact weights. If either can't load it, that side falls back to Segoe UI Variable, then Segoe UI. Letters Geist lacks still come from Windows' fallback fonts. Its licence (SIL Open Font License 1.1, with no reserved font name) ships as `Geist-LICENSE.txt` and is in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

**Why.** The owner chose one typeface for every app in the suite on 2026-11-07.

**Cost.** About 165 KB more in the exe. Geist is a little wider and taller than Segoe, so the Settings page list and window are 12 px wider, and a page's title sits 2 px closer to the line under it.

## The launcher shrinks to fit its results

**Decision.** The window is as tall as its rows, up to the row limit in Settings. Its top stays where it is and only the bottom moves. A Settings switch keeps it at full height for anyone who prefers that.

**Why.** Two results in a full-height window left a large dark gap, which was the first thing the owner's screenshot showed.

## Esc clears before it closes

**Decision.** With text in the search box, Esc empties it and the launcher stays open. With an empty box it goes back or closes, as before. In full Vim mode, Esc while typing still goes to normal mode first. While a password or code is on its way, Esc cancels that straight away, whatever is typed.

**Why.** The owner asked for it. Retyping is more annoying than one extra key press to close.

## macOS feel uses Alt, and replaces Alt+Left

**Decision.** With the option on, Alt+Left and Alt+Right go to the ends of the line, Alt+Backspace deletes to the start, Alt+Q sends close to all of the front app's windows and Alt+H minimizes them. It's off by default, runs in the existing hook, stays out of games and remote sessions, and has a per-app list.

**Why.** The owner moves between a Mac and this PC. Alt sits where Cmd does.

**Cost.** Alt+Left is Back in every browser and File Explorer. People who want that keep it by listing those apps. Quit is `WM_CLOSE`, not a kill: an app can still ask about unsaved work, and a tray app keeps running. Windows has no equivalent of a Mac's hide, so hide means minimize.

## Clipboard images are on once history is on

**Decision.** `clipboard.capture_images` now defaults to true. Clipboard history itself is still off until turned on, as the brief requires.

**Why.** The owner expected images to be included. People who turn on clipboard history mostly expect it to hold what they copied, and images are subject to the same exclusions and a size cap. Settings saved before this keep the value they had.

## Ctrl+K is a small menu over the list

**Decision.** Ctrl+K and the Orbit logo open a small menu above the footer, with its own search. The results stay in place behind it, and closing the menu restores them without another search. A click outside only closes it, and doesn't run what's under the pointer. The clipboard preview still uses the full list, because it needs the room.

**Why.** The owner asked for the menu to work as it does in Raycast, instead of replacing the whole list.

## A layered launcher over a separate blur plate

**Decision.** The launcher draws its own antialiased corners and shadow into a layered window. A click-through plate window below it holds the blur. Glass is less see-through than before: the panel covers 214 of 255 in dark and 224 in light.

**Why.** The owner asked for less prominent glass and rounder corners. Windows' own rounded corners stop at a small fixed radius, and a layered window can't take the acrylic blur itself, so the two jobs need two windows.

**Cost.** One more window to keep in step, more pixels to compose at each frame, and a larger window to clear. Show time and memory have to be measured again. The plate's blur and clipping haven't been seen on a real screen yet, and if they don't hold up the fallback is an opaque panel.

## The emoji list comes from Unicode's data

**Decision.** The list is `crates/orbit-core/data/emoji.tsv`, 1,609 emoji generated from Unicode's `emoji-test.txt` (Emoji 15.1) and the English keywords in CLDR's `en.xml`. Skin tone variants are left out to keep the grid short. Country flags are left out because Windows draws them as two plain letters. Both are Unicode-3.0 licensed: see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). Windows' colour font draws the glyphs, so newer emoji can show as empty boxes on older Windows versions.

**Why.** It's a fixed table with no network access or extra font, read only while the list is open.

## The launcher's hotkey can stand aside

**Decision.** The launcher's hotkey has a per-app list and a game setting, separate from HyperKey's. The game setting is on by default. Orbit releases the hotkey while such a program is in front, so the program really receives the key. See [SHORTCUT_SEMANTICS.md](SHORTCUT_SEMANTICS.md).

**Why.** The owner asked for the same control they have on HyperKey, for the main trigger. Alt+Space does something in many games, and the brief says Orbit must not interfere with them.

**Cost.** A game that goes fullscreen after coming to the front loses one press: nothing announces the change, so Orbit only finds out when the key is pressed. A second start of Orbit, or the tray icon, still opens the launcher while the hotkey stands aside.

## Settings save as you change them

**Decision.** The Settings window has no Save or Cancel. Switches, drop-downs and lists save at once. Numbers are drop-downs of sensible values, and a value set by hand in `settings.json` is offered as well, so opening Settings never changes it. The launcher's shortcut and each hotkey are recorded by pressing keys, so neither can hold something invalid. Only the hotkey tables, edited as text, can. They keep their last good value, show the problem beside them, and hold nothing else back. Closing with one showing warns once, and closing again leaves that text unsaved. "Saved" appears only after Orbit reports that the file was written, and a failed write stays on screen in red. Esc saves and closes. Enter presses a focused button or switch, and otherwise only saves, so it doesn't close anything. Holding Enter or Esc counts as one press. The forms for one snippet, quicklink or command keep their Save button, because each of those is a new item being finished.

**Why.** The owner asked for it, and it's how the launcher's own settings changes already work.

**Cost.** There's no way to back out of a change except changing it back. Changing the appearance rebuilds the window, because its controls take their colours when they're made. It stays in the same place, on the same page.

## Quicklink icons are fetched, and on by default

**Decision.** Orbit asks each quicklink's own website for its icon, once, and keeps the answer. The switch for it, "Show website icons", is on by default. Only the link's own site is asked, by host, with no cookies or Windows sign-in, and an icon named on another site is refused. A redirect the site itself gives for its icon is followed once, to a public HTTPS address, which is how Gmail gets its icon from Google's file server. Local and private addresses are never asked. See [PRIVACY.md](PRIVACY.md#quicklink-website-icons).

**Why.** The owner asked for icons like Raycast's, and chose on by default with a way to turn it off. The brief asks for no background network activity unless the user has turned it on, so this is a deliberate exception made by the owner. It's kept as small as it can be: no icon service, which would learn every link, and no refreshing.

**Cost.** Orbit is no longer free of network code. Some sites name their icon on another domain and get the plain link symbol. An icon that's only given as SVG is drawn with Direct2D's SVG support, which handles most site icons but not every SVG feature.

## Quicklinks can ask for more than one thing

**Decision.** Any `{name}` in a quicklink's address is a box to fill in, and `{name=text}` gives it a default. `{query}` is one of them, shown as "Search". Choosing such a link shows its boxes in the search bar. Tab and Shift+Tab move between them, and Enter opens the link, or goes to a box that still needs filling. Typing the alias first fills them in order, a word each, with the last box taking the rest. Values are percent-encoded, with a space as `%20` before the `?` and `+` after it. Values aren't remembered. A search link no longer needs an alias, because choosing it asks for the search.

**Why.** The owner asked for quicklinks with their own parameters. Writing them in the address keeps the settings format unchanged and works in the text editor for links too.

**Cost.** Braces holding a name are always a box, so an address that really contains `{word}` can't be saved as it is. Braces around anything else, such as `{}` or JSON, are left alone.

## The clipboard shows a preview beside its list

**Decision.** Clipboard history shows its list on the left and, on the right, the selected item with its details: where it was copied from, what it is, its size or counts, and when. The launcher keeps its width, and the clipboard always gets its full height. Only the selected item's content is loaded, on the data thread, and it's let go when the launcher hides. Pictures are scaled to fit once, never enlarged.

**Why.** The owner asked for Raycast's layout, which shows far more of a copy than a single line could.

**Cost.** Titles in the list are shorter. Windows doesn't say where a copied picture came from on disk, so only copied files have a location.

## Menus fit the launcher

**Decision.** A menu, such as Ctrl+K's, takes about 70% of the room between the search box and the footer, and scrolls for the rest. A short list keeps the launcher tall enough for a three-row menu, so opening or closing a menu never changes the launcher's height.

**Why.** The owner saw the launcher grow and shrink as menus opened, and asked for menus that don't reach the top.

**Cost.** A search with one or two results leaves some empty space below them.

## Quicklinks and snippets are made in the launcher

**Decision.** Creating or editing a quicklink or a snippet happens in the launcher itself, as a form under the search bar: a label and box per field, Tab and Shift+Tab to move, Enter to save (Ctrl+Enter in the snippet's own box, where Enter starts a new line), and Esc to cancel. Problems show in red under the field they're about. Settings' New and Edit buttons open the same form, and the launcher hides again when it's done. A form left open when the launcher hides comes back if the launcher is shown again within five minutes.

**Why.** The owner asked for these to be made inside the main popup rather than in extra windows.

**Cost.** The snippet's box is drawn by Orbit over a hidden edit control, so a click focuses it but doesn't place the caret, and Up and Down move by written lines, not wrapped ones. Commands, which need a file picker, and notes, which are long documents, still open their own windows. A snippet's description, category and favourite aren't in the form; they're kept as they were.

## Quicklinks open files and folders, with a chosen app, and fill themselves in

**Decision.** A quicklink's Link can be a website or a file or folder path, and its "Open with" can be any app in the index, or the usual one. A new quicklink fills in its Name, Link and Open with from the window you were in: the page in a browser, the folder in File Explorer, or the file an app has open when its title names one of Windows' recent files. It only fills a form nothing has been typed in yet.

**Why.** The owner asked for quicklinks to apps, and for Create Quicklink to suggest the open site, folder or file.

**Cost.** Reading a browser's address bar takes 70 to 220 ms on the owner's PC, off the UI thread. Apps whose title doesn't name the file, or whose file isn't in Windows' recent list, only get Open with filled in.

## The app list follows installs as they happen

**Decision.** The app index is rebuilt whenever the Start Menu folders change or Windows' package catalog reports a packaged app installed, updated or removed. Both are events the watcher thread sleeps on; there is no polling. Each start also runs the indexer once, at idle priority, for anything installed or removed while Orbit was closed; the saved list is used until it finishes. It runs on battery too, at idle priority. Only when memory is nearly full does it wait, until the launcher is next shown. Apps that are themselves uninstallers, such as Revo Uninstaller, are kept; only "Uninstall Foo" entries and programs named like `unins000.exe` are dropped.

**Why.** The owner found apps missing until Orbit restarted. Store apps never touch the Start Menu folders, and a change on battery used to be dropped until the next start. A start used to reindex only when the list was over a day old, so an app installed while Orbit was closed could stay missing for a day.

**Cost.** One short indexer run per install. The package catalog is a WinRT API, which adds a little to the build.

## Action menu keys work from the list

**Decision.** Every key shown in an action menu also works with the menu closed: Ctrl+Enter opens a new window, Ctrl+Shift+Enter runs as administrator (as in Start and Run), Ctrl+Shift+E opens the file location, Ctrl+Shift+F adds to or removes from favourites, Ctrl+Shift+Up and Down move a favourite, Ctrl+Shift+A sets an alias, Ctrl+Shift+H hides, Ctrl+E edits, Ctrl+Shift+P pins and Ctrl+Shift+Del deletes. The keys come from the menu's own entries, so the menu and the keys can't disagree.

**Why.** The owner asked for keys on these entries, with Ctrl+Enter for a new window and another key for administrator. Alt+Enter was suggested; Ctrl+Shift+Enter was chosen because Windows already uses it for administrator.

## The see-through blur takes the launcher's exact shape

**Decision.** The blur behind the see-through launcher is a composition visual (a host backdrop brush clipped to a rounded rectangle with the launcher's 22 px corners), in its own window that Windows doesn't round. If the compositor can't make one, Orbit falls back to Windows' own backdrop, set 6 px inside the launcher.

**Why.** Windows rounds a window's corners by 8 px at most. The blur's window either showed square-ish corners behind the launcher's round ones, or, pulled in to hide them, left an unblurred strip around the edge that the owner saw at the top corners.

**Cost.** A dispatcher queue on the UI thread and the WinRT composition bindings in the build. The brush is only blur; the launcher draws its own tint over it as before.

## Formatted copies keep their formatting

**Decision.** When copied text comes with HTML or RTF, clipboard history keeps those beside the text, and pasting it from history puts all three back. Formatted items also offer "Paste as plain text" (Ctrl+Shift+Enter). Links, pictures and files offer Open (Ctrl+O): a link in the browser, a picture in the default image viewer from a `.png` copy, and a single file or folder with its own program. Programs and scripts copied as files are never opened; their folder is shown instead.

**Why.** The owner asked to paste formatted items as plain text and to open pictures and links from history.

**Cost.** `clipboard.db` grows by the formatted copies, capped at 4 MB each.

## Settings shows its window before its pages

**Decision.** The Settings window appears, painted in the sidebar colour, as soon as its frame exists, and the pages are built into it after that. The lists that need the app index (the hotkey and quicklink app pickers) are filled on a background thread and refresh when it finishes. Drop-downs are filled with redraw off.

**Why.** The owner found Settings slow to open: about 200 ms the first time and 100 ms after. Building every page took about 230 ms on this PC before the change and about 106 ms after, measured with temporary timing marks that have since been removed. The window now appears before that work, not after it.

**Cost.** For a moment the window can show without its pages, and an app picker can be empty until the app list arrives.

## Settings can take any item out of search, and give it an alias or a hotkey

**Decision.** A Search page in Settings lists everything the launcher can find (Orbit's own commands, apps, quicklinks, your commands and integrations), grouped, with a filter box. Each row has an alias, a hotkey and a switch, in columns, as Raycast's extension list has. A switch that's off hides that item from search; the item itself is kept, and it comes back when the switch is turned on. It uses the same `hidden` list as "Hide from search" in the action menu, so either place can undo the other.

A click on the alias opens a box over it: Enter keeps it, Esc puts it back, and an empty one removes it. An alias is one word, and two items can't share one. A quicklink or command keeps its alias in itself, as its own form does; anything else keeps it in the list of aliases. A click on the hotkey listens for keys in the row, with the same rules as the Hotkeys page (and its question before taking keys something else has); Backspace removes it. From the keyboard, Left and Right move between alias, hotkey and switch, and Enter or Space changes the one that's lit; Delete clears an alias or a hotkey. Commands that ask before they act, such as Shut Down, can't have a hotkey, as on the Hotkeys page. The page is built the first time it's opened.

**Why.** The owner asked for the same per-item on and off that Raycast offers, and then for aliases and hotkeys in that list, editable there.

## Settings looks like Prism's, with Spotlight's sidebar

**Decision.** The Settings window has no Windows title bar. Its own strip across the top drags it and has Minimize and Close; Windows still gives it a shadow, rounded corners, snapping and a taskbar button, as Prism's settings do (a normal window that tells Windows it has no frame). The page list sits on a rail a shade off the page, under a "Settings" title, with each page's symbol on a rounded square of its own colour, as macOS's System Settings has, and the version at its foot. Each page opens with a larger square, its name, and a line on what it's for. Rows have their name, a line under it saying what it does, and their control at the right, centred on both, as in Prism. A choice of two to five short options is shown side by side, with the chosen one raised, when it fits; otherwise, and in high contrast, it stays a drop-down. Colours avoid orange, and in high contrast the squares use Windows' own colours.

**Why.** The owner asked for the settings to follow Prism's design and macOS Spotlight's settings.

**Cost.** A frameless window draws its own Close and Minimize. Developer previews keep Windows' frame and are cut to the inside, because an off-screen capture of a frameless window leaves some of its controls out.

## HyperKey has a mark of its own

**Decision.** Wherever keys are shown as caps, in the launcher and in Settings, the Hyper key is a square cap with a filled bolt instead of the word "Hyper", the same bolt that marks the HyperKey page. It is drawn as a shape rather than taken from a font, because the icon font only has the bolt in outline, which is too thin at cap size. Prose, such as "Hyper+V is bound more than once", still uses the word. The prompts for a new hotkey ask for "the keys you want" and give a shortcut such as Ctrl+Alt+K first, so they no longer read as if HyperKey were the only way.

**Why.** The owner found the word on the caps unattractive, and the old prompt ("Hold Caps Lock and press a key") implied Caps Lock was the only way to make a hotkey.

## Currencies convert at the European Central Bank's daily rates

**Decision.** The launcher converts money, as in "123 euro in pounds", "£50 to usd" or "100 usd = yen", using the European Central Bank's daily reference rates: about 30 currencies, one public file of about 2 KB, no account or key. The rates are fetched only when a conversion is typed and the kept copy is missing or more than 12 hours old, on a low-priority thread, and kept in `exchange-rates.xml`. Each answer shows the day of its rates, and Enter copies the number. There's no fetch at start and no timer. Reading a query is a few string checks that give up at the first thing that doesn't fit, so typing anything else costs nothing measurable, and the kept file is read once, at the first conversion. "Convert currencies" on the General page turns it off and deletes the file. Words that also mean something else, such as "real" or "peso", aren't taken as currencies.

**Why.** The owner asked for it, without a cost to performance.

**Cost.** A second kind of network request, documented in [PRIVACY.md](PRIVACY.md#exchange-rates). The bank's rates are set once each working day, so they're reference rates, not live market prices, and currencies it doesn't publish, such as the Russian rouble, aren't offered. Windows only for now.

## The list runs on under the footer and fades out

**Decision.** When there are more results than fit, the launcher's list doesn't stop at the last whole row with an empty band above the footer. The rows carry on under the footer and fade to nothing by the launcher's bottom edge, so the row past the end shows faintly between the footer's pills. The fade starts where the last row that can be chosen ends, which section titles move, since they're shorter than rows. Only the rows above the fade can be chosen, scrolled to or clicked, exactly as before; the faded ones are a hint that the list goes on. The fade blends the drawn rows back toward a copy of what was under them, on a strip about a footer tall, so it costs a few tens of thousands of pixel mixes a frame. With "Shrink to fit results" the launcher is only as tall as its rows, so there's nothing to fade.

**Why.** The owner asked for the results to continue to the bottom and fade out, rather than leaving the space empty.

## Weather is a view of its own, from Open-Meteo

**Decision.** Typing "weather", or "weather" and a place, offers the weather, and Enter opens it in place of the list: the temperature now with what it's like, today's high and low, feels-like, wind and humidity; the next 24 hours every three hours over a faint curve of the temperature; and seven days, each with its low-to-high range on one bar scaled to the whole week, coloured from blue when cold to amber when warm, with the chance of rain when it's 20% or more. Typing another place and pressing Enter looks that up instead. The forecast comes from Open-Meteo, which needs no account or key, is free for non-commercial use under CC BY 4.0, and is credited in the footer. A place's name is sent to find it, and then only its coordinates, rounded to about a kilometre, for the forecast. The last place found is kept so "weather" alone shows it. Fahrenheit and miles an hour are used where Windows is set to US measurements. The pictures are the system's colour emoji, drawn once at each size and dropped when the launcher hides. "Weather" on the General page turns it off and forgets the place.

**Why.** The owner asked to type "weather" and see it shown in Orbit.

**Cost.** A third kind of network request, documented in [PRIVACY.md](PRIVACY.md#weather), made only when the weather is opened and the forecast kept is more than 15 minutes old. Orbit doesn't find where you are by itself, so the first time you name a place. Windows only for now.

## Units can be written as words

**Decision.** Conversions accept common words and plurals as well as symbols: "123 pound to kilo", "2 kilometres in miles", "100 fahrenheit in celsius", "3 litres to pints?". The words are a fixed table that maps each to the unit's symbol, and the answer is shown with the symbol. A question mark or full stop at the end is ignored.

**Why.** The owner expected "123 pound to kilo" to work, since unit conversion needs nothing from outside.

## Speed Test runs Ookla's Speedtest CLI

**Decision.** "Speed Test" in the launcher tests the connection against Speedtest.net, as the owner asked, and draws it where the list would be: download and upload as large numbers with a trace of each as it was measured, then ping, jitter and packet loss, and the server it used. Speedtest.net has no public interface for other programs, and Ookla's free Speedtest CLI is its own way to test outside a browser, so Orbit runs it with no window and reads the progress it prints, about four lines a second, while the test runs. The first time, the launcher shows Ookla's terms and asks for Enter before anything is installed or run. If the program is missing, Orbit installs it with Windows Package Manager. Opening the command starts a test, Enter runs another, and leaving the view stops one. Ctrl+K copies the results, opens the result on Speedtest.net, or opens Ookla's terms.

**Why.** The owner asked for a speed test using Speedtest.net, shown in full as the weather is.

**Cost.** Another program, not part of Orbit, installed from Ookla on first use, and Ookla's terms limit it to personal, non-commercial use. Ookla sees the IP address and keeps each result. The program prints addresses that Orbit never reads. Windows only for now.

## Calculator answers have a menu

**Decision.** A worked-out answer, whether a sum, a unit conversion or a currency, has a Ctrl+K menu like any other row. Enter copies the bare number ("3.6576", "102.60"), or with "Copy answers with formatting" on the General page, the formatted one, with grouped thousands and its unit or currency sign ("3.6576 m", "£102.60"). Ctrl+Shift+Enter copies whichever Enter doesn't. Ctrl+Enter types the answer, as Enter would copy it, into the window the launcher was opened over. Ctrl+S, for conversions, asks the same conversion the other way round. When the answer is the only result, it's drawn as a card instead of a row: the question on the left and the answer on the right, large, each with the name of its unit or currency under it ("Feet", "British pound"), an arrow between, and for money the day of the rate below. The card rounds to four decimal places; copying keeps every digit.

**Why.** The owner asked for a menu on conversions, then for paste on Ctrl+Enter, a setting for which form Enter copies, and Ctrl+S for converting back.

## Command tiles are lit like app icons

**Decision.** A command's tile is drawn the way an app icon is, so it sits well beside real ones. It uses two colours per command, light at the top left and deeper, a little shifted in hue, at the bottom right, with a soft gloss from above and an edge that catches light at the top and falls into shade at the bottom. A small shadow lifts it off the glass. The symbol is white and heavier than the font's own weight, with a faint shadow of its own. The Settings window draws the same tiles. High contrast keeps plain symbols. A see-through "tinted glass" tile was tried and looked washed out beside app icons.

**Why.** The owner found the flat tiles with thin symbols plain next to the app icons around them.

## Orbit draws its own symbols

**Decision.** The symbols on rows, tiles, menus and key caps are drawn by Orbit itself (`symbols.rs`), as a few strokes each on a 24-unit grid, rather than taken from the Windows icon font. Strokes are 2 units wide, or 2.4 on a coloured tile, and never thinner than a pixel and a quarter, so small ones don't go faint. They keep the icon font's code points, and anything Orbit hasn't drawn still comes from the font. Settings uses the same symbols, and the same lit tiles for its page badges. The chosen row's tile brightens a little and glows in its own colour.

**Why.** The owner found the font's symbols too thin, in menus as well as on tiles. The glow on the chosen tile was tried at the owner's request and may be taken out.

## End task is in a running app's menu

**Decision.** A running app's Ctrl+K menu has End task, which ends every process of that program at once, as Task Manager does. It has no key of its own, so it can only be chosen on purpose. File Explorer, which is also the taskbar, and Orbit itself are never ended. Windows refuses for apps running as administrator, and Orbit says so.

**Why.** The owner asked for End task in the action menu rather than a separate list of processes.

## Window layouts move the window Orbit was opened over

**Decision.** Left, Right, Top and Bottom Half, Maximise, Almost Maximise (5% in from each edge), Centre, and Move to Next Display. They move the window that was in front when the launcher opened, or, from a hotkey, the one in front. They fit the work area, so the taskbar stays clear, and allow for the invisible resize borders Windows 10 and 11 put around windows. A window that covers its whole display without being maximised, such as a game or a full-screen video, is left alone.

**Why.** The owner asked for window layouts. Snap and Win+arrows do some of this already, but not centring, almost maximising or a hotkey per layout.

## System toggles

**Decision.** Toggle Dark Mode, Toggle Mute, Toggle Bluetooth and Toggle Wi-Fi. Dark mode writes the same two registry values Settings does, then tells open windows. Mute uses the speakers' own volume control. Bluetooth and Wi-Fi use Windows' radio interface on a thread of their own, since Windows answers in its own time. Each says what it did. There is no Do Not Disturb toggle: Windows has no public way for an app to change it.

**Why.** The owner asked for system toggles.

## Timer and stopwatch

**Decision.** Timer takes a time typed as "5", "5m", "90s", "1h 30m", "1:30" or "2 minutes" (a bare number is minutes, up to four days), from its own view or as "timer 5m" in the launcher. Its view is a ring that empties as it runs, with the time large in the middle and digits of one width, so it doesn't shuffle. Enter pauses and resumes; Ctrl+K adds a minute or five, or cancels. When it ends, Orbit beeps and Windows shows a notification. The stopwatch starts when first opened, and its ring goes round once a minute. Both keep going with the launcher closed. Neither ticks in the background: the timer sets one Windows timer for when it's due, and the launcher redraws once a second only while one of them is on screen. Times come from the wall clock, so a timer still ends on time after sleep.

**Why.** The owner asked for a timer. The rule against high-frequency timers is why the stopwatch shows whole seconds.

## Pick Colour

**Decision.** Pick Colour hides the launcher, takes one still picture of every display and shows it over them, with a magnifier beside the pointer showing the pixels around it, the one being picked outlined, and its hex. Click, Enter or Space picks; Esc, a right click or switching away cancels; the arrows move a pixel at a time, or ten with Shift. The launcher then shows the colour large with its HEX, RGB and HSL: Enter copies the hex, Ctrl+Enter the RGB, Ctrl+Shift+Enter the HSL. The picture is never kept.

**Why.** The owner asked for a colour picker. A still picture keeps the magnifier from picking itself, and needs no timer.

## Releases are an installer and a portable exe

**Decision.** Each release has two files. `Orbit-<version>-Setup.exe` is an Inno Setup wizard (`installer/orbit.iss`) that installs for the current user into `%LOCALAPPDATA%ProgramsOrbit`, with no admin prompt, can start Orbit at sign-in, and shows up in Settings → Apps. Before it replaces or removes Orbit it runs `orbit --quit`, which closes the Orbit using the same data folder and waits for it to go. Uninstalling removes the sign-in entry only if it still points at the installed copy, and asks before deleting the data, keeping it by default. `Orbit-<version>-Portable.exe` is the same exe: "portable" in its file name makes it keep its data in an `Orbit settings` folder beside itself. The "Start Orbit when I sign in" switch reads Windows' own entry, so it agrees with what the installer set.

**Why.** The same pattern as the owner's other apps. The previous zip needed a PowerShell script to uninstall.

## Things left out on purpose

No plugin framework, AI, cloud sync, embedded web server, driver or privileged daemon in the first version, as the brief requires. No periodic rescans: the app index is refreshed when asked or at start if stale.
