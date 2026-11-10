//! New versions of Orbit, from its releases on GitHub, installed with one click.
//!
//! Orbit looks a little after it starts and then once a day (Settings → General can turn that
//! off), or when asked: in Settings, from the tray menu, or with "Check for Updates" in the
//! launcher. A newer version downloads straight away, in the background, and is checked against
//! the size and SHA-256 GitHub lists for it. Then a notification offers to install it, and the
//! tray menu, Orbit's menu in the launcher and Settings have it too:
//!
//! - The installed copy runs the new Setup with no window. Setup closes Orbit as its own Quit
//!   does, replaces the exe, and starts the new version.
//! - The portable copy swaps its own exe for the new one under the same name, so shortcuts to it
//!   and its "Orbit settings" folder keep working, and starts it.
//! - Any other copy, such as a build from source, links to the release page instead.
//!
//! The check asks GitHub for the latest release and nothing else, with no cookies and no Windows
//! sign-in. Downloads come only from github.com. Network work runs on a thread of its own, which
//! posts what it found to the main window.

use crate::util::SendHwnd;
use orbit_core::update::{self as feed, Download, Kind, Release};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

/// The version this copy compares against. A test build can pretend to be older.
pub const CURRENT: &str = match option_env!("ORBIT_TEST_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};
/// For testing the whole path without publishing: a local copy of GitHub's answer, whose files'
/// addresses start `https://github.com/local/` and are read from beside it. Never set for a
/// release.
const TEST_FEED: Option<&str> = option_env!("ORBIT_TEST_FEED");

/// The first look, a little after starting, then a tick every hour that looks once a day.
const FIRST_CHECK_MS: u32 = 10_000;
const TICK_MS: u32 = 60 * 60 * 1000;
const DAY: Duration = Duration::from_secs(24 * 60 * 60);
/// How long Setup has to close Orbit before the update counts as failed.
const SETUP_MS: u32 = 3 * 60 * 1000;
/// The installed copy's download waits here, in Windows' temp folder.
const DOWNLOAD_FOLDER: &str = "Orbit update";
/// Passed to the new version once it has replaced the old one.
pub const UPDATED_ARG: &str = "--updated";

/// Where an update has got to, for Settings, the tray and the launcher.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Idle,
    Checking,
    UpToDate,
    /// A newer version is coming down.
    Downloading(String),
    /// Downloaded and checked: one click installs it.
    Ready(String),
    /// Newer, but this copy can't install it: the release page has it.
    Available(String),
    Installing(String),
    Failed(String),
}

/// Who asked for a look, which says where the answer goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// The daily look: only a new version is worth a word, and a failure waits for the next.
    Quietly,
    /// Settings shows the answer in its row.
    FromSettings,
    /// The launcher or the tray: the answer is reported, in the footer or as a notification.
    FromLauncher,
}

struct Updater {
    main: HWND,
    /// The data folder, which the portable copy's new version is started with.
    dir: PathBuf,
    kind: Kind,
    phase: Phase,
    ask: Ask,
    last_check: Option<SystemTime>,
    file: Option<PathBuf>,
    page: String,
    /// The version the notification has offered, so it's offered once.
    offered: Option<String>,
    /// The notification that offers it, so a click on it installs. See `tray::notify`.
    offer_note: Option<u32>,
    /// The notification that says Orbit was updated, so a click on it shows what's new.
    updated_note: Option<u32>,
    cleaned: bool,
}

thread_local! {
    static UPDATER: RefCell<Option<Updater>> = const { RefCell::new(None) };
    /// What a preview of Settings shows, as there's no updater then.
    static PREVIEW: RefCell<Option<Phase>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Updater) -> R) -> Option<R> {
    UPDATER.with(|updater| updater.borrow_mut().as_mut().map(f))
}

/// What the worker found.
enum Event {
    Found(Release),
    UpToDate,
    Failed(String),
    Downloaded(PathBuf),
    DownloadFailed(String),
}

// --- On the main thread ---------------------------------------------------------------------

