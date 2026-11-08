//! Vim-style keys for the launcher. No OS calls: this turns keys into commands, and the platform
//! layer carries them out.

/// How the launcher treats the keyboard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyboardMode {
    /// Typing always searches.
    #[default]
    Standard,
    /// Typing always searches, and holding Alt makes H J K L and a few others navigate.
    VimKeys,
    /// Opens ready to navigate. Press `i` to type, and Esc to navigate again.
    Vim,
}

/// Which of Vim's two modes the launcher is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VimMode {
    Normal,
    Insert,
}

/// Where the caret goes on the way to typing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsertAt {
    Caret,
    AfterCaret,
    Start,
    End,
}

/// What a delete covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delete {
    Chars(u32),
    Word,
    ToEnd,
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Moves the selected row by this many.
    Move(i32),
    /// Half a screen of rows, in this direction.
    HalfPage(i32),
    /// A screen of rows, in this direction.
    Page(i32),
    First,
    Last,
    /// A row counted from 0.
    GoTo(u32),
    Back,
    Open,
    Close,
    Insert(InsertAt),
    /// Starts a new search with this already typed, such as `:` for commands only.
    InsertText(char),
    Delete { what: Delete, then_insert: bool },
    CaretStart,
    CaretEnd,
    WordForward,
    WordBack,
    Undo,
    PasteBefore,
    PasteAfter,
}

/// The keys typed so far of a command that isn't finished, such as the first `g` of `gg`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VimKeys {
    pending: Option<char>,
    count: u32,
}

impl VimKeys {
    /// Forgets a half-typed command.
    pub fn reset(&mut self) {
        *self = VimKeys::default();
    }

    /// A character typed in normal mode. `None` means nothing to do yet, or nothing at all.
    pub fn on_char(&mut self, c: char) -> Option<Command> {
        if let Some(pending) = self.pending.take() {
            let count = std::mem::take(&mut self.count);
            return match (pending, c) {
                ('g', 'g') => Some(if count > 0 { Command::GoTo(count - 1) } else { Command::First }),
                ('d', 'd') => Some(Command::Delete { what: Delete::All, then_insert: false }),
                ('c', 'c') => Some(Command::Delete { what: Delete::All, then_insert: true }),
                ('d', 'w') => Some(Command::Delete { what: Delete::Word, then_insert: false }),
                ('c', 'w') => Some(Command::Delete { what: Delete::Word, then_insert: true }),
                ('d', '$') => Some(Command::Delete { what: Delete::ToEnd, then_insert: false }),
                ('c', '$') => Some(Command::Delete { what: Delete::ToEnd, then_insert: true }),
                _ => None,
            };
        }
        if c.is_ascii_digit() && (c != '0' || self.count > 0) {
            let digit = c as u32 - '0' as u32;
            self.count = (self.count.saturating_mul(10).saturating_add(digit)).min(9999);
            return None;
        }
        if matches!(c, 'g' | 'd' | 'c') {
            self.pending = Some(c);
            return None;
        }
        let count = std::mem::take(&mut self.count);
        let times = count.max(1);
        let steps = times as i32;
        Some(match c {
            'j' | 'n' => Command::Move(steps),
            'k' | 'N' => Command::Move(-steps),
            'G' => {
                if count > 0 {
                    Command::GoTo(count - 1)
                } else {
                    Command::Last
                }
            }
            'h' => Command::Back,
            'l' => Command::Open,
            'q' => Command::Close,
            'i' => Command::Insert(InsertAt::Caret),
            'a' => Command::Insert(InsertAt::AfterCaret),
            'I' => Command::Insert(InsertAt::Start),
            'A' => Command::Insert(InsertAt::End),
            '/' => Command::Delete { what: Delete::All, then_insert: true },
            ':' => Command::InsertText(':'),
            '?' => Command::InsertText('?'),
            'x' => Command::Delete { what: Delete::Chars(times), then_insert: false },
            's' => Command::Delete { what: Delete::Chars(times), then_insert: true },
            'S' => Command::Delete { what: Delete::All, then_insert: true },
            'D' => Command::Delete { what: Delete::ToEnd, then_insert: false },
            'C' => Command::Delete { what: Delete::ToEnd, then_insert: true },
            '0' | '^' => Command::CaretStart,
            '$' => Command::CaretEnd,
            'w' => Command::WordForward,
            'b' => Command::WordBack,
            'u' => Command::Undo,
            'p' => Command::PasteAfter,
            'P' => Command::PasteBefore,
            _ => return None,
        })
    }
}

