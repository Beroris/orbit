//! Internet speed tests, by Ookla's Speedtest CLI against Speedtest.net. Orbit doesn't measure
//! anything itself: it runs the program, which isn't part of Orbit, with no window, reads the lines
//! it prints as the test goes, and stops it if the test is left. The program is installed with
//! Windows Package Manager when asked, after its licence has been accepted in the launcher. What
//! the lines mean is read in `orbit_core::speedtest`.

use crate::util::SendHwnd;
use orbit_core::speedtest::Test;
use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
pub const WINGET_ID: &str = "Ookla.Speedtest.CLI";
const EXE: &str = "speedtest.exe";
pub const EULA: &str = "https://www.speedtest.net/about/eula";
pub const TERMS: &str = "https://www.speedtest.net/about/terms";
pub const PRIVACY: &str = "https://www.speedtest.net/about/privacy";
/// A test takes about 25 seconds. One that takes far longer has stuck.
const LONGEST: Duration = Duration::from_secs(120);

/// Lines read since the window last looked, and whether the program has ended.
#[derive(Default)]
struct Inbox {
    lines: Vec<String>,
    ended: Option<String>,
    run: u64,
}

static INBOX: Mutex<Option<Inbox>> = Mutex::new(None);

/// Where a test is from the window's side.
#[derive(Default)]
pub struct SpeedTest {
    pub test: Test,
    /// The program was looked for and isn't there.
    pub missing: bool,
    pub installing: bool,
    pub install_problem: Option<String>,
    /// The program while it runs, with the number of the test it's running.
    child: Arc<Mutex<Option<(u64, Child)>>>,
    run: u64,
}

