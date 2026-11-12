# Performance budget

The targets from the brief, how they're measured, and the results on the machines available. Every number here came from a run of `orbit-bench` or `scripts/macos/feature_checks.sh`, and the full reports are in [`bench-results/`](../bench-results).

These are results on one machine per platform, not guarantees for other hardware.

## Results against the budget

Windows figures are from a run on 2026-11-06 with the glass, DirectWrite text, typo-forgiving search and integrations, one clipboard fix before the final build. macOS figures are from a run on 2026-11-06 before the Windows redesign and hotkey features, which the macOS host doesn't have yet.

| Measurement | Stretch | Beta target | Windows | macOS |
|---|---:|---:|---:|---:|
| Background memory after idle | 25–35 MB | 45 MB (Windows), 55 MB (macOS) | 13.4 MB private, 3.9 MB working set | 29.8 MB footprint, **118 MB resident, over budget** |
| Highest memory in the run | | | 51.4 MB peak working set | 54.5 MB peak footprint |
| Hotkey to visible, p95 | 30 ms | 60 ms | 3.2 ms | 24.5 ms |
| Hotkey to usable focused input, p95 | 40 ms | 75 ms | 5.1 ms | 21.2 ms, hotkey to key window |
| Search after a keystroke, p95 | 15 ms | 40 ms | 4.6 ms key to frame | 8.9 ms key to rows |
| Shortcut dispatch, p95 | 20 ms | 50 ms | 2.4 ms Enter to dispatch | 1.75 ms Enter to dispatch |
| Idle CPU | about 0% | 0.2% | 0 ms in the 60 s measured | 0.05% of one core |
| Idle disk writes | 0 | rare | 0 bytes | 0 bytes |
| Extra background processes | 0 | 1 | 0, plus 1 while indexing | 0 |
| Restore after unlock or sleep | near instant | within seconds | **not tested** on real sleep | **not tested** |
| First query after idle | as warm | 100 ms | **not separately measured** | **not separately measured** |

The macOS resident figure includes shared system frameworks, which is why it's so far above the footprint. It's still over the budget's figure, so it's listed as a miss.

## Reference matrix

| Machine | State |
|---|---|
| Constrained Windows laptop, 8 GB | **not tested** |
| Windows gaming desktop, in a game session | **not tested.** The desktop below was used, with no game running and no frame-time measurement. |
| Apple Silicon Mac, 8 or 16 GB | **not tested.** The Mac available has 24 GB. |
| Intel Mac | **not tested** |

## Environment

**Windows.** AMD Ryzen 5 5600X (6 cores, 12 threads), 15.9 GB RAM, Windows 11 Pro 10.0.26200.9711, 2560x1440 at 300 Hz, AC power, about 40% memory load, release build. 10,000 notes and 10,000 clips, 60 opens, 60 s idle. File search doesn't exist, so it can't be on.

**macOS.** Apple M4 Pro, 24 GB RAM, macOS 27.2, AC power, 79% memory free at start, release build, empty data set. That data set is smaller than the Windows one, so don't compare the two directly.

## More Windows results

| Measurement | p50 | p95 | p99 |
|---|---:|---:|---:|
| Key injected to window visible | 2.88 ms | 3.17 ms | 3.74 ms |
| Key injected to foreground | 4.80 ms | 5.06 ms | 10.4 ms |
| Hotkey message to input focused | 7.54 ms | 8.03 ms | 18.2 ms |
| Typing: query seen to frame drawn | 2.57 ms | 3.90 ms | |
| Typing: key injected to frame drawn | 3.23 ms | 4.64 ms | |
| Enter to launch dispatched | 2.18 ms | 2.40 ms | |
| Enter to new process running | 48.8 ms | 98.5 ms | |

| Measurement | Result |
|---|---|
| Binary | 3,988 KB |
| Start to message loop | 34.6 ms, see the regression gate below |
| Shutdown | 26.2 ms |
| Search over 100,000 items | p95 7.52 ms |
| Note search over 10,000 notes | p95 16.9 ms |
| Clipboard search over 10,000 clips | p95 11.5 ms |
| Recent notes list | p95 0.41 ms |
| Calculator, Enter to process present | 312 ms, by polling `tasklist`, so coarse |
| HyperKey: hook callback to UI thread | p95 0.021 ms |
| HyperKey: chord to launcher focused | p95 4.70 ms |
| Open a 5 MB note | 574 ms |
| 100 note opens | 7.5 KB of private memory per open |

**Memory, private / working set.**

| Point | Private | Working set |
|---|---:|---:|
| Idle after start | 5.4 MB | 20.2 MB |
| After 60 opens | 6.3 MB | 25.7 MB |
| After 500 searches | 11.0 MB | 30.3 MB |
| Launcher open with results | 10.5 MB | 30.2 MB |
| After launching a real app | 13.6 MB | 50.4 MB |
| Working set after trim | | 0.1 MB |
| After 60 s idle | 13.4 MB | 3.9 MB |

