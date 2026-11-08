//! Timing capture for the benchmark harness.
//!
//! Timestamps are QPC ticks, which are comparable across processes on one machine, so the
//! external driver in `orbit-bench` can line them up with the moment it injected a key.
//! When no log path was given this module does nothing beyond one branch and a few counters.

use crate::util::qpc_frequency;
use std::io::Write;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default)]
pub struct ShowTimes {
    /// T0: hotkey message received.
    pub t0: i64,
    /// T1: show requested, after the first frame was drawn into the backbuffer.
    pub t1: i64,
    /// T2: window shown and first frame copied to it.
    pub t2: i64,
    /// T2b: `DwmFlush` returned after T2, so the compositor has processed the frame.
    /// Only captured in bench mode, because the flush blocks for up to a refresh interval.
    pub t2_composed: i64,
    /// T3: the launcher is the foreground window and the input has keyboard focus.
    pub t3: i64,
}

pub struct Metrics {
    log_path: Option<PathBuf>,
    lines: Vec<String>,
    pub bench_mode: bool,
    /// Bounded summaries for the diagnostics page. The last 64 of each kind.
    show_ms: Vec<f32>,
    query_ms: Vec<f32>,
    pub shows_total: u64,
    pub queries_total: u64,
}

const SAMPLES: usize = 64;

fn push_bounded(list: &mut Vec<f32>, value: f32) {
    if list.len() == SAMPLES {
        list.remove(0);
    }
    list.push(value);
}

pub fn percentile(values: &[f32], p: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f32::total_cmp);
    let index = ((sorted.len() - 1) as f32 * p).round() as usize;
    sorted[index]
}

impl Metrics {
    pub fn new(log_path: Option<PathBuf>) -> Self {
        let bench_mode = log_path.is_some();
        Metrics {
            log_path,
            lines: Vec::new(),
            bench_mode,
            show_ms: Vec::new(),
            query_ms: Vec::new(),
            shows_total: 0,
            queries_total: 0,
        }
    }

    pub fn record_show(&mut self, source: &str, t: ShowTimes) {
        let freq = qpc_frequency() as f64;
        self.shows_total += 1;
        push_bounded(&mut self.show_ms, ((t.t3 - t.t0) as f64 * 1000.0 / freq) as f32);
        if self.bench_mode {
            self.lines.push(format!("show,{source},{},{},{},{},{}", t.t0, t.t1, t.t2, t.t2_composed, t.t3));
        }
    }

    /// T4 query received, T5 results ranked, T6 results drawn.
    pub fn record_query(&mut self, query_len: usize, hits: usize, t4: i64, t5: i64, t6: i64) {
        let freq = qpc_frequency() as f64;
        self.queries_total += 1;
        push_bounded(&mut self.query_ms, ((t6 - t4) as f64 * 1000.0 / freq) as f32);
        if self.bench_mode {
            self.lines.push(format!("query,{query_len},{hits},{t4},{t5},{t6}"));
        }
    }

    /// Enter-key received to action issued.
    pub fn record_action(&mut self, kind: &str, t_enter: i64, t_dispatched: i64) {
        if self.bench_mode {
            self.lines.push(format!("action,{kind},{t_enter},{t_dispatched}"));
        }
    }

    pub fn record_hook_dispatch(&mut self, t_hook: i64, t_received: i64) {
        if self.bench_mode {
            self.lines.push(format!("hyper,{t_hook},{t_received}"));
        }
    }

    pub fn show_summary(&self) -> (f32, f32) {
        (percentile(&self.show_ms, 0.5), percentile(&self.show_ms, 0.95))
    }

    pub fn query_summary(&self) -> (f32, f32) {
        (percentile(&self.query_ms, 0.5), percentile(&self.query_ms, 0.95))
    }

    /// Writes out pending lines. Called at clean exit and when the bench driver asks.
    pub fn flush(&mut self) {
        let Some(path) = &self.log_path else {
            return;
        };
        if self.lines.is_empty() {
            return;
        }
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            for line in self.lines.drain(..) {
                let _ = writeln!(file, "{line}");
            }
        }
    }
}
