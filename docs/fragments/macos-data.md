# macOS data

## Location

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

## Export

Settings → Data → Export, or the launcher command, writes a new folder `~/Documents/Orbit Export YYYY-MM-DD HHMM` holding `orbit-export.json`, a copy of `settings.json`, and a `notes` folder with one Markdown file per note. The folder opens in Finder afterwards. Clipboard history isn't exported. `docs/DATA_PORTABILITY.md` describes the format and it's shared with Windows.

`feature_checks.sh` verifies that export writes the bundle into a test folder. The default `~/Documents` location was not run.

## Import

Settings → Data → Import a file reads an `orbit-export.json`, adds notes and snippets, skips duplicates, and leaves settings alone. If the event tap is running, Orbit refreshes its trigger list afterwards. `feature_checks.sh` verifies an import through a test path. The file picker and the refresh are not tested.

Moving between macOS and Windows was not tested.

## Delete All Orbit Data

Settings → Data, or the launcher command `cmd:delete-data`, asks for confirmation. Cancel is the first row. If you confirm, Orbit:

1. Turns the login item off if it's on.
2. Stops writing settings.
3. Quits through the normal exit path, which removes the Caps Lock mapping.
4. Closes the databases and removes these names: the files and folders in the table above other than `hidutil-journal`.
5. Removes the data folder if that left it empty.

It removes by name and never empties the folder. It keeps `hidutil-journal` if a mapping couldn't be removed, so the next start can try again. Exports in `~/Documents` stay.

`feature_checks.sh` verifies this in a private folder: Cancel removes nothing, confirming exits the process with status 0, every named file is gone, and an unrelated file stays. Removing the folder when it's left empty, and step 1, were not run.

## Uninstall

1. Quit Orbit from the menu bar icon. This removes any Caps Lock mapping. If you use Delete All Orbit Data instead, Orbit also quits.
2. If you didn't delete the data, remove `~/Library/Application Support/Orbit`. Keep or remove `~/Documents/Orbit Export *`.
3. Delete `Orbit.app`.
4. In System Settings → Privacy & Security, remove Orbit from Accessibility and Input Monitoring. In Automation, remove it if listed.
5. In System Settings → General → Login Items, check that Orbit is gone.

These steps were read from the code and not run as a whole on this machine. The tests didn't change TCC or register a login item.

If a crash left the mapping in place and you don't plan to start Orbit again, a reboot clears it. So does running `hidutil property --set '{"UserKeyMapping":[]}'`, but that also clears other tools' mappings.
