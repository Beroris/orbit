//! The emoji list and searching it. The table comes from Unicode's emoji test file and CLDR's
//! keywords (see docs/DECISIONS.md), without skin tone variants or country flags, which Windows
//! draws as two plain letters.

use crate::text::fold;

const TABLE: &str = include_str!("../data/emoji.tsv");
/// Orbit's own groups of symbols, from keyboard keys to currency, come after Unicode's emoji, from this
/// group on.
const FIRST_SYMBOL_GROUP: &str = "Keyboard";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Emoji {
    pub glyph: &'static str,
    pub name: &'static str,
    pub keywords: &'static str,
    /// Which of `groups()` it's in.
    pub group: u8,
    /// A plain symbol such as ⌘ or ×, drawn in the text colour rather than as a colour emoji.
    pub symbol: bool,
}

/// A group's title line starts with `#` and has no tab. Keycap # starts with one too, but has tabs.
fn group_title(line: &str) -> Option<&str> {
    if line.contains('\t') { None } else { line.strip_prefix('#') }
}

/// Group names, in the order the table lists them.
pub fn groups() -> Vec<&'static str> {
    TABLE.lines().filter_map(group_title).collect()
}

/// Everything, in Unicode's order. It's read when the list opens and dropped when it closes,
/// rather than held in memory.
pub fn table() -> Vec<Emoji> {
    let mut out = Vec::with_capacity(1700);
    let mut group = 0u8;
    let mut seen_group = false;
    let mut symbol = false;
    for line in TABLE.lines() {
        if let Some(title) = group_title(line) {
            symbol |= title == FIRST_SYMBOL_GROUP;
            if seen_group {
                group += 1;
            }
            seen_group = true;
            continue;
        }
        let mut parts = line.split('\t');
        let (Some(glyph), Some(name)) = (parts.next(), parts.next()) else {
            continue;
        };
        out.push(Emoji { glyph, name, keywords: parts.next().unwrap_or(""), group, symbol });
    }
    out
}

/// The name as it's shown: "black heart" reads as "Black Heart".
pub fn display_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut start = true;
    for c in name.chars() {
        if start {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
        start = c == ' ' || c == '-';
    }
    out
}

/// Which emoji match, best first, as positions in `table`. The emoji itself, pasted in, comes
/// first, then a word in the name beats a keyword. Symbols are also found by how they're typed
/// in plain text, such as "->" for → or "(c)" for ©, which are among their keywords.
pub fn search(table: &[Emoji], query: &str) -> Vec<u16> {
    let query = query.trim();
    let needle = fold(query);
    if needle.is_empty() {
        return (0..table.len() as u16).collect();
    }
    let bare = |text: &'static str| text.chars().filter(|c| !matches!(c, '\u{FE0E}' | '\u{FE0F}'));
    let is_glyph = |glyph: &'static str| bare(glyph).eq(query.chars().filter(|c| !matches!(c, '\u{FE0E}' | '\u{FE0F}')));
    let at_word_start = |text: &str| starts_a_word(text, &needle);
    let words: Vec<&str> = needle.split_whitespace().collect();
    let mut scored: Vec<(u8, u16)> = Vec::new();
    for (at, emoji) in table.iter().enumerate() {
        let name = fold(emoji.name);
        let keywords = fold(emoji.keywords);
        let score = if is_glyph(emoji.glyph) {
            0
        } else if name == needle || (emoji.symbol && keywords.split(' ').any(|keyword| keyword == needle && !keyword.chars().all(char::is_alphanumeric))) {
            1
        } else if name.starts_with(&needle) {
            2
        } else if at_word_start(&name) {
            3
        } else if at_word_start(&keywords) {
            4
        } else if name.contains(&needle) {
            5
        } else if keywords.contains(&needle) {
            6
        } else if words.len() > 1 && words.iter().all(|word| starts_a_word(&name, word) || starts_a_word(&keywords, word)) {
            // Every word, in the name or the keywords, in any order: "heavy right arrow" finds ➜.
            7
        } else {
            continue;
        };
        scored.push((score, at as u16));
    }
    scored.sort_unstable();
    scored.into_iter().map(|(_, at)| at).collect()
}

/// Where the plain symbols start in [groups]: every group from this one on is symbols.
pub fn first_symbol_group() -> usize {
    let groups = groups();
    groups.iter().position(|group| *group == FIRST_SYMBOL_GROUP).unwrap_or(groups.len())
}

/// Whether `word` is in `text` at the start of a word.
fn starts_a_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(at, _)| at == 0 || !text.as_bytes()[at - 1].is_ascii_alphanumeric())
}

