//! `orbit-bench`: measures Orbit and fails if it got slower or larger.
//!
//!   orbit-bench [core] [data] [e2e] [features] [--out DIR] [--name NAME] [--baseline FILE]
//!               [--write-baseline FILE] [--orbit PATH] [--opens N] [--idle-secs N]
//!               [--records N] [--pressure-mb N] [--real-app NAME:PROCESS] [--work-dir DIR]
//!
//! The e2e suite briefly takes the keyboard focus. It opens its own small window, injects
//! keys only while that window or Orbit is focused, and stops if anything else takes focus.

mod core_bench;
mod data_bench;
#[cfg(windows)]
mod e2e;
#[cfg(target_os = "macos")]
mod e2e_macos;
mod report;

use report::{Report, compare};
use std::path::PathBuf;

pub struct E2eOptions {
    pub orbit: PathBuf,
    pub opens: usize,
    pub idle_secs: u64,
    pub records: usize,
    pub pressure_mb: usize,
    pub real_app: Option<(String, String)>,
    pub work_dir: PathBuf,
}

/// `orbit-bench rank APPS_JSON QUERY`: prints the top results for a query, to check relevance.
fn rank(apps: &str, query: &str) {
    let text = std::fs::read_to_string(apps).expect("read apps");
    let value: serde_json::Value = serde_json::from_str(&text).expect("parse apps");
    let mut index = orbit_core::SearchIndex::new();
    for app in value.as_array().expect("array") {
        index.add(&orbit_core::SearchItem {
            key: app["id"].as_str().unwrap_or("").to_string(),
            title: app["name"].as_str().unwrap_or("").to_string(),
            ..Default::default()
        });
    }
    let mut scratch = orbit_core::QueryScratch::default();
    let mut hits = Vec::new();
    index.search(query, 8, &mut scratch, &mut hits);
    for hit in &hits {
        println!("{:>6} {:?} {}", hit.score, hit.class, index.title(hit.item));
    }
    println!("{} hits", hits.len());
}

