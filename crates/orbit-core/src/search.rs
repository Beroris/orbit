//! In-memory ranked search for apps, commands and links.
//!
//! The index is one string arena plus a compact fixed-size record per item. A query walks
//! the records with a bitmask reject first, allocates nothing per item, and keeps the best
//! results in a small sorted buffer.
//!
//! Scoring is deliberately explainable. Each hit has one [`MatchClass`], which sets a base
//! score. Small bonuses for match quality, pinning and bounded usage are added to it, and
//! the bonus ceiling is smaller than the gap above Prefix, so usage can reorder close
//! matches but can never beat an exact name or alias.

use crate::text::{char_mask, fold, fold_into, word_starts};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MatchClass {
    Metadata,
    Fuzzy,
    Typo,
    Substring,
    Acronym,
    WordPrefix,
    Prefix,
    AliasPrefix,
    ExactName,
    ExactAlias,
}

impl MatchClass {
    pub const fn base_score(self) -> i32 {
        match self {
            MatchClass::ExactAlias => 8000,
            MatchClass::ExactName => 7000,
            MatchClass::AliasPrefix => 5200,
            MatchClass::Prefix => 5000,
            MatchClass::WordPrefix => 4400,
            MatchClass::Acronym => 4100,
            MatchClass::Substring => 3000,
            MatchClass::Typo => 2500,
            MatchClass::Fuzzy => 2000,
            MatchClass::Metadata => 1000,
        }
    }
}

/// Largest match-quality bonus. Together with the two below it stays under 1750, the gap
/// between Prefix and ExactName.
pub const MAX_QUALITY_BONUS: i32 = 300;
pub const PIN_BONUS: i32 = 150;
pub const MAX_USAGE_BONUS: u16 = 800;

/// What the host adds to the index. `key` is a stable identity across rebuilds.
#[derive(Clone, Debug, Default)]
pub struct SearchItem {
    pub key: String,
    pub title: String,
    pub subtitle: String,
    pub alias: String,
    pub keywords: String,
    pub pinned: bool,
    pub usage: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub item: u32,
    pub score: i32,
    pub class: MatchClass,
}

#[derive(Clone, Copy, Default)]
struct Range {
    start: u32,
    len: u32,
}

#[derive(Clone, Default)]
struct Record {
    key: Range,
    title: Range,
    subtitle: Range,
    folded_title: Range,
    folded_alias: Range,
    folded_keywords: Range,
    word_starts: u64,
    keyword_starts: u64,
    mask: u64,
    usage: u16,
    pinned: bool,
}

#[derive(Default)]
pub struct SearchIndex {
    arena: String,
    records: Vec<Record>,
}

/// Reusable per-query buffers.
#[derive(Default)]
pub struct QueryScratch {
    folded: String,
}

fn push(arena: &mut String, text: &str) -> Range {
    let start = arena.len() as u32;
    arena.push_str(text);
    Range { start, len: text.len() as u32 }
}

impl SearchIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(items: usize) -> Self {
        SearchIndex { arena: String::with_capacity(items * 48), records: Vec::with_capacity(items) }
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Bytes held by the index, for the diagnostics panel.
    pub fn heap_bytes(&self) -> usize {
        self.arena.capacity() + self.records.capacity() * std::mem::size_of::<Record>()
    }

    pub fn add(&mut self, item: &SearchItem) -> u32 {
        let arena = &mut self.arena;
        let key = push(arena, &item.key);
        let title = push(arena, &item.title);
        let subtitle = push(arena, &item.subtitle);

        let start = arena.len();
        fold_into(&item.title, arena);
        let folded_title = Range { start: start as u32, len: (arena.len() - start) as u32 };
        let mask = char_mask(&arena[start..]);

        let start = arena.len();
        fold_into(&item.alias, arena);
        let folded_alias = Range { start: start as u32, len: (arena.len() - start) as u32 };

        let start = arena.len();
        fold_into(&item.keywords, arena);
        let folded_keywords = Range { start: start as u32, len: (arena.len() - start) as u32 };

        self.records.push(Record {
            key,
            title,
            subtitle,
            folded_title,
            folded_alias,
            folded_keywords,
            word_starts: word_starts(&item.title),
            keyword_starts: word_starts(&item.keywords),
            mask,
            usage: item.usage.min(MAX_USAGE_BONUS),
            pinned: item.pinned,
        });
        (self.records.len() - 1) as u32
    }

