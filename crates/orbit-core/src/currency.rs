//! Currency conversion: "123 euro in pounds", "£50 to usd", "100 usd in jpy". The rates are the
//! European Central Bank's daily reference rates, which the platform fetches and keeps. This module
//! only reads queries and rates, and does the sums. It never touches the network.

/// The currencies the European Central Bank publishes, and the words and signs that name each.
/// Words that mean something else as well, such as "real" or "peso", are left out on purpose.
const CURRENCIES: &[(&str, &[&str])] = &[
    ("EUR", &["eur", "euro", "euros", "€"]),
    ("USD", &["usd", "dollar", "dollars", "$", "us$", "bucks"]),
    ("GBP", &["gbp", "pound", "pounds", "£", "quid", "sterling"]),
    ("JPY", &["jpy", "yen", "¥"]),
    ("CHF", &["chf", "franc", "francs"]),
    ("CAD", &["cad", "c$"]),
    ("AUD", &["aud", "a$"]),
    ("NZD", &["nzd", "nz$"]),
    ("CNY", &["cny", "yuan", "rmb", "renminbi"]),
    ("HKD", &["hkd", "hk$"]),
    ("SGD", &["sgd", "s$"]),
    ("INR", &["inr", "rupee", "rupees", "₹"]),
    ("KRW", &["krw", "won", "₩"]),
    ("SEK", &["sek"]),
    ("NOK", &["nok"]),
    ("DKK", &["dkk"]),
    ("ISK", &["isk"]),
    ("PLN", &["pln", "zloty", "złoty", "zł"]),
    ("CZK", &["czk", "koruna"]),
    ("HUF", &["huf", "forint"]),
    ("RON", &["ron", "leu", "lei"]),
    ("BGN", &["bgn", "lev"]),
    ("TRY", &["try", "lira", "₺"]),
    ("BRL", &["brl", "r$", "reais"]),
    ("MXN", &["mxn"]),
    ("IDR", &["idr", "rupiah"]),
    ("ILS", &["ils", "shekel", "shekels", "₪"]),
    ("MYR", &["myr", "ringgit"]),
    ("PHP", &["php", "₱"]),
    ("THB", &["thb", "baht", "฿"]),
    ("ZAR", &["zar", "rand"]),
];

/// Currencies whose amounts aren't written with cents.
const NO_CENTS: &[&str] = &["JPY", "KRW", "HUF", "ISK", "IDR"];

/// A query that asks for a conversion.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub amount: f64,
    pub from: &'static str,
    pub to: &'static str,
}

/// Reference rates on one day: how much of each currency one euro buys.
#[derive(Clone, Debug, PartialEq)]
pub struct Rates {
    /// The day the bank set them, as it writes it: `2026-11-06`.
    pub date: String,
    per_euro: Vec<(String, f64)>,
}

fn code_for(word: &str) -> Option<&'static str> {
    let word = word.trim().trim_end_matches(['?', '.', '!']).trim();
    if word.is_empty() {
        return None;
    }
    let lower = word.to_lowercase();
    CURRENCIES.iter().find(|(_, names)| names.contains(&lower.as_str())).map(|(code, _)| *code)
}

/// A number as people type it: `123`, `1,234.50`, `0.5`. Thousands commas only before a dot.
fn amount(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit() || b == b',' || b == b'.') || !text.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    let value: f64 = text.replace(',', "").parse().ok()?;
    value.is_finite().then_some(value)
}

/// "123 eur", "123eur", "€123", "€ 123": an amount and the currency it's in.
fn money(text: &str) -> Option<(f64, &'static str)> {
    let text = text.trim();
    let digits_start = text.find(|c: char| c.is_ascii_digit())?;
    let digits_end = text[digits_start..].find(|c: char| !(c.is_ascii_digit() || c == ',' || c == '.')).map_or(text.len(), |at| digits_start + at);
    let value = amount(&text[digits_start..digits_end])?;
    let before = text[..digits_start].trim();
    let after = text[digits_end..].trim();
    match (before.is_empty(), after.is_empty()) {
        (true, false) => Some((value, code_for(after)?)),
        (false, true) => Some((value, code_for(before)?)),
        _ => None,
    }
}