/// Alt plus a letter, in either Vim setting.
pub fn alt_command(letter: char, shift: bool) -> Option<Command> {
    Some(match (letter.to_ascii_uppercase(), shift) {
        ('H', false) => Command::Back,
        ('J', false) => Command::Move(1),
        ('K', false) => Command::Move(-1),
        ('L', false) => Command::Open,
        ('D', false) => Command::HalfPage(1),
        ('U', false) => Command::HalfPage(-1),
        ('G', false) => Command::First,
        ('G', true) => Command::Last,
        _ => return None,
    })
}

/// Ctrl plus a letter, in normal mode. Ctrl+K isn't here because it opens the action menu.
pub fn ctrl_command(letter: char) -> Option<Command> {
    Some(match letter.to_ascii_uppercase() {
        'D' => Command::HalfPage(1),
        'U' => Command::HalfPage(-1),
        'F' => Command::Page(1),
        'B' => Command::Page(-1),
        'N' => Command::Move(1),
        'P' => Command::Move(-1),
        _ => return None,
    })
}

#[derive(PartialEq)]
enum CharClass {
    Space,
    Word,
    Other,
}

fn class_of(unit: u16) -> CharClass {
    match char::from_u32(unit as u32) {
        Some(c) if c.is_whitespace() => CharClass::Space,
        Some(c) if c.is_alphanumeric() || c == '_' => CharClass::Word,
        // Half of a surrogate pair is part of a word as far as moving goes.
        None => CharClass::Word,
        _ => CharClass::Other,
    }
}

/// Where `w` lands: the start of the next word, or the end of the text.
pub fn word_forward(text: &[u16], at: usize) -> usize {
    let mut i = at.min(text.len());
    if i < text.len() {
        let start = class_of(text[i]);
        if start != CharClass::Space {
            while i < text.len() && class_of(text[i]) == start {
                i += 1;
            }
        }
    }
    while i < text.len() && class_of(text[i]) == CharClass::Space {
        i += 1;
    }
    i
}

/// Where `b` lands: the start of this word, or of the one before.
pub fn word_back(text: &[u16], at: usize) -> usize {
    let mut i = at.min(text.len());
    while i > 0 && class_of(text[i - 1]) == CharClass::Space {
        i -= 1;
    }
    if i > 0 {
        let word = class_of(text[i - 1]);
        while i > 0 && class_of(text[i - 1]) == word {
            i -= 1;
        }
    }
    i
}

fn is_low_surrogate(unit: u16) -> bool {
    (0xDC00..0xE000).contains(&unit)
}

/// One character to the left, keeping surrogate pairs whole.
pub fn char_before(text: &[u16], at: usize) -> usize {
    let mut i = at.min(text.len()).saturating_sub(1);
    if i > 0 && text.get(i).copied().is_some_and(is_low_surrogate) {
        i -= 1;
    }
    i
}

/// One character to the right, keeping surrogate pairs whole.
pub fn char_after(text: &[u16], at: usize) -> usize {
    let mut i = (at + 1).min(text.len());
    if text.get(i).copied().is_some_and(is_low_surrogate) {
        i += 1;
    }
    i
}

