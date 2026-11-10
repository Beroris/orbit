//! Orbit for Windows. One resident process, native Win32, no web runtime.
#![windows_subsystem = "windows"]

mod app;
mod app_picker;
mod catalog;
mod clipboard;
mod displays;
mod editor;
mod everything;
mod extensions;
mod favicon;
mod files;
mod foreground;
mod frost;
mod forms;
mod game;
mod gfx;
mod grid;
mod hook;
mod icons;
mod indexer;
mod importer;
mod install;
mod instance;
mod inline_form;
mod integrations;
mod providers;
mod text;
mod keys;
mod launch;
mod link_form;
mod metrics;
mod rates;
mod rows;
mod screen_picker;
mod settings_preview;
mod settings_win;
mod setup;
mod tray;
mod ui;
mod update;
mod util;
mod view;
mod watch;
mod speedtest;
mod clock;
mod colour_pick;
mod symbols;
mod toggles;
mod window_layout;
mod weather;
mod worker;

use app::{Options, WM_ORBIT_TOGGLE};
use orbit_hotkeys::Chord;
use std::path::PathBuf;
use util::{default_data_dir, wide};
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{CreateMutexW, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, IsDialogMessageW, MSG, PostMessageW, TranslateMessage,
};
use windows::core::PCWSTR;

#[derive(Default)]
struct Args {
    data_dir: Option<PathBuf>,
    quit: bool,
    index_apps: bool,
    refresh_icons: bool,
    bench_log: Option<PathBuf>,
    hotkey: Option<Chord>,
    trim_ms: Option<u32>,
    no_trim: bool,
    no_reindex: bool,
    wait_pid: Option<u32>,
    backdrop: Option<ui::Backdrop>,
    preview: Option<app::Preview>,
    settings_preview: Option<settings_preview::SettingsPreview>,
    /// `--setup`, or an exe named like `Orbit-1.1.0-Setup.exe`: install Orbit.
    setup: bool,
    uninstall: bool,
    /// No window: `/S`, `/SILENT`, `/VERYSILENT` or `--silent`, as Inno Setup took.
    silent: bool,
    start: app::Start,
    /// Started by an update, as its new version.
    updated: bool,
}

fn parse_args() -> Args {
    let mut args = Args::default();
    let mut iter = std::env::args_os().skip(1);
    while let Some(arg) = iter.next() {
        match arg.to_string_lossy().as_ref() {
            "--data-dir" => args.data_dir = iter.next().map(PathBuf::from),
            // Closes the running Orbit, as the installer does before it replaces or removes it.
            "--quit" => args.quit = true,
            "--index-apps" => args.index_apps = true,
            "--refresh-icons" => args.refresh_icons = true,
            "--bench-log" => args.bench_log = iter.next().map(PathBuf::from),
            "--hotkey" => {
                args.hotkey = iter.next().and_then(|text| Chord::parse(&text.to_string_lossy()).ok());
            }
            "--trim-ms" => args.trim_ms = iter.next().and_then(|t| t.to_string_lossy().parse().ok()),
            "--no-trim" => args.no_trim = true,
            "--no-reindex" => args.no_reindex = true,
            "--setup" => args.setup = true,
            "--uninstall" => args.uninstall = true,
            "--show" => args.start = app::Start::Launcher,
            update::UPDATED_ARG => args.updated = true,
            // Setup.exe installed Orbit and hands the welcome over, with where its window was.
            "--setup-continue" => {
                let at = iter.next().and_then(|text| {
                    let text = text.to_string_lossy().into_owned();
                    let (x, y) = text.split_once(',')?;
                    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
                });
                args.start = app::Start::Welcome { at };
            }
            flag if install::is_silent_flag(flag) => args.silent = true,
            // For the end-to-end tests, which can only inject keys.
            "--accept-injected-input" => hook::ACCEPT_INJECTED.store(true, std::sync::atomic::Ordering::Relaxed),
            "--wait-pid" => args.wait_pid = iter.next().and_then(|t| t.to_string_lossy().parse().ok()),
            "--backdrop" => args.backdrop = iter.next().and_then(|t| ui::Backdrop::parse(&t.to_string_lossy())),
            // Draws one frame to a file and exits, without ever showing a window or taking focus.
            "--preview-frame" => {
                args.preview = iter.next().map(|out| app::Preview { out: PathBuf::from(out), query: String::new(), actions: false, scope: None, select: 0, step: 0 });
            }
            "--preview-select" => {
                let index = iter.next().and_then(|t| t.to_string_lossy().parse().ok()).unwrap_or(0);
                if let Some(preview) = &mut args.preview {
                    preview.select = index;
                }
            }
            "--preview-query" => {
                let text = iter.next().map(|t| t.to_string_lossy().into_owned()).unwrap_or_default();
                if let Some(preview) = &mut args.preview {
                    preview.query = text;
                }
            }
            "--preview-scope" => {
                let text = iter.next().map(|t| t.to_string_lossy().into_owned());
                if let Some(preview) = &mut args.preview {
                    preview.scope = text;
                }
            }
            "--preview-actions" => {
                if let Some(preview) = &mut args.preview {
                    preview.actions = true;
                }
            }
            // Developer previews of the forms. See `settings_preview.rs`.
            "--preview-settings" => {
                let what = iter.next().map(|t| t.to_string_lossy().into_owned()).unwrap_or_default();
                let out = iter.next().map(PathBuf::from).unwrap_or_default();
                args.settings_preview = Some(settings_preview::SettingsPreview { what, out, ..Default::default() });
            }
            "--preview-scroll" => {
                let pixels = iter.next().and_then(|t| t.to_string_lossy().parse().ok()).unwrap_or(0);
                if let Some(preview) = &mut args.settings_preview {
                    preview.scroll = pixels;
                }
            }
            "--preview-theme" => {
                let theme = iter.next().map(|t| t.to_string_lossy().to_lowercase());
                if let Some(preview) = &mut args.settings_preview {
                    preview.theme = match theme.as_deref() {
                        Some("dark") => Some(orbit_core::settings::Appearance::Dark),
                        Some("light") => Some(orbit_core::settings::Appearance::Light),
                        _ => None,
                    };
                }
            }
            "--preview-empty" => {
                if let Some(preview) = &mut args.settings_preview {
                    preview.empty_snippets = true;
                }
            }
            "--preview-arm-delete" => {
                if let Some(preview) = &mut args.settings_preview {
                    preview.arm_delete = true;
                }
            }
            "--preview-selfcheck" => {
                let path = iter.next().map(PathBuf::from);
                if let Some(preview) = &mut args.settings_preview {
                    preview.selfcheck = path;
                }
            }
            _ => {}
        }
    }
    args
}

