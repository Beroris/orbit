# Orbit extensions

An extension adds commands to Orbit's launcher. It's a folder with a manifest, `orbit-extension.json`, and a program that Orbit talks to over standard input and output, one JSON object per line. Write the program in anything that reads and writes lines: PowerShell, Python, Node, Go, Rust, a batch file.

## The ones that come with Orbit

Settings → Extensions lists every extension in one table, these first. Each has a switch. One that's off is gone from the launcher, its commands and keywords with it, and none of its code runs.

| Extension | Starts | Commands | Keyword |
|---|---|---|---|
| **Everything** | On, once Everything is there | Everything, and files under other results | none until you give it one |
| **1Password** | Off | Search 1Password | |
| **Ente Auth** | Off | Search Ente Auth | |
| **Weather** | On | Weather | |
| **Speed Test** | On | Speed Test | |
| **GitHub** | Off | My Issues, My Pull Requests, GitHub Notifications, Starred Repositories | `ghi`, `ghp`, `ghn`, `ghs` |

- **Everything** is described at the end.
- **GitHub** asks GitHub's own command-line tool, `gh`, so it uses `gh`'s sign-in and Orbit never sees a token. Its card in Settings installs `gh` with Windows Package Manager if it's missing, then opens `gh auth login` in a window of its own. A list is kept for a minute. Enter opens an item in the browser. In Notifications, Ctrl+Enter marks one as read.
  - **My Pull Requests** lists the open ones that involve you, then under **Recently closed** those closed or merged in the last 14 days. Each says where it stands in up to two coloured pills, the most pressing first: Review requested (from you), Conflicts, Checks failing, Changes requested, Draft, Approved and Checks running; one with nothing to say is just Open, and closed ones say Merged or Closed. Its symbol is coloured as GitHub colours it: green while open, grey for a draft, purple once merged and red if closed.
  - **My Issues** does the same for issues: Open, then Completed, Not planned or Closed.
  - **Filters.** The drop-down at the right of the search box, or Ctrl+P, narrows a list: open ones only, leaving out what was merged or closed; pull requests you created, ones waiting for your review, ones that need attention (conflicts, failing checks or changes requested) and drafts; issues you created or are assigned; and, when a list spans more than one, a single repository. Only filters with something in them are offered, each with how many. The choice is kept for each list until Orbit quits.
  - The action menu also copies an item's link, its reference such as `acme/web#482`, and a pull request's branch name.

Their ids (`files`, `1password`, `ente-auth`, `weather`, `speed-test`, `github`) are taken: a folder extension with one of them is listed as broken and never runs.

## Aliases and hotkeys

The Extensions table shows each extension with what it does and its switch. Each one opens like an accordion: click anywhere along its row, or its chevron, and its commands slide out under it, each in a row of its own with its alias and hotkey; click again to close it. Turning an extension on opens it, and turning it off closes it. One that's off can still be opened to see what it has, with its commands greyed out until it's on. A command's alias and hotkey can be set while it's off, and setting one turns the extension on, or says which tool it's waiting for. Click a command's Alias or Hotkey to change it, or use the keyboard: Left and Right move between the parts of a row (an extension's chevron and switch, or a command's alias and hotkey), and Enter or Space changes the one that's lit. Each command can have:

- **An alias.** For a command that opens a list, and for Everything, the alias is also its keyword: typing it and a space goes straight in, so `ghi bug` searches your issues for "bug". Clearing it leaves the command with no keyword.
- **A hotkey**, which opens the command from anywhere.

The filter above the table finds an extension by its name or what it does, and a command by its name or alias, opening the extension it's in. The Search page lists the same commands, with the same aliases and hotkeys, in a group for each extension that's on, and its **Shown** switch leaves one out of the launcher's search. Under the table, a card for each extension holds what it needs, such as a tool to install, and says what it sends.

Ctrl+K on a command in the launcher sets the same alias and hotkey without opening Settings.


## Installing one

Settings → Extensions → **Add extension…**, then pick the extension's folder. Orbit copies it into its own `extensions` folder (`%LOCALAPPDATA%\Orbit\extensions`, or `Orbit settings\extensions` beside a portable copy) and shows what it will run before you turn it on. An extension runs with your permissions, like any program you start, so add only ones you trust.

A folder copied straight into the `extensions` folder shows up too, switched off.

## The manifest

```json
{
  "id": "windows-settings",
  "name": "Windows Settings",
  "description": "Open any page of Windows Settings",
  "author": "Beroris",
  "version": "1.0.0",
  "run": ["main.ps1"],
  "commands": [
    {
      "id": "search",
      "title": "Search Windows Settings",
      "subtitle": "Bluetooth, display, sound and the rest",
      "keyword": "ws",
      "keywords": "control panel preferences",
      "placeholder": "Search Settings pages…"
    }
  ]
}
```