/// Fills a data folder with made-up clipboard items, to see how the list looks.
fn seed_clips(dir: &str) {
    use orbit_data::{NewClip, RetentionPolicy, Store};
    let dir = std::path::Path::new(dir);
    let policy = RetentionPolicy { max_items: 100, retention_days: 0 };
    let store = Store::open(dir, true).expect("store");
    for text in ["https://example.com/a-link-someone-copied", "Meeting moved to 3pm on Thursday", "cargo test --workspace --release"] {
        store.add_clip(&NewClip::text(text.to_string(), "notepad.exe"), &policy).expect("clip");
    }
    let images = dir.join("clipboard-images");
    std::fs::create_dir_all(&images).expect("images");
    for (name, width, height, tint) in [("a.dib", 240u32, 135u32, 0u32), ("b.dib", 120, 200, 1), ("c.dib", 640, 120, 2)] {
        let mut bytes = Vec::with_capacity(40 + (width * height * 4) as usize);
        bytes.extend_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&32u16.to_le_bytes());
        bytes.extend_from_slice(&[0; 24]);
        for y in 0..height {
            for x in 0..width {
                let (u, v) = ((x * 255 / width) as u8, (y * 255 / height) as u8);
                let pixel = match tint {
                    0 => [v, 80, u, 0],
                    1 => [u, v, 200, 0],
                    _ => [60, u, v, 0],
                };
                bytes.extend_from_slice(&pixel);
            }
        }
        std::fs::write(images.join(name), &bytes).expect("image");
        let clip = NewClip::image(name.to_string(), width, height, bytes.len() as u64, tint as u64 + 1, "mspaint.exe");
        store.add_clip(&clip, &policy).expect("clip");
    }
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("seed-clips") {
        seed_clips(&std::env::args().nth(2).expect("data folder"));
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("rank") {
        rank(&std::env::args().nth(2).expect("apps.json"), &std::env::args().nth(3).expect("query"));
        return;
    }
    let mut suites: Vec<String> = Vec::new();
    let mut out_dir = PathBuf::from("bench-results");
    let mut baseline: Option<PathBuf> = None;
    let mut write_baseline: Option<PathBuf> = None;
    let mut orbit: Option<PathBuf> = None;
    let mut opens = 60usize;
    let mut idle_secs = 30u64;
    let mut records = 0usize;
    let mut pressure_mb = 0usize;
    let mut real_app = None;
    let mut work_dir: Option<PathBuf> = None;
    let mut name = String::from("results");

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().unwrap_or_else(|| panic!("{arg} needs a value"));
        match arg.as_str() {
            "core" | "data" | "e2e" | "features" => suites.push(arg.clone()),
            "--out" => out_dir = PathBuf::from(value()),
            "--name" => name = value(),
            "--baseline" => baseline = Some(PathBuf::from(value())),
            "--write-baseline" => write_baseline = Some(PathBuf::from(value())),
            "--orbit" => orbit = Some(PathBuf::from(value())),
            "--opens" => opens = value().parse().expect("--opens"),
            "--idle-secs" => idle_secs = value().parse().expect("--idle-secs"),
            "--records" => records = value().parse().expect("--records"),
            "--pressure-mb" => pressure_mb = value().parse().expect("--pressure-mb"),
            "--work-dir" => work_dir = Some(PathBuf::from(value())),
            "--real-app" => {
                let text = value();
                let (app, process) = text.split_once(':').expect("--real-app NAME:PROCESS");
                real_app = Some((app.to_string(), process.to_string()));
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    if suites.is_empty() {
        suites = vec!["core".into(), "data".into()];
    }
    std::fs::create_dir_all(&out_dir).expect("create output directory");
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_default();

    let mut report = Report::default();
    report.environment.insert(
        "profile".into(),
        if cfg!(debug_assertions) { "debug (numbers are not representative)" } else { "release" }.into(),
    );
    report.environment.insert("os".into(), std::env::consts::OS.to_string());
    report.environment.insert("arch".into(), std::env::consts::ARCH.to_string());
    report.environment.insert("orbit version".into(), env!("CARGO_PKG_VERSION").to_string());
    #[cfg(windows)]
    e2e::describe_environment(&mut report);
    #[cfg(target_os = "macos")]
    e2e_macos::describe_environment(&mut report);

    for suite in &suites {
        eprintln!("running {suite}...");
        match suite.as_str() {
            "core" => core_bench::run(&mut report, 200),
            "data" => data_bench::run(&mut report, &std::env::temp_dir().join("orbit-bench-data")),
            "features" => {
                #[cfg(windows)]
                e2e::features::run(
                    &mut report,
                    &E2eOptions {
                        orbit: orbit.clone().unwrap_or_else(|| exe_dir.join("orbit.exe")),
                        opens,
                        idle_secs,
                        records,
                        pressure_mb,
                        real_app: real_app.clone(),
                        work_dir: std::env::temp_dir().join("orbit-bench-e2e"),
                    },
                );
                #[cfg(not(windows))]
                report.not_tested.push("features suite: the driver for this OS is not written yet".to_string());
            }
            "e2e" => {
                #[cfg(windows)]
                {
                    let _ = &work_dir;
                    let options = E2eOptions {
                        orbit: orbit.clone().unwrap_or_else(|| exe_dir.join("orbit.exe")),
                        opens,
                        idle_secs,
                        records,
                        pressure_mb,
                        real_app: real_app.clone(),
                        work_dir: std::env::temp_dir().join("orbit-bench-e2e"),
                    };
                    e2e::run(&mut report, &options);
                }
                #[cfg(target_os = "macos")]
                {
                    let options = E2eOptions {
                        orbit: orbit.clone().unwrap_or_else(|| exe_dir.join("Orbit")),
                        opens,
                        idle_secs,
                        records,
                        pressure_mb,
                        real_app: real_app.clone(),
                        work_dir: work_dir.clone().unwrap_or_else(|| std::env::temp_dir().join("orbit-bench-e2e")),
                    };
                    e2e_macos::run(&mut report, &options);
                }
                #[cfg(not(any(windows, target_os = "macos")))]
                {
                    let _ = (&orbit, &exe_dir, opens, idle_secs, records, pressure_mb, &real_app, &work_dir);
                    report.not_tested.push("e2e suite: the driver for this OS is not written yet".to_string());
                }
            }
            _ => unreachable!(),
        }
    }

    #[cfg(windows)]
    e2e::describe_environment_at_end(&mut report);

    let comparison = baseline.as_ref().map(|path| {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read baseline {}: {e}", path.display()));
        let base: Report = serde_json::from_str(&text).expect("parse baseline");
        compare(&base, &report)
    });

    let json = serde_json::to_string_pretty(&report).expect("serialize");
    let markdown = report.to_markdown(comparison.as_ref());
    std::fs::write(out_dir.join(format!("{name}.json")), &json).expect("write json");
    std::fs::write(out_dir.join(format!("{name}.md")), &markdown).expect("write markdown");
    println!("{markdown}");
    if let Some(path) = write_baseline {
        std::fs::write(&path, &json).expect("write baseline");
        eprintln!("baseline written to {}", path.display());
    }

    let failed_checks = report.checks.iter().filter(|c| c.passed != c.total).count();
    let regressions = comparison.as_ref().map_or(0, |c| c.regressions.len());
    if regressions > 0 {
        eprintln!("FAILED: {regressions} metric(s) regressed past the gate");
    }
    if failed_checks > 0 {
        eprintln!("FAILED: {failed_checks} check(s) did not pass");
    }
    if regressions > 0 || failed_checks > 0 {
        std::process::exit(1);
    }
}
