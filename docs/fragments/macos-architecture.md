# macOS architecture

Sources: `platforms/macos/Sources/Orbit`, `crates/orbit-ffi`. Tested on one Mac: Mac16,8 (M4 Pro, 24 GB), macOS 27.2, built-in Retina display, AC power. The package declares macOS 14 as its minimum, and nothing older than 27.2 has been run.

## Host structure

Orbit for macOS is one process with no helper and no web runtime. Swift and AppKit link the Rust crates statically through `crates/orbit-ffi`.

| File | What it does |
|---|---|
| `main.swift` | Parses flags, takes the per-folder instance lock, starts AppKit |
| `AppController.swift` | Launcher state and behaviour, the port of `platforms/windows/src/app.rs` |
| `LauncherPanel.swift` | The non-activating panel, and `ResultsView`, which draws rows and footer |
| `Rows.swift` | Scopes, rows and actions, the port of `rows.rs` and `view.rs` |
| `Hotkeys.swift` | Carbon hotkeys and key names |
| `HyperTap.swift` | The event tap, permissions, `hidutil` |
| `Clipboard.swift` | Pasteboard watcher, skip rules, typed insertion |
| `AppIndex.swift` | App scan, FSEvents watcher, icon cache |
| `Editors.swift`, `SettingsWindow.swift` | Note and snippet windows, Settings |
| `Metrics.swift`, `Bench.swift` | Timing lines, the benchmark's Darwin notifications, `--self-test` |

## Threads

| Thread | Runs |
|---|---|
| Main | All AppKit, all launcher state, and `OrbitEngine` |
| Data thread, started by `orbit_worker_start` | SQLite and blocking file work. Requests from the host never wait on disk. |
| `orbit-tap` | The event tap callback, only while HyperKey Internal or Synthetic or expansion is on |
| `orbit.index`, `orbit.icons`, `orbit.hidutil` dispatch queues | App scans and icon pruning, icon loading, `hidutil` runs |

The e2e driver counted 15 threads just after start and 4 after 60 s idle with the tap off.

## FFI ownership

`crates/orbit-ffi/include/orbit.h` is the contract.

- A returned `char *` belongs to the caller, who frees it with `orbit_string_free`.
- A returned `const char *` or `const OrbitRow *` is borrowed, for the lifetime stated on that function. `Row(_:)` in Swift copies what it keeps.
- `*_new` and `*_start` handles are freed once with the matching `*_free`.
- `OrbitEngine` is used from one thread, the main thread here. `OrbitHyper` and `OrbitTriggers` are used from the tap thread. Their key functions allocate nothing and do no I/O.
- Worker requests may come from any thread. `orbit_wake_fn` runs on the data thread and only schedules a drain on the main thread.
- No panic crosses the boundary. A panic yields that call's documented fallback and writes its location to `crash.log`.

## How the launcher draws

`ResultsView` draws each frame on the CPU into a new IOSurface and sets it as the layer's `contents`. The layer rounds the corners. On show, the frame is drawn before the panel is ordered front. On hide, the layer lets go of the surface, about 5 MB at 2x. The text field keeps its own AppKit layer.

The reason is memory. When `ResultsView` drew through AppKit's backing store, a frame that replaced visible rows made the process allocate graphics memory, charged to `phys_footprint`. A `footprint -p` capture during that spike showed 102 MB of dirty `Owned physical footprint (unmapped) (graphics)` in 47 regions, 19 MB of IOSurface and 9 MB of IOAccelerator. About 1.2 s after a frame, it became reclaimable and left the footprint. Typing kept drawing, so it stayed.

### What was measured

Each row is one process with a private data folder and stub rows, with no text input, driven by notification. Peak is the highest `proc_pid_rusage` `ri_phys_footprint` sampled every 50 ms after the rows were set. These are from a throwaway experiment build, not from the shipped code.

| Configuration | Peak MB |
|---|---:|
| Everything on, 20 glyph rows | 159 |
| Everything on, 20 app rows | 168 |
| One row, or 20 short rows | 155, 160 |
| App icons off | 157 |
| Glyph images off | 153 |
| Row and footer text off | 157 |
| Glyph images and text off | 150 |
| Regular instead of semibold title | 158 |
| CoreText `CTLineDraw` instead of `NSString.draw` | 160 |
| Selection fill off | 158 |
| Scrollbar off | 160 |
| Rounded clip off | 153 |
| `drawsAsynchronously` set off, or set on | 160, 160 |
| Field removed from the view | 157 |
| Only text on | 151 |
| Only glyph images on | 153 |
| Only the scrollbar on | 149 |
| Only the selection fill on | 32 |
| Only the rounded clip on | 32 |
| Glyphs, text, selection, scrollbar and clip all off | 33 |
| Private `CGBitmapContext`, image handed to `layer.contents` | 37 |

Two more results on the backing store path: a redraw with no content change stayed at 33 MB, and a first frame that already had rows peaked at 41 MB.

So rows were the trigger, typing and the text input stack were not, and no one drawing part caused it. A CPU-drawn frame avoids it. Which part of AppKit, Core Animation or the graphics driver makes the allocation was not found, and why the selection fill alone doesn't trigger it was not found either.

A `CGImage` in `contents` added about 9 ms between T1 and T2. An IOSurface removed most of that: over 30 opens, T0 to window server visible p95 was 31 to 32 ms with a `CGImage` and 26 ms with an IOSurface, and peak footprint was 55 MB against 46 MB.

Opening Settings or a note window through the normal AppKit path peaked at 39 MB and 34 MB. Typing in those windows was not measured.

Not tested: a display with another scale factor, moving between displays, and the light theme with this draw path.

## Memory measures

`footprint` is `phys_footprint`, what Activity Monitor calls Memory. It's read with `task_info(TASK_VM_INFO)` inside the process and `proc_pid_rusage` from outside. `resident` is `resident_size`, which includes shared framework pages, so it reads far higher than what Orbit alone costs. Diagnostics shows both.

Results from `orbit-bench e2e --opens 30 --idle-secs 60` on the final build, with an empty data set and no tap. A second run of the same build agreed within 1 MB on every line except as shown.

| Point | Footprint MB | Resident MB |
|---|---:|---:|
| Idle after start, before any open | 14.5 | 79.1 |
| After 30 opens | 30.1 | 100.8 |
| After typing | 33.5 | 113.4 |
| After 500 searches | 35.2 | 115.7 |
| After 60 s idle, hidden | 29.8 (29.5 in the second run) | 118.1 |
| Peak in the run | 54.5 | |

The earlier build, before the draw change, had 48.5 MB footprint after idle, 176.2 MB after typing and a 197.1 MB peak.

The brief's budget for background memory is 30 to 40 MB stretch and 55 MB beta. Footprint after idle is within stretch. Resident size is not within either figure, at 118 MB, and most of it is shared framework pages. Under memory pressure, not tested on this machine.