/// Sets up the updater for the main window, and the first look a little later.
pub fn start(main: HWND, dir: PathBuf) {
    let kind = this_copy();
    UPDATER.with(|updater| {
        *updater.borrow_mut() = Some(Updater {
            main,
            dir,
            kind,
            phase: Phase::Idle,
            ask: Ask::Quietly,
            last_check: None,
            file: None,
            page: feed::RELEASES_PAGE.to_string(),
            offered: None,
            offer_note: None,
            updated_note: None,
            cleaned: false,
        })
    });
    unsafe {
        SetTimer(Some(main), crate::app::TIMER_UPDATE_CHECK, FIRST_CHECK_MS, None);
    }
}

/// Where the update has got to.
pub fn phase() -> Phase {
    if let Some(phase) = PREVIEW.with(|preview| preview.borrow().clone()) {
        return phase;
    }
    with(|updater| updater.phase.clone()).unwrap_or(Phase::Idle)
}

/// For a preview of Settings: the row as it is at `phase`.
pub fn preview(phase: Phase) {
    PREVIEW.with(|preview| *preview.borrow_mut() = Some(phase));
}

/// The hourly tick. `wanted` is the "Check for updates" switch.
pub fn on_check_timer(wanted: bool) {
    let Some(main) = with(|updater| updater.main) else { return };
    unsafe {
        let _ = KillTimer(Some(main), crate::app::TIMER_UPDATE_CHECK);
        SetTimer(Some(main), crate::app::TIMER_UPDATE_CHECK, TICK_MS, None);
    }
    // By the clock on the wall, so a day asleep counts.
    let due = with(|updater| updater.last_check.is_none_or(|at| at.elapsed().map_or(true, |since| since >= DAY))).unwrap_or(false);
    if wanted && due {
        begin(Ask::Quietly);
    }
}

/// Setup was started but Orbit is still here: it didn't get as far as closing it.
pub fn on_setup_timer() {
    let Some(main) = with(|updater| updater.main) else { return };
    unsafe {
        let _ = KillTimer(Some(main), crate::app::TIMER_UPDATE_SETUP);
    }
    let failed = with(|updater| {
        if !matches!(updater.phase, Phase::Installing(_)) {
            return false;
        }
        updater.phase = Phase::Failed("The update didn't finish. Try again.".into());
        updater.file = None;
        true
    })
    .unwrap_or(false);
    if failed {
        changed();
    }
}

/// What the Settings button, the tray item and "Check for Updates" do, as the update has got
/// to: install what's downloaded, show the release page, or look now. Gives a line to report.
pub fn press(ask: Ask) -> Option<String> {
    match phase() {
        Phase::Ready(_) => install().err(),
        Phase::Available(_) => {
            open_page();
            None
        }
        Phase::Checking => Some("Already looking for a new version.".into()),
        Phase::Downloading(version) => Some(format!("Already downloading Orbit {version}.")),
        Phase::Installing(version) => Some(format!("Installing Orbit {version}.")),
        Phase::Idle | Phase::UpToDate | Phase::Failed(_) => {
            begin(ask);
            (ask == Ask::FromLauncher).then(|| "Looking for a new version\u{2026}".to_string())
        }
    }
}

fn begin(ask: Ask) {
    let Some((main, kind, clean)) = with(|updater| {
        if matches!(updater.phase, Phase::Checking | Phase::Downloading(_) | Phase::Ready(_) | Phase::Installing(_)) {
            return None;
        }
        updater.phase = Phase::Checking;
        updater.ask = ask;
        updater.last_check = Some(SystemTime::now());
        // By the first look, the Setup or old exe an update left has long finished with: it can go.
        let clean = !std::mem::replace(&mut updater.cleaned, true);
        Some((SendHwnd::new(updater.main), updater.kind, clean))
    })
    .flatten() else {
        return;
    };
    changed();
    let _ = std::thread::Builder::new().name("orbit-update".into()).spawn(move || {
        let post = |event: Event| main.post(crate::app::WM_ORBIT_UPDATE, crate::util::box_for_post(event), 0);
        if clean {
            clean_up();
        }
        match check(kind) {
            Err(error) => post(Event::Failed(error)),
            Ok(None) => post(Event::UpToDate),
            Ok(Some(release)) => {
                let download = release.download.clone().filter(|_| kind.installs_itself());
                post(Event::Found(release));
                if let Some(download) = download {
                    post(match fetch(kind, &download) {
                        Ok(file) => Event::Downloaded(file),
                        Err(error) => Event::DownloadFailed(error),
                    });
                }
            }
        }
    });
}

