# Privacy

What Orbit on Windows captures, where it keeps it, what it never writes down, and how to remove it. Everything here was checked against the source; files are named so you can check it too.

**There is no encryption at rest.** Notes, snippets, clipboard history, settings and exports are stored in plain text. See [Storage and encryption](#storage-and-encryption).

Related: [SHORTCUT_SEMANTICS.md](SHORTCUT_SEMANTICS.md), [DATA_PORTABILITY.md](DATA_PORTABILITY.md), [ARCHITECTURE.md](ARCHITECTURE.md).

## Summary

| Capability | Default | Needs a keyboard hook | What it sees | What it keeps |
|---|---|---|---|---|
| Launcher shortcut, direct shortcuts | On (launcher), none (others) | No | Nothing: Windows tells Orbit only that the chord was pressed | Nothing |
| HyperKey | Off | Yes, unless mode is `external` | Every key event system-wide, while the hook is installed | Nothing written. In memory: which keys are currently down |
| Snippet expansion | Off | Yes | Every key event system-wide, while the hook is installed | Nothing written. In memory: at most 32 recent characters |
| macOS feel | Off | Yes | Every key event system-wide, while the hook is installed. It only looks for Alt with these keys | Nothing written |
| Clipboard history | Off | No | Clipboard content, only after it changes and only if not excluded | Text, file paths and images, in a local database. Images can be turned off |
| 1Password, Ente Auth | Off | No | Item names, and the one value you choose | Nothing written. Names in memory for up to 10 minutes |
| Notes, snippets | n/a (user-created) | No | n/a | Stored locally, plain text |
| Usage counts | Always on | No | Which launcher item you ran | Item key, count, last-used time |
| Updates | On | No | GitHub's answer about Orbit's latest release, once a day, and the new version's file when there is one (see below) | A downloaded new version, until it's installed |
| Quicklink website icons | On | No | Each quicklink's website, on the internet or your own network, asked for its icon (see below) | The icons, in `link-icons\` |
| Currency conversion | On | No | The European Central Bank's public file of daily rates, when you convert (see below) | The bank's file, in `exchange-rates.xml` |
| Weather | On | No | The place you look up, sent to Open-Meteo, and that place's forecast (see below) | The last place found, in `settings.json`. The forecast in memory for 15 minutes |
| Speed test | Runs only when opened, after Ookla's terms are accepted | No | What Ookla's Speedtest CLI prints about the test (see below) | That the terms were accepted, in `settings.json`. The last result in memory only |
| GitHub | Off | No | Your open issues and pull requests, notifications and starred repositories, through GitHub's `gh` tool (see below) | Nothing written. Each list in memory for a minute |
| AI Usage | Off | No | Claude Code's and Codex's sign-ins on this PC, sent only to Anthropic and OpenAI to ask how much of your limits is used (see below) | Nothing written. The answer in memory for a minute |
| Pick Colour | Runs only when chosen | No | One still picture of every display, while you pick (see below) | Nothing written. The picked colour in memory while it's shown |
| Toggles, window layouts, End task | Run only when chosen | No | The window in front (layouts), the app you chose (End task) | Nothing. They change the Windows setting, window or app you chose |
| File search | On, if Everything is installed and running | No | What you type in Everything's search, and in the launcher from three characters on, sent to Everything on this PC (see below) | Nothing written. Opening a file adds one to Everything's own count for it |
| Import | Runs only when chosen | No | Other launchers' settings, and the export you choose, read and never changed (see below) | What you choose to bring over, as your own links, snippets and notes. Never the passphrase |
| Extensions | Each is off until you turn it on | No | What you type in its commands goes to its program (see below) | What the extension keeps is up to it. Orbit gives it `extension-data\<id>` for that |

The defaults are checked by the test `defaults_are_conservative` in `crates/orbit-core/src/settings.rs`: HyperKey, clipboard capture and expansion are all off until the user turns them on.

## Network and telemetry

Orbit has no telemetry and no account, and sends nothing about you or what you do. Its network code does five things, all described next: it looks for new versions of Orbit, fetches quicklink icons, the European Central Bank's exchange rates for currency conversion, the weather when you ask for it, and, with AI Usage on, Claude's and Codex's usage when you open it. The speed test is run by Ookla's own program, and GitHub's lists by GitHub's own `gh`, both also described below. Weather, Speed Test, GitHub and AI Usage are extensions: Settings → Extensions turns each off, and one that's off has no commands in the launcher and runs nothing. Quick links and URL actions are handed to Windows (`ShellExecuteExW`) to open in your default browser. Clipboard history never fetches or previews URLs.

### Updates

"Check for updates" on the General page is on by default, and turning it off stops the daily look. "Check now" beside it, the tray menu and "Check for Updates" in the launcher still look when you ask. The code is `platforms/windows/src/update.rs`, using WinHTTP; reading GitHub's answer and checking the download is in `crates/orbit-core/src/update.rs`.

- **When.** About 10 seconds after Orbit starts, then once a day, by the clock, so a day asleep counts. A look that fails, because there's no connection or GitHub is busy, says nothing and is tried again an hour later.
- **Who.** Only GitHub: `https://api.github.com/repos/Beroris/orbit/releases/latest`, and, when there's a newer version, its file from `github.com`, which GitHub serves from its own file servers. A file listed anywhere else is refused.
- **What it sends.** A plain `GET` with the user agent `Orbit/<version>`, no cookies and no Windows sign-in. Windows' proxy settings apply. GitHub sees your IP address and that a copy of Orbit asked about its latest release. Nothing about you or how you use Orbit is in it.
- **What it downloads.** Only a newer version, and only the file this copy installs from: `Orbit-<version>-Setup.exe` for the installed copy, `Orbit-<version>-Portable.exe` for the portable one. A copy built from source downloads nothing and points to the release page instead. The file must match the size and SHA-256 GitHub lists for it, or it's thrown away (`a_download_must_be_the_file_the_release_lists`).
- **What it keeps.** The installed copy's download in `%TEMP%\Orbit update`, and the portable copy's beside its exe, as `<name>.exe.new`. Nothing is installed until you say so: from the notification, the tray menu, Orbit's menu in the launcher, or Settings. Then the installed copy runs the new Setup with no window, which closes Orbit, puts the new version in place and starts it; the portable copy swaps its exe for the new one and starts it. The new version's first look deletes what the update left, including the old portable exe.

### Quicklink website icons

"Show website icons" on the Links & commands page is on by default, at the owner's request, and turning it off stops all of the below. The code is `platforms/windows/src/favicon.rs`, using Windows' own WinHTTP and image decoder.

- **When.** When Orbit starts, the website of every link without an icon is asked. Opening the launcher asks again for links still without one, at most every 30 minutes, so a site that was down, or couldn't be reached from where you were, gets another chance. A link's website is also asked when the link is added or its address changes, and saving a link from its form asks again even if it has an icon, which is how to refresh one. Nothing is kept about a site that gave no icon. A site whose icon is kept isn't asked again otherwise.
- **Who.** The link's own website, whether it's on the internet or on your own network. Orbit uses the host from the link's address, never its path or query, which might hold something personal. It reads the home page's `<link rel="icon">` tags, then tries `/apple-touch-icon.png`, `/favicon.ico`, `/favicon.svg` and `/favicon.png`, up to 8 addresses in all. Like a browser, and chosen by the owner, it goes where the site sends it, as notion.so sends it to notion.com, and loads icons the page keeps on another server, as Discord's are. That other server must be a public HTTPS address: nothing goes to a private address or a local name unless it's the link's own site, so a site on the internet can't point Orbit at something on your network. It sees your IP address and the icon's address, nothing else. Another server that turns Orbit away, or doesn't have two of the icons the page names, isn't asked for more. When the home page sent Orbit to another site, such as a sign-in page, the usual places on the link's own site are tried before that page's icons. A link whose host is itself a blank to fill in is never asked.
- **Sites on your own network.** A local name (`localhost`, `.local`, a name with no dot) or a private IP address is asked over HTTPS, then over plain HTTP if nothing answers there, as is any link whose address starts `http://`. Their certificates aren't checked, since such sites often make their own, and nothing in the request needs one: it carries no cookies and no sign-in, and an icon is all that comes back. The name is looked up through the network's DNS server, as opening the link would. On another network, the same name or address may be someone else's device, so a link with no icon yet could be given that device's icon; saving the link asks again.
- **What it sends.** A plain `GET` with the user agent `Mozilla/5.0 (compatible; Orbit)`, the form sites expect from a program that isn't a browser, since some turn away a plain `Orbit`. No cookies (`WINHTTP_DISABLE_COOKIES`), and no Windows sign-in (`WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH`), on your own network too, so a site that only answers people signed in to Windows gives no icon. The site sees your IP address, as with any visit, and so learns that someone has a quicklink to it. Windows' proxy settings apply. Requests time out after 8 seconds, a page is read up to 256 KB and an icon up to 1 MB, and up to 8 redirects are followed. Tests with a local server check the request (`a_request_names_orbit_and_carries_nothing_else`, `a_redirect_somewhere_private_is_not_followed`, `a_site_on_your_network_gives_its_icon_over_http`).
- **What it keeps.** Each icon as a 64 by 64 picture in `link-icons\`, named by a hash of the host. An icon given only as SVG is drawn into that picture on your PC with Windows' Direct2D. Nothing more is fetched for it. A site that gives no icon when asked again keeps the one it had. Earlier versions kept an empty `.none` file for a site that had none, so it wasn't asked again; each is removed when its site is next asked. Icons kept from before logos filled their whole square, drawn on a white tile, are fetched once more in the new style and the old file is removed. Turning the switch off deletes the folder, and so does deleting Orbit's data.

Other programs Orbit starts do use the network, and only because of something you did:

- **Install or Remove on the Extensions page.** For 1Password, this starts Windows Package Manager, which contacts Microsoft's package source and 1Password's servers. For Ente, it starts PowerShell, which reads the release list from GitHub's API and downloads a release from GitHub, identifying itself only as `Orbit`.
- **1Password and Ente's own tools.** They talk to their own services as they normally do. Ente's list syncs after showing what's saved on this PC, and reading a code doesn't sync.

### Exchange rates

"Convert currencies" on the General page is on by default, and turning it off stops all of the below and deletes the kept rates. The code is `platforms/windows/src/rates.rs`, with the same WinHTTP code as the icons; the sums are in `crates/orbit-core/src/currency.rs`.

- **When.** Only when you type a conversion, such as "123 euro in pounds", and the kept rates are missing or more than 12 hours old. Then once, on a thread of its own. After a failure, typing doesn't ask again for 10 minutes. Nothing is fetched at start or on a timer.
- **Who.** Only `https://www.ecb.europa.eu/stats/eurofxref/eurofxref-daily.xml`, the bank's public reference rates. Redirects are followed only within that site.
- **What it sends.** A plain `GET` with the user agent `Orbit`, no cookies, no Windows sign-in, Windows' proxy settings and a 5-second timeout. Nothing you type is in it: the address is always the same, and the conversion is worked out on your PC. The bank sees your IP address and that someone fetched its rates.
- **What it keeps.** The bank's file, about 2 KB, as `exchange-rates.xml`, kept only if it reads as the bank's rates. Each answer shows the day of the rates it used.

### Weather

Weather is on by default. Turning it off on the Extensions page takes it out of the launcher, stops all of the below and forgets the kept place. The code is `platforms/windows/src/weather.rs`, with the same WinHTTP code as the icons; reading the answers is in `crates/orbit-core/src/weather.rs`. Forecasts are by [Open-Meteo.com](https://open-meteo.com/), under CC BY 4.0, as the weather view's footer says.

- **When.** Only when you open the weather from the launcher or look up a place in it, on a thread of its own. A forecast for the same place from the last 15 minutes is shown without asking again. Nothing is fetched at start or on a timer.
- **Who and what it sends.** A new place's name, as you typed it, goes to `geocoding-api.open-meteo.com` to find where it is. Then its latitude and longitude, rounded to two decimal places (about a kilometre), go to `api.open-meteo.com` for the forecast. Nothing else you type goes anywhere, and Orbit never asks Windows where you are. The requests are plain `GET`s with the user agent `Orbit`, no cookies, no Windows sign-in, Windows' proxy settings, a 5-second timeout, and redirects only within the same site. Open-Meteo sees your IP address and the place.
- **What it keeps.** The last place found (its name, region, latitude and longitude) in `settings.json`, so that "weather" alone shows it next time. The forecast is kept in memory only, for 15 minutes, and its pictures are let go of when the launcher hides.

### Speed test

Orbit doesn't measure your connection itself. Speed Test, which the Extensions page can turn off, runs Speedtest CLI by Ookla, a separate program that isn't part of Orbit, against Speedtest.net's servers. The code is `platforms/windows/src/speedtest.rs`; reading what the program prints is in `crates/orbit-core/src/speedtest.rs`.

- **Ookla's terms first.** The first time, the launcher says what the test does and that Ookla allows it for personal, non-commercial use under its End User License Agreement, Terms of Use and Privacy Policy, with links to each in Ctrl+K. Nothing is installed or run until you press Enter to accept. Orbit then records that you accepted, in `settings.json`, and passes `--accept-license --accept-gdpr` to the program each time, so it doesn't stop to ask.
- **Installing it.** If the program isn't there, Orbit installs it with Windows Package Manager (`winget install --id Ookla.Speedtest.CLI`), which downloads it from Ookla. Orbit looks for it in Windows Package Manager's folder for that package, or on the PATH. A program on the PATH is run with `--version` first, and used only if it says it's Ookla's, since other programs are also called `speedtest`.
- **When.** Only when you open Speed Test or press Enter in it. Leaving it, or quitting Orbit, stops a test that's running. A test that runs for two minutes is stopped.
- **What's sent.** Ookla's program finds a nearby server, then downloads and uploads test data for about 20 seconds. Ookla and the test server see your IP address. Ookla keeps each result on Speedtest.net, which is where the result link comes from. Its own settings folder, `%APPDATA%\Ookla`, is the program's, not Orbit's.
- **What Orbit keeps.** The speeds, ping, jitter, packet loss, the server's name and place, your provider's name and the result link, in memory only, until the next test or until Orbit closes. The program also prints your local and public IP addresses and your network adapter's hardware address. Orbit never reads them (`addresses_are_never_kept`).

### GitHub and AI Usage

Both are off until you turn them on in Settings → Extensions. Both fetch only when you open one of their lists, or choose Refresh in it, and keep the answer in memory for a minute so going back and forth doesn't ask again. Nothing is fetched at start or on a timer, and nothing they show is written to disk. The code is `platforms/windows/src/providers.rs`; reading the answers is in `crates/orbit-core/src/github.rs` and `crates/orbit-core/src/ai_usage.rs`.

**GitHub** runs GitHub's own command-line tool, `gh`, with no window, so it's `gh` that signs in and talks to GitHub, and Orbit never reads or sees a token.

- **What it asks.** `gh api` for your open issues and pull requests (`search/issues` with `involves:@me`), your notifications, and your starred repositories. Marking a notification as read sends `PATCH notifications/threads/<id>`, and only when you choose it.
- **Who you are.** Settings shows who `gh` is signed in as, read from the `user:` line of `gh`'s own `hosts.yml`. Nothing else in that file is read.
- **An old token in the environment.** `gh` prefers a `GITHUB_TOKEN` or `GH_TOKEN` environment variable to its own sign-in, and an old one makes every request fail. When `gh` has a sign-in of its own, Orbit runs it with those two variables removed. Your environment isn't changed.
- **Installing and signing in.** GitHub's card in Settings can install `gh` with Windows Package Manager (`winget install --id GitHub.cli`), and opens `gh auth login --web` in a console window of its own, asking for the `notifications` scope. You sign in in your browser, on GitHub.

**AI Usage** reads the sign-ins that Claude Code and Codex already keep on this PC, and asks each one's own service how much of your plan's limits you've used.

- **What it reads.** Claude Code's `.credentials.json` in `%USERPROFILE%\.claude` (or `CLAUDE_CONFIG_DIR`), and Codex's `auth.json` in `%USERPROFILE%\.codex` (or `CODEX_HOME`). From each it takes the access token, and Claude's plan name and expiry time. A service with no sign-in on this PC is left out.
- **Who it sends them to.** Claude's token goes only to `https://api.anthropic.com/api/oauth/usage`, and Codex's token and account id only to `https://chatgpt.com/backend-api/wham/usage`: each service's own server, which already has it. The request is a `GET` with the user agent `Orbit`, no cookies, no Windows sign-in, no redirects at all (so a token can't be passed on elsewhere), and a 5-second timeout.
- **What it never does.** It never renews a sign-in, which could sign Claude Code or Codex out, never writes either file, and never shows, logs or keeps a token. An expired Claude sign-in is reported as expired.
- **Codex's log.** If OpenAI's server doesn't answer, Orbit reads the end (up to 4 MB) of Codex's newest session log in `sessions\` and uses the last rate-limit line in it, saying how old it is. Those logs also hold your Codex conversations. Orbit only looks for the rate-limit lines and keeps nothing else from them.

## Keyboard hook: HyperKey and snippet expansion

Orbit installs a low-level keyboard hook (`WH_KEYBOARD_LL`) only when HyperKey (in `internal` or `synthetic` mode), snippet expansion or macOS feel is on. With all of them off, no hook exists (`hook::config_from_settings`, test `no_hook_by_default`). The Settings pages for both features say so at the point where you enable them.

While the hook is installed, Windows gives Orbit's hook callback every keystroke on the desktop, including keystrokes into password fields. This is how the feature works; it is not avoidable with a hook. What the callback does with them (`platforms/windows/src/hook.rs`):

- It reads the key code, scan code, flags and event time.
- The HyperKey state machine keeps a set of which keys are **currently down**, and which of those it swallowed. That is state about the present, not a history. It is cleared on lock, suspend and reset.
- With expansion on, a fixed buffer holds at most 32 recently typed characters, so it can answer "do the last few characters end in a snippet trigger?". It is cleared on a match, navigation keys, foreground and focus changes, a pause of 5 s, a chord Orbit swallowed, and on reset. It is never written to disk or logged. The password check happens after a trigger matches, so characters typed into a password field, including a web password field, can sit in this buffer in RAM until one of those events; a match there is discarded (details in [SHORTCUT_SEMANTICS.md](SHORTCUT_SEMANTICS.md)).
- It posts to the main window only what it recognised: the code of a bound HyperKey key with a timestamp, or a snippet id and the trigger length.
- It does not store, log or transmit typed text. Nothing in the hook, the state machine or the matcher writes to a file. The only data kept about the hook for Diagnostics is its slowest callback time and a reinstall counter.

Keys injected by other programs aren't processed. Orbit never records a stream of keystrokes, and does not offer a feature that does. If the hook cannot be installed, Orbit says so and does not try another way. Expansion also loads the system's UI Automation library the first time a trigger matches, in order to ask whether the focused field is a password field. It reads only that yes-or-no property, never the field's contents.

Controls: turn the feature off in Settings; "Suspend Orbit Shortcuts" (tray menu or launcher) removes the hook entirely; quitting Orbit removes it. Per-app exclusions are in [Exclusions](#exclusions).

## 1Password and Ente Auth

Both are off until you turn them on, on the Extensions page of Settings or of the welcome, which can also install the command-line tool each one needs. Orbit never sees your master password, your Ente password or your vault as a whole.

- **Signing in** belongs to each app. 1Password asks through its desktop app, using whatever unlocks it on this PC. Ente's sign-in runs in its own terminal window, and its login stays in its own data folder.
- **The list.** `op item list` and `ente auth code list` give names, usernames and web addresses, not passwords or codes. Orbit keeps them in memory for up to 10 minutes, and drops them on lock and under memory pressure. They are never written to disk.
- **A value** is fetched only when you choose it. Orbit reads it from the tool's output, puts it on the clipboard, and wipes its own copy of the text. It isn't logged. Windows and the standard library may hold copies of pipe buffers that Orbit can't reach, for a short time.
- **The clipboard.** Passwords and codes are marked to stay out of Windows clipboard history, the cloud clipboard and clipboard tools that respect those marks, including Orbit's own history. They clear after 30 seconds, on lock and on quit, unless you've copied something since. Usernames carry the same marks and don't clear.
- **Arguments** contain an item's id and never a secret. Tools run directly, with no shell in between.

Stand-in tools are used in Orbit's own tests. Real vaults and accounts are checked by the owner.

## Clipboard history

Off by default (`clipboard.enabled = false`). When on:

**How changes are detected.** `AddClipboardFormatListener`; no polling. Pausing history or turning it off removes the listener, so Orbit receives no clipboard events at all while paused.

**What is read, in order of precedence.** If the clipboard holds a file list, only the file paths are read (up to 64 of them), not file contents. Otherwise Unicode text. Otherwise, if "Include images" is on, as it is by default, and Low Resource Mode is off, a bitmap.

**The preview beside the list** loads the full content of the selected item only, on Orbit's data thread, while the launcher is open, and lets it go when the launcher hides. It shows the text (up to 2,000 characters, with counts over all of it), the picture, or the file paths. For copied files it also reads their sizes from disk, unless there are more than 50 or one is on a network path. Nothing new is stored for it. The source shown is the app's name from Orbit's app index, worked out from the stored executable name when shown.

**What is stored** (`crates/orbit-data/src/clipboard.rs`): kind (text, url, files, image), the full text or the newline-separated paths, a 160-character single-line preview, a hash and byte size, the name of the source program's executable, the copy time, a pinned flag, and for images a file name and dimensions. Images are saved as raw `.dib` files in `clipboard-images/`, up to a size cap. Text copied with formatting, such as from a browser or Word, also keeps its HTML and RTF clipboard formats beside the plain text, up to 4 MB each, so pasting it gives the formatting back. "Paste as plain text" leaves them out. Choosing Open on a copied picture writes a `.png` copy beside its `.dib`, which is deleted with the item. Identical copies are merged (hash plus exact text comparison) and moved to the top rather than stored twice.

**What is not captured.** The checks happen before the content is opened (`platforms/windows/src/clipboard.rs`):

1. The change was Orbit's own write (for example when you paste from history).
2. You chose "Do Not Remember Next Copy", or history is paused.
3. The source program is on the clipboard exclusion list.
4. The source application marked the data as private using one of the Windows conventions: the formats `ExcludeClipboardContentFromMonitorProcessing` or `Clipboard Viewer Ignore`, or `CanIncludeInClipboardHistory` set to zero.
5. Text is empty or whitespace, or larger than `max_item_bytes` (default 256 KiB; oversized text is not copied out of the clipboard).

**What Orbit does not do.** It does not scan content for passwords, card numbers, tokens or one-time codes. Items 3 and 4 depend on the source program's name or its cooperation. A program that neither marks its copies nor is on your exclusion list will be recorded. Nothing here is a guarantee that secrets are never stored; the Settings page says the same.

**Retention.** Defaults: 500 items, 30 days (0 days = no age limit). Pinned items are exempt. Retention is applied after each new item, on startup and when you change the limits. Deleting a clip, or trimming it by retention, also deletes its image file.

**Memory-only mode.** With "Keep history after Orbit exits" off, history lives in an in-memory database and is gone when Orbit exits. Turning the setting off while Orbit runs deletes `clipboard.db` and its journal files. If `clipboard.db` is left over from an earlier session and Orbit starts with the setting already off, the old file is not removed by Orbit.

**Pause is not saved.** The tray and launcher pause lasts until Orbit restarts.

**Clearing.** "Clear Clipboard History" (launcher command, asks for confirmation; and the Clipboard settings page) deletes all unpinned items, deletes their image files, and compacts the database (`VACUUM`). Pinned items stay. Individual items can be deleted from the history list, including pinned ones.

**Pasting.** Choosing an item fetches its full text from the database and puts it on the clipboard (and sends Ctrl+V for "insert"). Search results carry only previews.

## Snippets

Snippets (name, trigger, content, description, category) are stored in plain text in `orbit.db`. Do not put secrets in them; there is no protection beyond the file permissions of your user profile.

- `{clipboard}` reads the clipboard only when a snippet that contains it is used. When such a snippet expands from a typed trigger, Orbit first shows a preview and waits for you to approve. Replacement text is never re-scanned, so clipboard content cannot inject other placeholders.
- `{date}` and `{time}` use the local clock.
- Expansion (the typing hook) is described above and in [SHORTCUT_SEMANTICS.md](SHORTCUT_SEMANTICS.md).

## Notes

Plain-text bodies and a title derived from the first line, in `orbit.db`, plus a full-text index of the same text. Moving a note to the trash keeps it for 30 days; the purge runs when the data thread starts (at Orbit startup), not on a timer. There is no "delete now" for a trashed note in the Windows host. Notes are included in exports.

## Create Quicklink reads the window you were in

Choosing Create Quicklink for a new quicklink reads the one window that was in front before the launcher opened, once, to fill in the form: a browser's address bar through UI Automation (Chromium and Firefox-based browsers), the folder a File Explorer window shows, or, for other apps, the window's title matched against Windows' own list of recent files (`%APPDATA%\Microsoft\Windows\Recent`). It runs on a thread of its own and reads nothing else. Nothing it finds is written anywhere unless you save the quicklink, and it is never logged. Editing an existing quicklink reads nothing. Orbit's own windows, and a browser's internal pages (settings, new tab and the like), are skipped.

## Pick Colour takes a picture of the screen

Pick Colour takes one still picture of all your displays when you start it, shows it full screen with a magnifier, and reads the one pixel you pick. The picture is held in memory only, never written or sent, and is dropped the moment you pick, press Esc or switch away. Protected video, and anything on the secure desktop, such as a UAC prompt, can't be pictured by any app. The colour you picked is kept in memory while the launcher shows it, and not after.

## Toggles and window layouts change Windows settings

Toggle Dark Mode writes the two values Settings → Personalisation → Colours writes (`AppsUseLightTheme` and `SystemUsesLightTheme` under `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize`). Toggle Mute mutes the default speakers, Toggle Bluetooth and Toggle Wi-Fi switch the radio, through the same Windows interfaces the quick settings use. Window layouts move the one window that was in front. End task ends every process of the app you chose it for, as Task Manager does, and never File Explorer or Orbit. None of these run unless you choose them, and none are logged.

## File search asks Everything

The Everything extension asks [Everything](https://www.voidtools.com/) by voidtools, if it's running, with a message to its window on this PC. Nothing leaves the PC. Everything sees each search: what you type in its search, and, while "Show files under other results" is on, what you type in the launcher once it's three characters long. Opening a file from Orbit tells Everything to count it, which is what sorting by "most opened" uses. Whether Everything keeps a history of searches or runs is set in Everything's own options. Orbit writes nothing about files, and keeps the last answer in memory only. If Everything isn't installed, Orbit can install it with Windows Package Manager when you ask, and Everything's installer asks Windows for permission.

## Import reads other launchers' files

The Import page, after installing or from Settings → Data, reads the settings of the launchers it finds: the files listed in [DATA_PORTABILITY.md](DATA_PORTABILITY.md#import), and Wox 2's database, opened read-only. It never changes them. It doesn't read Raycast's own data, which Raycast keeps encrypted, only an export you choose. A Raycast export's passphrase is used to open that file and then forgotten; it isn't written anywhere. What comes over is added to your settings and database like anything you made yourself.

## Extensions run programs you add

An extension is a program, run with your permissions, like any program you start. It's off when added, and Settings shows the command it runs before you turn it on. While its command is open, what you type goes to that program, which may do anything a program can, including use the network. Orbit doesn't watch what it does. It starts the program only when one of its commands is used, stops it a few minutes after the launcher was last used, and ends it when Orbit quits. An extension's links can't use schemes that run code, such as `javascript:` or `file:`. Removing an extension in Settings deletes its folder and its `extension-data` folder.

## Other data Orbit keeps

| Data | Where | Notes |
|---|---|---|
| Usage counts | `command_usage` table in `orbit.db` | Item key (for example an app id or command key), count, last used time. Never the text you typed. There is no UI to clear it. |
| Settings | `settings.json` | Shortcuts, HyperKey bindings, quick links, custom commands (program, arguments, working directory), aliases, pinned and hidden item keys, exclusion lists. Anything you type into a command's arguments is stored here as plain text. |
| App index and icons | `apps.json`, `icons/` | Installed apps found in the Start Menu and Windows app listing: names, ids or paths, icons. Cache; rebuilt if deleted, and whenever an app is installed, updated or removed (Start Menu folder changes, and Windows' package catalog events for Store and other packaged apps). |
| Crash log | `crash.log` | Thread name and source `file:line` of a panic. The panic message is deliberately not written, because it could quote user data. |
| Benchmark timing log | A path you pass with `--bench-log` | Only if you start Orbit with that option. Timestamps, a source label, query **length** and result count. Never query text. |

## What is never logged

- Clipboard content, file paths copied, or image data.
- Typed characters, key sequences or snippet trigger buffers.
- Note or snippet bodies.
- Search queries (usage records and the timing log hold item keys and lengths only).
- Panic messages (only thread name and location).

Orbit has no general logging facility: the code contains no `println!`, `log` or `tracing` calls. User-visible error messages (for example "Could not open <path>") are shown on screen and in notifications; they are not written to a file.

## Exclusions

Three separate lists in `settings.json`, edited in Settings. Each entry is an executable file name such as `keepass.exe`, matched case-insensitively against the whole name. No paths, no wildcards. If Orbit cannot read a process's image path (some protected processes), the name is empty and no entry matches.

| List | Default | Effect |
|---|---|---|
| `clipboard.excluded_apps` | Password-manager list (`DEFAULT_SECRET_APPS`) | Copies whose source program is listed are never read or stored. |
| `snippets.excluded_apps` | Same list | No expansion while that program is in the foreground; the trigger buffer is not fed. |
| `hyper.excluded_apps` | Empty | HyperKey passes through while that program is in the foreground. |
| `hyper.pass_through_in_games` | On | HyperKey passes through in a fullscreen game. To recognise one, Orbit reads the program paths in Windows' own game list (`HKCU\System\GameConfigStore\Children`) and keeps them in memory. It does not store or send them. |

`DEFAULT_SECRET_APPS` contains `1password.exe`, `bitwarden.exe`, `keepass.exe`, `keepassxc.exe`, `lastpass.exe`, `dashlane.exe`, `enpass.exe`, `nordpass.exe`, `protonpass.exe`, `authy.exe`, and four names without `.exe` (`1Password`, `Bitwarden`, `KeePassXC`, `Keychain Access`) that do not match any Windows executable name and have no effect on Windows. The list is a best effort and covers only those programs. Once settings are saved, the list is stored in your `settings.json`; later changes to the built-in default list do not update it.

## Storage and encryption

**There is no encryption at rest.** The code contains no encryption: `rusqlite` is built with the bundled plain SQLite (no SQLCipher), there are no calls to DPAPI or any crypto API, and settings, exports and image files are ordinary files. Anyone or anything that can read your user profile folder, or the disk, can read the database files directly.

What does protect the data is Windows itself:

- Files are created under `%LOCALAPPDATA%\Orbit`, which Windows normally restricts to your account. Orbit does not set or change file permissions; it inherits whatever the folder has.
- Full-disk encryption such as BitLocker, if you use it, covers these files like any others. That is outside Orbit's control.

The brief asks for encryption at rest or a documented tradeoff and an option for non-persistent history. The non-persistent option exists (memory-only clipboard history). Encryption does not exist yet; this is a known gap.

**Deleted data may remain on disk.** The code does not enable SQLite's `secure_delete`, and `VACUUM` runs only when you clear clipboard history. Deleted notes (after the trash purge), deleted snippets, deleted single clips and retention-trimmed clips can leave their text in free database pages or the write-ahead log until the space is reused. Image files are removed with an ordinary file delete, not overwritten.

**Exports are plain files.** Export writes to a folder under `Documents` (see [DATA_PORTABILITY.md](DATA_PORTABILITY.md)), including a copy of `settings.json`. Clipboard history is deliberately never part of an export (test `export_never_contains_clipboard_data`). The export folder is yours to protect or delete; Orbit does not track it afterwards.

## Data locations

| Location | What |
|---|---|
| `%LOCALAPPDATA%\Orbit\` (or the folder given with `--data-dir`) | Everything listed in the table in [ARCHITECTURE.md](ARCHITECTURE.md#data-layout): `settings.json`, `orbit.db`, `clipboard.db`, `clipboard-images\`, `apps.json`, `icons\`, `link-icons\`, `exchange-rates.xml`, `crash.log`, and their journal files |
| `Documents\Orbit Export <date> <time>\` | Folders created by Export |
| `extensions\` and `extension-data\` in the data folder | Extensions you added, and what each keeps for itself |
| `%LOCALAPPDATA%\Programs\Orbit\` | The installed program and its licence files. With it go its entry under `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall` and its Start menu and desktop shortcuts. Not there for a portable copy |
| `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `Orbit` | Only if "launch at login" is on |
| Your `--bench-log` path | Only if you used that option |

Settings → Data shows the exact data folder and has an "Open folder" button.

## How to delete your data

**Clipboard history.** Launcher: "Clear Clipboard History", or Settings → Clipboard → "Clear history". Unpinned items go; pinned items stay unless you delete them individually from the history list. To keep nothing on disk, turn off "Keep history after Orbit exits".

**Individual items.** Delete a snippet or a clip from its action menu in the launcher. Trash a note; it is purged at the next Orbit start after 30 days.

**Everything.** Settings → Data → "Delete all data", with its Delete button pressed twice, or the launcher command "Delete All Orbit Data", which asks first. Orbit turns off launch at login, removes its databases, the copies made before upgrades, settings, the app index, icons, clipboard images and logs by name, then quits. It removes the folder only if that left it empty, and it never empties a folder as a whole, since `--data-dir` can point anywhere. To do the same by hand:

1. Turn off "launch at login" in Settings (this removes the `Run` registry value), then quit Orbit from the tray menu. Orbit must not be running, or it will keep the database files open.
2. Delete the data folder, for example in PowerShell: `Remove-Item -Recurse -Force "$env:LOCALAPPDATA\Orbit"`.
3. Delete any `Orbit Export …` folders in `Documents` that you no longer want.
4. If you used `--data-dir` or `--bench-log`, delete those locations too.

Because deletion is an ordinary file delete, apply disk-level tools if you need the contents to be unrecoverable.

Quitting Orbit stops the hook and the clipboard listener, releases any virtual modifier keys, and removes the tray icon. Uninstalling from Settings → Apps quits Orbit, removes the program, its shortcuts, its Settings → Apps entry and its sign-in entry, and asks whether to delete the data folder, keeping it unless you say yes. The silent uninstall is tested by `scripts/test-install-windows.ps1`, in a sandbox in the temp folder; the window that asks about the data hasn't been clicked through yet. A portable copy keeps everything in the `Orbit settings` folder beside it, so deleting both removes it. For a copy built from source, `scripts/uninstall-windows.ps1` stops Orbit and removes the sign-in entry, and with `-DeleteData` also removes the data folder. That script has not been run against a real install.

## Known gaps against the brief

- No encryption at rest (above).
- No content-based detection of secrets or one-time codes in the clipboard; exclusion is by program name and by the opt-out formats.
- Password-field detection for expansion depends on what a field reports to UI Automation, and runs after the in-memory buffer has seen the keys.
- HyperKey has no password-field check, only the per-app exclusion list.
- No UI to clear usage counts, and no "delete now" for trashed notes. "Clear history" keeps pinned clipboard items.
- First-run consent is a notification ("Welcome to Orbit"), not a blocking consent screen. The consent text is in Settings next to each feature.
- Clipboard export (opt-in, with a warning) is not offered at all.
- Orbit does not set file permissions itself.

## Limits of app attribution

The per-app clipboard exclusion looks up the program that owns the clipboard. A copy with no owner window is attributed to the app in the foreground instead, so a background program that copies with no window is judged by whichever app is in front. It could be recorded even though that program is on the list, or skipped because the app in front is.

## macOS

Sources: `platforms/macos/Sources/Orbit/Clipboard.swift`, `HyperTap.swift`, `Metrics.swift`, `crates/orbit-ffi/src/lib.rs`.

### Permissions

| Permission | Used for | Without it |
|---|---|---|
| Accessibility | The event tap, and typing snippets with `CGEvent` | HyperKey Internal and Synthetic and expansion are off. Snippets go to the clipboard with a message. |
| Input Monitoring | The event tap | HyperKey Internal and Synthetic and expansion are off |
| Automation for Finder | Empty Trash, only when you choose it | That one command fails with a message. Not tested: tests don't run Empty Trash. |

The launcher, direct shortcuts, notes and clipboard history need none of them.

Orbit reads grants with `AXIsProcessTrusted` and `CGPreflight*`, which never prompt. It won't call `CGEvent.tapCreate` without the grants, because that call makes macOS show its own dialog. `feature_checks.sh` verifies the refusal when Orbit starts through LaunchServices with no grant. Settings → HyperKey shows the state and has buttons that open the two panes.

Started from a terminal, macOS answers these checks for the terminal rather than Orbit. Settings and Diagnostics say so.

An ad-hoc-signed build needs granting again after each rebuild.

macOS 15.4 added a per-app pasteboard access setting. Set to ask, each read can prompt. Set to deny, history stays empty. Not tested.

### Pasteboard polling

macOS has no copy notification. With clipboard history on and not paused, a timer on the main thread reads `NSPasteboard.changeCount` every 0.75 s. After 40 unchanged reads, about 30 s, it slows to every 3 s, and the next change returns it to 0.75 s. Timer tolerance is half the interval. With history off or paused, there's no timer. `feature_checks.sh` verifies that no watcher runs by default.

Orbit reads pasteboard contents only after the count changes, and only if the change wasn't Orbit's own write and "Do Not Remember Next Copy" isn't set.

### What is skipped

`evaluatePasteboard` checks these before it reads any body:

- `org.nspasteboard.ConcealedType`, `org.nspasteboard.TransientType` or `org.nspasteboard.AutoGeneratedType` on the pasteboard.
- An excluded app, matched against the frontmost app's name and bundle ID and the declared `org.nspasteboard.source`, ignoring case.

Text over the size limit is read to measure it and then dropped. Images are read only if "Include images" is on and Low Resource Mode is off.

This is best effort. An app that doesn't mark its copies and isn't on your list will be recorded. The frontmost app at the moment of the check may not be the app that copied. Orbit doesn't scan content for passwords or tokens.

`--self-test` and `feature_checks.sh` verify the skip rules on a private named pasteboard, and that a concealed copy doesn't reach `clipboard.db` or its WAL file.

### What is logged

A normal run writes no log of keys, queries or clipboard contents.

- `crash.log` in the data folder, only after a Rust panic: the source location and thread name, never the panic message.
- With `--bench-log`, a timing CSV at the path you give. It has timings, counts, permission states, messages shown to you (`report` lines), the skip decision for each copy, and in `state` lines the first row's title. A `state` line can therefore contain text you typed in the launcher. `--bench-log` is a test flag, and the default run doesn't write this file. `feature_checks.sh` verifies that clipboard bodies stay out of it.

The event tap stores nothing except an in-memory bounded trigger buffer for expansion, which is never written anywhere.

### What Orbit sends

The shared code and the host have no network code of their own. Opening a link or a web search row hands the URL to macOS, which opens your browser or the app for that scheme.

Blocked schemes such as `javascript:` and `file:` are refused with a message. A scheme that's neither on the safe list nor blocked opens with a notice naming the scheme. `feature_checks.sh` verifies both messages with `--no-open`, so the actual hand-off to macOS for those schemes wasn't run.

