//! The timer and stopwatch: reading a length of time someone typed, and writing one out.

/// The longest timer Orbit sets: four days, in seconds.
pub const LONGEST: u64 = 4 * 24 * 3600;

/// Seconds in a length of time typed as "5", "5m", "90s", "1h 30m", "1h30", "2 minutes",
/// "1.5 hours", "1:30" or "1:02:03". A number on its own is minutes. `None` for anything else, for
/// nothing, or for more than [`LONGEST`].
pub fn parse_duration(text: &str) -> Option<u64> {
    let text = text.trim().to_lowercase();
    if text.is_empty() {
        return None;
    }
    let seconds = if text.contains(':') { clock_parts(&text)? } else { worded(&text)? };
    (seconds > 0 && seconds <= LONGEST).then_some(seconds)
}

/// "1:30" is a minute and a half, "1:02:03" an hour, two minutes and three seconds.
fn clock_parts(text: &str) -> Option<u64> {
    let parts: Vec<u64> = text.split(':').map(|part| part.trim().parse::<u64>().ok()).collect::<Option<_>>()?;
    match parts.as_slice() {
        [minutes, seconds] if *seconds < 60 => Some(minutes * 60 + seconds),
        [hours, minutes, seconds] if *minutes < 60 && *seconds < 60 => Some(hours * 3600 + minutes * 60 + seconds),
        _ => None,
    }
}

fn unit_seconds(word: &str) -> Option<f64> {
    Some(match word {
        "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
        "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3600.0,
        _ => return None,
    })
}

/// Numbers each followed by a unit, such as "1h 30m" or "2 minutes 10 seconds". A unit left off
/// the last number is the next one down from the unit before it, so "1h30" is an hour and a half.
fn worded(text: &str) -> Option<u64> {
    let mut total = 0.0f64;
    let mut chars = text.chars().peekable();
    let mut last_unit: Option<f64> = None;
    let mut numbers = 0;
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace() || *c == ',') {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        let mut number = String::new();
        while chars.peek().is_some_and(|c| c.is_ascii_digit() || *c == '.') {
            number.push(chars.next()?);
        }
        let value: f64 = number.parse().ok()?;
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let mut word = String::new();
        while chars.peek().is_some_and(|c| c.is_alphabetic()) {
            word.push(chars.next()?);
        }
        if word == "and" {
            return None;
        }
        numbers += 1;
        let unit = if word.is_empty() {
            match last_unit {
                None => 60.0,
                Some(last) if last > 60.0 => 60.0,
                Some(last) if last > 1.0 => 1.0,
                Some(_) => return None,
            }
        } else {
            unit_seconds(&word)?
        };
        if last_unit.is_some_and(|last| unit >= last) {
            return None;
        }
        total += value * unit;
        last_unit = Some(unit);
        // Another word joined on, as in "1 hour and 5 minutes".
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let rest: String = chars.clone().take(3).collect();
        if rest == "and" {
            chars.nth(2);
        }
    }
    if numbers == 0 || !total.is_finite() {
        return None;
    }
    Some(total.round() as u64)
}

/// A time as a clock shows it: "0:09", "4:59", "1:04:05".
pub fn clock(seconds: u64) -> String {
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 { format!("{hours}:{minutes:02}:{seconds:02}") } else { format!("{minutes}:{seconds:02}") }
}

/// A length of time in words: "5 minutes", "1 hour 30 minutes", "1 minute 30 seconds".
pub fn describe(seconds: u64) -> String {
    let parts = [(seconds / 3600, "hour"), (seconds / 60 % 60, "minute"), (seconds % 60, "second")];
    let words: Vec<String> = parts.iter().filter(|(count, _)| *count > 0).map(|(count, unit)| format!("{count} {unit}{}", if *count == 1 { "" } else { "s" })).collect();
    if words.is_empty() { "0 seconds".to_string() } else { words.join(" ") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_ways_people_type_a_time() {
        assert_eq!(parse_duration("5"), Some(300));
        assert_eq!(parse_duration("5m"), Some(300));
        assert_eq!(parse_duration("5 min"), Some(300));
        assert_eq!(parse_duration("2 minutes"), Some(120));
        assert_eq!(parse_duration("90s"), Some(90));
        assert_eq!(parse_duration("1h 30m"), Some(5400));
        assert_eq!(parse_duration("1h30m"), Some(5400));
        assert_eq!(parse_duration("1h30"), Some(5400));
        assert_eq!(parse_duration("1 hour and 5 minutes"), Some(3900));
        assert_eq!(parse_duration("1.5 hours"), Some(5400));
        assert_eq!(parse_duration("2m 10s"), Some(130));
        assert_eq!(parse_duration("1:30"), Some(90));
        assert_eq!(parse_duration("1:02:03"), Some(3723));
        assert_eq!(parse_duration(" 10 MIN "), Some(600));
    }

    #[test]
    fn turns_down_what_isnt_a_time() {
        for text in ["", "abc", "5 parsecs", "0", "1:75", "10m 1h", "and", "5 weeks", "1:2:3:4", "100000"] {
            assert_eq!(parse_duration(text), None, "{text}");
        }
    }

    #[test]
    fn writes_times_out() {
        assert_eq!(clock(9), "0:09");
        assert_eq!(clock(299), "4:59");
        assert_eq!(clock(3845), "1:04:05");
        assert_eq!(describe(300), "5 minutes");
        assert_eq!(describe(5400), "1 hour 30 minutes");
        assert_eq!(describe(61), "1 minute 1 second");
    }
}