    #[inline]
    fn get(&self, range: Range) -> &str {
        &self.arena[range.start as usize..(range.start + range.len) as usize]
    }

    pub fn key(&self, item: u32) -> &str {
        self.get(self.records[item as usize].key)
    }
    pub fn title(&self, item: u32) -> &str {
        self.get(self.records[item as usize].title)
    }
    pub fn subtitle(&self, item: u32) -> &str {
        self.get(self.records[item as usize].subtitle)
    }
    pub fn is_pinned(&self, item: u32) -> bool {
        self.records[item as usize].pinned
    }

    pub fn find_key(&self, key: &str) -> Option<u32> {
        self.records.iter().position(|r| self.get(r.key) == key).map(|i| i as u32)
    }

    pub fn set_usage(&mut self, item: u32, usage: u16) {
        self.records[item as usize].usage = usage.min(MAX_USAGE_BONUS);
    }

    pub fn set_pinned(&mut self, item: u32, pinned: bool) {
        self.records[item as usize].pinned = pinned;
    }

    /// Pinned items, then frequent and recent ones. Used when the query is empty.
    pub fn suggestions(&self, limit: usize, out: &mut Vec<Hit>) {
        out.clear();
        for (i, r) in self.records.iter().enumerate() {
            if r.pinned || r.usage > 0 {
                let score = r.usage as i32 + if r.pinned { 10_000 } else { 0 };
                self.insert_top(out, limit, Hit { item: i as u32, score, class: MatchClass::Metadata });
            }
        }
    }

    pub fn search(&self, query: &str, limit: usize, scratch: &mut QueryScratch, out: &mut Vec<Hit>) {
        out.clear();
        scratch.folded.clear();
        fold_into(query.trim(), &mut scratch.folded);
        let q = scratch.folded.as_str();
        if q.is_empty() || limit == 0 {
            return;
        }
        let q_mask = char_mask(q);
        let typos = typo_edits(q);

        for (i, record) in self.records.iter().enumerate() {
            let title = self.get(record.folded_title);
            let alias = self.get(record.folded_alias);
            let missing = (q_mask & !record.mask).count_ones();
            let mut best = None;

            if !alias.is_empty() {
                best = score_alias(alias, q);
            }
            if best.is_none() {
                if missing == 0 {
                    best = score_title(title, record.word_starts, q);
                }
                // A misspelled word beats letters that merely appear in order: "zne" means
                // Zen, not any long title with a z, an n and an e somewhere in it.
                let weak = matches!(best, None | Some((MatchClass::Fuzzy, _)));
                if weak && typos > 0 && missing <= typos as u32 {
                    if let Some(typo) = score_typo(title, record.word_starts, q, typos) {
                        best = Some(typo);
                    }
                }
                if best.is_none() && record.folded_keywords.len > 0 {
                    best = score_keywords(
                        self.get(record.folded_keywords),
                        record.keyword_starts,
                        q,
                    );
                }
            }
            if let Some((class, quality)) = best {
                let score = class.base_score()
                    + quality.clamp(0, MAX_QUALITY_BONUS)
                    + record.usage as i32
                    + if record.pinned { PIN_BONUS } else { 0 };
                self.insert_top(out, limit, Hit { item: i as u32, score, class });
            }
        }
    }

    /// Keeps `out` sorted best-first and at most `limit` long. Ties break on shorter title,
    /// then title, then key, so ranking is fully deterministic.
    #[inline]
    fn insert_top(&self, out: &mut Vec<Hit>, limit: usize, hit: Hit) {
        if out.len() == limit {
            let worst = out[limit - 1];
            if !self.ranks_before(&hit, &worst) {
                return;
            }
            out.pop();
        }
        let at = out.partition_point(|existing| self.ranks_before(existing, &hit));
        out.insert(at, hit);
    }

    fn ranks_before(&self, a: &Hit, b: &Hit) -> bool {
        if a.score != b.score {
            return a.score > b.score;
        }
        let ra = &self.records[a.item as usize];
        let rb = &self.records[b.item as usize];
        if ra.title.len != rb.title.len {
            return ra.title.len < rb.title.len;
        }
        match self.get(ra.title).cmp(self.get(rb.title)) {
            std::cmp::Ordering::Equal => self.get(ra.key) < self.get(rb.key),
            ordering => ordering.is_lt(),
        }
    }
}

