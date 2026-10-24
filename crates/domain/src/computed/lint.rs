//! The numeric-literal lint: numbers in the model's prose that did not
//! come from an engine.
//!
//! Prose should cite engine numbers through slots ([`super::slots`]). This
//! lint finds the other numeric literals in a markdown text so the UI can
//! mark them as unverified. It is a cheap guard, not a parser, and it errs
//! on the side of not flagging. It skips:
//!
//! - slots, code fences, inline code, TeX math (`$..$`, `$$..$$`), link
//!   targets and bare URLs;
//! - small unsigned integers up to 10, which are counts and steps far more
//!   often than results ("3 files", "step 2");
//! - years 1900–2100 and ISO dates (`2026-10-24`);
//! - ordinals (`2nd`), list markers (`1.`, `2)` at the start of a line),
//!   `2D`/`3D`, hex and binary literals (`0x1F`);
//! - version strings (`1.2.3`, `v2.0`) and numbers that are part of a word
//!   (`x1`, `H2O`, `sha256`, `#42`, hashes like `9c1e`);
//! - numbers that appear in any of the user-provided texts given to it
//!   (the user's own message or a quoted file), compared without sign and
//!   thousands separators.

use std::collections::HashSet;
use std::ops::Range;

use super::slots::parse_slots;

/// A numeric literal that is not from an engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnverifiedNumber {
    /// Byte range in the linted text, sign included.
    pub range: Range<usize>,
    /// The literal as written.
    pub text: String,
}

/// The unverified numbers of `markdown`; numbers that also appear in one of
/// `user_texts` are allowed.
#[must_use]
pub fn lint_numbers(markdown: &str, user_texts: &[&str]) -> Vec<UnverifiedNumber> {
    let quoted: HashSet<String> = user_texts
        .iter()
        .flat_map(|text| scan(text, &vec![false; text.len()]))
        .map(|t| normalize(&t.text))
        .collect();
    let mask = mask(markdown);
    scan(markdown, &mask)
        .into_iter()
        .filter(|t| !t.allowed && !quoted.contains(&normalize(&t.text)))
        .map(|t| UnverifiedNumber {
            range: t.range,
            text: t.text,
        })
        .collect()
}

/// A literal as compared with user text: no sign, no thousands separator.
fn normalize(text: &str) -> String {
    text.trim_start_matches(['-', '−', '+'])
        .chars()
        .filter(|c| *c != ',')
        .collect()
}

/// Bytes that are code, slots, link targets or URLs.
fn mask(text: &str) -> Vec<bool> {
    let mut mask = vec![false; text.len()];
    let mut set = |range: Range<usize>| {
        for b in &mut mask[range] {
            *b = true;
        }
    };
    let mut fence: Option<(char, usize)> = None;
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        let range = line_start..line_start + line.len();
        line_start = range.end;
        let trimmed = line.trim_start_matches(' ');
        let marker = trimmed
            .chars()
            .next()
            .filter(|c| (*c == '`' || *c == '~') && line.len() - trimmed.len() <= 3);
        let run = marker.map_or(0, |c| trimmed.chars().take_while(|x| *x == c).count());
        match (fence, marker) {
            (None, Some(c)) if run >= 3 => {
                fence = Some((c, run));
                set(range);
            }
            (Some((c, len)), Some(m)) if m == c && run >= len => {
                fence = None;
                set(range);
            }
            (Some(_), _) => set(range),
            (None, _) => {}
        }
    }
    mask_inline_code(text, &mut mask);
    mask_math(text, &mut mask);
    for slot in parse_slots(text) {
        mask[slot.range].fill(true);
    }
    for (start, _) in text.match_indices("](") {
        let end = text[start..]
            .find(')')
            .map_or(text.len(), |e| start + e + 1);
        mask[start..end].fill(true);
    }
    for scheme in ["http://", "https://"] {
        for (start, _) in text.match_indices(scheme) {
            let end = text[start..]
                .find(char::is_whitespace)
                .map_or(text.len(), |e| start + e);
            mask[start..end].fill(true);
        }
    }
    mask
}

/// Masks `` `code` `` spans: a run of backticks up to the next run of the
/// same length.
fn mask_inline_code(text: &str, mask: &mut [bool]) {
    let bytes = text.as_bytes();
    let run_at = |i: usize| bytes[i..].iter().take_while(|b| **b == b'`').count();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'`' || mask[i] {
            i += 1;
            continue;
        }
        let open = run_at(i);
        let mut j = i + open;
        let mut close = None;
        while j < bytes.len() {
            if bytes[j] == b'`' {
                let len = run_at(j);
                if len == open {
                    close = Some(j + len);
                    break;
                }
                j += len;
            } else {
                j += 1;
            }
        }
        match close {
            Some(end) => {
                mask[i..end].fill(true);
                i = end;
            }
            None => i += open,
        }
    }
}

