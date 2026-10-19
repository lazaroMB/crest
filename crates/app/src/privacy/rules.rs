//! The built-in detectors: secrets (always on) and personal data (one
//! switch per category), each a regex plus an optional check that weeds
//! out look-alikes (Luhn for cards, mod-97 for IBANs, the control letter
//! of a DNI).

use regex::Regex;

/// What a rule belongs to, for the `app.opt.privacy.pii` switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Category {
    /// Keys, tokens and passwords; cannot be switched off.
    Secret,
    Email,
    Card,
    Iban,
    NationalId,
    Phone,
    /// A pattern from `app.opt.privacy.patterns`.
    Custom,
}

/// One detector.
#[derive(Debug, Clone)]
pub(super) struct Rule {
    /// Tag of the placeholder: `[REDACTED:<kind>]`.
    pub(super) kind: String,
    pub(super) category: Category,
    pub(super) regex: Regex,
    /// Capture group that is masked; 0 masks the whole match.
    pub(super) group: usize,
    /// Extra check of the masked text; `false` keeps it.
    pub(super) check: Option<fn(&str) -> bool>,
}

impl Rule {
    fn new(kind: &str, category: Category, pattern: &str) -> Self {
        Self {
            kind: kind.to_owned(),
            category,
            // The built-in patterns are constants covered by the tests.
            regex: Regex::new(pattern).unwrap_or_else(|err| panic!("rule {kind}: {err}")),
            group: 0,
            check: None,
        }
    }

    fn group(mut self, group: usize) -> Self {
        self.group = group;
        self
    }

    fn check(mut self, check: fn(&str) -> bool) -> Self {
        self.check = Some(check);
        self
    }

    /// A pattern of the user: masks group 1 when there is one.
    pub(super) fn custom(name: &str, regex: Regex) -> Self {
        let group = usize::from(regex.captures_len() > 1);
        Self {
            kind: name.to_owned(),
            category: Category::Custom,
            regex,
            group,
            check: None,
        }
    }
}

/// Names that hint at a secret on the left of `=` or `:`.
const SECRET_KEYS: &str = r"password|passwd|pwd|passphrase|secret|token|api[_-]?key|apikey|access[_-]?key|private[_-]?key|client[_-]?secret|credentials?";

