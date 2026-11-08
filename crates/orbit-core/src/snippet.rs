//! Snippet templates and the rolling trigger matcher for text expansion.

/// What a snippet needs filled in at the moment of insertion. Clipboard use is reported
/// so the host can show a preview before exposing clipboard content.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Expanded {
    pub text: String,
    /// Number of characters the caret must move left after insertion for `{cursor}`.
    pub cursor_back: Option<usize>,
    pub used_clipboard: bool,
}

pub struct TemplateContext<'a> {
    pub date: &'a str,
    pub time: &'a str,
    pub clipboard: Option<&'a str>,
}

/// Replaces `{date}`, `{time}`, `{clipboard}` and `{cursor}`. `{{` gives a literal `{`.
/// Unknown placeholders stay as typed, so a typo is visible rather than silently removed.
/// Replacement text is never re-scanned, so clipboard content cannot inject placeholders.
pub fn expand_template(template: &str, ctx: &TemplateContext<'_>) -> Expanded {
    let mut out = Expanded { text: String::with_capacity(template.len()), ..Default::default() };
    let mut cursor_at: Option<usize> = None;
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.text.push_str(&rest[..open]);
        rest = &rest[open..];
        if let Some(after) = rest.strip_prefix("{{") {
            out.text.push('{');
            rest = after;
            continue;
        }
        let Some(close) = rest.find('}') else {
            break;
        };
        let name = &rest[1..close];
        match name {
            "date" => out.text.push_str(ctx.date),
            "time" => out.text.push_str(ctx.time),
            "clipboard" => {
                out.used_clipboard = true;
                out.text.push_str(ctx.clipboard.unwrap_or(""));
            }
            "cursor" => {
                if cursor_at.is_none() {
                    cursor_at = Some(out.text.len());
                }
            }
            _ => out.text.push_str(&rest[..=close]),
        }
        rest = &rest[close + 1..];
    }
    out.text.push_str(rest);
    if let Some(at) = cursor_at {
        out.cursor_back = Some(out.text[at..].chars().count());
    }
    out
}

pub fn uses_clipboard(template: &str) -> bool {
    template.contains("{clipboard}")
}

/// Longest trigger Orbit will match. It also caps the rolling buffer, so the buffer can never
/// hold more than this many recently typed characters.
pub const MAX_TRIGGER_CHARS: usize = 32;

/// Tiny fixed rolling buffer that only ever answers "does the recent text end in a trigger?".
/// It holds the last few characters in memory, never persists them, and is cleared on any
/// navigation key, focus change or match. Longest trigger wins; ties break on lower id.
pub struct TriggerMatcher {
    buffer: [char; MAX_TRIGGER_CHARS],
    filled: usize,
    /// Sorted longest-first so the first suffix hit is the longest match.
    triggers: Vec<(Vec<char>, i64)>,
    last_chars_mask: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TriggerHit {
    pub snippet_id: i64,
    pub trigger_chars: usize,
}

/// Why a trigger cannot be accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TriggerProblem {
    Empty,
    /// One character would fire during ordinary typing.
    TooShort,
    TooLong,
    ContainsWhitespace,
}

pub fn validate_trigger(trigger: &str) -> Result<(), TriggerProblem> {
    let count = trigger.chars().count();
    if count == 0 {
        Err(TriggerProblem::Empty)
    } else if count < 2 {
        Err(TriggerProblem::TooShort)
    } else if count > MAX_TRIGGER_CHARS {
        Err(TriggerProblem::TooLong)
    } else if trigger.chars().any(char::is_whitespace) {
        Err(TriggerProblem::ContainsWhitespace)
    } else {
        Ok(())
    }
}

fn char_bit(c: char) -> u64 {
    1u64 << (c as u32 % 64)
}

impl Default for TriggerMatcher {
    fn default() -> Self {
        Self::new(std::iter::empty())
    }
}

