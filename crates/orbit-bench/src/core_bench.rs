//! Search latency on deterministic synthetic catalogs of 1k, 10k and 100k items.

use crate::report::Report;
use orbit_core::{Hit, QueryScratch, SearchIndex, SearchItem};
use std::hint::black_box;
use std::time::Instant;

const WORDS: &[&str] = &[
    "Adobe", "Visual", "Studio", "Code", "Chrome", "Google", "Firefox", "Terminal", "Windows", "Microsoft", "Office",
    "Word", "Excel", "Power", "Point", "Outlook", "Teams", "Slack", "Discord", "Spotify", "Steam", "Calculator",
    "Notepad", "Paint", "Photo", "Video", "Editor", "Player", "Manager", "Settings", "Control", "Panel", "System",
    "Monitor", "Network", "Remote", "Desktop", "Browser", "Mail", "Calendar", "Notes", "Reader", "Writer", "Viewer",
    "Studio", "Tools", "Utility", "Backup", "Sync", "Cloud", "Drive", "Files", "Explorer", "Python", "Rust", "Node",
    "Docker", "Git", "Blender", "Unity", "Unreal", "Audio", "Music", "Zoom", "Figma", "Sketch", "Linear", "Notion",
];

/// A small deterministic generator, so every run searches identical data.
pub struct Rng(pub u64);

impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

pub fn phrase(rng: &mut Rng, words: usize) -> String {
    let mut out = String::new();
    for i in 0..words {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(WORDS[rng.below(WORDS.len())]);
    }
    out
}

fn build(count: usize) -> SearchIndex {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut index = SearchIndex::with_capacity(count);
    for i in 0..count {
        let words = 1 + rng.below(3);
        let title = phrase(&mut rng, words);
        let title = if i % 3 == 0 { format!("{title} {i}") } else { title };
        let subtitle = format!("C:\\Program Files\\{}\\app{i}.exe", phrase(&mut rng, 1));
        let usage = if i % 17 == 0 { rng.below(600) as u16 } else { 0 };
        index.add(&SearchItem {
            key: format!("app:{i}"),
            title,
            subtitle,
            alias: String::new(),
            keywords: String::new(),
            pinned: i % 251 == 0,
            usage,
        });
    }
    index
}

const QUERIES: &[&str] = &[
    "c", "ch", "chr", "chro", "chrom", "chrome", "v", "vs", "vsc", "vis", "visual stu", "calc", "calculator", "note",
    "term", "chorme", "calcualtor", "zzzqqq", "app123", "program files", "gc", "ms", "power point", "blend", "spot",
];

pub fn run(report: &mut Report, rounds: usize) {
    for (label, count) in [("1k", 1_000usize), ("10k", 10_000), ("100k", 100_000)] {
        let start = Instant::now();
        let index = build(count);
        let build_ms = start.elapsed().as_secs_f64() * 1000.0;
        report.add(format!("search {label}: build index"), build_ms, "ms", false);
        report.add(format!("search {label}: index heap"), index.heap_bytes() as f64 / 1024.0, "KB", true);

        let mut scratch = QueryScratch::default();
        let mut hits: Vec<Hit> = Vec::with_capacity(30);
        // The largest catalog is far beyond any real app list, so fewer rounds keep the run short.
        let rounds = if count >= 100_000 { rounds / 10 } else { rounds };
        let mut samples = Vec::with_capacity(QUERIES.len() * rounds);
        for _ in 0..rounds {
            for query in QUERIES {
                let t = Instant::now();
                index.search(query, 30, &mut scratch, &mut hits);
                black_box(&hits);
                samples.push(t.elapsed().as_secs_f64() * 1000.0);
            }
        }
        report.add_timings(&format!("search {label}: query"), &samples, true);
    }
}