/// The built-in rules, most specific first: on overlapping matches of the
/// same length the earlier rule names the placeholder.
pub(super) fn builtin() -> Vec<Rule> {
    use Category::{Card, Email, Iban, NationalId, Phone, Secret};
    vec![
        Rule::new(
            "private_key",
            Secret,
            r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----(?:.*?-----END [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----|.*)",
        ),
        Rule::new("aws_key", Secret, r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b"),
        Rule::new(
            "github_token",
            Secret,
            r"\b(?:gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{22,})",
        ),
        Rule::new("anthropic_key", Secret, r"\bsk-ant-[A-Za-z0-9_-]{20,}"),
        Rule::new(
            "openai_key",
            Secret,
            r"\bsk-(?:proj-|svcacct-|admin-)?[A-Za-z0-9_-]{20,}",
        ),
        Rule::new("slack_token", Secret, r"\bxox[abprs]-[A-Za-z0-9-]{10,}"),
        Rule::new(
            "stripe_key",
            Secret,
            r"\b(?:sk|rk|pk)_(?:live|test)_[A-Za-z0-9]{16,}",
        ),
        Rule::new("google_key", Secret, r"\bAIza[0-9A-Za-z_-]{35}"),
        Rule::new(
            "jwt",
            Secret,
            r"\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
        ),
        Rule::new(
            "token",
            Secret,
            r"(?i)\b(?:bearer|basic)\s+([A-Za-z0-9._~+/-]{16,}=*)",
        )
        .group(1)
        .check(mixed),
        Rule::new(
            "password",
            Secret,
            r"\b[a-zA-Z][a-zA-Z0-9+.-]*://[^\s:/@]+:([^\s:/@]+)@",
        )
        .group(1)
        .check(not_placeholder),
        Rule::new(
            "secret",
            Secret,
            &format!(
                r#"(?i)[\w.-]*(?:{SECRET_KEYS})[\w.-]*["']?\s*(?::|=|=>)\s*["']?([A-Za-z0-9_\-+/=~!@#%^&*.:]{{8,}})"#
            ),
        )
        .group(1)
        .check(secret_value),
        Rule::new(
            "email",
            Email,
            r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}\b",
        )
        .check(real_email),
        Rule::new("iban", Iban, r"\b[A-Z]{2}\d{2}(?: ?[A-Z0-9]{4}){2,7}(?: ?[A-Z0-9]{1,4})?\b")
            .check(iban),
        Rule::new("card", Card, r"\b[2-6]\d{3}(?:[ -]?\d){9,15}\b").check(card),
        Rule::new("national_id", NationalId, r"\b(?:\d{8}|[XYZ]\d{7})-?[A-Z]\b").check(dni),
        Rule::new("national_id", NationalId, r"\b\d{3}-\d{2}-\d{4}\b").check(ssn),
        Rule::new(
            "phone",
            Phone,
            r"(?:^|[^\w+])(\+\d{1,3}[ .-]?(?:\(\d{1,4}\)[ .-]?)?\d{2,4}(?:[ .-]?\d{2,4}){2,4})\b",
        )
        .group(1)
        .check(phone),
    ]
}

/// Letters and digits, as tokens are; plain words are not.
fn mixed(value: &str) -> bool {
    value.bytes().any(|b| b.is_ascii_digit()) && value.bytes().any(|b| b.is_ascii_alphabetic())
}

/// Not a template, a variable or a filler value.
fn not_placeholder(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let filler = [
        "changeme",
        "example",
        "placeholder",
        "your_",
        "your-",
        "xxxx",
        "****",
        "redacted",
        "password",
        "secret",
        "dummy",
    ];
    !value.starts_with(['$', '<', '{', '%', '*'])
        && !value.ends_with(['>', '}'])
        && !filler.iter().any(|f| lower.contains(f))
}

/// The value of `password = …` and the like: a literal, not an
/// identifier, a field access, an environment variable name or a number.
fn secret_value(value: &str) -> bool {
    let value = value.trim_end_matches([':', '.']);
    let env_name = value
        .bytes()
        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
    let number = value
        .bytes()
        .all(|b| b.is_ascii_digit() || b == b'_' || b == b'.');
    let has_digit = value.bytes().any(|b| b.is_ascii_digit());
    let has_symbol = value.bytes().any(|b| b"+/=~!@#%^&*".contains(&b));
    value.len() >= 8
        && !env_name
        && !number
        && (has_digit || has_symbol)
        && !value.contains("::")
        && not_placeholder(value)
}

/// Not a test or service address (`git@github.com`, `x@example.com`).
fn real_email(email: &str) -> bool {
    let lower = email.to_ascii_lowercase();
    let (local, domain) = lower.split_once('@').unwrap_or_default();
    let reserved = ["example.com", "example.org", "example.net", "localhost"];
    !(local == "git"
        || local.contains("noreply")
        || local.contains("no-reply")
        || domain.ends_with("noreply.github.com")
        || domain.ends_with(".test")
        || domain.ends_with(".invalid")
        || domain.ends_with(".example")
        || reserved
            .iter()
            .any(|r| domain == *r || domain.ends_with(&format!(".{r}"))))
}

fn digits(text: &str) -> Vec<u32> {
    text.chars().filter_map(|c| c.to_digit(10)).collect()
}

/// 13 to 19 digits passing the Luhn check, not all the same.
fn card(text: &str) -> bool {
    let digits = digits(text);
    if !(13..=19).contains(&digits.len()) || digits.iter().all(|d| *d == digits[0]) {
        return false;
    }
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| match (i % 2 == 1, d * 2) {
            (true, doubled) if doubled > 9 => doubled - 9,
            (true, doubled) => doubled,
            (false, _) => d,
        })
        .sum();
    sum % 10 == 0
}

/// 15 to 34 characters passing the ISO 13616 mod-97 check.
fn iban(text: &str) -> bool {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if !(15..=34).contains(&compact.len()) {
        return false;
    }
    let (head, tail) = compact.split_at(4);
    let mut remainder = 0u32;
    for c in tail.chars().chain(head.chars()) {
        let value = match c.to_digit(36) {
            Some(v) => v,
            None => return false,
        };
        let width = if value > 9 { 100 } else { 10 };
        remainder = (remainder * width + value) % 97;
    }
    remainder == 1
}

/// A Spanish DNI or NIE whose control letter matches.
fn dni(text: &str) -> bool {
    const LETTERS: &[u8; 23] = b"TRWAGMYFPDXBNJZSQVHLCKE";
    let compact: String = text.chars().filter(|c| *c != '-').collect();
    let (body, letter) = compact.split_at(compact.len() - 1);
    let number: String = match body.chars().next() {
        Some('X') => format!("0{}", &body[1..]),
        Some('Y') => format!("1{}", &body[1..]),
        Some('Z') => format!("2{}", &body[1..]),
        _ => body.to_owned(),
    };
    let Ok(n) = number.parse::<usize>() else {
        return false;
    };
    letter.as_bytes().first() == Some(&LETTERS[n % 23])
}

/// A US social security number with valid area, group and serial.
fn ssn(text: &str) -> bool {
    let mut parts = text.split('-');
    let (Some(area), Some(group), Some(serial)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    area != "000" && area != "666" && !area.starts_with('9') && group != "00" && serial != "0000"
}

/// 8 to 15 digits, as E.164 numbers have.
fn phone(text: &str) -> bool {
    (8..=15).contains(&digits(text).len())
}