fn install_crash_handlers(dir: PathBuf) {
    // Whatever goes wrong, never leave virtual modifiers held down in the OS.
    std::panic::set_hook(Box::new(move |info| {
        hook::release_synthetic_modifiers();
        // Location only: a panic message could quote user data.
        let location = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let thread = std::thread::current().name().unwrap_or("?").to_string();
        let line = format!("panic on thread {thread} at {location}\n");
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("crash.log")) {
            let _ = file.write_all(line.as_bytes());
        }
    }));
    unsafe extern "system" fn on_crash(
        _info: *const windows::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS,
    ) -> i32 {
        hook::release_synthetic_modifiers();
        0 // EXCEPTION_CONTINUE_SEARCH
    }
    unsafe {
        windows::Win32::System::Diagnostics::Debug::SetUnhandledExceptionFilter(Some(on_crash));
    }
}

fn main() {
    let args = parse_args();
    // Before anything global: no single-instance check, hook, hotkey or tray icon.
    if let Some(preview) = args.settings_preview {
        std::process::exit(settings_preview::run(preview, args.data_dir));
    }
    // Setup and the uninstaller run before anything global, and never as the launcher.
    if args.uninstall {
        std::process::exit(setup::run_uninstall(args.silent));
    }
    if args.setup || setup::named_as_setup() {
        std::process::exit(setup::run_install(args.silent));
    }
    let dir = args.data_dir.clone().or_else(instance::portable_dir).unwrap_or_else(default_data_dir);
    if args.quit {
        std::process::exit(if instance::quit(&dir) { 0 } else { 1 });
    }
    if args.index_apps {
        let _ = std::fs::create_dir_all(&dir);
        std::process::exit(indexer::run(&dir, args.refresh_icons));
    }
    if let Some(pid) = args.wait_pid {
        unsafe {
            if let Ok(process) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
                // An update's old version may take a moment to close.
                let result = WaitForSingleObject(process, 15_000);
                debug_assert!(result == WAIT_OBJECT_0 || result.0 != 0);
                let _ = CloseHandle(process);
            }
        }
    }

    // One instance per data directory. A second launch just opens the first one's window.
    let name = instance::name(&dir);
    let mutex_name = wide(&format!("Local\\OrbitLauncher-{name}"));
    let mutex = unsafe { CreateMutexW(None, true, PCWSTR(mutex_name.as_ptr())) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            if let Some(existing) = instance::find(&dir).or_else(instance::unmarked) {
                let _ = PostMessageW(Some(existing), WM_ORBIT_TOGGLE, Default::default(), Default::default());
            }
        }
        return;
    }
    let _ = mutex;

    let _ = std::fs::create_dir_all(&dir);
    install_crash_handlers(dir.clone());

    // Empties the working set after the launcher has been hidden for a while. Measured in
    // docs/DECISIONS.md: the pages stay in the OS standby list, so reopening is unaffected.
    let trim_after_ms = if args.no_trim { None } else { Some(args.trim_ms.unwrap_or(15_000)) };

    let options = Options {
        data_dir: dir,
        bench_log: args.bench_log,
        hotkey_override: args.hotkey,
        trim_after_ms,
        no_reindex: args.no_reindex,
        backdrop: args.backdrop,
        preview: args.preview,
        start: args.start,
        updated: args.updated,
    };
    if app::startup(options).is_err() {
        return;
    }

    let mut message = MSG::default();
    unsafe {
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            if settings_win::eats_key_text(&message) {
                continue;
            }
            // Tab and arrow navigation in the settings and snippet windows.
            if let Some(dialog) = editor::form_for(&message) {
                if IsDialogMessageW(dialog, &message).as_bool() {
                    settings_win::focus_moved(dialog);
                    continue;
                }
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