/// The worker's news arrived. Gives a line to report when the launcher or the tray asked.
pub fn on_message(pointer: usize) -> Option<String> {
    let event = *crate::util::take_posted::<Event>(pointer)?;
    let (report, offer_it) = with(|updater| {
        let asked = updater.ask == Ask::FromLauncher;
        match event {
            Event::Found(release) => {
                updater.page = release.page.clone();
                let version = release.version;
                if updater.kind.installs_itself() && release.download.is_some() {
                    updater.phase = Phase::Downloading(version.clone());
                    (asked.then(|| format!("Downloading Orbit {version}\u{2026}")), false)
                } else {
                    updater.phase = Phase::Available(version.clone());
                    // Nothing to install here, so the release page is the news.
                    (asked.then(|| format!("Orbit {version} is out. Get it from its release page on GitHub.")), updater.ask == Ask::Quietly)
                }
            }
            Event::UpToDate => {
                updater.phase = Phase::UpToDate;
                (asked.then(|| format!("Orbit {CURRENT} is the newest version.")), false)
            }
            Event::Downloaded(file) => {
                if let Phase::Downloading(version) = &updater.phase {
                    updater.phase = Phase::Ready(version.clone());
                    updater.file = Some(file);
                    if asked {
                        // Asked for, so it's offered again even if it was before.
                        updater.offered = None;
                    }
                    // Settings shows it in the row that was pressed.
                    return (None, updater.ask != Ask::FromSettings);
                }
                (None, false)
            }
            Event::Failed(error) | Event::DownloadFailed(error) => {
                if updater.ask == Ask::Quietly {
                    // No connection, or GitHub being busy, will pass: stay quiet and look again
                    // at the next tick.
                    updater.phase = Phase::Idle;
                    updater.last_check = None;
                    (None, false)
                } else {
                    updater.phase = Phase::Failed(error.clone());
                    (asked.then_some(error), false)
                }
            }
        }
    })
    .unwrap_or((None, false));
    changed();
    if offer_it {
        offer();
    }
    report
}

/// Settings shows the news. The tray menu and Orbit's menu read it when they open.
fn changed() {
    crate::settings_win::update_changed();
}

/// The notification that offers the new version, once: to install it, or, for a copy that can't
/// install it, to get it from its release page.
fn offer() {
    let Some((main, version, ready)) = with(|updater| match &updater.phase {
        Phase::Ready(version) | Phase::Available(version) if updater.offered.as_ref() != Some(version) => {
            Some((updater.main, version.clone(), matches!(updater.phase, Phase::Ready(_))))
        }
        _ => None,
    })
    .flatten() else {
        return;
    };
    let note = if ready {
        crate::tray::notify(
            main,
            crate::app::WM_ORBIT_TRAY,
            &format!("Orbit {version} is ready"),
            "Click here to install it. Orbit closes and starts again by itself. The tray menu has it too.",
        )
    } else {
        crate::tray::notify(main, crate::app::WM_ORBIT_TRAY, &format!("Orbit {version} is out"), "Click here to see what's new and get it from GitHub.")
    };
    with(|updater| {
        updater.offered = Some(version);
        updater.offer_note = Some(note);
    });
}