fn score_alias(alias: &str, q: &str) -> Option<(MatchClass, i32)> {
    if alias == q {
        Some((MatchClass::ExactAlias, 0))
    } else if alias.starts_with(q) {
        Some((MatchClass::AliasPrefix, closeness(q.len(), alias.len())))
    } else {
        None
    }
}

/// Higher when the query covers more of the target.
#[inline]
fn closeness(q_len: usize, target_len: usize) -> i32 {
    if target_len == 0 { 0 } else { (MAX_QUALITY_BONUS as usize * q_len / target_len) as i32 }
}

#[inline]
fn is_word_start(starts: u64, at: usize) -> bool {
    at < 64 && starts & (1u64 << at) != 0
}

/// Finds `q` as a word prefix. Returns the byte offset.
fn find_word_prefix(text: &str, starts: u64, q: &str) -> Option<usize> {
    let mut bits = starts;
    while bits != 0 {
        let at = bits.trailing_zeros() as usize;
        bits &= bits - 1;
        if text.as_bytes().get(at..).is_some_and(|rest| rest.starts_with(q.as_bytes())) {
            return Some(at);
        }
    }
    None
}

fn score_title(title: &str, starts: u64, q: &str) -> Option<(MatchClass, i32)> {
    if title == q {
        return Some((MatchClass::ExactName, 0));
    }
    if title.starts_with(q) {
        return Some((MatchClass::Prefix, closeness(q.len(), title.len())));
    }
    if let Some(at) = find_word_prefix(title, starts, q) {
        // Earlier words rank a little higher.
        let position_penalty = (at as i32).min(40);
        return Some((MatchClass::WordPrefix, closeness(q.len(), title.len()) - position_penalty));
    }
    // "vis cod" must match word prefixes of the title, in any order.
    if q.contains(' ') {
        let tokens_match = q
            .split_ascii_whitespace()
            .all(|token| find_word_prefix(title, starts, token).is_some());
        if tokens_match {
            return Some((MatchClass::WordPrefix, closeness(q.len(), title.len()) / 2));
        }
    }
    if is_acronym(title, starts, q) {
        return Some((MatchClass::Acronym, closeness(q.len(), starts.count_ones() as usize)));
    }
    if q.len() >= 2 {
        if let Some(at) = title.find(q) {
            return Some((MatchClass::Substring, closeness(q.len(), title.len()) - (at as i32).min(40)));
        }
    }
    score_fuzzy(title, starts, q).map(|quality| (MatchClass::Fuzzy, quality))
}

/// True if every query byte matches a successive word start: "vsc" for "Visual Studio Code".
fn is_acronym(title: &str, starts: u64, q: &str) -> bool {
    if q.len() < 2 || q.len() > starts.count_ones() as usize {
        return false;
    }
    let bytes = title.as_bytes();
    let mut bits = starts;
    let mut qi = 0;
    while bits != 0 && qi < q.len() {
        let at = bits.trailing_zeros() as usize;
        bits &= bits - 1;
        if bytes.get(at) == Some(&q.as_bytes()[qi]) {
            qi += 1;
        }
    }
    qi == q.len()
}

/// Forward subsequence match. Rejects scattered matches, which are noise rather than signal:
/// the matched span may be at most about three times the query, unless the match starts a word.
fn score_fuzzy(title: &str, starts: u64, q: &str) -> Option<i32> {
    if q.len() < 2 {
        return None;
    }
    let t = title.as_bytes();
    let qb = q.as_bytes();
    if qb.len() > t.len() {
        return None;
    }
    let first = t.iter().position(|&b| b == qb[0])?;
    let mut qi = 1;
    let mut last = first;
    let mut word_hits = i32::from(is_word_start(starts, first));
    let mut i = first + 1;
    while i < t.len() && qi < qb.len() {
        if t[i] == qb[qi] {
            word_hits += i32::from(is_word_start(starts, i));
            last = i;
            qi += 1;
        }
        i += 1;
    }
    if qi < qb.len() {
        return None;
    }
    let span = last - first + 1;
    if span > qb.len() * 3 + 2 && word_hits == 0 {
        return None;
    }
    let gaps = (span - qb.len()) as i32;
    Some((150 + word_hits * 30 - gaps * 8).max(0))
}