/// In normal mode the caret sits on a character, so it can't go past the last one.
pub fn normal_caret(text: &[u16], at: usize) -> usize {
    if at >= text.len() { char_before(text, text.len()) } else { at }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(keys: &str) -> Vec<Command> {
        let mut vim = VimKeys::default();
        keys.chars().filter_map(|c| vim.on_char(c)).collect()
    }

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn the_four_directions() {
        assert_eq!(run("jklh"), [Command::Move(1), Command::Move(-1), Command::Open, Command::Back]);
    }

    #[test]
    fn counts_repeat_and_choose_a_row() {
        assert_eq!(run("3j"), [Command::Move(3)]);
        assert_eq!(run("12k"), [Command::Move(-12)]);
        assert_eq!(run("10j"), [Command::Move(10)]);
        assert_eq!(run("5G"), [Command::GoTo(4)]);
        assert_eq!(run("5gg"), [Command::GoTo(4)]);
        assert_eq!(run("3x"), [Command::Delete { what: Delete::Chars(3), then_insert: false }]);
    }

    #[test]
    fn a_count_applies_once() {
        assert_eq!(run("3jj"), [Command::Move(3), Command::Move(1)]);
    }

    #[test]
    fn zero_alone_is_the_start_of_the_line() {
        assert_eq!(run("0"), [Command::CaretStart]);
    }

    #[test]
    fn two_key_commands_wait_for_the_second_key() {
        let mut vim = VimKeys::default();
        assert_eq!(vim.on_char('g'), None);
        assert_eq!(vim.on_char('g'), Some(Command::First));
        assert_eq!(run("G"), [Command::Last]);
        assert_eq!(run("dd"), [Command::Delete { what: Delete::All, then_insert: false }]);
        assert_eq!(run("cc"), [Command::Delete { what: Delete::All, then_insert: true }]);
        assert_eq!(run("dw"), [Command::Delete { what: Delete::Word, then_insert: false }]);
        assert_eq!(run("c$"), [Command::Delete { what: Delete::ToEnd, then_insert: true }]);
    }

    #[test]
    fn a_wrong_second_key_cancels_and_does_nothing() {
        assert_eq!(run("gz"), []);
        assert_eq!(run("gzj"), [Command::Move(1)]);
        assert_eq!(run("3gzj"), [Command::Move(1)], "the count went with it");
    }

    #[test]
    fn reset_forgets_a_half_typed_command() {
        let mut vim = VimKeys::default();
        vim.on_char('3');
        vim.on_char('d');
        vim.reset();
        assert_eq!(vim.on_char('j'), Some(Command::Move(1)));
    }

    #[test]
    fn keys_with_no_meaning_do_nothing() {
        assert_eq!(run("zZ!"), []);
    }

    #[test]
    fn entering_insert_mode() {
        assert_eq!(run("iaIA"), [
            Command::Insert(InsertAt::Caret),
            Command::Insert(InsertAt::AfterCaret),
            Command::Insert(InsertAt::Start),
            Command::Insert(InsertAt::End),
        ]);
        assert_eq!(run("/"), [Command::Delete { what: Delete::All, then_insert: true }]);
        assert_eq!(run(":"), [Command::InsertText(':')]);
    }

    #[test]
    fn alt_keys_navigate_and_leave_others_alone() {
        assert_eq!(alt_command('J', false), Some(Command::Move(1)));
        assert_eq!(alt_command('k', false), Some(Command::Move(-1)));
        assert_eq!(alt_command('H', false), Some(Command::Back));
        assert_eq!(alt_command('L', false), Some(Command::Open));
        assert_eq!(alt_command('G', true), Some(Command::Last));
        assert_eq!(alt_command('J', true), None);
        assert_eq!(alt_command('X', false), None);
    }

    #[test]
    fn ctrl_k_stays_the_action_menu() {
        assert_eq!(ctrl_command('K'), None);
        assert_eq!(ctrl_command('D'), Some(Command::HalfPage(1)));
    }

    #[test]
    fn word_motions() {
        let text = units("foo.bar  baz");
        assert_eq!(word_forward(&text, 0), 3);
        assert_eq!(word_forward(&text, 3), 4);
        assert_eq!(word_forward(&text, 4), 9);
        assert_eq!(word_forward(&text, 9), 12);
        assert_eq!(word_forward(&text, 12), 12);
        assert_eq!(word_back(&text, 12), 9);
        assert_eq!(word_back(&text, 9), 4);
        assert_eq!(word_back(&text, 4), 3);
        assert_eq!(word_back(&text, 3), 0);
        assert_eq!(word_back(&text, 0), 0);
    }

    #[test]
    fn the_caret_keeps_surrogate_pairs_whole() {
        let text = units("a\u{1F600}b");
        assert_eq!(text.len(), 4);
        assert_eq!(char_after(&text, 1), 3);
        assert_eq!(char_before(&text, 3), 1);
        assert_eq!(char_before(&text, 1), 0);
    }

    #[test]
    fn the_normal_mode_caret_stays_on_a_character() {
        let text = units("abc");
        assert_eq!(normal_caret(&text, 3), 2);
        assert_eq!(normal_caret(&text, 1), 1);
        assert_eq!(normal_caret(&[], 0), 0);
    }

    #[test]
    fn the_setting_is_saved_by_name() {
        assert_eq!(serde_json::to_string(&KeyboardMode::VimKeys).unwrap(), "\"vim_keys\"");
        assert_eq!(serde_json::from_str::<KeyboardMode>("\"vim\"").unwrap(), KeyboardMode::Vim);
    }
}
