# Data portability

The file formats Orbit reads and writes, their versions, what migration exists, and how to back up and roll back. This describes the code in this checkout (Windows host). Sources: `crates/orbit-data/src/{export,lib}.rs`, `crates/orbit-core/src/settings.rs`, `platforms/windows/src/{worker,app,settings_win}.rs`.

Related: [PRIVACY.md](PRIVACY.md) (what the files contain), [ARCHITECTURE.md](ARCHITECTURE.md#data-layout) (where they live).

## Formats and versions

| Artifact | Format | Version marker | Current version | Where |
|---|---|---|---|---|
| Settings | JSON | `schema_version` field | 1 | `settings.json` in the data folder |
| Export bundle | JSON | `format` = `"orbit-export"` and `version` | 1 | `orbit-export.json` in an export folder |
| Notes as text | Markdown files, one per note | None | n/a | `notes\` in an export folder |
| Main database | SQLite, WAL mode | `PRAGMA user_version` | 1 | `orbit.db` |
| Clipboard database | SQLite, WAL mode | `PRAGMA user_version` | 1 | `clipboard.db` |

Each of the two databases and the settings file has its own version number. They are independent.

## Settings file (`settings.json`)

One JSON document written with pretty formatting. It is written to a temporary file and renamed over the old one, so a crash leaves either the old or the new file, not a partial one (`util::write_atomic`).

### Reading rules (`Settings::parse`)

| Case | Result |
|---|---|
| File missing | Defaults. |
| Missing sections or fields | Defaults for those (`#[serde(default)]` on the struct and sections). |
| Unknown fields | Ignored. Older and newer files can be read by each other. Unknown fields are lost the next time Orbit saves. |
| `schema_version` lower than or equal to the current one | Loaded, `migrate()` runs, `schema_version` is set to the current value. `migrate()` has no steps yet. |
| `schema_version` higher than the current one | Loaded best effort, and Orbit treats settings as read-only for the session: changes are not saved, and a notification says so. |
| Invalid JSON, or a field with the wrong type | The whole file is treated as damaged: it is renamed `settings.damaged.json` (replacing an earlier one), defaults are used and a notification says so. There is no partial recovery. |

### Shape

Top-level keys: `schema_version`, `launcher`, `hyper`, `shortcuts`, `clipboard`, `snippets`, `links`, `custom_commands`, `aliases`, `pinned`, `hidden`, `emoji`, `appearance`, `low_resource_mode`, `launch_at_login`, `onboarded`.

| Section | Fields (default) |
|---|---|
| `launcher` | `hotkey` (null = automatic: `Alt+Space`, then fallbacks; see [SHORTCUT_SEMANTICS.md](SHORTCUT_SEMANTICS.md#4-conflicts)), `preserve_query` (false), `max_visible_rows` (8, clamped to 3–12 at use), `focus_running_app` (true), `hotkey_toggles_app` (true), `keyboard` (`"standard"`, `"vim_keys"`, `"vim"`), `favorites_only` (false), `glass` (true), `fit_results` (true), `excluded_apps` (empty), `pass_through_in_games` (true), `screen` (`"active"`, `"pointer"`, `"main"` for the screen Windows marks as main, or `{ "specific": { "id", "name" } }` with Windows' path for the monitor and its name; a value this version doesn't know reads as `"active"` instead of damaging the file) |
| `emoji` | `pinned` (empty), `recent` (empty, newest first and capped): the emoji themselves, not when they were used |
| `mac_feel` | `enabled` (false), `text_keys` (true), `app_keys` (true), `excluded_apps` (empty) |
| `integrations` | `one_password` (false), `ente_auth` (false) |
| `hyper` | `enabled` (false), `trigger` (`"CapsLock"`), `mode` (`"internal"`, `"synthetic"`, `"external"`), `tap` (`"escape"`, `"original"`, `"none"`), `tap_timeout_ms` (250), `bindings`, `excluded_apps` (empty), `pass_through_in_games` (true) |
| `shortcuts`, `hyper.bindings` | Lists of `{ "chord" or "key", "action", "enabled" (true) }` |
| `clipboard` | `enabled` (false), `persist` (true), `capture_images` (true), `max_items` (500), `retention_days` (30), `max_item_bytes` (262144), `excluded_apps` |
| `snippets` | `expansion_enabled` (false), `excluded_apps` |
| `links` | List of `{ id, name, url, alias }`; `{query}` in the URL makes it a search template. One default link is created (Google search, alias `g`). |
| `custom_commands` | List of `{ id, name, program, arguments, working_dir, alias }` |
| `aliases` | Object: item key → alias text |
| `pinned`, `hidden` | Lists of item keys |
| `appearance` | `"system"`, `"light"`, `"dark"` |

Chords are strings: `"Alt+Space"`, `"Ctrl+Alt+K"`, `"Hyper+F13"`. The saved file prints the Windows or Command modifier as `Super`; the parser also accepts `Win`, `Windows`, `Cmd`, `Command`, `Meta`. Key names are logical names, not Windows key codes ([SHORTCUT_SEMANTICS.md](SHORTCUT_SEMANTICS.md)).

Actions are objects tagged by `kind` (snake case): `focus_or_launch_app` (`id`, `name`), `launch_app`, `open_path` (`path`), `url` (`url`), `builtin` (`command`), `custom` (`id`), `open_note` (`id`), `insert_snippet` (`id`).

Example (shape taken from the serde attributes in `action.rs` and `settings.rs`):

```json
{
  "schema_version": 1,
  "launcher": { "hotkey": "Alt+Space" },
  "hyper": {
    "enabled": true,
    "bindings": [
      { "key": "C", "action": { "kind": "builtin", "command": "clipboard_history" }, "enabled": true },
      { "key": "G", "action": { "kind": "url", "url": "https://github.com" } }
    ]
  }
}
```

Things that do not travel well between machines: `focus_or_launch_app` ids come from the local app index; `open_path` and `custom_commands` hold local paths; `open_note` and `insert_snippet` hold database ids, which are not preserved by export and import; exclusion lists hold Windows executable names.

Moving settings to another Windows PC: copy the exported `settings.json`, then use Settings → Data → Import → "Choose file…" on the other PC, or copy the file into its data folder while Orbit is quit.

## Export

Run it from Settings → Data → "Export now", or the launcher command "Export Orbit Data". It runs on the data thread and writes a new folder:

```
%USERPROFILE%\Documents\Orbit Export YYYY-MM-DD HHMM\
    settings.json          copy of the current settings
    orbit-export.json      the bundle (below)
    notes\00001 <title>.md one Markdown file per note
```

The location comes from the Documents known folder, so a redirected Documents folder (for example by OneDrive) is honoured. If it can't be found, the temp folder is used. When the export finishes, Orbit opens the folder. A later export never reuses a folder name, so it can't overwrite an earlier one.

There is no ZIP archive. Settings → Data says "one JSON file", which understates it: an export is a folder of three kinds of file.

### Bundle (`orbit-export.json`, `format` `"orbit-export"`, `version` 1)

```json
{
  "format": "orbit-export",
  "version": 1,
  "exported_at": 1762380000,
  "notes": [
    { "body": "Title\nbody text", "created_at": 1762300000, "updated_at": 1762300500, "pinned": false }
  ],
  "snippets": [
    { "name": "Signature", "trigger": ";sig", "content": "Regards,\nSean", "description": "", "category": "", "favourite": false }
  ]
}
```

Timestamps are Unix seconds. `body`, `created_at`, `updated_at`, `name` and `content` are required; the other fields default to false or empty. Unknown fields are ignored on import.

| Included | Not included |
|---|---|
| Notes that are not in the trash (including pinned, with the pinned flag) | Trashed notes, note ids, note titles (rebuilt from the first line), the archived flag |
| All snippets with name, trigger, content, description, category, favourite | Snippet ids, the enabled flag, created and updated times |
| | Clipboard history (never, by design; there is a test for it), usage counts, quick links, custom commands, aliases, shortcuts (these are in `settings.json` in the same folder) |

### Note files (`notes\*.md`)

The file content is the note body, unchanged. The name is a five-digit counter and the note title, where control characters and `< > : " / \ | ? *` become `_`, the title is cut to 60 characters and trailing dots and spaces are removed. A name cannot contain a path separator (test `file_names_cannot_escape_the_folder`). Timestamps and the pinned flag are not in the Markdown files; they are only in the bundle.

## Import

Settings → Data → Import, or the launcher command "Import from Another Launcher", opens the Import page of Orbit Setup. The same page is the first step after installing. It lists the other launchers Orbit finds on this PC, each with what it would bring, and has a "Choose file…" row for an export.

Nothing is changed in the other launcher. Its files are only read: JSON and INI files as text, and Wox 2's database read-only, so Wox can stay open. Nothing read is run or opened. Orbit only adds to what it has, except when the chosen file is Orbit's own `settings.json` (below).

### What each launcher brings

| Launcher | Where Orbit looks | What comes over |
|---|---|---|
| Raycast (Windows or Mac) | Only an export, as Raycast keeps its own data encrypted. Orbit offers the newest `.rayconfig` in Downloads, Documents or on the desktop, or one you choose. | Quicklinks with their aliases and the app they open with, snippets with their keywords, notes, the launcher hotkey |
| Flow Launcher | `%APPDATA%\FlowLauncher\Settings`, or `UserData\Settings` beside a portable copy | Web searches, Explorer's quick-access folders, the hotkey |
| Wox 2 | `%USERPROFILE%\.wox\wox-user\wox.db`, or the folder named in `.wox\.userdata.location` | Web searches, the hotkey |
| Wox 1 | `%APPDATA%\Wox\Settings` | Web searches, the hotkey |
| PowerToys Run | `%LOCALAPPDATA%\Microsoft\PowerToys\PowerToys Run\settings.json` | The hotkey |
| Command Palette | `%LOCALAPPDATA%\Packages\Microsoft.CommandPalette_8wekyb3d8bbwe\LocalState` | Bookmarks, the hotkey |
| Keypirinha | `%APPDATA%\Keypirinha\User` (`Keypirinha.ini`, `WebSearch.ini`) | Web searches, the hotkey |

A launcher's hotkey isn't taken over by itself. The Shortcut page after Import offers it, and says if that launcher is running and still holds it.

What Orbit has no place for is named on the page and left behind: for example extensions from Raycast's store, aliases and hotkeys on Raycast's own commands, and Flow Launcher's query shortcuts and plugin hotkeys.

### Raycast's exports

"Choose file…" reads:

- Settings & Data exports (`.rayconfig`) from Raycast 2 for Windows and Mac, and from Raycast for Mac before it. Raycast 2 encrypts them with a passphrase you choose (scrypt and AES-256-GCM), so the page asks for it. Older Mac exports use AES-256-CBC with a password that is `12345678` unless it was changed; Orbit tries that first and asks only if it fails. Early Raycast 2 builds wrapped the export in gzip, which is read too.
- The JSON files from Raycast's Export Snippets and Export Quicklinks.

The passphrase is used once to open the file and is not kept. A wrong one says so and lets you try again.

Placeholders become Orbit's where Orbit has the same one: `{query}` and `{argument name="city" default="London"}` (as `{city=London}`) in quicklinks, and `{clipboard}`, `{date}`, `{time}`, `{datetime}` and `{cursor}` in snippets. Others, such as `{selection}`, stay as typed, and the page says how many snippets have them.

### Other launchers' files

"Choose file…" also takes Flow Launcher's `Settings.json` and its Web Searches and Explorer plugin settings, and Command Palette's `bookmarks.json`, for example copied from another PC.

### How things are added

- Quicklinks go into `settings.json`. One with the same address as a link Orbit has (ignoring case) is skipped. One whose alias Orbit already uses comes over without the alias. One Orbit can't open on Windows, such as a Mac's `~/Downloads`, is skipped. A link that opened with a particular app opens with that app if Orbit finds one by the same name.
- Snippets and notes go through the same import as an Orbit export (below), in one transaction. Snippets are put in a category named after the launcher. A keyword another snippet already uses is dropped, and the snippet kept.
- The page then says what came over, what was already there, and what was left behind.

### Orbit's own files

"Choose file…" takes `orbit-export.json` bundles, which add notes and snippets, and a file named `settings.json` with Orbit's settings in it, which replaces your settings. It does not import Markdown files or a ZIP.

A `settings.json` must be under 4 MB and must load cleanly. Otherwise nothing changes and Orbit says so. Bundles follow these rules:

Rules (`Store::import_bundle`, `worker::import`):

- The file size is checked before it is read; larger than 256 MiB is refused. The whole file is then read into memory and parsed before anything is written.
- `format` must be `"orbit-export"`, `version` must not be greater than 1, and the JSON must parse; otherwise nothing is changed and an error is shown (`NewerFormat`, `Malformed`).
- All inserts happen in one transaction (`BEGIN IMMEDIATE`). It applies completely or not at all; on any storage error it rolls back.
- Duplicates are skipped: a note is skipped if a non-trashed note with identical body exists; a snippet is skipped if one with the same name and content exists. Importing the same file twice adds nothing the second time (test `round_trips_and_is_idempotent`). Duplicates within one file are skipped the same way.
- Notes keep their created and updated times and pinned flag. Snippets get the import time as created and updated time. Imported snippets are always enabled.
- Text is stored as plain data. Nothing in a bundle is executed, opened or used as a file path.
- The result message gives counts of added and skipped records.

Not done on import of an Orbit bundle:

- Snippet triggers are not validated. A trigger that is too short or contains whitespace is stored but ignored by the matcher. A trigger already used by another snippet is stored; the older snippet wins.
- Expansion triggers do refresh after an import, so imported triggers work at once.

## Databases

### Migration mechanism (`orbit-data/src/lib.rs`)

Each database has an ordered list of SQL migration steps (`MAIN_MIGRATIONS`, `CLIP_MIGRATIONS`). On open, Orbit reads `PRAGMA user_version`, runs every step after that version, each in its own transaction that also sets the new `user_version`, and stops there. Migration runs on the `orbit-data` thread, never on the UI or hook thread. Both lists currently have one step, so every existing database is at version 1 and no migration has ever had to change existing data.

A database whose `user_version` is **higher** than the code knows is refused, not modified (test `newer_database_is_refused_not_damaged`). The data thread then reports "Could not open Orbit's database". Launching apps still works; notes, snippets and clipboard history are unavailable. If either of the two databases is newer, the whole store is unavailable, because they are opened together.

### Version 1 schema

`orbit.db`:

| Table | Columns |
|---|---|
| `notes` | `id`, `title`, `body`, `created_at`, `updated_at`, `pinned`, `archived`, `deleted_at` |
| `snippets` | `id`, `name`, `trigger`, `content`, `description`, `category`, `favourite`, `enabled`, `created_at`, `updated_at` |
| `command_usage` | `key` (primary key), `count`, `last_used_at` |
| `notes_fts`, `snippets_fts` | FTS5 external-content indexes kept current by triggers |

`clipboard.db`:

| Table | Columns |
|---|---|
| `clip_items` | `id`, `kind`, `text`, `preview`, `hash`, `size`, `source_app`, `created_at`, `pinned`, `blob_path`, `width`, `height` |
| `clip_fts` | FTS5 external-content index of `text` |

Image bytes are not in the database; `blob_path` names a file in `clipboard-images\`. The databases are standard SQLite and can be opened with any SQLite tool while Orbit is not running. Orbit's own use sets WAL mode, so a copy of `orbit.db` alone is complete only if the `-wal` file was checkpointed into it. A clean quit checkpoints `orbit.db` (`PRAGMA wal_checkpoint(TRUNCATE)`), but copy the whole folder to be safe.

## Backups

Before a migration upgrades an existing database, Orbit writes a copy beside it as `<database>.before-v<N>.bak` using `VACUUM INTO`. It also keeps `settings.damaged.json` when a settings file cannot be parsed. Nothing else is backed up automatically.

To back up:

1. **Export** (above) gives portable notes, snippets and settings. It excludes clipboard history, usage counts, ids and trashed notes.
2. **Full copy**: quit Orbit from the tray menu, then copy the data folder (`%LOCALAPPDATA%\Orbit`, or your `--data-dir`) including `-wal` and `-shm` files. This also captures clipboard history, usage counts and trash.

Caches that can be left out of a backup: `apps.json` and `icons\` (rebuilt by the indexer if missing). If you drop `clipboard-images\` but keep `clipboard.db`, image clips remain in the list and report "That image is no longer available".

Back-ups of clipboard history and exports are plain, unencrypted copies. See [PRIVACY.md](PRIVACY.md).

## Rollback

There is no downgrade path built in. What the code guarantees:

- A newer settings file or a newer database is never overwritten by an older Orbit. Settings load read-only, and the database is refused unchanged.
- A migration of existing data leaves `<database>.before-v<N>.bak` beside the database. Only schema version 1 exists so far, so no upgrade has happened on real data.

To go back to an older version after a newer one changed your data:

1. Quit Orbit.
2. Replace the database with its `.before-v<N>.bak` copy, or replace the data folder with a full copy you made before upgrading, or delete `orbit.db`, `orbit.db-wal`, `orbit.db-shm` and import an export.
3. Start the older Orbit.

Without a backup made before the upgrade, you can only keep using the newer version or start from an export.

## Known gaps against the brief

- No ZIP archive, and no Markdown or other-format import. Only the JSON bundle can be imported.
- Clipboard history cannot be exported at all, so the brief's opt-in clipboard export with a warning is not implemented.
- Clipboard history cannot be exported at all, so the brief's opt-in clipboard export with a warning is not implemented.
- "Reset All Shortcuts" and "Delete All Orbit Data" exist, in the launcher and on Settings → Data.
- The pre-migration copy is the only rollback tool. There's no rollback command.
- Only schema version 1 exists. The brief's "migration test from older versions" has nothing to test yet; the code tests that reopening keeps data, that migrations are idempotent, and that newer databases and newer bundles are refused.
- Signed updates, and rollback from a failed update, don't exist. See ROADMAP.md.
- Import does not carry the enabled flag, ids or snippet timestamps, and export does not export them.
- Plain-text snippet import and export aren't implemented.

## macOS

### Location

Everything lives in `~/Library/Application Support/Orbit`, or the folder given with `--data-dir`.

| File | Contents |
|---|---|
| `settings.json` | Settings. A damaged file is renamed `settings.damaged.json` and defaults are used, with a message. |
| `orbit.db` with `-wal` and `-shm` | Notes, snippets, usage |
| `clipboard.db` with `-wal` and `-shm` | Clipboard history, when history is on and kept after exit |
| `orbit.db.before-v<N>.bak`, `clipboard.db.before-v<N>.bak` | The copy taken before a database migration |
| `clipboard-images/` | Clipboard images, when images are on |
| `icons/` | Cached app icons as raw 56 by 56 pixels. Each app scan removes icons no indexed app leads to. |
| `apps.json` | The app index |
| `instance.lock` | One instance per data folder. The kernel releases the lock when the process ends. |
| `hidutil-journal` | Present only while Orbit's Caps Lock mapping may be applied |
| `crash.log` | Panic locations, if there were any |

No test on this machine touched this default folder. All runs used private data folders.

### Export

Settings → Data → Export, or the launcher command, writes a new folder `~/Documents/Orbit Export YYYY-MM-DD HHMM` holding `orbit-export.json`, a copy of `settings.json`, and a `notes` folder with one Markdown file per note. The folder opens in Finder afterwards. Clipboard history isn't exported. `docs/DATA_PORTABILITY.md` describes the format and it's shared with Windows.

`feature_checks.sh` verifies that export writes the bundle into a test folder. The default `~/Documents` location was not run.

### Import

Settings → Data → Import a file reads an `orbit-export.json`, adds notes and snippets, skips duplicates, and leaves settings alone. If the event tap is running, Orbit refreshes its trigger list afterwards. `feature_checks.sh` verifies an import through a test path. The file picker and the refresh are not tested.

Moving between macOS and Windows was not tested.

### Delete All Orbit Data

Settings → Data, or the launcher command `cmd:delete-data`, asks for confirmation. Cancel is the first row. If you confirm, Orbit:

1. Turns the login item off if it's on.
2. Stops writing settings.
3. Quits through the normal exit path, which removes the Caps Lock mapping.
4. Closes the databases and removes these names: the files and folders in the table above other than `hidutil-journal`.
5. Removes the data folder if that left it empty.

It removes by name and never empties the folder. It keeps `hidutil-journal` if a mapping couldn't be removed, so the next start can try again. Exports in `~/Documents` stay.

`feature_checks.sh` verifies this in a private folder: Cancel removes nothing, confirming exits the process with status 0, every named file is gone, and an unrelated file stays. Removing the folder when it's left empty, and step 1, were not run.

### Uninstall

1. Quit Orbit from the menu bar icon. This removes any Caps Lock mapping. If you use Delete All Orbit Data instead, Orbit also quits.
2. If you didn't delete the data, remove `~/Library/Application Support/Orbit`. Keep or remove `~/Documents/Orbit Export *`.
3. Delete `Orbit.app`.
4. In System Settings → Privacy & Security, remove Orbit from Accessibility and Input Monitoring. In Automation, remove it if listed.
5. In System Settings → General → Login Items, check that Orbit is gone.

These steps were read from the code and not run as a whole on this machine. The tests didn't change TCC or register a login item.

If a crash left the mapping in place and you don't plan to start Orbit again, a reboot clears it. So does running `hidutil property --set '{"UserKeyMapping":[]}'`, but that also clears other tools' mappings.

