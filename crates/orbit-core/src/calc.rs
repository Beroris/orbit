//! Local calculator and fixed-table unit conversion. No network. Currencies are in `currency.rs`.

#[derive(Clone, Debug, PartialEq)]
pub enum CalcOutcome {
    /// The query does not look like maths, so show no calculator row.
    NotMath,
    /// `copy` is the bare number. `reverse` is a conversion asked the other way round, such as
    /// "3.6576 m in ft" for "12 ft in m".
    /// `units` is a conversion's amount as given and its two units, "12", "ft", "m".
    Value { display: String, copy: String, note: Option<String>, reverse: Option<String>, units: Option<(String, &'static str, &'static str)> },
    /// It looks like maths but has no answer. Shown so an error is not mistaken for no input.
    Error(String),
}

/// True for input worth attempting. Keeps plain words and lone numbers out of the results.
fn looks_like_math(q: &str) -> bool {
    let has_digit = q.bytes().any(|b| b.is_ascii_digit());
    let lower = q.to_ascii_lowercase();
    let starts_with_fn = FUNCTIONS.iter().any(|f| {
        lower.strip_prefix(f).is_some_and(|rest| rest.trim_start().starts_with('('))
    });
    if !has_digit && !starts_with_fn && !lower.contains("pi") {
        return false;
    }
    let first = q.chars().next().unwrap_or(' ');
    if !(first.is_ascii_digit() || matches!(first, '(' | '-' | '+' | '.') || starts_with_fn || lower.starts_with("pi")) {
        return false;
    }
    let has_operator = q.bytes().any(|b| matches!(b, b'+' | b'*' | b'/' | b'^' | b'%' | b'(' ))
        || q.get(1..).is_some_and(|rest| rest.contains('-'));
    has_operator || starts_with_fn
}

pub fn evaluate(query: &str) -> CalcOutcome {
    let q = query.trim();
    if q.is_empty() {
        return CalcOutcome::NotMath;
    }
    if let Some(outcome) = try_conversion(q) {
        return outcome;
    }
    if !looks_like_math(q) {
        return CalcOutcome::NotMath;
    }
    match (Parser { s: q.as_bytes(), i: 0, depth: 0 }).parse_all() {
        Ok(value) if value.is_finite() => {
            let text = format_number(value);
            CalcOutcome::Value { display: text.clone(), copy: text, note: None, reverse: None, units: None }
        }
        Ok(value) if value.is_nan() => CalcOutcome::Error("Not a number".into()),
        Ok(_) => CalcOutcome::Error("Result is too large".into()),
        // Words Orbit does not know mean this is ordinary text like "7-zip" or "3-day plan".
        Err(message) if message.starts_with("Unknown word") || trailing_words(q) => {
            let _ = message;
            CalcOutcome::NotMath
        }
        Err(message) => CalcOutcome::Error(message),
    }
}

/// True if the text ends in letters, like "10-20 things", rather than an unfinished sum.
fn trailing_words(q: &str) -> bool {
    q.split_ascii_whitespace().last().is_some_and(|word| {
        word.chars().all(|c| c.is_ascii_alphabetic()) && !matches!(word.to_ascii_lowercase().as_str(), "pi" | "e")
    })
}