/// A notification of Orbit's was clicked. Says whether it was one of the updater's.
pub fn notification_clicked() -> bool {
    let last = crate::tray::last_note();
    let (offer, updated) = with(|updater| (updater.offer_note == Some(last), updater.updated_note == Some(last))).unwrap_or((false, false));
    if offer {
        match phase() {
            Phase::Ready(_) => {
                if let Err(error) = install() {
                    report_failure(&error);
                }
            }
            Phase::Available(_) => open_page(),
            _ => {}
        }
        return true;
    }
    if updated {
        open_url(&feed::release_page(CURRENT));
        return true;
    }
    false
}

/// Just started as the new version: says so, once.
pub fn announce_updated() {
    let Some(main) = with(|updater| updater.main) else { return };
    let note = crate::tray::notify(main, crate::app::WM_ORBIT_TRAY, &format!("Orbit is updated to {CURRENT}"), "Click here to see what's new.");
    with(|updater| updater.updated_note = Some(note));
}

fn report_failure(error: &str) {
    if let Some(main) = with(|updater| updater.main) {
        crate::tray::notify(main, crate::app::WM_ORBIT_TRAY, "Couldn't update Orbit", error);
    }
}

/// Installs the downloaded version, and lets it take over: Setup closes Orbit, and the portable
/// copy quits once its new version is started.
pub fn install() -> Result<(), String> {
    let Some((kind, file, version, dir, main)) = with(|updater| match (&updater.phase, &updater.file) {
        (Phase::Ready(version), Some(file)) => Some((updater.kind, file.clone(), version.clone(), updater.dir.clone(), updater.main)),
        _ => None,
    })
    .flatten() else {
        return Err("There's no update downloaded to install.".into());
    };
    match start_install(kind, &file, &dir) {
        Ok(()) => {
            with(|updater| updater.phase = Phase::Installing(version));
            changed();
            unsafe {
                match kind {
                    // Setup closes Orbit as its own Quit does, so it knows to start it again.
                    Kind::Installed => {
                        SetTimer(Some(main), crate::app::TIMER_UPDATE_SETUP, SETUP_MS, None);
                    }
                    _ => SendHwnd::new(main).post(crate::app::WM_ORBIT_RUN_BUILTIN, usize::MAX, 0),
                }
            }
            Ok(())
        }
        Err(error) => {
            with(|updater| {
                updater.phase = Phase::Failed(error.clone());
                updater.file = None;
            });
            changed();
            Err(error)
        }
    }
}

/// Opens a page in the default browser.
fn open_url(url: &str) {
    if let Some(main) = with(|updater| updater.main) {
        crate::launch::spawn(crate::launch::LaunchRequest::open(url), SendHwnd::new(main), crate::app::WM_ORBIT_LAUNCH_FAILED);
    }
}

/// The new version's release page, with its notes.
pub fn open_page() {
    let page = with(|updater| updater.page.clone()).unwrap_or_else(|| feed::RELEASES_PAGE.to_string());
    if page.starts_with(feed::DOWNLOADS_FROM) {
        open_url(&page);
    }
}

/// What Settings' Updates row says, its button, and whether the button can be pressed.
pub fn row() -> (String, &'static str, bool) {
    match phase() {
        Phase::Idle => ("See whether there's a newer version.".into(), "Check now", true),
        Phase::Checking => ("Looking for a new version\u{2026}".into(), "Checking\u{2026}", false),
        Phase::UpToDate => ("This is the newest version.".into(), "Check now", true),
        Phase::Downloading(version) => (format!("Downloading Orbit {version}\u{2026}"), "Downloading\u{2026}", false),
        Phase::Ready(version) => (format!("Orbit {version} is ready. Orbit restarts to install it."), "Install", true),
        Phase::Available(version) => (format!("Orbit {version} is out. This copy can't install it itself."), "Get it", true),
        Phase::Installing(version) => (format!("Installing Orbit {version}\u{2026}"), "Installing\u{2026}", false),
        Phase::Failed(error) => (error, "Try again", true),
    }
}

/// The tray menu's item, when there's a version to have.
pub fn tray_item() -> Option<String> {
    match phase() {
        Phase::Ready(version) => Some(format!("Install Orbit {version}")),
        Phase::Available(version) => Some(format!("Get Orbit {version}\u{2026}")),
        _ => None,
    }
}

