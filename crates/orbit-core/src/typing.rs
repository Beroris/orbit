//! A typing test, scored the way Monkeytype scores one, with Monkeytype's English word lists and
//! quotes (GPL-3.0, as Orbit is; see `docs/THIRD_PARTY_NOTICES.md`).
//!
//! It knows nothing of windows or clocks: the platform says what was typed and when, in
//! milliseconds from any fixed point, and draws what [`Test`] holds.
//!
//! - **wpm** is the characters of the words typed right, and the space after each, a minute, over
//!   five.
//! - **raw** is every character typed a minute, right or not, over five.
//! - **accuracy** is the share of key presses that were the next character wanted.
//! - **consistency** is how steady raw speed was from second to second, as Monkeytype works it out.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const ENGLISH: &str = include_str!("../data/monkeytype/english.txt");
const ENGLISH_1K: &str = include_str!("../data/monkeytype/english_1k.txt");
const QUOTES: &str = include_str!("../data/monkeytype/english_quotes.tsv");

/// The longest a quote can be and still count as short, in characters, as Monkeytype splits them.
const SHORT_QUOTE: usize = 100;
/// How many characters can be typed past the end of a word.
const MOST_EXTRA: usize = 20;
/// Words kept ready past the one being typed, in a timed test that has no end to its words.
const AHEAD: usize = 60;
/// Below this accuracy a result isn't kept as a best, as on Monkeytype.
const LEAST_ACCURACY_FOR_BEST: f32 = 75.0;

pub const TIMES: [u32; 4] = [15, 30, 60, 120];
pub const WORD_COUNTS: [u32; 4] = [10, 25, 50, 100];

/// What ends a test.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Type for this many seconds.
    Time(u32),
    /// Type this many words.
    Words(u32),
    /// Type one quote.
    Quote(QuoteLength),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuoteLength {
    Short,
    Medium,
}

/// Which of Monkeytype's lists the words come from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WordList {
    /// The 200 most common English words.
    #[default]
    English,
    /// The 1,000 most common.
    English1k,
}

impl WordList {
    pub const ALL: [WordList; 2] = [WordList::English, WordList::English1k];

    /// As Monkeytype names it.
    pub const fn name(self) -> &'static str {
        match self {
            WordList::English => "english",
            WordList::English1k => "english 1k",
        }
    }

    pub fn words(self) -> impl Iterator<Item = &'static str> {
        let text = match self {
            WordList::English => ENGLISH,
            WordList::English1k => ENGLISH_1K,
        };
        text.lines().map(str::trim).filter(|word| !word.is_empty())
    }
}

/// How the test is set up, kept in settings between tests.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TypingSettings {
    pub mode: Mode,
    pub punctuation: bool,
    pub numbers: bool,
    pub words: WordList,
    /// The best wpm for each kind of test, by [`Options::best_key`].
    pub best: BTreeMap<String, f32>,
}

impl Default for TypingSettings {
    fn default() -> Self {
        TypingSettings { mode: Mode::Time(30), punctuation: false, numbers: false, words: WordList::English, best: BTreeMap::new() }
    }
}

impl TypingSettings {
    pub fn options(&self) -> Options {
        Options { mode: self.mode, punctuation: self.punctuation, numbers: self.numbers, words: self.words }
    }

    pub fn best(&self, options: &Options) -> Option<f32> {
        self.best.get(&options.best_key()).copied()
    }

    /// Keeps a finished test's result if it's the best yet for its kind. Says whether it was.
    pub fn record(&mut self, test: &Test) -> bool {
        let Some(stats) = test.result() else { return false };
        if !stats.counts_as_best() {
            return false;
        }
        let key = test.options.best_key();
        let better = self.best.get(&key).is_none_or(|&best| stats.wpm > best);
        if better {
            self.best.insert(key, stats.wpm);
        }
        better
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    pub mode: Mode,
    pub punctuation: bool,
    pub numbers: bool,
    pub words: WordList,
}

impl Options {
    /// Punctuation and numbers don't change a quote, which is typed as written.
    fn punctuation(&self) -> bool {
        self.punctuation && !matches!(self.mode, Mode::Quote(_))
    }

    fn numbers(&self) -> bool {
        self.numbers && !matches!(self.mode, Mode::Quote(_))
    }