/// How many mistakes a query may contain. Short queries get little room, because nearly any
/// word is one edit away from three letters; those are handled in [`typo_at`].
fn typo_edits(q: &str) -> usize {
    if !q.is_ascii() {
        return 0; // Typo matching is for Latin text. Other scripts match exactly above.
    }
    match q.len() {
        0..=2 => 0,
        3..=6 => 1,
        _ => 2,
    }
}

/// Finds a word in the title that `q` misspells. Multi-word queries may misspell each word.
fn score_typo(title: &str, starts: u64, q: &str, edits: usize) -> Option<(MatchClass, i32)> {
    let t = title.as_bytes();
    let qb = q.as_bytes();
    let mut bits = starts;
    while bits != 0 {
        let at = bits.trailing_zeros() as usize;
        bits &= bits - 1;
        let word = word_len(t, at, bits);
        if typo_at(qb, &t[at.min(t.len())..], word, edits) {
            // A whole word outranks the start of a longer one, and shorter titles and
            // earlier words come first.
            let whole = word.abs_diff(qb.len()) <= 1;
            let quality = if whole { 200 } else { 100 } + closeness(qb.len(), t.len()) / 3
                - (at as i32).min(40);
            return Some((MatchClass::Typo, quality.max(0)));
        }
    }
    if !q.contains(' ') {
        return None;
    }
    // "vsual code": each word is either right or misspelled, in any order.
    let mut typed = 0;
    for token in q.split_ascii_whitespace() {
        typed += 1;
        if find_word_prefix(title, starts, token).is_some() {
            continue;
        }
        let allowed = typo_edits(token).min(1);
        let mut bits = starts;
        let mut found = false;
        while bits != 0 && !found {
            let at = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let word = word_len(t, at, bits);
            found = allowed > 0 && typo_at(token.as_bytes(), &t[at.min(t.len())..], word, allowed);
        }
        if !found {
            return None;
        }
    }
    (typed > 1).then_some((MatchClass::Typo, 50))
}

/// Bytes in the word that starts at `at`. `next_starts` holds the word starts after it.
fn word_len(t: &[u8], at: usize, next_starts: u64) -> usize {
    let limit = if next_starts != 0 { next_starts.trailing_zeros() as usize } else { t.len() };
    t.get(at..limit.min(t.len()))
        .map_or(0, |word| word.iter().take_while(|b| b.is_ascii_alphanumeric()).count())
}

/// True if `q` is a misspelling of the word starting `t`, whose first `word` bytes are the word.
fn typo_at(q: &[u8], t: &[u8], word: usize, edits: usize) -> bool {
    let n = q.len();
    if t.len() < 2 || n < 3 {
        return false;
    }
    if n == 3 {
        // Three letters are close to most of the dictionary, so only two mistakes count: a
        // swap ("zne"), or one wrong letter in a word that is itself three letters ("zrn").
        let swapped = (q[0] == t[1] && q[1] == t[0] && t.get(2) == Some(&q[2]))
            || (q[0] == t[0] && t.get(1) == Some(&q[2]) && t.get(2) == Some(&q[1]));
        let wrong_letter = word == 3
            && q[0] == t[0]
            && q.iter().zip(t).filter(|(a, b)| a != b).count() == 1;
        return swapped || wrong_letter;
    }
    // The first letter is the most reliable signal, and requiring it keeps noise low: "note"
    // should not match "vote". Longer queries have enough letters left to allow it.
    if q[0] != t[0] {
        if q[0] == t[1] && q[1] == t[0] {
            return within_edits(&q[2..], &t[2..], edits - 1);
        }
        return n >= 5
            && edits >= 1
            && (within_edits(&q[1..], &t[1..], edits - 1) || within_edits(&q[1..], t, edits - 1));
    }
    within_edits(q, t, edits)
}

/// True if `q` is at most `edits` substitutions, insertions, deletions or adjacent swaps from
/// some prefix of `t`. Branches at most four ways per edit, and `edits` is at most 2.
fn within_edits(q: &[u8], t: &[u8], edits: usize) -> bool {
    let same = q.iter().zip(t).take_while(|(a, b)| a == b).count();
    let (q, t) = (&q[same..], &t[same..]);
    if q.is_empty() {
        return true;
    }
    if t.is_empty() {
        return q.len() <= edits;
    }
    if edits == 0 {
        return false;
    }
    within_edits(&q[1..], &t[1..], edits - 1)
        || within_edits(&q[1..], t, edits - 1)
        || within_edits(q, &t[1..], edits - 1)
        || (q.len() >= 2
            && t.len() >= 2
            && q[0] == t[1]
            && q[1] == t[0]
            && within_edits(&q[2..], &t[2..], edits - 1))
}

