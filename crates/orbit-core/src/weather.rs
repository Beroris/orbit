//! Weather: what "weather" and "weather London" mean, and Open-Meteo's answers read into a forecast
//! for the launcher to draw. The platform fetches. Nothing here touches the network.

use serde::{Deserialize, Serialize};

/// A place found by name, kept so that plain "weather" shows it next time.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Place {
    pub name: String,
    /// Its region and country, such as "England, United Kingdom".
    pub region: String,
    pub latitude: f64,
    pub longitude: f64,
}

impl Place {
    pub fn label(&self) -> String {
        if self.region.is_empty() { self.name.clone() } else { format!("{}, {}", self.name, self.region) }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Forecast {
    pub now: Now,
    /// The next 24 hours, starting with this one.
    pub hours: Vec<Hour>,
    /// Today and the six days after it.
    pub days: Vec<Day>,
    pub fahrenheit: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Now {
    pub temperature: f64,
    pub feels_like: f64,
    pub humidity: f64,
    pub wind: f64,
    pub code: u8,
    pub is_day: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hour {
    /// "Now", then the hour on a 24-hour clock: "15".
    pub label: String,
    pub temperature: f64,
    pub code: u8,
    pub is_day: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Day {
    /// "Today", then "Mon", "Tue".
    pub label: String,
    pub high: f64,
    pub low: f64,
    pub code: u8,
    /// The highest chance of rain in the day, in percent.
    pub rain: Option<u8>,
}

/// "new york" as "New York", for showing a place as typed. Anything with a capital is left alone.
pub fn capitalized(place: &str) -> String {
    if place.chars().any(char::is_uppercase) {
        return place.to_string();
    }
    let mut out = String::with_capacity(place.len());
    let mut start = true;
    for c in place.chars() {
        if start {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
        start = c == ' ' || c == '-';
    }
    out
}

/// "weather" is `Some("")`, "weather London" or "weather in London" is `Some("London")`.
pub fn parse_query(query: &str) -> Option<&str> {
    let query = query.trim();
    let head = query.get(..7)?;
    if !head.eq_ignore_ascii_case("weather") {
        return None;
    }
    let rest = &query[7..];
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim();
    let rest = rest.strip_prefix("in ").or_else(|| rest.strip_prefix("In ")).unwrap_or(rest).trim();
    Some(rest)
}

fn encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.trim().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(byte as char),
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Open-Meteo's place search, for one result in English. Only the name typed is sent.
pub fn place_url(name: &str) -> String {
    format!("https://geocoding-api.open-meteo.com/v1/search?name={}&count=1&language=en&format=json", encode(name))
}

/// Open-Meteo's forecast. The place goes to two decimals, which is about a kilometre: plenty for
/// weather, and no more exact than that.
pub fn forecast_url(place: &Place, fahrenheit: bool) -> String {
    let units = if fahrenheit { "&temperature_unit=fahrenheit&wind_speed_unit=mph" } else { "" };
    format!(
        "https://api.open-meteo.com/v1/forecast?latitude={:.2}&longitude={:.2}\
         &current=temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,wind_speed_10m,is_day\
         &hourly=temperature_2m,weather_code,is_day&daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max\
         &timezone=auto&forecast_days=7&forecast_hours=24{units}",
        place.latitude, place.longitude
    )
}

#[derive(Deserialize)]
struct Found {
    #[serde(default)]
    results: Vec<FoundPlace>,
}

#[derive(Deserialize)]
struct FoundPlace {
    name: String,
    latitude: f64,
    longitude: f64,
    #[serde(default)]
    admin1: Option<String>,
    #[serde(default)]
    country: Option<String>,
}

/// The first place in an answer from [`place_url`], or `None` if there wasn't one.
pub fn parse_place(json: &str) -> Option<Place> {
    let found: Found = serde_json::from_str(json).ok()?;
    let first = found.results.into_iter().next()?;
    let mut region: Vec<String> = Vec::new();
    for part in [first.admin1, first.country].into_iter().flatten() {
        if !part.is_empty() && part != first.name && !region.contains(&part) {
            region.push(part);
        }
    }
    Some(Place { name: first.name, region: region.join(", "), latitude: first.latitude, longitude: first.longitude })
}

#[derive(Deserialize)]
struct Answer {
    current: Current,
    hourly: Hourly,
    daily: Daily,
}

#[derive(Deserialize)]
struct Current {
    temperature_2m: f64,
    apparent_temperature: f64,
    relative_humidity_2m: f64,
    weather_code: u8,
    wind_speed_10m: f64,
    is_day: u8,
}

#[derive(Deserialize)]
struct Hourly {
    time: Vec<String>,
    temperature_2m: Vec<Option<f64>>,
    weather_code: Vec<Option<u8>>,
    is_day: Vec<Option<u8>>,
}

#[derive(Deserialize)]
struct Daily {
    time: Vec<String>,
    weather_code: Vec<Option<u8>>,
    temperature_2m_max: Vec<Option<f64>>,
    temperature_2m_min: Vec<Option<f64>>,
    #[serde(default)]
    precipitation_probability_max: Vec<Option<f64>>,
}

/// An answer from [`forecast_url`].
pub fn parse_forecast(json: &str, fahrenheit: bool) -> Option<Forecast> {
    let answer: Answer = serde_json::from_str(json).ok()?;
    let c = answer.current;
    let now = Now {
        temperature: c.temperature_2m,
        feels_like: c.apparent_temperature,
        humidity: c.relative_humidity_2m,
        wind: c.wind_speed_10m,
        code: c.weather_code,
        is_day: c.is_day != 0,
    };
    let h = answer.hourly;
    let hours = (0..h.time.len())
        .filter_map(|at| {
            Some(Hour {
                label: if at == 0 { "Now".to_string() } else { h.time[at].get(11..13)?.to_string() },
                temperature: (*h.temperature_2m.get(at)?)?,
                code: h.weather_code.get(at).copied().flatten().unwrap_or(0),
                is_day: h.is_day.get(at).copied().flatten().unwrap_or(1) != 0,
            })
        })
        .collect::<Vec<_>>();
    let d = answer.daily;
    let days = (0..d.time.len())
        .filter_map(|at| {
            Some(Day {
                label: if at == 0 { "Today".to_string() } else { weekday(&d.time[at])?.to_string() },
                high: (*d.temperature_2m_max.get(at)?)?,
                low: (*d.temperature_2m_min.get(at)?)?,
                code: d.weather_code.get(at).copied().flatten().unwrap_or(0),
                rain: d.precipitation_probability_max.get(at).copied().flatten().map(|chance| chance.clamp(0.0, 100.0) as u8),
            })
        })
        .collect::<Vec<_>>();
    (!hours.is_empty() && !days.is_empty()).then_some(Forecast { now, hours, days, fahrenheit })
}

/// "2026-11-09" is "Mon".
fn weekday(date: &str) -> Option<&'static str> {
    let mut parts = date.split('-').map(|part| part.parse::<i64>().ok());
    let (y, m, d) = (parts.next()??, parts.next()??, parts.next()??);
    // Days since 1970-01-01, from Howard Hinnant's days_from_civil. That day was a Thursday.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    const NAMES: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    Some(NAMES[days.rem_euclid(7) as usize])
}

/// What a WMO weather code means, in a few words.
pub fn describe(code: u8) -> &'static str {
    match code {
        0 => "Clear",
        1 => "Mostly clear",
        2 => "Partly cloudy",
        3 => "Cloudy",
        45 | 48 => "Fog",
        51 | 53 | 55 => "Drizzle",
        56 | 57 => "Freezing drizzle",
        61 => "Light rain",
        63 => "Rain",
        65 => "Heavy rain",
        66 | 67 => "Freezing rain",
        71 => "Light snow",
        73 => "Snow",
        75 => "Heavy snow",
        77 => "Snow grains",
        80 | 81 => "Showers",
        82 => "Heavy showers",
        85 | 86 => "Snow showers",
        95 => "Thunderstorm",
        96 | 99 => "Thunderstorm with hail",
        _ => "Unknown",
    }
}

/// The same code as an emoji, drawn in its own colours.
pub fn emoji(code: u8, is_day: bool) -> &'static str {
    match code {
        0 | 1 if !is_day => "\u{1F319}",
        0 => "\u{2600}\u{FE0F}",
        1 => "\u{1F324}\u{FE0F}",
        2 if !is_day => "\u{2601}\u{FE0F}",
        2 => "\u{26C5}",
        3 => "\u{2601}\u{FE0F}",
        45 | 48 => "\u{1F32B}\u{FE0F}",
        51..=57 | 61 | 80 | 81 if is_day && code >= 80 => "\u{1F326}\u{FE0F}",
        51..=67 | 80..=82 => "\u{1F327}\u{FE0F}",
        71..=77 | 85 | 86 => "\u{1F328}\u{FE0F}",
        95..=99 => "\u{26C8}\u{FE0F}",
        _ => "\u{1F321}\u{FE0F}",
    }
}

/// A temperature as it's shown: rounded, with a degree sign. "-0" is "0".
pub fn degrees(value: f64) -> String {
    let rounded = value.round();
    format!("{}°", if rounded == 0.0 { 0.0 } else { rounded })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORECAST: &str = r#"{"latitude":51.5,"longitude":-0.12,"timezone":"Europe/London",
        "current":{"time":"2026-11-08T14:00","temperature_2m":12.4,"apparent_temperature":10.1,"relative_humidity_2m":71,"weather_code":2,"wind_speed_10m":18.3,"is_day":1},
        "hourly":{"time":["2026-11-08T14:00","2026-11-08T15:00","2026-11-08T16:00"],"temperature_2m":[12.4,12.0,null],"weather_code":[2,3,61],"is_day":[1,1,0]},
        "daily":{"time":["2026-11-08","2026-11-09","2026-11-10"],"weather_code":[2,61,0],"temperature_2m_max":[13.1,11.0,9.6],"temperature_2m_min":[7.2,6.0,-0.4],"precipitation_probability_max":[10,80,null]}}"#;

    #[test]
    fn what_weather_queries_mean() {
        assert_eq!(parse_query("weather"), Some(""));
        assert_eq!(capitalized("new york"), "New York");
        assert_eq!(capitalized("stoke-on-trent"), "Stoke-On-Trent");
        assert_eq!(capitalized("McAllen"), "McAllen");
        assert_eq!(parse_query("Weather London"), Some("London"));
        assert_eq!(parse_query("weather in New York"), Some("New York"));
        assert_eq!(parse_query("weatherproof"), None);
        assert_eq!(parse_query("the weather"), None);
    }

    #[test]
    fn a_forecast_is_read_and_gaps_are_skipped() {
        let forecast = parse_forecast(FORECAST, false).expect("a forecast");
        assert_eq!(forecast.now.code, 2);
        assert_eq!(degrees(forecast.now.temperature), "12°");
        assert_eq!(forecast.hours.iter().map(|h| h.label.as_str()).collect::<Vec<_>>(), ["Now", "15"]);
        assert_eq!(forecast.days.iter().map(|d| d.label.as_str()).collect::<Vec<_>>(), ["Today", "Mon", "Tue"]);
        assert_eq!(forecast.days[1].rain, Some(80));
        assert_eq!(forecast.days[2].rain, None);
        assert_eq!(degrees(forecast.days[2].low), "0°");
        assert_eq!(parse_forecast("{\"error\":true}", false), None);
    }

    #[test]
    fn a_place_is_found_and_named() {
        let json = r#"{"results":[{"name":"London","latitude":51.50853,"longitude":-0.12574,"admin1":"England","country":"United Kingdom"}]}"#;
        let place = parse_place(json).expect("a place");
        assert_eq!(place.label(), "London, England, United Kingdom");
        assert_eq!(parse_place(r#"{"generationtime_ms":0.5}"#), None);
        let url = forecast_url(&place, false);
        assert!(url.contains("latitude=51.51&longitude=-0.13"), "{url}");
        assert_eq!(place_url("São Paulo"), "https://geocoding-api.open-meteo.com/v1/search?name=S%C3%A3o+Paulo&count=1&language=en&format=json");
    }

    #[test]
    fn every_code_has_words_and_a_picture() {
        for code in [0, 1, 2, 3, 45, 51, 61, 63, 65, 71, 80, 82, 95, 99] {
            assert_ne!(describe(code), "Unknown", "{code}");
            assert!(!emoji(code, true).is_empty() && !emoji(code, false).is_empty());
        }
    }
}