/// Ends test `run`'s program, if it's still the one running.
fn end(holder: &Mutex<Option<(u64, Child)>>, run: u64) {
    let mut slot = holder.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if slot.as_ref().is_some_and(|(of, _)| *of == run) {
        if let Some((_, mut child)) = slot.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl SpeedTest {
    /// Starts a test, unless one is running.
    pub fn start(&mut self, main: SendHwnd, message: u32) {
        if self.test.running() || self.installing {
            return;
        }
        let Some(program) = find() else {
            self.missing = true;
            return;
        };
        self.missing = false;
        self.run_program(&program, main, message);
    }

    fn run_program(&mut self, program: &Path, main: SendHwnd, message: u32) {
        self.run += 1;
        let run = self.run;
        if let Ok(mut inbox) = INBOX.lock() {
            *inbox = Some(Inbox { run, ..Inbox::default() });
        }
        let spawned = Command::new(&program)
            // The licence was accepted in the launcher before this could run.
            .args(["--accept-license", "--accept-gdpr", "--format=jsonl", "--progress=yes", "--progress-update-interval=250"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(_) => {
                self.test = Test::starting();
                self.test.fail("Speedtest wouldn't start.");
                return;
            }
        };
        self.test = Test::starting();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        *self.child.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((run, child));
        let holder = Arc::clone(&self.child);
        let reader = std::thread::Builder::new().name("orbit-speedtest".into()).stack_size(256 * 1024).spawn(move || {
            if let Some(stdout) = stdout {
                // Bytes, not `lines()`: a name in the computer's own code page isn't UTF-8, and
                // `lines()` would end the test there.
                let mut reader = BufReader::new(stdout);
                let mut bytes = Vec::with_capacity(1024);
                loop {
                    bytes.clear();
                    match reader.read_until(b'\n', &mut bytes) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    let line = String::from_utf8_lossy(&bytes).into_owned();
                    let first = deliver(run, |inbox| inbox.lines.push(line));
                    // One message waits for the window at a time. It takes every line there is.
                    if first {
                        main.post(message, 0, 0);
                    }
                }
            }
            end(&holder, run);
            // Ookla's program says why it stopped on its other output, after its licence notice.
            let mut said = String::new();
            if let Some(mut stderr) = stderr {
                let _ = std::io::Read::read_to_string(&mut stderr, &mut said);
            }
            let why = said.lines().rev().map(str::trim).find(|line| line.to_ascii_lowercase().contains("error")).map(|line| line.trim_start_matches("[error]").trim().to_string());
            deliver(run, |inbox| {
                inbox.ended.get_or_insert_with(|| why.unwrap_or_default());
            });
            main.post(message, 0, 0);
        });
        if reader.is_err() {
            self.stop();
            self.test.fail("Speedtest wouldn't start.");
            return;
        }
        // A test that's stuck, printing nothing, is stopped. This sleeps once; it isn't a timer.
        let holder = Arc::clone(&self.child);
        let _ = std::thread::Builder::new().name("orbit-speedtest-limit".into()).stack_size(64 * 1024).spawn(move || {
            std::thread::sleep(LONGEST);
            let still_running = holder.lock().is_ok_and(|child| child.as_ref().is_some_and(|(of, _)| *of == run));
            if still_running {
                deliver(run, |inbox| inbox.ended = Some("The test took too long, so it was stopped.".to_string()));
                end(&holder, run);
                main.post(message, 0, 0);
            }
        });
    }

    /// Lines have arrived. Gives back true if anything changed.
    pub fn arrived(&mut self) -> bool {
        let Some((lines, ended)) = INBOX.lock().ok().and_then(|mut inbox| {
            let inbox = inbox.as_mut().filter(|inbox| inbox.run == self.run)?;
            Some((std::mem::take(&mut inbox.lines), inbox.ended.take()))
        }) else {
            return false;
        };
        for line in &lines {
            self.test.read(line);
        }
        if let Some(why) = &ended {
            if self.test.running() {
                self.test.fail(if why.is_empty() { "The test stopped before it finished. Check the connection, then press Enter to try again." } else { why });
            }
        }
        !lines.is_empty() || ended.is_some()
    }

    /// Stops a test that's running. What it measured so far is kept, marked as stopped.
    pub fn stop(&mut self) {
        if let Some((_, mut child)) = self.child.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if self.test.running() {
            self.run += 1; // anything still on its way is for a test that's over
            self.test.fail("Stopped.");
        }
    }

    /// Looks for the program again, as when the view is opened.
    pub fn check(&mut self) {
        self.missing = find().is_none();
    }

    /// Installs the program with Windows Package Manager, and posts `message` when it's done.
    pub fn install(&mut self, main: SendHwnd, message: u32) {
        if self.installing {
            return;
        }
        self.installing = true;
        self.install_problem = None;
        let started = std::thread::Builder::new().name("orbit-speedtest-install".into()).spawn(move || {
            if let (Err(problem), Ok(mut slot)) = (install_now(), INSTALL_PROBLEM.lock()) {
                *slot = Some(problem);
            }
            main.post(message, 0, 0);
        });
        if started.is_err() {
            self.installing = false;
            self.install_problem = Some("Couldn't start installing it.".into());
        }
    }

    /// The install finished.
    pub fn installed(&mut self) {
        self.installing = false;
        self.install_problem = INSTALL_PROBLEM.lock().ok().and_then(|mut slot| slot.take());
        self.check();
    }
}

static INSTALL_PROBLEM: Mutex<Option<String>> = Mutex::new(None);

/// Puts something in the inbox for this run. True if the inbox was empty, so the window needs telling.
fn deliver(run: u64, put: impl FnOnce(&mut Inbox)) -> bool {
    let Ok(mut inbox) = INBOX.lock() else { return false };
    let Some(inbox) = inbox.as_mut().filter(|inbox| inbox.run == run) else { return false };
    let was_empty = inbox.lines.is_empty() && inbox.ended.is_none();
    put(inbox);
    was_empty
}

fn install_now() -> Result<(), String> {
    let Some(winget) = crate::integrations::winget() else {
        return Err("Windows Package Manager isn't available. Speedtest CLI can be downloaded from speedtest.net/apps/cli.".into());
    };
    // The licence was shown and accepted in the launcher first.
    let args = ["install", "--id", WINGET_ID, "-e", "--silent", "--accept-source-agreements", "--accept-package-agreements", "--disable-interactivity"];
    let ran = Command::new(&winget).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(CREATE_NO_WINDOW).status();
    match (ran, find()) {
        (_, Some(_)) => Ok(()),
        (Ok(_), None) | (Err(_), None) => Err("Windows Package Manager couldn't install it. Check the connection and try again.".into()),
    }
}

/// Ookla's program, where Windows Package Manager puts it or on the PATH. It's looked for when the
/// test is opened, which is a few folder reads unless it's only on the PATH. Another program called
/// `speedtest`, such as the Python one, takes different options, so it's checked to be Ookla's.
pub fn find() -> Option<PathBuf> {
    let local = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
    let package = std::fs::read_dir(local.join("Microsoft").join("WinGet").join("Packages")).ok().and_then(|entries| {
        entries.flatten().find_map(|entry| {
            let file = entry.path().join(EXE);
            (entry.file_name().to_string_lossy().starts_with(WINGET_ID) && file.is_file()).then_some(file)
        })
    });
    // Windows Package Manager's folder for Ookla's package can only hold Ookla's program.
    package.or_else(|| crate::integrations::find_on_path(EXE).filter(|file| is_ookla(file)))
}

fn is_ookla(program: &Path) -> bool {
    let Ok(child) = Command::new(program).arg("--version").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).creation_flags(CREATE_NO_WINDOW).spawn() else {
        return false;
    };
    child.wait_with_output().is_ok_and(|out| String::from_utf8_lossy(&out.stdout).contains("Speedtest by Ookla"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs a real test, so it's only run when asked, with Ookla's program at `ORBIT_SPEEDTEST_EXE`:
    /// `cargo test -p orbit-windows -- --ignored a_real_test`.
    #[test]
    #[ignore]
    fn a_real_test_is_read_as_it_runs() {
        let program = PathBuf::from(std::env::var("ORBIT_SPEEDTEST_EXE").expect("ORBIT_SPEEDTEST_EXE"));
        let mut speed = SpeedTest::default();
        speed.run_program(&program, SendHwnd(0), 0);
        let started = std::time::Instant::now();
        let mut updates = 0;
        while speed.test.running() && started.elapsed() < Duration::from_secs(90) {
            std::thread::sleep(Duration::from_millis(100));
            if speed.arrived() {
                updates += 1;
            }
        }
        let test = &speed.test;
        println!("{updates} updates; {}; {:?}", test.summary(), test.problem);
        assert_eq!(test.phase, orbit_core::speedtest::Phase::Done);
        assert!(test.download.is_some_and(|speed| speed > 0.0) && test.upload.is_some_and(|speed| speed > 0.0));
        assert!(test.download_samples.len() > 5 && test.upload_samples.len() > 5);
        assert!(speed.child.lock().unwrap().is_none(), "the program is left running");
    }
}
