//! Exchange rates for currency conversion in the launcher. They're the European Central Bank's daily
//! reference rates: one small public file, fetched on a thread of its own only when a conversion is
//! typed and the copy on disk is more than 12 hours old. There's no timer and nothing at start.
//! Nothing typed is sent. The parsing and the sums are in `orbit_core::currency`.

use crate::util::SendHwnd;
use orbit_core::currency::Rates;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

const URL: &str = "https://www.ecb.europa.eu/stats/eurofxref/eurofxref-daily.xml";
const SITE: &str = "www.ecb.europa.eu";
/// The file is about 2 KB. Anything much bigger isn't it.
const LIMIT: usize = 64 * 1024;
const FILE: &str = "exchange-rates.xml";
/// The bank publishes once each working day, so twice a day is plenty.
const FRESH_FOR: Duration = Duration::from_secs(12 * 60 * 60);
/// After a failure, typing doesn't ask again for this long.
const RETRY_AFTER: Duration = Duration::from_secs(10 * 60);

pub enum Status<'a> {
    Ready(&'a Rates),
    /// None kept yet, and they're being fetched.
    Waiting,
    /// None kept, and the last try failed.
    Unavailable,
}

#[derive(Default)]
pub struct ExchangeRates {
    rates: Option<Rates>,
    read_disk: bool,
    fetched_at: Option<SystemTime>,
    fetching: bool,
    failed_at: Option<Instant>,
}

fn file(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

impl ExchangeRates {
    /// The rates for a conversion just typed. Reads the kept copy the first time, and starts a
    /// fetch when it's missing or old. `fetch` is false in previews and tests, which never go online.
    pub fn get(&mut self, dir: &Path, fetch: bool, main: SendHwnd, message: u32) -> Status<'_> {
        if !self.read_disk {
            self.read_disk = true;
            let path = file(dir);
            self.rates = std::fs::read_to_string(&path).ok().and_then(|xml| Rates::parse_ecb(&xml));
            self.fetched_at = self.rates.as_ref().and_then(|_| std::fs::metadata(&path).ok()?.modified().ok());
        }
        let stale = self.fetched_at.is_none_or(|at| at.elapsed().map_or(true, |age| age > FRESH_FOR));
        let resting = self.failed_at.is_some_and(|at| at.elapsed() < RETRY_AFTER);
        if fetch && stale && !self.fetching && !resting {
            self.fetching = start(dir.to_path_buf(), main, message);
        }
        match &self.rates {
            Some(rates) => Status::Ready(rates),
            None if self.fetching => Status::Waiting,
            None => Status::Unavailable,
        }
    }

    /// A fetch finished. On success the new copy is on disk, and it's read at the next conversion.
    pub fn arrived(&mut self, saved: bool) {
        self.fetching = false;
        if saved {
            self.read_disk = false;
            self.failed_at = None;
        } else {
            self.failed_at = Some(Instant::now());
        }
    }

    /// Turned off: the kept copy goes too.
    pub fn forget(&mut self, dir: &Path) {
        let _ = std::fs::remove_file(file(dir));
        *self = ExchangeRates { fetching: self.fetching, ..ExchangeRates::default() };
    }
}

/// Fetches on a low-priority thread, checks it really is the bank's file, and replaces the kept
/// copy in one step. Posts `message` with 1 if it saved, 0 if not.
fn start(dir: PathBuf, main: SendHwnd, message: u32) -> bool {
    std::thread::Builder::new()
        .name("orbit-exchange-rates".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            crate::favicon::lower_priority();
            let saved = (|| {
                let body = crate::favicon::get_from(URL, SITE, LIMIT)?;
                let text = String::from_utf8(body).ok()?;
                Rates::parse_ecb(&text)?;
                let partial = file(&dir).with_extension("xml.partial");
                std::fs::write(&partial, text.as_bytes()).ok()?;
                std::fs::rename(&partial, file(&dir)).ok()
            })()
            .is_some();
            main.post(message, saved as usize, 0);
        })
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kept_copy_is_used_and_nothing_is_fetched_when_told_not_to() {
        let dir = std::env::temp_dir().join(format!("orbit-rates-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut rates = ExchangeRates::default();
        assert!(matches!(rates.get(&dir, false, SendHwnd(0), 0), Status::Unavailable));
        std::fs::write(file(&dir), "<Cube time='2026-11-06'><Cube currency='GBP' rate='0.85'/></Cube>").unwrap();
        let mut rates = ExchangeRates::default();
        match rates.get(&dir, false, SendHwnd(0), 0) {
            Status::Ready(found) => assert_eq!(found.per_euro("GBP"), Some(0.85)),
            _ => panic!("expected the kept rates"),
        }
        assert!(!rates.fetching);
        rates.forget(&dir);
        assert!(!file(&dir).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Goes online, so it runs only when asked: `cargo test -p orbit-windows -- --ignored banks_file`.
    #[test]
    #[ignore]
    fn the_banks_file_is_where_orbit_asks_for_it() {
        let body = crate::favicon::get_from(URL, SITE, LIMIT).expect("an answer");
        let rates = Rates::parse_ecb(&String::from_utf8(body).unwrap()).expect("the bank's rates");
        for code in ["USD", "GBP", "JPY", "CHF", "INR", "ZAR"] {
            assert!(rates.per_euro(code).is_some(), "{code}");
        }
        println!("rates of {}", rates.day());
    }
}