/// Masks TeX math: `$$..$$` (across lines) and then `$..$` within a line,
/// with pandoc's rule that the opening `$` is followed by a non-space and
/// the closing one preceded by a non-space and not followed by a digit, so
/// prices (`$5 and $10`) stay prose.
fn mask_math(text: &str, mask: &mut [bool]) {
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(open) = text[i..].find("$$").map(|o| o + i) {
        if mask[open] {
            i = open + 2;
            continue;
        }
        let Some(close) = text[open + 2..].find("$$").map(|c| c + open + 2) else {
            break;
        };
        mask[open..close + 2].fill(true);
        i = close + 2;
    }
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'$' || mask[i] || bytes.get(i + 1).is_none_or(u8::is_ascii_whitespace) {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        let mut close = None;
        while j < bytes.len() && bytes[j] != b'\n' {
            if bytes[j] == b'$'
                && !mask[j]
                && !bytes[j - 1].is_ascii_whitespace()
                && !bytes.get(j + 1).is_some_and(u8::is_ascii_digit)
            {
                close = Some(j);
                break;
            }
            j += 1;
        }
        match close {
            Some(end) => {
                mask[i..=end].fill(true);
                i = end + 1;
            }
            None => i += 1,
        }
    }
}

/// A literal found by [`scan`], with whether a whitelist rule allows it.
struct Token {
    range: Range<usize>,
    text: String,
    allowed: bool,
}

/// Every numeric literal outside `mask`.
fn scan(text: &str, mask: &[bool]) -> Vec<Token> {
    let bytes = text.as_bytes();
    let digit = |i: usize| bytes.get(i).is_some_and(u8::is_ascii_digit);
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // Only ASCII bytes start a number, so `i` is a char boundary below.
        let starts = !mask[i] && (digit(i) || (bytes[i] == b'.' && digit(i + 1)));
        if !starts {
            i += 1;
            continue;
        }
        let prev = text[..i].chars().next_back();
        if prev.is_some_and(|c| c.is_ascii_digit()) {
            i += 1;
            continue;
        }
        // The number itself: digits, `,ddd` groups, `.d` parts, exponent.
        let mut j = i;
        let mut dots = 0;
        let mut commas = false;
        loop {
            while digit(j) {
                j += 1;
            }
            let group = bytes.get(j) == Some(&b',')
                && (1..=3).all(|k| digit(j + k))
                && !digit(j + 4)
                && j > i;
            if group {
                commas = true;
                j += 4;
            } else if bytes.get(j) == Some(&b'.') && digit(j + 1) {
                dots += 1;
                j += 1;
            } else {
                break;
            }
        }
        if matches!(bytes.get(j), Some(b'e' | b'E')) {
            let sign = usize::from(matches!(bytes.get(j + 1), Some(b'+' | b'-')));
            if digit(j + 1 + sign) {
                j += 1 + sign;
                while digit(j) {
                    j += 1;
                }
            }
        }
        let number = &text[i..j];
        // A word glued after the number (`ms`, `nd`, `c1e`).
        let word_len = text[j..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(text.len() - j);
        let word = &text[j..j + word_len];
        let integer = dots == 0 && !commas && !number.starts_with('.');

        let mut end = j;
        let mut allowed = false;
        if prev.is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '#' | '@')) || dots >= 2 {
            allowed = true;
            end = j + word_len;
        } else if !word.is_empty() {
            allowed = word.chars().any(|c| c.is_ascii_digit())
                || (integer && is_ordinal(number, word))
                || (number.len() == 1 && word.eq_ignore_ascii_case("d"))
                || (number == "0" && word.starts_with(['x', 'X', 'b', 'B', 'o', 'O']));
            if allowed {
                end = j + word_len;
            }
        } else if integer && number.len() == 4 && is_iso_date_tail(&text[j..]) {
            allowed = true;
            end = j + 6;
        } else if integer
            && ((number.len() == 4 && number.parse::<u32>().is_ok_and(is_year))
                || is_list_marker(text, i, j))
        {
            allowed = true;
        }

        // A sign right before the number belongs to it, unless it joins two
        // words (`a-1`).
        let mut start = i;
        if let Some(sign @ ('-' | '−' | '+')) = prev {
            let before = text[..i - sign.len_utf8()].chars().next_back();
            if !before.is_some_and(char::is_alphanumeric) {
                start = i - sign.len_utf8();
            }
        }
        // Small counts: an unsigned integer up to 10.
        if start == i && integer && number.parse::<u32>().is_ok_and(|n| n <= 10) {
            allowed = true;
        }
        tokens.push(Token {
            range: start..j,
            text: text[start..j].to_owned(),
            allowed,
        });
        i = end.max(i + 1);
    }
    tokens
}

fn is_year(n: u32) -> bool {
    (1900..=2100).contains(&n)
}