/// The Unicode code points of an emoji or symbol, as "U+2318", or "U+1F1EC U+1F1E7" for one made
/// of several. The variation selector that only asks for colour or text style is left out.
pub fn code_points(glyph: &str) -> String {
    glyph.chars().filter(|c| !matches!(c, '\u{FE0E}' | '\u{FE0F}')).map(|c| format!("U+{:04X}", c as u32)).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(query: &str) -> Vec<&'static str> {
        let table = table();
        search(&table, query).into_iter().map(|at| table[at as usize].glyph).collect()
    }

    #[test]
    fn the_table_reads_cleanly() {
        let table = table();
        assert!(table.len() > 1500, "{}", table.len());
        let groups = groups();
        assert_eq!(groups.len(), 13);
        assert_eq!(groups[0], "Smileys & People");
        assert!(table.iter().all(|e| (e.group as usize) < groups.len()));
        assert!(table.iter().all(|e| !e.glyph.is_empty() && !e.name.is_empty()));
        assert_eq!(table[0].glyph, "😀");
    }

    #[test]
    fn symbols_come_after_the_emoji_and_are_found_by_what_they_are() {
        let table = table();
        assert!(!table[0].symbol);
        assert!(table.iter().all(|e| e.symbol == (e.group as usize >= groups().iter().position(|g| *g == FIRST_SYMBOL_GROUP).unwrap())));
        assert_eq!(found("cmd")[0], "\u{2318}");
        // The symbol itself, pasted in, finds its own tile first, with or without VS16.
        assert_eq!(found("\u{2192}")[0], "\u{2192}");
        assert_eq!(found("\u{279C}")[0], "\u{279C}");
        assert_eq!(found("\u{00D7}")[0], "\u{00D7}");
        assert_eq!(found("\u{2764}\u{FE0F}")[0], found("\u{2764}")[0]);
        assert_eq!(found("command")[0], "\u{2318}");
        assert!(found("multiply").contains(&"\u{00D7}"));
        assert!(found("option").contains(&"\u{2325}"));
        assert!(found("euro").contains(&"\u{20AC}"));
        assert!(found("hashtag").contains(&"#"), "a lone # is a symbol, not a group title");
        assert_eq!(found("grave")[0], "`");
        assert_eq!(found("backtick")[0], "`");
        assert!(found("percent").contains(&"%"));
        assert!(found("backslash").contains(&"\\"));
    }

    #[test]
    fn keycap_hash_is_an_emoji_and_not_a_group_title() {
        assert!(table().iter().any(|e| e.name == "keycap: #"));
    }

    #[test]
    fn no_skin_tones_or_country_flags() {
        assert!(!table().iter().any(|e| e.name.contains("skin tone") || e.name.starts_with("flag: ")));
    }

    #[test]
    fn names_come_before_keywords_and_exact_comes_first() {
        assert_eq!(found("red heart")[0], "❤️");
        assert!(found("heart").len() > 10);
        assert_eq!(found("rocket")[0], "🚀");
        assert!(found("zzzqqq").is_empty());
    }

    #[test]
    fn keywords_find_things_their_names_do_not() {
        assert!(found("astronomy").iter().any(|glyph| *glyph != "🔭"), "a keyword");
        assert!(found("telescope").contains(&"🔭"));
    }

    #[test]
    fn an_empty_search_is_everything() {
        assert_eq!(search(&table(), "  ").len(), table().len());
    }

    #[test]
    fn display_names_are_capitalised() {
        assert_eq!(display_name("black heart"), "Black Heart");
        assert_eq!(display_name("man-woman"), "Man-Woman");
    }

    #[test]
    fn symbols_are_found_by_how_they_are_typed_in_plain_text() {
        for (typed, glyph) in [("->", "\u{2192}"), ("<=", "\u{2264}"), ("!=", "\u{2260}"), ("+-", "\u{00B1}"), ("(c)", "\u{00A9}"), ("...", "\u{2026}"), ("-->", "\u{27F6}"), ("1/2", "\u{00BD}")] {
            assert_eq!(found(typed)[0], glyph, "{typed}");
        }
    }

    #[test]
    fn several_words_are_found_in_any_order() {
        assert!(found("heavy right arrow").contains(&"\u{279C}"));
        assert!(found("arrow heavy").contains(&"\u{279C}"));
        assert!(!found("heavy banana arrow").contains(&"\u{279C}"));
    }

    #[test]
    fn code_points_are_written_the_usual_way() {
        assert_eq!(code_points("\u{2318}"), "U+2318");
        assert_eq!(code_points("\u{2764}\u{FE0F}"), "U+2764");
        assert_eq!(code_points("\u{1F1EC}\u{1F1E7}"), "U+1F1EC U+1F1E7");
        assert_eq!(code_points("A"), "U+0041");
    }
}