    /// Tests of the same kind share a best.
    pub fn best_key(&self) -> String {
        let mode = match self.mode {
            Mode::Time(seconds) => format!("time {seconds}"),
            Mode::Words(count) => format!("words {count}"),
            Mode::Quote(QuoteLength::Short) => "quote short".into(),
            Mode::Quote(QuoteLength::Medium) => "quote medium".into(),
        };
        match self.mode {
            Mode::Quote(_) => mode,
            _ => format!("{mode} {}{}{}", self.words.name(), if self.punctuation { " punctuation" } else { "" }, if self.numbers { " numbers" } else { "" }),
        }
    }

    /// Such as "time 30 · english", as Monkeytype shows a test's kind under its result.
    pub fn describe(&self) -> String {
        let mut parts = vec![match self.mode {
            Mode::Time(seconds) => format!("time {seconds}"),
            Mode::Words(count) => format!("words {count}"),
            Mode::Quote(QuoteLength::Short) => "quote short".into(),
            Mode::Quote(QuoteLength::Medium) => "quote medium".into(),
        }];
        if !matches!(self.mode, Mode::Quote(_)) {
            parts.push(self.words.name().into());
        }
        if self.punctuation() {
            parts.push("punctuation".into());
        }
        if self.numbers() {
            parts.push("numbers".into());
        }
        parts.join(" \u{00B7} ")
    }
}

/// One of Monkeytype's quotes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quote {
    pub text: &'static str,
    pub source: &'static str,
}

pub fn quotes(length: QuoteLength) -> impl Iterator<Item = Quote> {
    QUOTES
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(text, source)| Quote { text, source })
        .filter(move |quote| (quote.text.len() <= SHORT_QUOTE) == (length == QuoteLength::Short))
}

/// A small, quick random number generator. Words don't need better.
#[derive(Clone, Debug)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15 | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    /// True this often, from 0 to 1.
    fn chance(&mut self, p: f64) -> bool {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64 <= p
    }
}

/// What a typed character was, as drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// Not reached yet.
    Untyped,
    Correct,
    Incorrect,
    /// Typed past the end of the word.
    Extra,
}

/// A result, or how it stands so far.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub wpm: f32,
    pub raw: f32,
    /// From 0 to 100.
    pub accuracy: f32,
    /// From 0 to 100.
    pub consistency: f32,
    pub correct: u32,
    pub incorrect: u32,
    pub extra: u32,
    pub missed: u32,
    pub seconds: f32,
}

impl Stats {
    fn counts_as_best(&self) -> bool {
        self.wpm > 0.0 && self.accuracy >= LEAST_ACCURACY_FOR_BEST
    }

    /// The result in a line, for the clipboard.
    pub fn summary(&self, options: &Options) -> String {
        format!(
            "{:.0} wpm, {:.0}% accuracy ({}) \u{00B7} raw {:.0} \u{00B7} consistency {:.0}% \u{00B7} characters {}/{}/{}/{} \u{00B7} {:.0}s",
            self.wpm, self.accuracy, options.describe(), self.raw, self.consistency, self.correct, self.incorrect, self.extra, self.missed, self.seconds
        )
    }
}

/// One second of a test, for the chart.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Second {
    /// The wpm so far, at the end of this second.
    pub wpm: f32,
    /// Characters typed in this second alone, as a speed.
    pub raw: f32,
    pub errors: u32,
}

#[derive(Clone, Debug)]
pub struct Test {
    pub options: Options,
    pub words: Vec<String>,
    /// What's been typed for each word so far. The last is the word being typed.
    pub typed: Vec<String>,
    /// Where the quote is from, in quote mode.
    pub source: Option<&'static str>,
    started: Option<f64>,
    ended: Option<f64>,
    right_keys: u32,
    wrong_keys: u32,
    /// Characters and mistakes typed in each second, counted from the start.
    chars_in: Vec<u32>,
    errors_in: Vec<u32>,
    /// The seconds that have ended.
    pub history: Vec<Second>,
    rng: Rng,
    list: Vec<&'static str>,
}