/// The title and subtitle of the row in Orbit's own menu.
pub fn menu_row() -> (String, String) {
    match phase() {
        Phase::Ready(version) => (format!("Install Orbit {version}"), "Orbit restarts".into()),
        Phase::Available(version) => (format!("Get Orbit {version}"), "From GitHub".into()),
        Phase::Downloading(version) => (format!("Downloading Orbit {version}\u{2026}"), String::new()),
        Phase::Installing(version) => (format!("Installing Orbit {version}\u{2026}"), String::new()),
        Phase::Checking => ("Looking for Updates\u{2026}".into(), String::new()),
        Phase::UpToDate => ("Check for Updates".into(), "Up to date".into()),
        Phase::Idle | Phase::Failed(_) => ("Check for Updates".into(), String::new()),
    }
}

// --- On the worker ---------------------------------------------------------------------------

fn check(kind: Kind) -> Result<Option<Release>, String> {
    let body = get(feed::LATEST_RELEASE, true, 1 << 20)?;
    feed::parse_release(&body, kind, CURRENT)
}

/// Downloads `download`, checks it, and puts it where this kind of copy installs it from.
fn fetch(kind: Kind, download: &Download) -> Result<PathBuf, String> {
    let destination = download_path(kind, download)?;
    let limit = if download.size > 0 { download.size as usize } else { 64 << 20 };
    let body = get(&download.url, false, limit).map_err(|error| {
        if error.starts_with("Couldn't reach") { "Couldn't download the update. Check your connection.".to_string() } else { error }
    })?;
    feed::check_download(&body, download)?;
    let cant_save = |folder: &Path| format!("Couldn't save the update in {}.", folder.display());
    let folder = destination.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(folder).map_err(|_| cant_save(folder))?;
    let partial = sibling(&destination, ".part");
    std::fs::write(&partial, &body).map_err(|_| cant_save(folder))?;
    let _ = std::fs::remove_file(&destination);
    if std::fs::rename(&partial, &destination).is_err() {
        let _ = std::fs::remove_file(&partial);
        return Err(cant_save(folder));
    }
    Ok(destination)
}

/// Closes a WinHTTP handle when it goes.
struct Net(*mut core::ffi::c_void);

impl Drop for Net {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = windows::Win32::Networking::WinHttp::WinHttpCloseHandle(self.0);
            }
        }
    }
}

