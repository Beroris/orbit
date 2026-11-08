//! Weather in the launcher. Open-Meteo finds a place by name and gives its forecast. Both are asked
//! only when weather is opened or a place is looked up, on a thread of their own, and a forecast is
//! kept in memory for 15 minutes. The reading of answers is in `orbit_core::weather`.

use crate::util::SendHwnd;
use orbit_core::weather::{Forecast, Place, forecast_url, parse_forecast, parse_place, place_url};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const FRESH_FOR: Duration = Duration::from_secs(15 * 60);
const LIMIT: usize = 256 * 1024;

/// What a lookup found, handed from its thread to the window's.
pub enum Outcome {
    Found { place: Place, forecast: Forecast },
    NoSuchPlace(String),
    Offline,
}

static DONE: Mutex<Option<(u64, Outcome)>> = Mutex::new(None);

#[derive(Default)]
pub struct Weather {
    pub place: Option<Place>,
    pub forecast: Option<Forecast>,
    fetched: Option<Instant>,
    /// Which lookup is running, so a slow old one can't replace a newer answer.
    asked: u64,
    pub busy: bool,
    pub problem: Option<String>,
    /// Emoji drawn at a size, worked out once each.
    pub icons: Vec<(&'static str, i32, Vec<u8>)>,
}

impl Weather {
    /// Looks up `name`, or with an empty name the place already known. A forecast for the same place
    /// from the last 15 minutes is used as it is.
    pub fn look_up(&mut self, name: &str, known: Option<&Place>, fahrenheit: bool, main: SendHwnd, message: u32) {
        let name = name.trim();
        let same_place = name.is_empty() || self.place.as_ref().is_some_and(|place| place.name.eq_ignore_ascii_case(name));
        let fresh = self.fetched.is_some_and(|at| at.elapsed() < FRESH_FOR) && self.forecast.as_ref().is_some_and(|f| f.fahrenheit == fahrenheit);
        if same_place && fresh && self.place.is_some() {
            return;
        }
        let place = if name.is_empty() { known.cloned().or_else(|| self.place.clone()) } else { None };
        if name.is_empty() && place.is_none() {
            return;
        }
        if !same_place {
            self.forecast = None;
        }
        self.problem = None;
        self.busy = true;
        self.asked += 1;
        let asked = self.asked;
        let name = name.to_string();
        let started = std::thread::Builder::new().name("orbit-weather".into()).stack_size(256 * 1024).spawn(move || {
            crate::favicon::lower_priority();
            let outcome = fetch(&name, place, fahrenheit);
            if let Ok(mut done) = DONE.lock() {
                *done = Some((asked, outcome));
            }
            main.post(message, 0, 0);
        });
        if started.is_err() {
            self.busy = false;
            self.problem = Some("Couldn't start looking that up.".into());
        }
    }

    /// A lookup finished. Returns the place it found, for keeping, if it's a new one.
    pub fn arrived(&mut self) -> Option<Place> {
        let (asked, outcome) = DONE.lock().ok()?.take()?;
        if asked != self.asked {
            return None;
        }
        self.busy = false;
        match outcome {
            Outcome::Found { place, forecast } => {
                let new = self.place.as_ref() != Some(&place);
                self.place = Some(place.clone());
                self.forecast = Some(forecast);
                self.fetched = Some(Instant::now());
                new.then_some(place)
            }
            Outcome::NoSuchPlace(name) => {
                self.problem = Some(format!("Open-Meteo doesn't know a place called \u{201C}{name}\u{201D}."));
                None
            }
            Outcome::Offline => {
                self.problem = Some("Couldn't reach Open-Meteo. Check the connection, then press Enter to try again.".into());
                None
            }
        }
    }

    /// Turned off, or the launcher shedding memory: nothing is kept.
    pub fn forget(&mut self) {
        let asked = self.asked;
        *self = Weather { asked, ..Weather::default() };
    }
}

fn fetch(name: &str, place: Option<Place>, fahrenheit: bool) -> Outcome {
    let place = match place {
        Some(place) => place,
        None => {
            let Some(body) = crate::favicon::get_from(&place_url(name), "geocoding-api.open-meteo.com", LIMIT) else { return Outcome::Offline };
            match parse_place(&String::from_utf8_lossy(&body)) {
                Some(place) => place,
                None => return Outcome::NoSuchPlace(name.to_string()),
            }
        }
    };
    let Some(body) = crate::favicon::get_from(&forecast_url(&place, fahrenheit), "api.open-meteo.com", LIMIT) else { return Outcome::Offline };
    match parse_forecast(&String::from_utf8_lossy(&body), fahrenheit) {
        Some(forecast) => Outcome::Found { place, forecast },
        None => Outcome::Offline,
    }
}

/// Fahrenheit and miles an hour where Windows is set to US measurements.
pub fn uses_fahrenheit() -> bool {
    use windows::Win32::Globalization::{GetLocaleInfoEx, LOCALE_IMEASURE};
    let mut value = [0u16; 4];
    let length = unsafe { GetLocaleInfoEx(windows::core::PCWSTR::null(), LOCALE_IMEASURE, Some(&mut value)) };
    length > 0 && value[0] == u16::from(b'1')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goes online, so it runs only when asked: `cargo test -p orbit-windows -- --ignored open_meteo`.
    #[test]
    #[ignore]
    fn open_meteo_answers_as_orbit_reads_it() {
        match fetch("London", None, false) {
            Outcome::Found { place, forecast } => {
                println!("{} now {}", place.label(), orbit_core::weather::degrees(forecast.now.temperature));
                assert_eq!(forecast.days.len(), 7);
                assert!(forecast.hours.len() >= 20);
            }
            Outcome::NoSuchPlace(_) => panic!("no London"),
            Outcome::Offline => panic!("offline"),
        }
    }
}