impl TriggerMatcher {
    pub fn new<'a>(triggers: impl Iterator<Item = (&'a str, i64)>) -> Self {
        let mut list: Vec<(Vec<char>, i64)> = triggers
            .filter(|(t, _)| validate_trigger(t).is_ok())
            .map(|(t, id)| (t.chars().collect(), id))
            .collect();
        list.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(a.1.cmp(&b.1)));
        let last_chars_mask = list.iter().fold(0, |mask, (t, _)| mask | char_bit(t[t.len() - 1]));
        TriggerMatcher { buffer: ['\0'; MAX_TRIGGER_CHARS], filled: 0, triggers: list, last_chars_mask }
    }

    pub fn is_empty(&self) -> bool {
        self.triggers.is_empty()
    }

    pub fn clear(&mut self) {
        self.buffer = ['\0'; MAX_TRIGGER_CHARS];
        self.filled = 0;
    }

    pub fn backspace(&mut self) {
        if self.filled > 0 {
            self.filled -= 1;
            self.buffer[self.filled] = '\0';
        }
    }

    /// Feed one typed character. Constant work per keystroke, no allocation.
    #[inline]
    pub fn push(&mut self, c: char) -> Option<TriggerHit> {
        if self.filled == MAX_TRIGGER_CHARS {
            self.buffer.copy_within(1.., 0);
            self.filled -= 1;
        }
        self.buffer[self.filled] = c;
        self.filled += 1;
        if self.last_chars_mask & char_bit(c) == 0 {
            return None;
        }
        let recent = &self.buffer[..self.filled];
        for (trigger, id) in &self.triggers {
            if recent.ends_with(trigger) {
                let hit = TriggerHit { snippet_id: *id, trigger_chars: trigger.len() };
                self.clear();
                return Some(hit);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(clipboard: Option<&'a str>) -> TemplateContext<'a> {
        TemplateContext { date: "2026-11-05", time: "21:30", clipboard }
    }

    #[test]
    fn expands_known_placeholders() {
        let e = expand_template("Hi {date} at {time}", &ctx(None));
        assert_eq!(e.text, "Hi 2026-11-05 at 21:30");
        assert_eq!(e.cursor_back, None);
        assert!(!e.used_clipboard);
    }

    #[test]
    fn keeps_unknown_and_unterminated_text() {
        assert_eq!(expand_template("{nope} x", &ctx(None)).text, "{nope} x");
        assert_eq!(expand_template("a {date", &ctx(None)).text, "a {date");
        assert_eq!(expand_template("{{date}", &ctx(None)).text, "{date}");
    }

    #[test]
    fn clipboard_is_flagged_and_not_rescanned() {
        let e = expand_template("Got: {clipboard}", &ctx(Some("{date} {cursor}")));
        assert_eq!(e.text, "Got: {date} {cursor}");
        assert!(e.used_clipboard);
        assert_eq!(e.cursor_back, None);
        assert!(uses_clipboard("x{clipboard}"));
    }

    #[test]
    fn cursor_marker_counts_characters_not_bytes() {
        let e = expand_template("Dear {cursor},\nRégards", &ctx(None));
        assert_eq!(e.text, "Dear ,\nRégards");
        assert_eq!(e.cursor_back, Some(9));
    }

    fn matcher(triggers: &[(&str, i64)]) -> TriggerMatcher {
        TriggerMatcher::new(triggers.iter().copied())
    }

    fn type_text(m: &mut TriggerMatcher, text: &str) -> Vec<TriggerHit> {
        text.chars().filter_map(|c| m.push(c)).collect()
    }

    #[test]
    fn expands_exactly_once() {
        let mut m = matcher(&[(";sig", 1)]);
        let hits = type_text(&mut m, "hello ;sig more ;si");
        assert_eq!(hits, vec![TriggerHit { snippet_id: 1, trigger_chars: 4 }]);
    }

    #[test]
    fn longest_trigger_wins() {
        let mut m = matcher(&[(";a", 1), ("x;a", 2)]);
        assert_eq!(type_text(&mut m, "x;a")[0].snippet_id, 2);
        let mut m = matcher(&[(";a", 1), ("x;a", 2)]);
        assert_eq!(type_text(&mut m, "y;a")[0].snippet_id, 1);
    }

    #[test]
    fn equal_triggers_resolve_deterministically() {
        let mut m = matcher(&[(";a", 9), (";a", 3)]);
        assert_eq!(type_text(&mut m, ";a")[0].snippet_id, 3);
    }

    #[test]
    fn clear_and_backspace_affect_matching() {
        let mut m = matcher(&[(";sig", 1)]);
        type_text(&mut m, ";si");
        m.clear();
        assert!(type_text(&mut m, "g").is_empty());
        type_text(&mut m, ";six");
        m.backspace();
        assert_eq!(type_text(&mut m, "g").len(), 1);
    }

    #[test]
    fn buffer_is_bounded_and_keeps_matching_after_long_input() {
        let mut m = matcher(&[(";sig", 1)]);
        let long = "x".repeat(10_000);
        assert!(type_text(&mut m, &long).is_empty());
        assert!(m.filled <= MAX_TRIGGER_CHARS);
        assert_eq!(type_text(&mut m, ";sig").len(), 1);
    }

    #[test]
    fn invalid_triggers_are_dropped() {
        assert_eq!(validate_trigger(""), Err(TriggerProblem::Empty));
        assert_eq!(validate_trigger("a b"), Err(TriggerProblem::ContainsWhitespace));
        assert_eq!(validate_trigger(&"x".repeat(33)), Err(TriggerProblem::TooLong));
        let m = matcher(&[("", 1), ("a b", 2)]);
        assert!(m.is_empty());
    }

    #[test]
    fn unicode_triggers_work() {
        let mut m = matcher(&[("→é", 7)]);
        assert_eq!(type_text(&mut m, "a→é")[0].trigger_chars, 2);
    }
}