/// GETs `url` over HTTPS, following GitHub's redirects to its file servers, with no more than
/// `limit` bytes back. `api` asks for the API's JSON.
fn get(url: &str, api: bool, limit: usize) -> Result<Vec<u8>, String> {
    use windows::Win32::Networking::WinHttp::*;
    use windows::core::{PCWSTR, w};
    if let Some(feed_file) = TEST_FEED {
        return test_get(feed_file, url);
    }
    let offline = || "Couldn't reach GitHub. Check your connection.".to_string();
    let rest = url.strip_prefix("https://").ok_or_else(offline)?;
    let (host, path) = match rest.split_once('/') {
        Some((host, path)) => (host, format!("/{path}")),
        None => (rest, "/".to_string()),
    };
    unsafe {
        let agent = crate::util::wide(&format!("Orbit/{}", env!("CARGO_PKG_VERSION")));
        let session = Net(WinHttpOpen(PCWSTR(agent.as_ptr()), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0));
        if session.0.is_null() {
            return Err(offline());
        }
        // Each step gives up rather than wait for ever: resolving, connecting, sending, and each
        // read.
        let _ = WinHttpSetTimeouts(session.0, 10_000, 10_000, 15_000, 30_000);
        let host = crate::util::wide(host);
        let connection = Net(WinHttpConnect(session.0, PCWSTR(host.as_ptr()), 443, 0));
        if connection.0.is_null() {
            return Err(offline());
        }
        let path = crate::util::wide(&path);
        let request = Net(WinHttpOpenRequest(connection.0, w!("GET"), PCWSTR(path.as_ptr()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE));
        if request.0.is_null() {
            return Err(offline());
        }
        let option = |option: u32, value: u32| {
            let _ = WinHttpSetOption(Some(request.0), option, Some(&value.to_le_bytes()));
        };
        option(WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_DISABLE_COOKIES);
        option(WINHTTP_OPTION_AUTOLOGON_POLICY, WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH);
        let headers: Vec<u16> = if api { "Accept: application/vnd.github+json\r\nX-GitHub-Api-Version: 2022-11-28\r\n" } else { "" }.encode_utf16().collect();
        WinHttpSendRequest(request.0, (!headers.is_empty()).then_some(&headers[..]), None, 0, 0, 0).map_err(|_| offline())?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(|_| offline())?;
        let (mut status, mut size) = (0u32, 4u32);
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut core::ffi::c_void),
            &mut size,
            std::ptr::null_mut(),
        )
        .map_err(|_| offline())?;
        match status {
            200 => {}
            404 => return Err("Couldn't find Orbit's releases on GitHub.".into()),
            403 | 429 => return Err("GitHub is busy. Try again later.".into()),
            other => return Err(format!("GitHub answered {other}. Try again later.")),
        }
        let mut body = Vec::new();
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            WinHttpReadData(request.0, chunk.as_mut_ptr() as *mut core::ffi::c_void, chunk.len() as u32, &mut read)
                .map_err(|_| "The download stopped. Check your connection and try again.".to_string())?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..read as usize]);
            if body.len() > limit {
                return Err("GitHub sent more than the release lists. Try again later.".into());
            }
        }
        Ok(body)
    }
}

/// [`get`] for a test build: GitHub's answer and the files come from a folder.
fn test_get(feed_file: &str, url: &str) -> Result<Vec<u8>, String> {
    std::thread::sleep(Duration::from_millis(300));
    let path = if url == feed::LATEST_RELEASE {
        PathBuf::from(feed_file)
    } else {
        let name = url.strip_prefix("https://github.com/local/").filter(|name| feed::is_plain_file_name(name)).ok_or("Not a test file.")?;
        Path::new(feed_file).with_file_name(name)
    };
    std::fs::read(path).map_err(|_| "Couldn't reach GitHub. Check your connection.".to_string())
}

// --- Installing -------------------------------------------------------------------------------

/// Where this kind of copy keeps the new version until it's installed.
fn download_path(kind: Kind, download: &Download) -> Result<PathBuf, String> {
    match kind {
        Kind::Installed => Ok(std::env::temp_dir().join(DOWNLOAD_FOLDER).join(&download.name)),
        // Beside the exe, so moving it into place can't fail for being on another drive.
        Kind::Portable => std::env::current_exe().map(|exe| sibling(&exe, ".new")).map_err(|_| "Couldn't find where Orbit is.".to_string()),
        Kind::Other => Err("This copy of Orbit can't update itself.".into()),
    }
}

/// `path` with `suffix` added to its whole file name.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// Starts installing the downloaded version.
fn start_install(kind: Kind, file: &Path, dir: &Path) -> Result<(), String> {
    match kind {
        Kind::Installed => {
            // Silent, Setup keeps the choices made when Orbit was installed, shows nothing, and
            // starts Orbit again once it's done, as it was running.
            std::process::Command::new(file)
                .arg("/VERYSILENT")
                .current_dir(std::env::temp_dir())
                .spawn()
                .map_err(|_| "Couldn't start the installer. Try again.".to_string())?;
        }
        Kind::Portable => {
            let exe = std::env::current_exe().map_err(|_| "Couldn't find where Orbit is.".to_string())?;
            swap_in(&exe, file)?;
            let started = std::process::Command::new(&exe)
                .arg("--wait-pid")
                .arg(std::process::id().to_string())
                .arg("--data-dir")
                .arg(dir)
                .arg(UPDATED_ARG)
                .spawn();
            if started.is_err() {
                // The new version is in place, so opening Orbit again starts it.
                return Err("Updated. Open Orbit again to start the new version.".into());
            }
        }
        Kind::Other => return Err("This copy of Orbit can't update itself.".into()),
    }
    Ok(())
}