| Field | |
|---|---|
| `id` | Lower case letters, digits, dots, dashes and underscores. Settings refer to the extension by it. |
| `name` | Shown in Settings. Up to 64 characters. |
| `description`, `author`, `version` | Shown in Settings. Optional. |
| `icon` | A PNG or ICO in the folder. Optional. |
| `run` | The program and its arguments. A script named first runs in the program made for it: `.ps1` in Windows PowerShell (with `-NoProfile -ExecutionPolicy Bypass`), `.py` in Python, `.js` in Node, `.cmd` and `.bat` in cmd. A bare first word, such as `node` or `main.exe`, is looked for in the extension's folder first, then on the PATH. The program starts in the extension's folder, with `ORBIT_EXTENSION_DIR` and `ORBIT_EXTENSION_DATA`, a folder it may keep things in, in its environment. |
| `commands` | One to 32 commands, each a row in the launcher. |
| `protocol` | The protocol version it was written for. Optional; this page describes 1. |

Each command:

| Field | |
|---|---|
| `id` | Unique within the extension. Orbit sends it with every message. |
| `title`, `subtitle` | The row in the launcher. |
| `keyword` | Optional. Typing it and a space goes straight into the command, so `ws blue` searches for "blue". It can be changed in Settings. |
| `keywords` | More words to find the command by. |
| `mode` | `"list"` (the default) opens a list the extension fills as you type. `"run"` runs once with no list. |
| `placeholder` | What the search box says in the command's list. |

## Messages

Orbit starts the program the first time one of its commands is used, and keeps it running while the launcher is in use, so it can keep things in memory between searches. It's stopped a few minutes after the launcher was last used, when it's switched off, and when Orbit quits. If it exits on its own, Orbit starts it again next time.

Every message is one line of UTF-8 JSON. Anything else the program prints is ignored, so a stray debug line doesn't break it, but keep logging on standard error.

### From Orbit

```json
{"type":"hello","orbit":"1.1.0","protocol":1}
{"type":"search","id":12,"command":"search","query":"blue"}
{"type":"run","id":13,"command":"night"}
{"type":"action","id":14,"command":"search","item":"bt","action":"pair","query":"blue"}
```

- `hello` comes once, first.
- `search` comes when a list command opens, with an empty query, and again each time the text changes. Answer each with `items`, using the same `id`. Orbit shows only the answer to the newest search, so a slow answer to an old one is harmless. If the list offered filters and one other than the first is chosen, `search` and `action` carry its id as `filter`, such as `"filter":"mine"`.
- `run` comes when a `run` command is chosen. Answer with `done`.
- `action` comes when an item's `callback` action is chosen. Answer with `done`, or with `items` to replace the list.

### To Orbit

```json
{"type":"items","id":12,"items":[ ... ],"empty":"No pages match"}
{"type":"done","id":13,"message":"Night light is on","close":true}
{"type":"error","id":12,"message":"Couldn't reach the server"}
{"type":"notify","message":"Signed in"}
```

- `items` replaces the list. `empty` is shown when there are none. Up to 200 items. It may also offer `filters`, below.
- `done` ends a `run` or a callback. `message` is shown; `close` hides the launcher.
- `error` is shown in place of the list.
- `notify` shows a message at any time.

An item:

```json
{
  "id": "bt",
  "title": "Bluetooth & devices",
  "subtitle": "ms-settings:bluetooth",
  "accessory": "Devices",
  "icon": "icons/bluetooth.png",
  "actions": [
    {"title": "Open", "type": "open", "target": "ms-settings:bluetooth"},
    {"title": "Copy link", "type": "copy", "text": "ms-settings:bluetooth", "icon": "link"}
  ]
}
```

`icon` is a picture in the extension's folder, or the full path of any file or folder, which shows that file's own icon. The first action runs on Enter, the second on Ctrl+Enter, and all of them are in the Ctrl+K menu. An action's optional `shortcut`, such as `"Ctrl+R"`, is shown beside it there, and its optional `icon` names its symbol in that menu: `open`, `globe`, `link`, `hash`, `branch`, `terminal`, `copy`, `paste`, `text`, `folder`, `file`, `image`, `note`, `edit`, `trash`, `check`, `refresh`, `search`, `star`, `pin`, `tag`, `person`, `key`, `clock`, `download`, `play`, `add`, `eye-off`, `settings` or `swap`. Without one, or with another name, the kind of action chooses: an arrow out of a box to open, a folder to reveal, copy, paste, play for a callback and search for `set_query`.