impl Test {
    pub fn new(options: Options, seed: u64) -> Test {
        let mut test = Test {
            options,
            words: Vec::new(),
            typed: vec![String::new()],
            source: None,
            started: None,
            ended: None,
            right_keys: 0,
            wrong_keys: 0,
            chars_in: Vec::new(),
            errors_in: Vec::new(),
            history: Vec::new(),
            rng: Rng::new(seed),
            list: options.words.words().collect(),
        };
        match options.mode {
            Mode::Quote(length) => {
                let all: Vec<Quote> = quotes(length).collect();
                let quote = all[test.rng.below(all.len())];
                test.words = quote.text.split_whitespace().map(str::to_string).collect();
                test.source = Some(quote.source);
            }
            Mode::Words(count) => {
                for _ in 0..count {
                    test.push_word();
                }
                // With punctuation, the test ends a sentence.
                if options.punctuation()
                    && let Some(last) = test.words.last_mut()
                    && !last.ends_with(['.', '?', '!'])
                {
                    last.truncate(last.trim_end_matches([',', ':', ';']).len());
                    last.push('.');
                }
            }
            Mode::Time(_) => test.fill_ahead(),
        }
        test
    }

    /// The same kind of test again, with new words.
    pub fn again(&self, seed: u64) -> Test {
        Test::new(self.options, seed)
    }

    fn push_word(&mut self) {
        let previous = self.words.last().map(|word| word.to_lowercase());
        let mut word = loop {
            let word = self.list[self.rng.below(self.list.len())];
            // Never the same word twice running.
            if previous.as_deref().is_none_or(|previous| previous.trim_matches(|c: char| !c.is_alphanumeric()) != word.to_lowercase()) || self.list.len() < 2 {
                break word.to_string();
            }
        };
        if self.options.numbers() && self.rng.chance(0.1) {
            let digits = 1 + self.rng.below(4);
            word = (0..digits).map(|at| char::from(b'0' + (if at == 0 && digits > 1 { 1 + self.rng.below(9) } else { self.rng.below(10) }) as u8)).collect();
        }
        if self.options.punctuation() {
            word = self.punctuate(word);
        }
        self.words.push(word);
    }

    /// Capitals at the start of a sentence, and now and then a full stop, a comma, a question,
    /// quotes or brackets, as Monkeytype's punctuation mode adds them.
    fn punctuate(&mut self, word: String) -> String {
        let sentence_start = self.words.last().is_none_or(|last| last.ends_with(['.', '?', '!']));
        let mut word = if sentence_start {
            let mut chars = word.chars();
            chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or(word)
        } else {
            word
        };
        if self.words.is_empty() {
            return word;
        }
        let roll = self.rng.below(1000);
        match roll {
            0..=99 => word.push('.'),
            100..=129 => word.push(if self.rng.chance(0.5) { '?' } else { '!' }),
            130..=209 => word.push(','),
            210..=229 => word = format!("\"{word}\""),
            230..=244 => word = format!("({word})"),
            245..=259 => word.push(if self.rng.chance(0.5) { ':' } else { ';' }),
            260..=269 if !sentence_start => word = "-".into(),
            270..=279 => word = format!("'{word}'"),
            _ => {}
        }
        word
    }

    fn fill_ahead(&mut self) {
        while self.words.len() < self.typed.len() + AHEAD {
            self.push_word();
        }
    }

    /// Which word is being typed.
    pub fn at(&self) -> usize {
        self.typed.len() - 1
    }

    pub fn started(&self) -> bool {
        self.started.is_some()
    }

    pub fn finished(&self) -> bool {
        self.ended.is_some()
    }

    pub fn running(&self) -> bool {
        self.started() && !self.finished()
    }

    /// Seconds since the first key, stopping when the test ends.
    pub fn elapsed(&self, now: f64) -> f64 {
        match (self.started, self.ended) {
            (Some(start), Some(end)) => (end - start) / 1000.0,
            (Some(start), None) => ((now - start) / 1000.0).max(0.0),
            _ => 0.0,
        }
    }

    /// Seconds left in a timed test.
    pub fn remaining(&self, now: f64) -> Option<f64> {
        match self.options.mode {
            Mode::Time(seconds) => Some((seconds as f64 - self.elapsed(now)).max(0.0)),
            _ => None,
        }
    }

    /// How many words there are to type, if the test has a set number.
    pub fn word_goal(&self) -> Option<usize> {
        match self.options.mode {
            Mode::Time(_) => None,
            _ => Some(self.words.len()),
        }
    }

    fn second_of(&self, now: f64) -> usize {
        self.elapsed(now) as usize
    }

    fn count_key(&mut self, now: f64, right: bool) {
        let second = self.second_of(now);
        if self.chars_in.len() <= second {
            self.chars_in.resize(second + 1, 0);
            self.errors_in.resize(second + 1, 0);
        }
        self.chars_in[second] += 1;
        if right {
            self.right_keys += 1;
        } else {
            self.wrong_keys += 1;
            self.errors_in[second] += 1;
        }
    }