/// Up to 12 significant digits with no trailing zeros, so 0.1+0.2 shows 0.3.
pub fn format_number(value: f64) -> String {
    if value == 0.0 {
        return "0".into();
    }
    let magnitude = value.abs();
    if !(1e-6..1e15).contains(&magnitude) {
        return format!("{value:.6e}");
    }
    let decimals = (11 - magnitude.log10().floor() as i32).clamp(0, 15) as usize;
    let text = format!("{value:.decimals$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    }
}

/// A number from [`format_number`] with its thousands grouped: "1234567.5" is "1,234,567.5".
/// Numbers written with an exponent are left as they are.
pub fn group_thousands(text: &str) -> String {
    if text.contains('e') {
        return text.to_string();
    }
    let (sign, rest) = text.strip_prefix('-').map_or(("", text), |rest| ("-", rest));
    let (whole, fraction) = rest.split_once('.').map_or((rest, None), |(whole, fraction)| (whole, Some(fraction)));
    let mut out = String::with_capacity(text.len() + whole.len() / 3);
    out.push_str(sign);
    for (at, digit) in whole.chars().enumerate() {
        if at > 0 && (whole.len() - at) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    if let Some(fraction) = fraction {
        out.push('.');
        out.push_str(fraction);
    }
    out
}

const FUNCTIONS: &[&str] = &[
    "sqrt", "abs", "ln", "log", "sin", "cos", "tan", "round", "floor", "ceil",
];

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    depth: u32,
}

impl Parser<'_> {
    fn parse_all(mut self) -> Result<f64, String> {
        let value = self.expr()?;
        self.ws();
        if self.i < self.s.len() {
            return Err("Could not read the whole expression".into());
        }
        Ok(value)
    }

    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(u8::is_ascii_whitespace) {
            self.i += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.s.get(self.i).copied()
    }

    fn expr(&mut self) -> Result<f64, String> {
        let mut value = self.term()?;
        loop {
            match self.peek() {
                Some(b'+') => {
                    self.i += 1;
                    value += self.term()?;
                }
                Some(b'-') => {
                    self.i += 1;
                    value -= self.term()?;
                }
                _ => return Ok(value),
            }
        }
    }

    fn term(&mut self) -> Result<f64, String> {
        let mut value = self.unary()?;
        loop {
            match self.peek() {
                Some(b'*') | Some(b'x') => {
                    self.i += 1;
                    value *= self.unary()?;
                }
                Some(b'/') => {
                    self.i += 1;
                    let divisor = self.unary()?;
                    if divisor == 0.0 {
                        return Err("Division by zero".into());
                    }
                    value /= divisor;
                }
                Some(b'%') => {
                    self.i += 1;
                    // A trailing % means percent, between operands it means modulo.
                    match self.peek() {
                        None | Some(b')') | Some(b'+') | Some(b'-') | Some(b'*') | Some(b'/') => {
                            value /= 100.0
                        }
                        _ => {
                            let divisor = self.unary()?;
                            if divisor == 0.0 {
                                return Err("Division by zero".into());
                            }
                            value %= divisor;
                        }
                    }
                }
                _ => return Ok(value),
            }
        }
    }

    fn unary(&mut self) -> Result<f64, String> {
        match self.peek() {
            Some(b'-') => {
                self.i += 1;
                Ok(-self.unary()?)
            }
            Some(b'+') => {
                self.i += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Result<f64, String> {
        let base = self.atom()?;
        if self.peek() == Some(b'^') {
            self.i += 1;
            let exponent = self.unary()?; // right associative
            return Ok(base.powf(exponent));
        }
        Ok(base)
    }

    fn atom(&mut self) -> Result<f64, String> {
        self.depth += 1;
        if self.depth > 64 {
            return Err("Expression is nested too deeply".into());
        }
        let result = self.atom_inner();
        self.depth -= 1;
        result
    }

    fn atom_inner(&mut self) -> Result<f64, String> {
        match self.peek() {
            None => Err("Expression is incomplete".into()),
            Some(b'(') => {
                self.i += 1;
                let value = self.expr()?;
                if self.peek() != Some(b')') {
                    return Err("Missing closing bracket".into());
                }
                self.i += 1;
                Ok(value)
            }
            Some(c) if c.is_ascii_digit() || c == b'.' => self.number(),
            Some(c) if c.is_ascii_alphabetic() => self.word(),
            Some(c) => Err(format!("Unexpected character {:?}", c as char)),
        }
    }

    fn number(&mut self) -> Result<f64, String> {
        let start = self.i;
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_digit() || matches!(c, b'.' | b',' | b'_')) {
            self.i += 1;
        }
        if matches!(self.s.get(self.i), Some(b'e' | b'E'))
            && self.s.get(self.i + 1).is_some_and(|c| c.is_ascii_digit() || matches!(c, b'-' | b'+'))
        {
            self.i += 2;
            while self.s.get(self.i).is_some_and(u8::is_ascii_digit) {
                self.i += 1;
            }
        }
        let text: String = self.s[start..self.i]
            .iter()
            .filter(|c| !matches!(c, b',' | b'_'))
            .map(|&c| c as char)
            .collect();
        text.parse().map_err(|_| "Could not read a number".to_string())
    }

    fn word(&mut self) -> Result<f64, String> {
        let start = self.i;
        while self.s.get(self.i).is_some_and(u8::is_ascii_alphabetic) {
            self.i += 1;
        }
        let word = std::str::from_utf8(&self.s[start..self.i]).unwrap_or("").to_ascii_lowercase();
        match word.as_str() {
            "pi" => return Ok(std::f64::consts::PI),
            "e" => return Ok(std::f64::consts::E),
            _ => {}
        }
        if !FUNCTIONS.contains(&word.as_str()) {
            return Err(format!("Unknown word {word:?}"));
        }
        if self.peek() != Some(b'(') {
            return Err(format!("{word} needs brackets"));
        }
        self.i += 1;
        let x = self.expr()?;
        if self.peek() != Some(b')') {
            return Err("Missing closing bracket".into());
        }
        self.i += 1;
        Ok(match word.as_str() {
            "sqrt" => {
                if x < 0.0 {
                    return Err("Square root of a negative number".into());
                }
                x.sqrt()
            }
            "abs" => x.abs(),
            "ln" => x.ln(),
            "log" => x.log10(),
            "sin" => x.sin(),
            "cos" => x.cos(),
            "tan" => x.tan(),
            "round" => x.round(),
            "floor" => x.floor(),
            "ceil" => x.ceil(),
            _ => unreachable!(),
        })
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Dim {
    Length,
    Mass,
    Volume,
    Time,
    Data,
    Speed,
    Area,
    Temperature,
}

/// Name, dimension, factor to the dimension's base unit. Fixed definitions only.
const UNITS: &[(&str, Dim, f64)] = &[
    ("mm", Dim::Length, 0.001),
    ("cm", Dim::Length, 0.01),
    ("m", Dim::Length, 1.0),
    ("km", Dim::Length, 1000.0),
    ("in", Dim::Length, 0.0254),
    ("ft", Dim::Length, 0.3048),
    ("yd", Dim::Length, 0.9144),
    ("mi", Dim::Length, 1609.344),
    ("mg", Dim::Mass, 0.001),
    ("g", Dim::Mass, 1.0),
    ("kg", Dim::Mass, 1000.0),
    ("t", Dim::Mass, 1_000_000.0),
    ("oz", Dim::Mass, 28.349523125),
    ("lb", Dim::Mass, 453.59237),
    ("st", Dim::Mass, 6350.29318),
    ("ml", Dim::Volume, 0.001),
    ("l", Dim::Volume, 1.0),
    ("gal", Dim::Volume, 3.785411784),
    ("pt", Dim::Volume, 0.473176473),
    ("cup", Dim::Volume, 0.2365882365),
    ("ms", Dim::Time, 0.001),
    ("s", Dim::Time, 1.0),
    ("min", Dim::Time, 60.0),
    ("h", Dim::Time, 3600.0),
    ("day", Dim::Time, 86_400.0),
    ("week", Dim::Time, 604_800.0),
    ("b", Dim::Data, 1.0),
    ("kb", Dim::Data, 1e3),
    ("mb", Dim::Data, 1e6),
    ("gb", Dim::Data, 1e9),
    ("tb", Dim::Data, 1e12),
    ("kib", Dim::Data, 1024.0),
    ("mib", Dim::Data, 1_048_576.0),
    ("gib", Dim::Data, 1_073_741_824.0),
    ("tib", Dim::Data, 1_099_511_627_776.0),
    ("kph", Dim::Speed, 1.0 / 3.6),
    ("kmh", Dim::Speed, 1.0 / 3.6),
    ("mph", Dim::Speed, 0.44704),
    ("mps", Dim::Speed, 1.0),
    ("knots", Dim::Speed, 0.514444),
    ("sqm", Dim::Area, 1.0),
    ("sqft", Dim::Area, 0.09290304),
    ("acre", Dim::Area, 4046.8564224),
    ("hectare", Dim::Area, 10_000.0),
    ("c", Dim::Temperature, 0.0),
    ("f", Dim::Temperature, 0.0),
    ("k", Dim::Temperature, 0.0),
];

/// Unit names as people write them, to the short name above, which answers are given in.
const UNIT_WORDS: &[(&str, &str)] = &[
    ("millimetre", "mm"), ("millimetres", "mm"), ("millimeter", "mm"), ("millimeters", "mm"),
    ("centimetre", "cm"), ("centimetres", "cm"), ("centimeter", "cm"), ("centimeters", "cm"),
    ("metre", "m"), ("metres", "m"), ("meter", "m"), ("meters", "m"),
    ("kilometre", "km"), ("kilometres", "km"), ("kilometer", "km"), ("kilometers", "km"), ("kms", "km"),
    ("inch", "in"), ("inches", "in"), ("foot", "ft"), ("feet", "ft"), ("yard", "yd"), ("yards", "yd"), ("mile", "mi"), ("miles", "mi"),
    ("milligram", "mg"), ("milligrams", "mg"), ("gram", "g"), ("grams", "g"),
    ("kilo", "kg"), ("kilos", "kg"), ("kilogram", "kg"), ("kilograms", "kg"), ("kgs", "kg"),
    ("tonne", "t"), ("tonnes", "t"), ("ounce", "oz"), ("ounces", "oz"),
    ("pound", "lb"), ("pounds", "lb"), ("lbs", "lb"), ("stone", "st"), ("stones", "st"),
    ("millilitre", "ml"), ("millilitres", "ml"), ("milliliter", "ml"), ("milliliters", "ml"),
    ("litre", "l"), ("litres", "l"), ("liter", "l"), ("liters", "l"),
    ("gallon", "gal"), ("gallons", "gal"), ("pint", "pt"), ("pints", "pt"), ("cups", "cup"),
    ("second", "s"), ("seconds", "s"), ("sec", "s"), ("secs", "s"), ("minute", "min"), ("minutes", "min"), ("mins", "min"),
    ("hour", "h"), ("hours", "h"), ("hr", "h"), ("hrs", "h"), ("days", "day"), ("weeks", "week"),
    ("byte", "b"), ("bytes", "b"), ("kilobyte", "kb"), ("kilobytes", "kb"), ("megabyte", "mb"), ("megabytes", "mb"),
    ("gigabyte", "gb"), ("gigabytes", "gb"), ("terabyte", "tb"), ("terabytes", "tb"),
    ("km/h", "kph"), ("acres", "acre"), ("hectares", "hectare"),
    ("celsius", "c"), ("centigrade", "c"), ("fahrenheit", "f"), ("kelvin", "k"),
];

fn find_unit(name: &str) -> Option<&'static (&'static str, Dim, f64)> {
    let name = name.trim().trim_end_matches(['?', '.', '!']).trim().trim_start_matches('°').to_ascii_lowercase();
    let name = UNIT_WORDS.iter().find(|(word, _)| *word == name).map_or(name.as_str(), |(_, short)| *short);
    UNITS.iter().find(|(unit, _, _)| *unit == name)
}

fn to_kelvin(value: f64, unit: &str) -> f64 {
    match unit {
        "c" => value + 273.15,
        "f" => (value - 32.0) * 5.0 / 9.0 + 273.15,
        _ => value,
    }
}

fn from_kelvin(kelvin: f64, unit: &str) -> f64 {
    match unit {
        "c" => kelvin - 273.15,
        "f" => (kelvin - 273.15) * 9.0 / 5.0 + 32.0,
        _ => kelvin,
    }
}

/// Handles `10 km to mi`, `72f in c`, `5.5kg to lb`.
fn try_conversion(q: &str) -> Option<CalcOutcome> {
    let lower = q.to_ascii_lowercase();
    let (left, right) = lower.split_once(" to ").or_else(|| lower.split_once(" in "))?;
    let left = left.trim();
    let split = left
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_digit() || matches!(c, '.' | ',' | '-' | '+')))
        .map(|(i, _)| i)?;
    let value: f64 = left[..split].replace(',', "").trim().parse().ok()?;
    let from = find_unit(&left[split..])?;
    let to = find_unit(right)?;
    if from.1 != to.1 {
        return Some(CalcOutcome::Error(format!("Cannot convert {} to {}", from.0, to.0)));
    }
    let result = if from.1 == Dim::Temperature {
        from_kelvin(to_kelvin(value, from.0), to.0)
    } else {
        value * from.2 / to.2
    };
    let number = format_number(result);
    Some(CalcOutcome::Value {
        display: format!("{number} {}", to.0),
        note: Some(format!("{} {} in {}", format_number(value), from.0, to.0)),
        reverse: Some(format!("{number} {} in {}", to.0, from.0)),
        units: Some((format_number(value), from.0, to.0)),
        copy: number,
    })
}

/// A number from [`format_number`] cut to four decimal places for showing large, "37.7778" for
/// "37.7777777778". Very small numbers and ones with an exponent are left whole. Copying keeps
/// every digit.
pub fn rounded_for_show(text: &str) -> String {
    match text.parse::<f64>() {
        Ok(value) if !text.contains('e') && value.abs() >= 0.01 => {
            let fixed = format!("{value:.4}");
            if fixed.contains('.') { fixed.trim_end_matches('0').trim_end_matches('.').to_string() } else { fixed }
        }
        _ => text.to_string(),
    }
}

/// A unit's name, for showing beside an amount of it: "ft" is "Feet".
pub fn unit_name(unit: &str) -> &'static str {
    match unit {
        "mm" => "Millimetres",
        "cm" => "Centimetres",
        "m" => "Metres",
        "km" => "Kilometres",
        "in" => "Inches",
        "ft" => "Feet",
        "yd" => "Yards",
        "mi" => "Miles",
        "mg" => "Milligrams",
        "g" => "Grams",
        "kg" => "Kilograms",
        "t" => "Tonnes",
        "oz" => "Ounces",
        "lb" => "Pounds",
        "st" => "Stone",
        "ml" => "Millilitres",
        "l" => "Litres",
        "gal" => "US gallons",
        "pt" => "US pints",
        "cup" => "US cups",
        "ms" => "Milliseconds",
        "s" => "Seconds",
        "min" => "Minutes",
        "h" => "Hours",
        "day" => "Days",
        "week" => "Weeks",
        "b" => "Bytes",
        "kb" => "Kilobytes",
        "mb" => "Megabytes",
        "gb" => "Gigabytes",
        "tb" => "Terabytes",
        "kib" => "Kibibytes",
        "mib" => "Mebibytes",
        "gib" => "Gibibytes",
        "tib" => "Tebibytes",
        "kph" | "kmh" => "Kilometres per hour",
        "mph" => "Miles per hour",
        "mps" => "Metres per second",
        "knots" => "Knots",
        "sqm" => "Square metres",
        "sqft" => "Square feet",
        "acre" => "Acres",
        "hectare" => "Hectares",
        "c" => "Celsius",
        "f" => "Fahrenheit",
        "k" => "Kelvin",
        _ => "",
    }
}