An item can show up to three **tags** before its `accessory`, each a word or two in a coloured pill, such as a pull request's state. `color` (or `colour`) is `green`, `red`, `orange`, `purple`, `blue` or `grey`, and anything else is grey. Tags are up to 24 characters, and ones that don't fit beside the title are left off, the last first. The item's own `color` colours its symbol, when it has no picture of its own. Items with a `section` are grouped under it, with its name as a heading wherever it changes, so send them in order:

```json
{"id": "482", "title": "Move settings to the new layout", "section": "Open", "color": "green",
 "tags": [{"text": "Conflicts", "color": "red"}, {"text": "Draft", "color": "grey"}]}
```

An item can show up to three **meters** on the right in place of `accessory`, each a short label over a bar, for how full something is:

```json
"meters": [
  {"label": "5h 32%", "value": 0.32},
  {"label": "Week 71%", "value": 0.71}
]
```

`value` runs from 0 to 1. Anything outside is brought inside, and a meter with no `value` is left out. The bar is drawn in the accent colour, amber from 0.7 and red from 0.9. Labels are up to 24 characters, and short ones read best.

An `items` answer can offer **filters**, up to 60, shown in a drop-down at the right of the search box (Ctrl+P opens it). The first is everything, and is what's chosen until another is. `subtitle` is shown beside a filter's title, such as how many it has. Keep offering the same filters while one is chosen, so the others stay on offer; with fewer than two, there's no drop-down.

```json
{"type": "items", "id": 12, "items": [ ... ],
 "filters": [{"id": "", "title": "All pages"}, {"id": "mine", "title": "Mine", "subtitle": "4"}]}
```

| Action `type` | Fields | Does |
|---|---|---|
| `open` | `target` | Opens a web address, an app's link such as `ms-settings:` or `spotify:`, or a file or folder by its full path. `javascript:`, `file:`, `data:` and other schemes that can run code are refused. |
| `reveal` | `path` | Shows a file or folder in File Explorer. |
| `copy` | `text` | Copies the text. |
| `paste` | `text` | Types the text into the window Orbit was opened over. |
| `callback` | `id` | Sends `action` back to the extension. |
| `set_query` | `query` | Puts the text in the search box. |

## A whole extension in PowerShell

`examples/extensions/windows-settings` in Orbit's source is a complete one, with about 60 pages of Windows Settings in `pages.json`. Add its folder from Settings → Extensions to try it, then type `ws` and a space. The heart of it:

```powershell
# Read and write UTF-8, whatever code page the console has.
$utf8 = [Text.UTF8Encoding]::new($false)
$in = [IO.StreamReader]::new([Console]::OpenStandardInput(), $utf8)
$out = [IO.StreamWriter]::new([Console]::OpenStandardOutput(), $utf8)
$out.AutoFlush = $true

$pages = Get-Content -Raw -Encoding UTF8 "$PSScriptRoot\pages.json" | ConvertFrom-Json
while ($null -ne ($line = $in.ReadLine())) {
    $message = $line | ConvertFrom-Json
    if ($message.type -ne 'search') { continue }
    $found = @($pages | Where-Object { $_.name.ToLower().Contains("$($message.query)".ToLower()) } | ForEach-Object {
        @{ id = $_.uri; title = $_.name; subtitle = $_.uri
           actions = @(@{ type = 'open'; target = $_.uri }, @{ type = 'copy'; title = 'Copy Link'; text = $_.uri }) }
    })
    $out.WriteLine((@{ type = 'items'; id = $message.id; items = $found } | ConvertTo-Json -Depth 6 -Compress))
}
```

Windows PowerShell reads standard input in the console's code page, not UTF-8, unless told otherwise, so read and write through UTF-8 streams as above. Without them, a search for "café" arrives as something else. Python gets `PYTHONIOENCODING=utf-8` from Orbit, and Node reads UTF-8 already.

## Everything

Orbit's own file search asks [Everything](https://www.voidtools.com/), free from voidtools, which keeps an index of every file name on your drives. Orbit asks it directly, the way voidtools' own SDK does, and ships no part of it.

- Type `everything` and press Enter, then type. Give it an alias in the Extensions table, such as `ev`, and that alias and a space search at once: `ev report`. Everything's search syntax works: `ext:pdf invoice`, `*.psd`, `"exact name"`.
- A few matching files also show under the other results when you search in the launcher. Everything's card in Settings → Extensions turns that off and sets which files come first: most recently changed, most opened, or by name.
- Enter opens a file. Ctrl+Enter shows it in File Explorer. Ctrl+K has the rest: copy the path, copy the file itself, open with another app, run a program as administrator.
- If Everything isn't installed, the Everything command offers to install it with Windows Package Manager. Its installer asks Windows for permission once, because Everything reads the drives' own file tables. If it's installed but not running, it offers to start it.