fn is_ordinal(number: &str, suffix: &str) -> bool {
    let suffix = suffix.to_ascii_lowercase();
    let last = number.as_bytes().last().copied();
    let teen = number.len() >= 2 && number.as_bytes()[number.len() - 2] == b'1';
    match suffix.as_str() {
        "st" => last == Some(b'1') && !teen,
        "nd" => last == Some(b'2') && !teen,
        "rd" => last == Some(b'3') && !teen,
        "th" => true,
        _ => false,
    }
}

/// Whether `rest` starts with `-MM-DD` (the tail of an ISO date).
fn is_iso_date_tail(rest: &str) -> bool {
    let b = rest.as_bytes();
    b.len() >= 6
        && b[0] == b'-'
        && b[1].is_ascii_digit()
        && b[2].is_ascii_digit()
        && b[3] == b'-'
        && b[4].is_ascii_digit()
        && b[5].is_ascii_digit()
        && !b.get(6).is_some_and(u8::is_ascii_digit)
}

/// Whether the integer at `start..end` is an ordered-list marker: only
/// spaces, `>` or `#` before it on its line, then `.` or `)` and a space.
fn is_list_marker(text: &str, start: usize, end: usize) -> bool {
    let line_start = text[..start].rfind('\n').map_or(0, |n| n + 1);
    let lead_ok = text[line_start..start]
        .chars()
        .all(|c| c == ' ' || c == '\t' || c == '>' || c == '#');
    let mut after = text[end..].chars();
    let punct = matches!(after.next(), Some('.' | ')'));
    let space = after.next().is_none_or(char::is_whitespace);
    lead_ok && punct && space
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flagged(text: &str) -> Vec<String> {
        lint_numbers(text, &[])
            .into_iter()
            .map(|n| n.text)
            .collect()
    }

    #[test]
    fn flags_plain_numbers_with_sign_units_and_separators() {
        assert_eq!(
            flagged("The mean was 3.4 and the effect -1.48 (p = .03), over 1,234 rows."),
            ["3.4", "-1.48", ".03", "1,234"]
        );
        assert_eq!(
            flagged("took 12ms, ratio 45% and −2e-3"),
            ["12", "45", "−2e-3"]
        );
    }

    #[test]
    fn ranges_point_at_the_literal() {
        let text = "x is 42.";
        let found = lint_numbers(text, &[]);
        assert_eq!(found.len(), 1);
        assert_eq!(&text[found[0].range.clone()], "42");
    }

    #[test]
    fn whitelists_years_dates_ordinals_and_list_markers() {
        let text = "In 2024, on 2026-10-24, the 2nd and 11th runs, a 3D plot.\n\
                    1. first\n  2) second\n## 3. Heading\n> 4. quoted";
        assert!(flagged(text).is_empty(), "{:?}", flagged(text));
        assert_eq!(flagged("in 1850 and 21st but 11st"), ["1850", "11"]);
        assert_eq!(flagged("a 1.5 mark"), ["1.5"]);
    }

    #[test]
    fn whitelists_versions_identifiers_and_hashes() {
        let text = "crest 0.14.0, v2.0, x1, H2O, sha256, #42, 9c1e3a, 0x1F, snake_2 and @3";
        assert!(flagged(text).is_empty(), "{:?}", flagged(text));
        assert_eq!(flagged("range a-11 to 15"), ["11", "15"]);
    }

    #[test]
    fn skips_code_slots_links_and_urls() {
        let text = "Use `n = 30` and ⟦c1.t⟧ or [[calc:c1.p]].\n\
                    ```python\nx = 12.5\n```\n\
                    See [docs](https://e.com/v/3) and https://e.com/a/77 ok\n\
                    ``a ` 9 ``";
        assert!(flagged(text).is_empty(), "{:?}", flagged(text));
        // An unclosed backtick is not code.
        assert_eq!(flagged("a ` 19"), ["19"]);
        // An unclosed fence masks to the end.
        assert!(flagged("~~~\n5\n").is_empty());
    }

    #[test]
    fn numbers_from_user_text_are_allowed() {
        let text = "You said 1,500 users and 0.25; I think 17.";
        assert_eq!(
            lint_numbers(text, &["we have 1500 users at a 0.25 rate"])
                .into_iter()
                .map(|n| n.text)
                .collect::<Vec<_>>(),
            ["17"]
        );
    }

    #[test]
    fn small_counts_and_math_are_not_flagged() {
        assert!(flagged("3 files in 2 steps, 10 runs").is_empty());
        assert_eq!(flagged("a drop of -3 and 11 runs"), ["-3", "11"]);
        let text = "where $x = 4.5$ and $$\\sigma = 12.3$$ but $5 and $20 stay";
        assert_eq!(flagged(text), ["20"]);
        assert_eq!(flagged("cost $12 and $13"), ["12", "13"]);
    }
}