/// A unit as it's usually printed after a number: degrees get their sign, "c" is "°C".
pub fn unit_symbol(unit: &str) -> &str {
    match unit {
        "c" => "\u{00B0}C",
        "f" => "\u{00B0}F",
        "k" => "K",
        "kb" => "KB",
        "mb" => "MB",
        "gb" => "GB",
        "tb" => "TB",
        "kib" => "KiB",
        "mib" => "MiB",
        "gib" => "GiB",
        "tib" => "TiB",
        "b" => "B",
        "kph" | "kmh" => "km/h",
        "mps" => "m/s",
        "sqm" => "m\u{00B2}",
        "sqft" => "ft\u{00B2}",
        other => other,
    }
}

/// A sum as it's written on paper: "16*3.5/2" is "16 × 3.5 ÷ 2".
pub fn pretty_sum(sum: &str) -> String {
    let mut out = String::with_capacity(sum.len() + 8);
    for c in sum.trim().chars() {
        match c {
            '*' => out.push_str(" \u{00D7} "),
            '/' => out.push_str(" \u{00F7} "),
            '+' => out.push_str(" + "),
            '-' if out.trim_end().chars().last().is_some_and(|last| last.is_ascii_digit() || last == ')') => out.push_str(" \u{2212} "),
            ' ' => {}
            other => out.push(other),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(q: &str) -> String {
        match evaluate(q) {
            CalcOutcome::Value { display, .. } => display,
            other => panic!("{q}: expected a value, got {other:?}"),
        }
    }

    #[test]
    fn sums_and_units_are_written_out_for_the_answer_card() {
        assert_eq!(pretty_sum("16*3.5+4"), "16 \u{00D7} 3.5 + 4");
        assert_eq!(pretty_sum("-2 - 3/4"), "-2 \u{2212} 3 \u{00F7} 4");
        assert_eq!(pretty_sum("(2+3)-1"), "(2 + 3) \u{2212} 1");
        for (unit, _, _) in UNITS {
            assert!(!unit_name(unit).is_empty(), "{unit} has no name");
        }
        assert_eq!(unit_symbol("f"), "\u{00B0}F");
        assert_eq!(rounded_for_show("37.7777777778"), "37.7778");
        assert_eq!(rounded_for_show("3.6576"), "3.6576");
        assert_eq!(rounded_for_show("60"), "60");
        assert_eq!(rounded_for_show("0.000123"), "0.000123");
        match evaluate("12 foot in m") {
            CalcOutcome::Value { units, .. } => assert_eq!(units, Some(("12".to_string(), "ft", "m"))),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn thousands_are_grouped_for_copying_with_formatting() {
        assert_eq!(group_thousands("1234567.5"), "1,234,567.5");
        assert_eq!(group_thousands("-1000"), "-1,000");
        assert_eq!(group_thousands("999"), "999");
        assert_eq!(group_thousands("1.5e20"), "1.5e20");
        match evaluate("12 ft in m") {
            CalcOutcome::Value { reverse, .. } => assert_eq!(reverse.as_deref(), Some("3.6576 m in ft")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(value("16*3.5+4"), "60");
        assert_eq!(value("2+3*4"), "14");
        assert_eq!(value("(2+3)*4"), "20");
        assert_eq!(value("2^3^2"), "512");
        assert_eq!(value("-2^2"), "-4");
        assert_eq!(value("10/4"), "2.5");
        assert_eq!(value("10 % 3"), "1");
        assert_eq!(value("50%"), "0.5");
        assert_eq!(value("1,000+1"), "1001");
        assert_eq!(value("1e3+1"), "1001");
    }

    #[test]
    fn floating_point_noise_is_hidden() {
        assert_eq!(value("0.1+0.2"), "0.3");
    }

    #[test]
    fn functions_and_constants() {
        assert_eq!(value("sqrt(16)"), "4");
        assert_eq!(value("round(2.5)+1"), "4");
        assert_eq!(value("pi*2"), "6.28318530718");
    }

    #[test]
    fn plain_text_and_lone_numbers_are_not_maths() {
        for q in ["", "code", "chrome", "2022", "7-zip", "notepad++", "1password", "a+b", "3-day plan", "10-20 things"] {
            assert_eq!(evaluate(q), CalcOutcome::NotMath, "{q}");
        }
    }

    #[test]
    fn errors_differ_from_empty() {
        assert_eq!(evaluate("1/0"), CalcOutcome::Error("Division by zero".into()));
        assert!(matches!(evaluate("2+"), CalcOutcome::Error(_)));
        assert!(matches!(evaluate("(2+3"), CalcOutcome::Error(_)));
        assert!(matches!(evaluate("sqrt(-1)"), CalcOutcome::Error(_)));
        assert!(matches!(evaluate("9^9^9^9"), CalcOutcome::Error(_)));
    }

    #[test]
    fn deep_nesting_is_refused_not_a_stack_overflow() {
        let bomb = format!("{}1{}", "(".repeat(100_000), ")".repeat(100_000));
        assert!(matches!(evaluate(&bomb), CalcOutcome::Error(_)));
    }

    #[test]
    fn conversions() {
        assert_eq!(value("10 km to mi"), "6.21371192237 mi");
        assert_eq!(value("212 f to c"), "100 c");
        assert_eq!(value("0c in f"), "32 f");
        assert_eq!(value("123 pound to kilo"), "55.79186151 kg");
        assert_eq!(value("2 kilometres in miles"), "1.24274238447 mi");
        assert_eq!(value("100 fahrenheit in celsius"), "37.7777777778 c");
        assert_eq!(value("3 litres to pints?"), "6.3401292566 pt");
        assert_eq!(value("1 gib to mib"), "1024 mib");
        assert_eq!(value("5.5kg to g"), "5500 g");
        assert!(matches!(evaluate("10 km to kg"), CalcOutcome::Error(_)));
        // Unknown units are ordinary text, not errors.
        assert_eq!(evaluate("how to win"), CalcOutcome::NotMath);
        assert_eq!(evaluate("open in code"), CalcOutcome::NotMath);
    }
}