    /// A character typed. Says whether anything changed.
    pub fn type_char(&mut self, c: char, now: f64) -> bool {
        if self.finished() || c.is_control() {
            return false;
        }
        self.tick(now);
        if self.finished() {
            return true;
        }
        let at = self.at();
        if c == ' ' {
            // A space with nothing typed yet would skip a word, and starts nothing.
            if self.typed[at].is_empty() {
                return false;
            }
            let right = self.typed[at] == self.words[at];
            self.count_key(now, right);
            if at + 1 >= self.words.len() {
                self.end(now);
            } else {
                self.typed.push(String::new());
                if matches!(self.options.mode, Mode::Time(_)) {
                    self.fill_ahead();
                }
            }
            return true;
        }
        if self.started.is_none() {
            self.started = Some(now);
        }
        let word = &self.words[at];
        let typed_len = self.typed[at].chars().count();
        if typed_len >= word.chars().count() + MOST_EXTRA {
            return false;
        }
        let right = word.chars().nth(typed_len) == Some(c);
        self.typed[at].push(c);
        self.count_key(now, right);
        // The last word ends the test as soon as it's right, without a space after it.
        if at + 1 == self.words.len() && self.typed[at] == self.words[at] && self.word_goal().is_some() {
            self.end(now);
        }
        true
    }

    /// Backspace, or Ctrl+Backspace for the whole word. It can go back into the word before only
    /// when that word has a mistake in it, as on Monkeytype.
    pub fn backspace(&mut self, whole_word: bool, now: f64) -> bool {
        if self.finished() {
            return false;
        }
        self.tick(now);
        let at = self.at();
        if self.typed[at].is_empty() {
            if at == 0 || self.typed[at - 1] == self.words[at - 1] {
                return false;
            }
            self.typed.pop();
            if whole_word {
                self.typed[at - 1].clear();
            }
            return true;
        }
        if whole_word {
            self.typed[at].clear();
        } else {
            self.typed[at].pop();
        }
        true
    }

    /// Notes the seconds that have gone by, and ends a timed test when its time is up. Says
    /// whether the test ended.
    pub fn tick(&mut self, now: f64) -> bool {
        if !self.running() {
            return false;
        }
        let elapsed = self.elapsed(now);
        if let Mode::Time(seconds) = self.options.mode
            && elapsed >= seconds as f64
        {
            // It ends when the time was up, however late this is asked.
            let end = self.started.unwrap_or(now) + seconds as f64 * 1000.0;
            self.end(end);
            return true;
        }
        while (self.history.len() as f64) + 1.0 <= elapsed {
            self.close_second(self.history.len() + 1);
        }
        false
    }

    fn close_second(&mut self, seconds: usize) {
        let at = seconds - 1;
        let chars = self.chars_in.get(at).copied().unwrap_or(0);
        let errors = self.errors_in.get(at).copied().unwrap_or(0);
        // The wpm so far, from what's been typed by now. Close enough for the chart: what's typed
        // is taken as it stands, rather than as it stood at the end of that second.
        let counts = self.counts(false);
        let wpm = speed(counts.word_chars + counts.spaces, seconds as f64);
        self.history.push(Second { wpm, raw: speed(chars, 1.0), errors });
    }

    fn end(&mut self, now: f64) {
        let Some(start) = self.started else { return };
        self.ended = Some(now.max(start));
        let elapsed = self.elapsed(now);
        let whole = elapsed.floor() as usize;
        while self.history.len() < whole {
            self.close_second(self.history.len() + 1);
        }
        // What's left of the last second, if something was typed in it, scaled to a second.
        let part = elapsed - whole as f64;
        let chars = self.chars_in.get(whole).copied().unwrap_or(0);
        if part > 0.0 && chars > 0 {
            let counts = self.counts(true);
            let wpm = speed(counts.word_chars + counts.spaces, elapsed);
            self.history.push(Second { wpm, raw: speed(chars, part.max(0.25)), errors: self.errors_in.get(whole).copied().unwrap_or(0) });
        }
    }