/// Puts `new` where `exe` is, keeping the old one as "….old" until it has quit. Windows lets a
/// running exe be renamed, though not deleted.
fn swap_in(exe: &Path, new: &Path) -> Result<(), String> {
    let old = sibling(exe, ".old");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old).map_err(|_| "Couldn't make room for the new version.".to_string())?;
    if std::fs::rename(new, exe).is_err() {
        let _ = std::fs::rename(&old, exe);
        return Err("Couldn't put the new version in place.".into());
    }
    Ok(())
}

/// Clears what an update left: the old exe, a download that never finished, and the Setup that
/// ran.
fn clean_up() {
    if let Ok(exe) = std::env::current_exe() {
        for suffix in [".old", ".new", ".new.part"] {
            let leftover = sibling(&exe, suffix);
            if leftover.exists() {
                let _ = std::fs::remove_file(leftover);
            }
        }
    }
    let folder = std::env::temp_dir().join(DOWNLOAD_FOLDER);
    if folder.exists() {
        let _ = std::fs::remove_dir_all(folder);
    }
}

/// How this copy updates: by the portable name, or by being the copy Setup installed.
fn this_copy() -> Kind {
    if crate::instance::portable_dir().is_some() {
        Kind::Portable
    } else if crate::install::running_installed() {
        Kind::Installed
    } else {
        Kind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_portable_swap_keeps_the_name() {
        let dir = std::env::temp_dir().join(format!("orbit-swap-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("Orbit-1.1.0-Portable.exe");
        let new = sibling(&exe, ".new");
        std::fs::write(&exe, b"old").unwrap();
        std::fs::write(&new, b"new").unwrap();
        swap_in(&exe, &new).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(std::fs::read(sibling(&exe, ".old")).unwrap(), b"old");
        assert!(!new.exists());
        // Nothing to swap in: the old one stays where it was.
        assert!(swap_in(&exe, &new).is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn each_phase_has_words_for_settings_and_the_menus() {
        for (phase, label, can_press) in [
            (Phase::Idle, "Check now", true),
            (Phase::Checking, "Checking\u{2026}", false),
            (Phase::Ready("9.0.0".into()), "Install", true),
            (Phase::Available("9.0.0".into()), "Get it", true),
            (Phase::Failed("Couldn't reach GitHub. Check your connection.".into()), "Try again", true),
        ] {
            preview(phase.clone());
            let (text, shown, pressable) = row();
            assert_eq!((shown, pressable), (label, can_press), "{phase:?}");
            assert!(!text.is_empty() && text.chars().count() <= 60, "one line in Settings: {text}");
        }
        preview(Phase::Ready("9.0.0".into()));
        assert_eq!(tray_item().as_deref(), Some("Install Orbit 9.0.0"));
        assert_eq!(menu_row().0, "Install Orbit 9.0.0");
        preview(Phase::UpToDate);
        assert_eq!(tray_item(), None, "the tray menu stays as it was with nothing to have");
        PREVIEW.with(|preview| *preview.borrow_mut() = None);
    }

    /// Asks GitHub for real, and downloads and checks the latest release's portable exe. Run by
    /// hand: `cargo test -p orbit-windows -- --ignored the_real_release`.
    #[test]
    #[ignore]
    fn the_real_release_downloads_and_checks_out() {
        let body = get(feed::LATEST_RELEASE, true, 1 << 20).expect("GitHub answers");
        let release = feed::parse_release(&body, Kind::Portable, "0.0.1").expect("it makes sense").expect("newer than 0.0.1");
        let download = release.download.expect("a portable exe");
        assert!(download.sha256.is_some(), "GitHub lists a digest for {}", download.name);
        let file = get(&download.url, false, download.size as usize).expect("it downloads, following GitHub's redirect");
        feed::check_download(&file, &download).expect("it's the file the release lists");
    }
}
