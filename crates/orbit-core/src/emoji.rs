//! The emoji list and searching it. The table comes from Unicode's emoji test file and CLDR's
//! keywords (see docs/DECISIONS.md), without skin tone variants or country flags, which Windows
//! draws as two plain letters.

use crate::text::fold;

const TABLE: &str = include_str!("../data/emoji.tsv");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Emoji {
    pub glyph: &'static str,
    pub name: &'static str,
    pub keywords: &'static str,
    /// Which of `groups()` it's in.
    pub group: u8,
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
    for line in TABLE.lines() {
        if group_title(line).is_some() {
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
        out.push(Emoji { glyph, name, keywords: parts.next().unwrap_or(""), group });
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

/// Which emoji match, best first, as positions in `table`. A word in the name beats a keyword.
pub fn search(table: &[Emoji], query: &str) -> Vec<u16> {
    let needle = fold(query.trim());
    if needle.is_empty() {
        return (0..table.len() as u16).collect();
    }
    let at_word_start = |text: &str| {
        text.match_indices(needle.as_str()).any(|(at, _)| at == 0 || !text.as_bytes()[at - 1].is_ascii_alphanumeric())
    };
    let mut scored: Vec<(u8, u16)> = Vec::new();
    for (at, emoji) in table.iter().enumerate() {
        let name = fold(emoji.name);
        let score = if name == needle {
            0
        } else if name.starts_with(&needle) {
            1
        } else if at_word_start(&name) {
            2
        } else {
            let keywords = fold(emoji.keywords);
            if at_word_start(&keywords) {
                3
            } else if name.contains(&needle) {
                4
            } else if keywords.contains(&needle) {
                5
            } else {
                continue;
            }
        };
        scored.push((score, at as u16));
    }
    scored.sort_unstable();
    scored.into_iter().map(|(_, at)| at).collect()
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
        assert_eq!(groups.len(), 8);
        assert_eq!(groups[0], "Smileys & People");
        assert!(table.iter().all(|e| (e.group as usize) < groups.len()));
        assert!(table.iter().all(|e| !e.glyph.is_empty() && !e.name.is_empty()));
        assert_eq!(table[0].glyph, "😀");
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
}