    /// How each character of a word is to be drawn: the word's own, then any typed past its end.
    pub fn marks(&self, word: usize) -> Vec<(char, Mark)> {
        let target: Vec<char> = self.words[word].chars().collect();
        let typed: Vec<char> = self.typed.get(word).map(|typed| typed.chars().collect()).unwrap_or_default();
        let mut out: Vec<(char, Mark)> = target
            .iter()
            .enumerate()
            .map(|(at, &c)| match typed.get(at) {
                None => (c, Mark::Untyped),
                Some(&t) if t == c => (c, Mark::Correct),
                // A wrong letter shows what was wanted, in red, as Monkeytype shows it.
                Some(_) => (c, Mark::Incorrect),
            })
            .collect();
        out.extend(typed.iter().skip(target.len()).map(|&c| (c, Mark::Extra)));
        out
    }

    /// Whether a word that's been left has a mistake in it, to underline it.
    pub fn word_wrong(&self, word: usize) -> bool {
        word < self.at() && self.typed[word] != self.words[word]
    }

    /// The characters, counted as Monkeytype counts them. The word being typed counts as right
    /// so far if what's typed of it is right, when `whole` says the test is over.
    fn counts(&self, whole: bool) -> Counts {
        let mut counts = Counts::default();
        let at = self.at();
        for (index, (word, typed)) in self.words.iter().zip(&self.typed).enumerate() {
            let current = index == at;
            if current && !self.finished() && !whole {
                // Still being typed: right so far adds to the raw speed only.
                let w: Vec<char> = word.chars().collect();
                for (i, c) in typed.chars().enumerate() {
                    match w.get(i) {
                        Some(&wc) if wc == c => counts.correct += 1,
                        Some(_) => counts.incorrect += 1,
                        None => counts.extra += 1,
                    }
                }
                continue;
            }
            let w: Vec<char> = word.chars().collect();
            let t: Vec<char> = typed.chars().collect();
            let submitted = !current;
            if typed == word {
                counts.correct += w.len() as u32;
                counts.word_chars += w.len() as u32;
                // The last word of a test with an end gets no space after it.
                if submitted {
                    counts.spaces += 1;
                }
                continue;
            }
            // The word the time ran out in counts if what's typed of it is right.
            if current && w.starts_with(&t) {
                counts.correct += t.len() as u32;
                counts.word_chars += t.len() as u32;
                continue;
            }
            for (i, c) in t.iter().enumerate() {
                match w.get(i) {
                    Some(wc) if wc == c => counts.correct += 1,
                    Some(_) => counts.incorrect += 1,
                    None => counts.extra += 1,
                }
            }
            if submitted || self.word_goal().is_some() {
                counts.missed += w.len().saturating_sub(t.len()) as u32;
            }
            if submitted {
                counts.wrong_spaces += 1;
            }
        }
        counts
    }

    /// How it's going, or how it went.
    pub fn stats(&self, now: f64) -> Stats {
        let seconds = self.elapsed(now);
        let counts = self.counts(self.finished());
        let keys = self.right_keys + self.wrong_keys;
        let raws: Vec<f32> = self.history.iter().map(|second| second.raw).collect();
        Stats {
            wpm: speed(counts.word_chars + counts.spaces, seconds),
            raw: speed(counts.correct + counts.incorrect + counts.extra + counts.spaces + counts.wrong_spaces, seconds),
            accuracy: if keys == 0 { 100.0 } else { self.right_keys as f32 * 100.0 / keys as f32 },
            consistency: consistency(&raws),
            correct: counts.correct,
            incorrect: counts.incorrect,
            extra: counts.extra,
            missed: counts.missed,
            seconds: seconds as f32,
        }
    }

    /// The result, once the test is over.
    pub fn result(&self) -> Option<Stats> {
        self.ended.map(|end| self.stats(end))
    }
}

#[derive(Default)]
struct Counts {
    correct: u32,
    incorrect: u32,
    extra: u32,
    missed: u32,
    /// Characters of words typed right.
    word_chars: u32,
    /// Spaces after words typed right, and after words typed wrong.
    spaces: u32,
    wrong_spaces: u32,
}

/// Characters in this many seconds, in words a minute.
fn speed(chars: u32, seconds: f64) -> f32 {
    if seconds <= 0.0 {
        return 0.0;
    }
    (chars as f64 * 60.0 / seconds / 5.0) as f32
}

