//! SQLite: search, list and write latency at 10k records, on a real file in a temp directory.

use crate::core_bench::{Rng, phrase};
use crate::report::Report;
use orbit_data::{NewClip, RetentionPolicy, Store};
use std::path::Path;
use std::time::Instant;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// Fills a store with deterministic notes and clips. Used here and by the e2e data profile.
pub fn seed(store: &Store, notes: usize, clips: usize) {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    let policy = RetentionPolicy { max_items: (clips as u32).max(1), retention_days: 0 };
    for i in 0..notes {
        let title = phrase(&mut rng, 4);
        let body_words = 20 + rng.below(120);
        let body = phrase(&mut rng, body_words);
        store.create_note(&format!("{title} {i}\n{body}")).expect("seed note");
    }
    for i in 0..clips {
        let words = 3 + rng.below(40);
        let text = format!("{} {i}", phrase(&mut rng, words));
        store.add_clip(&NewClip::text(text, "bench.exe"), &policy).expect("seed clip");
    }
}

pub fn run(report: &mut Report, dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).expect("temp dir");
    let start = Instant::now();
    let store = Store::open(dir, true).expect("open store");
    report.add("data: open store (new)", ms(start), "ms", false);

    let count = 10_000;
    let start = Instant::now();
    seed(&store, count, count);
    report.add("data: seed 10k notes + 10k clips", ms(start), "ms", false);
    drop(store);

    let start = Instant::now();
    let store = Store::open(dir, true).expect("reopen store");
    report.add("data: open store (10k)", ms(start), "ms", true);

    let queries = ["chrome", "visual studio", "notes", "rust docker", "zzzqqq", "cal", "power", "sync cloud"];
    let mut note_samples = Vec::new();
    let mut clip_samples = Vec::new();
    for _ in 0..40 {
        for query in queries {
            let t = Instant::now();
            let _ = store.search_notes(query, 50).expect("search");
            note_samples.push(ms(t));
            let t = Instant::now();
            let _ = store.search_clips(query, 200).expect("search");
            clip_samples.push(ms(t));
        }
    }
    report.add_timings("data 10k: note search", &note_samples, true);
    report.add_timings("data 10k: clipboard search", &clip_samples, true);

    let mut samples = Vec::new();
    for _ in 0..100 {
        let t = Instant::now();
        let _ = store.recent_clips(200).expect("recent");
        samples.push(ms(t));
    }
    report.add_timings("data 10k: recent clipboard list", &samples, true);

    let mut samples = Vec::new();
    for _ in 0..100 {
        let t = Instant::now();
        let _ = store.recent_notes(200).expect("recent");
        samples.push(ms(t));
    }
    report.add_timings("data 10k: recent notes list", &samples, true);

    let policy = RetentionPolicy { max_items: count as u32, retention_days: 0 };
    let mut samples = Vec::new();
    for i in 0..200 {
        let t = Instant::now();
        store.add_clip(&NewClip::text(format!("new clip {i}"), "bench.exe"), &policy).expect("add");
        samples.push(ms(t));
    }
    report.add_timings("data 10k: add clip", &samples, true);

    let mut samples = Vec::new();
    let id = store.create_note("autosave target").expect("note");
    for i in 0..200 {
        let t = Instant::now();
        store.save_note(id, &format!("autosave target\nedit number {i}")).expect("save");
        samples.push(ms(t));
    }
    report.add_timings("data 10k: note autosave", &samples, true);

    report.add("data 10k: SQLite heap after queries", Store::sqlite_memory_bytes() as f64 / 1024.0, "KB", true);
    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}
