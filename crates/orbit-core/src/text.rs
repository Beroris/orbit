//! Text folding for search: lowercase plus diacritic stripping, with no Unicode tables
//! beyond those in `std`. Characters outside the folded ranges keep their lowercase form,
//! so CJK and other scripts still match exactly.

/// Maps a Latin letter with a diacritic to its base letter or letters.
fn strip_diacritic(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'ď' | 'đ' | 'ð' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'ĵ' => "j",
        'ķ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'œ' => "oe",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' => "s",
        'ß' => "ss",
        'ţ' | 'ť' | 'ŧ' => "t",
        'þ' => "th",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => return None,
    })
}

/// Appends the folded form of `text` to `out`.
pub fn fold_into(text: &str, out: &mut String) {
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c.to_ascii_lowercase());
        } else {
            for lower in c.to_lowercase() {
                match strip_diacritic(lower) {
                    Some(base) => out.push_str(base),
                    None => out.push(lower),
                }
            }
        }
    }
}

pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    fold_into(text, &mut out);
    out
}

/// One bit per ASCII letter and digit, plus one bit standing in for all other bytes.
/// A query can only be a subsequence of a name if its mask is a subset of the name's mask.
#[inline]
pub fn char_mask(folded: &str) -> u64 {
    let mut mask = 0u64;
    for &b in folded.as_bytes() {
        mask |= match b {
            b'a'..=b'z' => 1u64 << (b - b'a'),
            b'0'..=b'9' => 1u64 << (26 + b - b'0'),
            b' ' | b'-' | b'_' | b'.' => 0,
            _ => 1u64 << 36,
        };
    }
    mask
}

/// Bit `i` is set if a word starts at byte `i` of the folded text (first 64 bytes only).
/// Word starts follow separators and lower-to-upper transitions in the original, so
/// "PowerShell" has starts at P and S. Folding can change byte offsets, so the original is
/// walked in step with its folded form.
pub fn word_starts(original: &str) -> u64 {
    let mut starts = 0u64;
    let mut offset = 0usize;
    let mut prev_alnum = false;
    let mut prev_lower = false;
    let mut scratch = String::new();
    for c in original.chars() {
        scratch.clear();
        let mut buf = [0u8; 4];
        fold_into(c.encode_utf8(&mut buf), &mut scratch);
        let alnum = c.is_alphanumeric();
        let begins_word = alnum && (!prev_alnum || (prev_lower && c.is_uppercase()));
        if begins_word && offset < 64 {
            starts |= 1u64 << offset;
        }
        prev_alnum = alnum;
        prev_lower = c.is_lowercase();
        offset += scratch.len();
    }
    starts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_case_and_diacritics() {
        assert_eq!(fold("Résumé"), "resume");
        assert_eq!(fold("ÅNGSTRÖM"), "angstrom");
        assert_eq!(fold("Straße"), "strasse");
        assert_eq!(fold("Œuvre"), "oeuvre");
        assert_eq!(fold("微信 Chat"), "微信 chat");
    }

    #[test]
    fn word_starts_handle_separators_and_camel_case() {
        assert_eq!(word_starts("PowerShell"), 1 | 1 << 5);
        assert_eq!(word_starts("Visual Studio Code"), 1 | 1 << 7 | 1 << 14);
        assert_eq!(word_starts("7-Zip"), 1 | 1 << 2);
        assert_eq!(word_starts("VSCode"), 1);
    }

    #[test]
    fn word_starts_track_folded_offsets() {
        // "Straße Maps" folds to "strasse maps", so the M is at byte 8 of the folded text.
        assert_eq!(word_starts("Straße Maps"), 1 | 1 << 8);
    }
}