**Typing right after the shortcut.** With keys sent 0 ms after the shortcut, 0 of 20 rounds kept every key. At 5 ms, 20 of 20 did, and at 15 and 30 ms all rounds were clean. A person can't type that fast, but a key pressed at the same moment as the shortcut can reach the previous app.

## Drawing a frame

How long the launcher takes to draw one frame, before it's shown. Measured on 2026-11-11 with hidden previews (`--preview-frame`) of the same data, 96 DPI, release builds, p50 of 400 frames, comparing 1.3.1 with 1.4.0, which added motion. The timing loop was added for the measurement and isn't in the code. These times are part of "query seen to frame drawn" above, which wasn't run again.

| Frame | 1.3.1 | 1.4.0 |
|---|---:|---:|
| Home list, drawn again | 3.00 ms | 0.87 ms |
| Results for "co", drawn again | 3.05 ms | 1.14 ms |
| Results with the actions menu open | 5.00 ms | 1.72 ms |
| First frame after opening, home list | 4.55 ms | 1.94 ms |
| First frame after opening, actions menu | 7.71 ms | 3.03 ms |
| A frame while the bottom edge moves | | 1.0 to 1.7 ms |

Most of the time went on drawing symbols pixel by pixel and on laying out and rasterizing text on every frame. Both are now kept between frames (see [ARCHITECTURE.md](ARCHITECTURE.md)), which costs up to 3 MB for text while the launcher is open, let go when the working set is trimmed. The three frames compared came out the same, byte for byte, as 1.3.1's. Showing the frame (`UpdateLayeredWindow`) takes about 0.1 ms on top.

The real window, from `orbit-bench e2e` on 2026-11-12 (Ryzen 5 5600X, 2560x1440 at 300 Hz), 1.3.1 then 1.4.0, one run each, p50:

| Measure | 1.3.1 | 1.4.0 |
|---|---:|---:|
| Key injected to window visible | 5.21 ms | 2.46 ms |
| Key injected to Orbit in front | 5.40 ms | 3.29 ms |
| Hotkey to window shown and painted | 7.55 ms | 2.97 ms |
| Key injected to the frame with its results | 4.15 ms | 3.21 ms |
| Rounds with every key caught, typing 5 ms after the hotkey | 18 of 20 | 20 of 20 |
| Private memory after the trim | 9.3 MB | 9.4 MB |
| Private memory with the launcher open, with results | 9.7 MB | 11.9 MB |

The bench's trim message now does what the trim timer does 15 s after the launcher closes, so what's kept for drawing goes too, and the heap is asked to give its free blocks back. Idle CPU and disk writes stay at zero, with one more thread, the frame clock, which sleeps while nothing moves.

## Method

- **T0 to T6** are QueryPerformanceCounter timestamps in the app, written only with `--bench-log`: T0 hotkey message, T1 frame ready, T2 window shown, T3 input focused, T4 query seen, T5 results built, T6 frame drawn. "Key injected" times start in the benchmark process, just before `SendInput`.
- **Those keys are injected, not physical.** The time for a real keyboard to reach Windows isn't included, and a compositor frame is on top of "visible". `DwmFlush` returned at p95 10.8 ms from T0 in the same run.
- **Memory** is private bytes and working set, labelled separately.
- **Idle** is process CPU time, I/O counters and context switches across 60 s with no input. CPU time comes from a counter with about 15.6 ms resolution, so one tick can show up as 15.6 ms/min and did in one rerun.
- **Pressure.** One `--pressure-mb 4949` run reached 92% load, with key to foreground p95 3.1 ms over 8 opens. It's from before today's UI work. That isn't a RAM-exhausted or hard-fault test: the true test hasn't been done.
- **The game-detection query** was measured outside the app: p50 57 µs, p99 88 µs, max 300 µs over 2,000 calls via .NET P/Invoke.

## Regression gate

`orbit-bench` fails if a gated metric is more than 15% worse than `bench-results/baseline-windows.json`, plus a small absolute allowance for timer noise. **The latest run didn't pass.** It flagged 29 metrics against a baseline set before the typo-forgiving search, the glass and DirectWrite. All of them are still well inside the budget above. Frames now take about twice as long to draw (typing, query seen to frame drawn, p95 1.85 to 3.90 ms) and private memory after idle rose from 9.8 to 13.4 MB, which is the expected direction for the new text path, though nothing has isolated each cause. Search over 100,000 items went from p95 2.55 to 7.52 ms, and that code wasn't touched by the text work, so the likely cause is the typo-forgiving search. The owner chose appearance and features over speed, and the baseline hasn't been reset, so this stays visible. Start to message loop is flagged as before, and an earlier comparison found the machine rather than the code behind that one.

Run it with:

```sh
target/release/orbit-bench core data e2e --orbit target/release/orbit.exe --opens 60 --idle-secs 60 \
    --records 10000 --real-app Calculator:CalculatorApp.exe --baseline bench-results/baseline-windows.json
```