fn score_keywords(keywords: &str, starts: u64, q: &str) -> Option<(MatchClass, i32)> {
    if find_word_prefix(keywords, starts, q).is_some() {
        Some((MatchClass::Metadata, closeness(q.len(), keywords.len().max(1))))
    } else {
        None
    }
}

/// Bounded usage signal. Doubling the use count adds a fixed amount and old use decays with
/// a two-week half-life, down to a floor so a long-term favourite is not forgotten entirely.
pub fn usage_bonus(count: u32, age_seconds: i64) -> u16 {
    if count == 0 {
        return 0;
    }
    let age_days = age_seconds.max(0) as f64 / 86_400.0;
    let decay = 0.5f64.powf(age_days / 14.0).max(0.2);
    let raw = (1.0 + count as f64).log2() * 100.0 * decay;
    raw.round().clamp(0.0, MAX_USAGE_BONUS as f64) as u16
}

/// Folds text for external callers such as note search query building.
pub fn folded(text: &str) -> String {
    fold(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(items: &[(&str, &str, u16, bool)]) -> SearchIndex {
        let mut index = SearchIndex::new();
        for (i, (title, alias, usage, pinned)) in items.iter().enumerate() {
            index.add(&SearchItem {
                key: format!("k{i}"),
                title: title.to_string(),
                alias: alias.to_string(),
                usage: *usage,
                pinned: *pinned,
                ..Default::default()
            });
        }
        index
    }

    fn run(index: &SearchIndex, q: &str) -> Vec<(String, MatchClass)> {
        let mut out = Vec::new();
        index.search(q, 10, &mut QueryScratch::default(), &mut out);
        out.iter().map(|h| (index.title(h.item).to_string(), h.class)).collect()
    }

    fn top(index: &SearchIndex, q: &str) -> Option<String> {
        run(index, q).into_iter().next().map(|(title, _)| title)
    }

    const APPS: &[(&str, &str, u16, bool)] = &[
        ("Visual Studio Code", "", 0, false),
        ("Visual Studio 2022", "", 0, false),
        ("Google Chrome", "", 0, false),
        ("Command Prompt", "", 0, false),
        ("Calculator", "", 0, false),
        ("Control Panel", "", 0, false),
        ("PowerShell", "", 0, false),
        ("Notepad", "", 0, false),
        ("Notepad++", "", 0, false),
        ("Spotify", "", 0, false),
        ("Steam", "", 0, false),
        ("Firefox", "", 0, false),
        ("File Explorer", "", 0, false),
    ];

    #[test]
    fn exact_name_beats_longer_prefix() {
        let idx = index(APPS);
        let results = run(&idx, "notepad");
        assert_eq!(results[0], ("Notepad".to_string(), MatchClass::ExactName));
        assert_eq!(results[1], ("Notepad++".to_string(), MatchClass::Prefix));
    }

    #[test]
    fn acronym_finds_multiword_apps() {
        let idx = index(APPS);
        assert_eq!(run(&idx, "vsc")[0], ("Visual Studio Code".to_string(), MatchClass::Acronym));
        assert_eq!(top(&idx, "ps"), Some("PowerShell".into()));
        assert_eq!(top(&idx, "gc"), Some("Google Chrome".into()));
    }

    #[test]
    fn word_prefix_matches_later_words() {
        let idx = index(APPS);
        assert_eq!(run(&idx, "code")[0], ("Visual Studio Code".to_string(), MatchClass::WordPrefix));
        assert_eq!(run(&idx, "shell")[0], ("PowerShell".to_string(), MatchClass::WordPrefix));
        assert_eq!(top(&idx, "vis cod"), Some("Visual Studio Code".into()));
        assert_eq!(top(&idx, "cod vis"), Some("Visual Studio Code".into()));
    }

    #[test]
    fn fuzzy_and_typo_tolerance() {
        let idx = index(APPS);
        assert_eq!(top(&idx, "chrme"), Some("Google Chrome".into())); // missing letter
        assert_eq!(top(&idx, "fierfox"), Some("Firefox".into())); // adjacent swap
        assert_eq!(top(&idx, "spotfy"), Some("Spotify".into())); // missing letter
        assert_eq!(top(&idx, "spitify"), Some("Spotify".into())); // substitution
        assert_eq!(top(&idx, "calcualtor"), Some("Calculator".into())); // adjacent swap
    }

    #[test]
    fn a_misspelled_word_beats_letters_in_order() {
        // From a real launcher: "zne" showed the other two and not Zen.
        let idx = index(&[
            ("Zen Private Browsing", "", 0, false),
            ("7-Zip File Manager", "", 0, false),
            ("Zen", "", 0, false),
        ]);
        let results = run(&idx, "zne");
        assert_eq!(results[0], ("Zen".to_string(), MatchClass::Typo));
        assert_eq!(results[1], ("Zen Private Browsing".to_string(), MatchClass::Typo));
        assert_eq!(results[2].1, MatchClass::Fuzzy);
    }

    #[test]
    fn typos_in_short_and_long_queries() {
        let idx = index(APPS);
        assert_eq!(top(&idx, "stema"), Some("Steam".into())); // swap at the end
        assert_eq!(top(&idx, "sptoify"), Some("Spotify".into()));
        assert_eq!(top(&idx, "xalculator"), Some("Calculator".into())); // wrong first letter
        assert_eq!(top(&idx, "calcualtro"), Some("Calculator".into())); // two swaps
        assert_eq!(top(&idx, "fierfxo"), Some("Firefox".into()));
        assert_eq!(top(&idx, "vsual code"), Some("Visual Studio Code".into()));
        assert_eq!(top(&idx, "gogle chrom"), Some("Google Chrome".into()));

        let idx = index(&[("Zen", "", 0, false), ("Zone Manager", "", 0, false)]);
        assert_eq!(top(&idx, "zrn"), Some("Zen".into())); // a wrong letter in a 3-letter word
        assert_eq!(run(&idx, "zrn").len(), 1, "but not in a longer word");
    }

    #[test]
    fn typos_stay_quiet_on_unrelated_words() {
        let idx = index(APPS);
        assert!(run(&idx, "xyz").is_empty());
        assert!(run(&idx, "qqqq").is_empty());
        // Three letters aren't enough to guess at a longer word from one wrong letter.
        assert_eq!(top(&idx, "not"), Some("Notepad".into()));
        assert!(run(&idx, "nor").is_empty());
        // A first letter is only forgiven with enough letters left to be sure.
        assert!(run(&idx, "vote").is_empty());
    }

    #[test]
    fn correct_spelling_always_outranks_a_typo() {
        let idx = index(&[("Stem", "", MAX_USAGE_BONUS, false), ("Steam", "", 0, false)]);
        assert_eq!(top(&idx, "steam"), Some("Steam".into()));
        let idx = index(&[("Control Panel", "", 0, false), ("Cortana", "", 0, false)]);
        assert_eq!(run(&idx, "cont")[0].1, MatchClass::Prefix);
    }

    #[test]
    fn scattered_subsequences_are_rejected() {
        let idx = index(&[("Application Programming Interface Reference Library", "", 0, false)]);
        assert!(run(&idx, "zzzz").is_empty());
        let idx = index(&[("abcdefghijklmnopqrstuvwxyz", "", 0, false)]);
        assert!(run(&idx, "bz").is_empty() || run(&idx, "bz")[0].1 == MatchClass::Fuzzy);
        assert!(run(&idx, "bdz").is_empty(), "span 25 for a 3-char query with no word start");
    }

    #[test]
    fn exact_alias_wins_over_everything() {
        let idx = index(&[
            ("Code", "", 800, true),
            ("Terminal", "code", 0, false),
        ]);
        assert_eq!(run(&idx, "code")[0], ("Terminal".to_string(), MatchClass::ExactAlias));
    }

    #[test]
    fn usage_cannot_beat_an_exact_match() {
        let idx = index(&[
            ("Notepad++", "", MAX_USAGE_BONUS, true),
            ("Notepad", "", 0, false),
        ]);
        assert_eq!(top(&idx, "notepad"), Some("Notepad".into()));
    }

    #[test]
    fn usage_can_reorder_prefix_and_word_prefix() {
        let idx = index(&[
            ("Control Panel", "", 0, false),
            ("Visual Studio Code", "", 800, false),
        ]);
        assert_eq!(top(&idx, "co"), Some("Visual Studio Code".into()));
        let idx = index(&[
            ("Control Panel", "", 0, false),
            ("Visual Studio Code", "", 0, false),
        ]);
        assert_eq!(top(&idx, "co"), Some("Control Panel".into()));
    }

    #[test]
    fn bonus_ceiling_is_below_the_exact_gap() {
        let highest_prefix =
            MatchClass::Prefix.base_score() + MAX_QUALITY_BONUS + PIN_BONUS + MAX_USAGE_BONUS as i32;
        assert!(highest_prefix < MatchClass::ExactName.base_score());
    }

    #[test]
    fn ties_are_deterministic() {
        let idx = index(&[("Beta App", "", 0, false), ("Alfa App", "", 0, false)]);
        assert_eq!(top(&idx, "app"), Some("Alfa App".into()));
        // Insertion order does not matter.
        let idx = index(&[("Alfa App", "", 0, false), ("Beta App", "", 0, false)]);
        assert_eq!(top(&idx, "app"), Some("Alfa App".into()));
    }

    #[test]
    fn unicode_names_and_diacritics() {
        let idx = index(&[
            ("Résumé Builder", "", 0, false),
            ("微信", "", 0, false),
            ("Das Örtliche", "", 0, false),
        ]);
        assert_eq!(top(&idx, "resume"), Some("Résumé Builder".into()));
        assert_eq!(top(&idx, "RÉSUMÉ"), Some("Résumé Builder".into()));
        assert_eq!(top(&idx, "微信"), Some("微信".into()));
        assert_eq!(top(&idx, "ortliche"), Some("Das Örtliche".into()));
    }

    #[test]
    fn multibyte_titles_do_not_panic_on_any_query() {
        let idx = index(&[("日本語のアプリ", "", 0, false), ("Ñandú 🚀 launcher", "", 0, false)]);
        for q in ["日", "本語", "n", "🚀", "launcher", "xyzw", "aaaa", "é"] {
            let _ = run(&idx, q);
        }
    }

    #[test]
    fn keywords_rank_below_title_matches() {
        let mut idx = SearchIndex::new();
        idx.add(&SearchItem {
            key: "lock".into(),
            title: "Lock Screen".into(),
            keywords: "security desktop".into(),
            ..Default::default()
        });
        idx.add(&SearchItem { key: "sec".into(), title: "Security Center".into(), ..Default::default() });
        let results = run(&idx, "security");
        assert_eq!(results[0].0, "Security Center");
        assert_eq!(results[1], ("Lock Screen".to_string(), MatchClass::Metadata));
    }

    #[test]
    fn limit_keeps_the_best() {
        let mut idx = SearchIndex::new();
        for i in 0..1000 {
            idx.add(&SearchItem { key: i.to_string(), title: format!("App {i:04}"), ..Default::default() });
        }
        idx.add(&SearchItem { key: "exact".into(), title: "App".into(), ..Default::default() });
        let mut out = Vec::new();
        idx.search("app", 5, &mut QueryScratch::default(), &mut out);
        assert_eq!(out.len(), 5);
        assert_eq!(idx.title(out[0].item), "App");
        assert_eq!(idx.title(out[1].item), "App 0000");
    }

    #[test]
    fn empty_query_gives_pins_then_usage() {
        let idx = index(&[
            ("Rarely", "", 10, false),
            ("Often", "", 500, false),
            ("Unused", "", 0, false),
            ("Pinned", "", 0, true),
        ]);
        let mut out = Vec::new();
        idx.suggestions(10, &mut out);
        let titles: Vec<_> = out.iter().map(|h| idx.title(h.item)).collect();
        assert_eq!(titles, ["Pinned", "Often", "Rarely"]);
        assert!(run(&idx, "   ").is_empty());
    }

    #[test]
    fn usage_bonus_is_bounded_and_decays() {
        assert_eq!(usage_bonus(0, 0), 0);
        assert!(usage_bonus(1, 0) > 0);
        assert!(usage_bonus(100, 0) > usage_bonus(10, 0));
        assert!(usage_bonus(100, 86_400 * 30) < usage_bonus(100, 0));
        assert!(usage_bonus(100, 86_400 * 3650) > 0, "floor keeps long-term favourites");
        assert!(usage_bonus(u32::MAX, 0) <= MAX_USAGE_BONUS);
        assert_eq!(usage_bonus(5, -100), usage_bonus(5, 0), "clock skew is harmless");
    }
}