/// Reads "123 euro in pounds", "£50 to usd", "100 usd = jpy". Anything else is `None`, quickly:
/// this runs on every keystroke, so it gives up at the first thing that doesn't fit.
pub fn parse(query: &str) -> Option<Query> {
    let query = query.trim();
    if query.len() > 64 || !query.contains(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let lower = query.to_lowercase();
    let (left, right) = [" to ", " in ", " into ", " as ", " = ", "="].iter().find_map(|word| lower.split_once(word))?;
    let (amount, from) = money(left)?;
    let to = code_for(right)?;
    (from != to).then_some(Query { amount, from, to })
}

impl Rates {
    /// The bank's daily file, `eurofxref-daily.xml`: one `time` and a `currency`, `rate` pair for
    /// each currency. Read by hand, as it's short and fixed.
    pub fn parse_ecb(xml: &str) -> Option<Rates> {
        let date = attribute(xml, "time")?.to_string();
        let mut per_euro = Vec::new();
        let mut rest = xml;
        while let Some(at) = rest.find("currency=") {
            rest = &rest[at..];
            let code = attribute(rest, "currency")?;
            let rate: f64 = attribute(rest, "rate")?.parse().ok()?;
            if code.len() == 3 && rate.is_finite() && rate > 0.0 {
                per_euro.push((code.to_ascii_uppercase(), rate));
            }
            rest = &rest["currency=".len()..];
        }
        (!per_euro.is_empty()).then_some(Rates { date, per_euro })
    }

    /// How much of `code` one euro buys.
    pub fn per_euro(&self, code: &str) -> Option<f64> {
        if code == "EUR" {
            return Some(1.0);
        }
        self.per_euro.iter().find(|(known, _)| known == code).map(|(_, rate)| *rate)
    }

    pub fn convert(&self, query: &Query) -> Option<f64> {
        Some(query.amount / self.per_euro(query.from)? * self.per_euro(query.to)?)
    }

    /// The day as people say it: `6 Nov 2026`.
    pub fn day(&self) -> String {
        const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        let mut parts = self.date.split('-');
        match (parts.next(), parts.next().and_then(|m| m.parse::<usize>().ok()), parts.next().and_then(|d| d.parse::<u32>().ok())) {
            (Some(year), Some(month @ 1..=12), Some(day)) => format!("{day} {} {year}", MONTHS[month - 1]),
            _ => self.date.clone(),
        }
    }
}

/// The value of `name='…'` or `name="…"` after the start of `text`.
fn attribute<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let at = text.find(&format!("{name}="))? + name.len() + 1;
    let quote = text[at..].chars().next().filter(|c| *c == '\'' || *c == '"')?;
    let value = &text[at + 1..];
    Some(&value[..value.find(quote)?])
}

/// Money as it's written: grouped thousands, and cents unless the currency has none.
/// `1234.5` in GBP is `1,234.50`.
pub fn format_amount(value: f64, code: &str) -> String {
    let places = if NO_CENTS.contains(&code) { 0 } else { 2 };
    let fixed = format!("{:.*}", places, value.abs());
    let (whole, cents) = fixed.split_once('.').map_or((fixed.as_str(), None), |(w, c)| (w, Some(c)));
    let mut grouped = String::new();
    for (at, digit) in whole.chars().enumerate() {
        if at > 0 && (whole.len() - at) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    let sign = if value < 0.0 && fixed.bytes().any(|b| b.is_ascii_digit() && b != b'0') { "-" } else { "" };
    match cents {
        Some(cents) => format!("{sign}{grouped}.{cents}"),
        None => format!("{sign}{grouped}"),
    }
}

/// A currency's name in English: "GBP" is "British pound".
pub fn name(code: &str) -> &'static str {
    match code {
        "EUR" => "Euro",
        "USD" => "US dollar",
        "GBP" => "British pound",
        "JPY" => "Japanese yen",
        "CHF" => "Swiss franc",
        "CAD" => "Canadian dollar",
        "AUD" => "Australian dollar",
        "NZD" => "New Zealand dollar",
        "CNY" => "Chinese yuan",
        "HKD" => "Hong Kong dollar",
        "SGD" => "Singapore dollar",
        "INR" => "Indian rupee",
        "KRW" => "South Korean won",
        "SEK" => "Swedish krona",
        "NOK" => "Norwegian krone",
        "DKK" => "Danish krone",
        "ISK" => "Icelandic kr\u{00F3}na",
        "PLN" => "Polish z\u{0142}oty",
        "CZK" => "Czech koruna",
        "HUF" => "Hungarian forint",
        "RON" => "Romanian leu",
        "BGN" => "Bulgarian lev",
        "TRY" => "Turkish lira",
        "BRL" => "Brazilian real",
        "MXN" => "Mexican peso",
        "IDR" => "Indonesian rupiah",
        "ILS" => "Israeli shekel",
        "MYR" => "Malaysian ringgit",
        "PHP" => "Philippine peso",
        "THB" => "Thai baht",
        "ZAR" => "South African rand",
        _ => "",
    }
}