/// How steady these speeds are, from 0 to 100, as Monkeytype works it out: the coefficient of
/// variation, mapped so that 0 is perfectly steady.
pub fn consistency(speeds: &[f32]) -> f32 {
    if speeds.is_empty() {
        return 0.0;
    }
    let n = speeds.len() as f64;
    let mean = speeds.iter().map(|&s| s as f64).sum::<f64>() / n;
    if mean <= 0.0 {
        return 0.0;
    }
    let deviation = (speeds.iter().map(|&s| (s as f64 - mean).powi(2)).sum::<f64>() / n).sqrt();
    let cov = deviation / mean;
    let kogasa = 100.0 * (1.0 - (cov + cov.powi(3) / 3.0 + cov.powi(5) / 5.0).tanh());
    kogasa.clamp(0.0, 100.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(mode: Mode) -> Options {
        Options { mode, punctuation: false, numbers: false, words: WordList::English }
    }

    fn type_text(test: &mut Test, text: &str, mut now: f64, gap: f64) -> f64 {
        for c in text.chars() {
            test.type_char(c, now);
            now += gap;
        }
        now
    }

    #[test]
    fn the_lists_are_there() {
        assert_eq!(WordList::English.words().count(), 200);
        assert_eq!(WordList::English1k.words().count(), 1000);
        assert!(quotes(QuoteLength::Short).count() > 500);
        assert!(quotes(QuoteLength::Medium).count() > 500);
        assert!(quotes(QuoteLength::Short).all(|quote| quote.text.len() <= SHORT_QUOTE && !quote.source.is_empty()));
    }

    #[test]
    fn a_words_test_has_its_words_and_ends_on_the_last_one() {
        let mut test = Test::new(options(Mode::Words(10)), 7);
        assert_eq!(test.words.len(), 10);
        assert!(test.words.windows(2).all(|pair| pair[0] != pair[1]), "no word twice running");
        let text = test.words.join(" ");
        let end = type_text(&mut test, &text, 0.0, 100.0);
        assert!(test.finished(), "typing the last word right ends it, with no space");
        let stats = test.result().unwrap();
        assert_eq!(stats.accuracy, 100.0);
        assert_eq!(stats.incorrect + stats.extra + stats.missed, 0);
        // 100 ms a character is 600 characters a minute, which is 120 wpm.
        let chars = text.chars().count() as f32;
        let seconds = (end - 100.0) / 1000.0;
        assert!((stats.wpm - chars * 60.0 / seconds as f32 / 5.0).abs() < 0.01, "{}", stats.wpm);
        assert!((stats.wpm - 120.0).abs() < 4.0, "a little over, as the clock starts on the first key: {}", stats.wpm);
        assert_eq!(stats.wpm, stats.raw, "nothing wrong, so raw is the same");
    }

    #[test]
    fn mistakes_cost_wpm_and_accuracy_but_not_raw() {
        let mut test = Test::new(options(Mode::Words(10)), 3);
        let mut words = test.words.clone();
        words[0] = format!("{}x", words[0]);
        let text = words.join(" ");
        type_text(&mut test, &text, 0.0, 100.0);
        let stats = test.result().unwrap();
        assert_eq!(stats.extra, 1);
        assert!(stats.accuracy < 100.0);
        assert!(stats.wpm < stats.raw, "the wrong word and its space don't count");
        assert!(test.word_wrong(0));
        assert!(!test.word_wrong(1));
    }

    #[test]
    fn a_timed_test_ends_when_the_time_is_up() {
        let mut test = Test::new(options(Mode::Time(15)), 1);
        assert!(!test.started());
        test.type_char(' ', 0.0);
        assert!(!test.started(), "a space first starts nothing");
        let word = test.words[0].clone();
        type_text(&mut test, &word, 1000.0, 100.0);
        assert!(test.running());
        assert!(!test.tick(10_000.0));
        assert!(test.tick(16_500.0));
        assert!(test.finished());
        let stats = test.result().unwrap();
        assert!((stats.seconds - 15.0).abs() < 0.001, "{}", stats.seconds);
        assert_eq!(test.history.len(), 15);
        assert!(stats.wpm > 0.0, "the word typed right counts when time runs out");
        assert!(!test.type_char('a', 17_000.0), "nothing more once it's over");
    }

    #[test]
    fn timed_tests_never_run_out_of_words() {
        let mut test = Test::new(options(Mode::Time(60)), 9);
        for _ in 0..200 {
            let at = test.at();
            let word = test.words[at].clone();
            type_text(&mut test, &format!("{word} "), 0.0, 0.0);
        }
        assert!(test.words.len() > test.at() + 10);
    }

    #[test]
    fn backspace_goes_back_only_into_a_wrong_word() {
        let mut test = Test::new(options(Mode::Words(10)), 5);
        let first = test.words[0].clone();
        type_text(&mut test, &format!("{first} "), 0.0, 50.0);
        assert_eq!(test.at(), 1);
        assert!(!test.backspace(false, 1000.0), "the word before was right");
        assert_eq!(test.at(), 1);

        let mut test = Test::new(options(Mode::Words(10)), 5);
        type_text(&mut test, "zzz ", 0.0, 50.0);
        assert!(test.backspace(false, 1000.0), "it was wrong, so it can be fixed");
        assert_eq!(test.at(), 0);
        assert_eq!(test.typed[0], "zzz");
        assert!(test.backspace(true, 1000.0));
        assert_eq!(test.typed[0], "");
    }

    #[test]
    fn marks_show_right_wrong_and_extra() {
        let mut test = Test::new(options(Mode::Words(10)), 2);
        test.words[0] = "abc".into();
        type_text(&mut test, "axcde", 0.0, 10.0);
        let marks: Vec<Mark> = test.marks(0).into_iter().map(|(_, mark)| mark).collect();
        assert_eq!(marks, [Mark::Correct, Mark::Incorrect, Mark::Correct, Mark::Extra, Mark::Extra]);
        let untyped: Vec<Mark> = test.marks(1).into_iter().map(|(_, mark)| mark).collect();
        assert!(untyped.iter().all(|&mark| mark == Mark::Untyped));
    }

    #[test]
    fn a_quote_is_typed_as_written() {
        let test = Test::new(Options { punctuation: true, numbers: true, ..options(Mode::Quote(QuoteLength::Short)) }, 11);
        let source = test.source.unwrap();
        assert!(quotes(QuoteLength::Short).any(|quote| quote.source == source && quote.text.split_whitespace().eq(test.words.iter().map(String::as_str))));
        assert_eq!(test.options.describe(), "quote short");
    }

    #[test]
    fn punctuation_and_numbers_change_the_words() {
        let test = Test::new(Options { punctuation: true, numbers: true, ..options(Mode::Words(100)) }, 4);
        assert!(test.words[0].chars().next().unwrap().is_uppercase(), "a sentence starts with a capital");
        assert!(test.words.last().unwrap().ends_with(['.', '?', '!']), "and the test ends with one");
        assert!(test.words.iter().any(|word| word.chars().all(|c| c.is_ascii_digit())));
        assert!(test.words.iter().any(|word| word.ends_with(',')));
    }

    #[test]
    fn consistency_is_high_when_steady() {
        assert_eq!(consistency(&[60.0, 60.0, 60.0]), 100.0);
        let steady = consistency(&[58.0, 62.0, 60.0, 61.0]);
        let ragged = consistency(&[20.0, 90.0, 40.0, 110.0]);
        assert!(steady > 90.0 && ragged < steady, "{steady} {ragged}");
        assert_eq!(consistency(&[]), 0.0);
    }

    #[test]
    fn bests_are_kept_by_kind() {
        let mut settings = TypingSettings::default();
        let mut test = Test::new(options(Mode::Words(10)), 8);
        let text = test.words.join(" ");
        type_text(&mut test, &text, 0.0, 80.0);
        assert!(settings.record(&test), "the first is the best yet");
        assert!(!settings.record(&test), "the same again isn't better");
        assert_eq!(settings.best(&test.options), test.result().map(|stats| stats.wpm));
        assert_eq!(settings.best(&options(Mode::Words(25))), None);

        let mut sloppy = Test::new(options(Mode::Words(10)), 8);
        let text: String = sloppy.words.join(" ").chars().map(|c| if c == ' ' { ' ' } else { 'q' }).collect();
        type_text(&mut sloppy, &text, 0.0, 10.0);
        assert!(!TypingSettings::default().record(&sloppy), "too many mistakes to be a best");
    }

    #[test]
    fn settings_read_back() {
        let settings = TypingSettings { mode: Mode::Quote(QuoteLength::Medium), punctuation: true, ..TypingSettings::default() };
        let json = serde_json::to_string(&settings).unwrap();
        assert_eq!(serde_json::from_str::<TypingSettings>(&json).unwrap(), settings);
        assert_eq!(serde_json::from_str::<TypingSettings>("{}").unwrap(), TypingSettings::default());
    }
}