/// An amount as it's usually written with its sign, "£1,234.50", or with its code where it has
/// no sign of its own, "1,234.50 SEK".
pub fn with_symbol(value: f64, code: &str) -> String {
    let symbol = match code {
        "EUR" => "\u{20AC}",
        "USD" => "$",
        "GBP" => "\u{00A3}",
        "JPY" => "\u{00A5}",
        "INR" => "\u{20B9}",
        "KRW" => "\u{20A9}",
        "TRY" => "\u{20BA}",
        "ILS" => "\u{20AA}",
        "PHP" => "\u{20B1}",
        "THB" => "\u{0E3F}",
        _ => return format!("{} {code}", format_amount(value, code)),
    };
    let amount = format_amount(value, code);
    match amount.strip_prefix('-') {
        Some(positive) => format!("-{symbol}{positive}"),
        None => format!("{symbol}{amount}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_currency_has_a_name() {
        for (code, _) in CURRENCIES {
            assert!(!name(code).is_empty(), "{code}");
        }
    }

    const ECB: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<gesmes:Envelope xmlns:gesmes=\"http://www.gesmes.org/xml/2002-08-01\" xmlns=\"http://www.ecb.int/vocabulary/2002-08-01/eurofxref\">
\t<gesmes:subject>Reference rates</gesmes:subject>
\t<Cube>
\t\t<Cube time='2026-11-06'>
\t\t\t<Cube currency='USD' rate='1.0800'/>
\t\t\t<Cube currency='JPY' rate='160.00'/>
\t\t\t<Cube currency='GBP' rate='0.8500'/>
\t\t</Cube>
\t</Cube>
</gesmes:Envelope>";

    fn q(amount: f64, from: &'static str, to: &'static str) -> Option<Query> {
        Some(Query { amount, from, to })
    }

    #[test]
    fn queries_people_type() {
        assert_eq!(parse("123 euro in pounds"), q(123.0, "EUR", "GBP"));
        assert_eq!(parse("123 Euros to GBP"), q(123.0, "EUR", "GBP"));
        assert_eq!(parse("£50 to usd"), q(50.0, "GBP", "USD"));
        assert_eq!(parse("€ 1,234.50 in $"), q(1234.5, "EUR", "USD"));
        assert_eq!(parse("100usd in yen"), q(100.0, "USD", "JPY"));
        assert_eq!(parse("20 quid in euros?"), q(20.0, "GBP", "EUR"));
        assert_eq!(parse("5 chf = eur"), q(5.0, "CHF", "EUR"));
    }

    #[test]
    fn other_things_are_left_alone() {
        for text in ["10 km to mi", "open in code", "123", "eur to gbp", "5 eur in eur", "3 apples in pounds", "1 2 eur to gbp", "pounds in kg", "move 5 files to usb"] {
            assert_eq!(parse(text), None, "{text}");
        }
    }

    #[test]
    fn the_banks_file_and_the_sums() {
        let rates = Rates::parse_ecb(ECB).expect("rates");
        assert_eq!(rates.date, "2026-11-06");
        assert_eq!(rates.day(), "6 Nov 2026");
        assert_eq!(rates.per_euro("EUR"), Some(1.0));
        let pounds = rates.convert(&Query { amount: 123.0, from: "EUR", to: "GBP" }).unwrap();
        assert!((pounds - 104.55).abs() < 1e-9);
        let yen = rates.convert(&Query { amount: 10.0, from: "USD", to: "JPY" }).unwrap();
        assert!((yen - 1481.481481).abs() < 1e-3);
        assert_eq!(rates.convert(&Query { amount: 1.0, from: "EUR", to: "ZAR" }), None);
        assert_eq!(Rates::parse_ecb("<html>Service unavailable</html>"), None);
    }

    #[test]
    fn money_is_written_with_cents_and_commas() {
        assert_eq!(format_amount(104.55, "GBP"), "104.55");
        assert_eq!(format_amount(1234.5, "USD"), "1,234.50");
        assert_eq!(format_amount(1481.48, "JPY"), "1,481");
        assert_eq!(format_amount(1_000_000.0, "EUR"), "1,000,000.00");
        assert_eq!(format_amount(0.004, "EUR"), "0.00");
    }

    #[test]
    fn every_currency_has_its_code_as_a_name() {
        for (code, names) in CURRENCIES {
            assert!(names.contains(&code.to_ascii_lowercase().as_str()), "{code}");
        }
    }
}

