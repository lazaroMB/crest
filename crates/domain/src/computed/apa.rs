//! APA 7 formatting of statistics.
//!
//! The rules: exact p to three decimals below .1 and two above, `p < .001`
//! below that and `p > .99` at the top; no leading zero for statistics that
//! cannot exceed 1 (p, r, R², proportions); degrees of freedom without a
//! thousands separator and as an integer when they are whole; intervals as
//! `95% CI [lo, hi]`; negative numbers with a true minus sign (U+2212).
//!
//! Statistic symbols are italic in APA. An [`ApaLine`] is a list of
//! [`ApaToken`]s so the UI can style the italic ones; [`ApaLine::plain`]
//! and [`ApaLine::markdown`] flatten it for copying. Greek symbols (χ², τ)
//! are not italic.

use serde::{Deserialize, Serialize};

use super::Interval;

/// The minus sign APA uses for negative numbers.
pub const MINUS: char = '\u{2212}';

/// Replaces ASCII hyphen-minus with [`MINUS`].
#[must_use]
pub fn with_minus(text: &str) -> String {
    text.replace('-', &MINUS.to_string())
}

/// `x` with `decimals` decimals and a true minus sign; `-0.00` becomes
/// `0.00`.
#[must_use]
pub fn number(x: f64, decimals: u32) -> String {
    let text = format!("{:.*}", decimals as usize, x);
    match text.strip_prefix('-') {
        Some(rest) if rest.chars().all(|c| c == '0' || c == '.') => rest.to_owned(),
        Some(rest) => format!("{MINUS}{rest}"),
        None => text,
    }
}

/// Like [`number`], without the leading zero (`.45`, `−.07`), for
/// statistics bounded by 1.
#[must_use]
pub fn bounded(x: f64, decimals: u32) -> String {
    let text = number(x, decimals);
    if let Some(rest) = text.strip_prefix(MINUS) {
        let rest = rest.strip_prefix('0').unwrap_or(rest);
        format!("{MINUS}{rest}")
    } else {
        text.strip_prefix('0')
            .filter(|rest| rest.starts_with('.'))
            .map_or(text.clone(), str::to_owned)
    }
}

/// `x` rounded to `sig` significant digits (at least one).
#[must_use]
pub fn significant(x: f64, sig: u32) -> String {
    if x == 0.0 || !x.is_finite() {
        return number(x, sig.saturating_sub(1));
    }
    let sig = i32::try_from(sig.max(1)).unwrap_or(i32::MAX);
    // Exponent of the leading digit; `log10` of a finite non-zero number is
    // small enough for i32.
    #[allow(clippy::cast_possible_truncation)]
    let exponent = x.abs().log10().floor() as i32;
    let decimals = sig - 1 - exponent;
    if decimals >= 0 {
        number(x, decimals.unsigned_abs())
    } else {
        let scale = 10f64.powi(-decimals);
        number((x / scale).round() * scale, 0)
    }
}

/// An APA p-value without the `p`: `.043`, `.38`, `< .001`, `> .99`.
#[must_use]
pub fn p_value_text(p: f64) -> String {
    let (relation, value) = p_value_parts(p);
    if relation == "=" {
        value
    } else {
        format!("{relation} {value}")
    }
}

/// The relation (`=`, `<` or `>`) and number of an APA p-value.
#[must_use]
pub fn p_value_parts(p: f64) -> (&'static str, String) {
    if p < 0.001 {
        return ("<", ".001".into());
    }
    let decimals = if p < 0.1 { 3 } else { 2 };
    let text = bounded(p, decimals);
    if text.starts_with('1') {
        (">", ".99".into())
    } else {
        ("=", text)
    }
}

/// Degrees of freedom: `18` when whole, else two decimals (`18.33`); never
/// a thousands separator.
#[must_use]
pub fn df_text(df: f64) -> String {
    if (df - df.round()).abs() < 1e-9 {
        number(df, 0)
    } else {
        number(df, 2)
    }
}

/// `95% CI [−11.28, −3.21]`. `bounded` drops the leading zeros.
#[must_use]
pub fn ci_text(level: f64, lo: f64, hi: f64, decimals: u32, bounded_stat: bool) -> String {
    let fmt = |x| {
        if bounded_stat {
            bounded(x, decimals)
        } else {
            number(x, decimals)
        }
    };
    let percent = number(level * 100.0, 1);
    let percent = percent.strip_suffix(".0").unwrap_or(&percent);
    format!("{percent}% CI [{}, {}]", fmt(lo), fmt(hi))
}

/// A piece of an APA line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "style", content = "text", rename_all = "snake_case")]
pub enum ApaToken {
    /// Upright text.
    Plain(String),
    /// A statistic symbol, drawn in italics.
    Italic(String),
}

/// An APA results line: comma-separated parts, each a run of tokens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApaLine {
    parts: Vec<Vec<ApaToken>>,
}

/// Italic for Latin symbols, plain for Greek ones (χ², τ, ρ).
fn symbol(symbol: &str) -> ApaToken {
    let greek = symbol
        .chars()
        .next()
        .is_some_and(|c| ('\u{0370}'..='\u{03ff}').contains(&c));
    if greek {
        ApaToken::Plain(symbol.to_owned())
    } else {
        ApaToken::Italic(symbol.to_owned())
    }
}

impl ApaLine {
    /// An empty line.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a part made of `tokens`.
    #[must_use]
    pub fn part(mut self, tokens: Vec<ApaToken>) -> Self {
        self.parts.push(tokens);
        self
    }

    /// Adds `symbol(df, ...) = value`, e.g. `t(18.33) = −3.77`; no
    /// parentheses when `df` is empty.
    #[must_use]
    pub fn stat(self, sym: &str, df: &[f64], value: &str) -> Self {
        let mut tokens = vec![symbol(sym)];
        if !df.is_empty() {
            let df: Vec<String> = df.iter().map(|d| df_text(*d)).collect();
            tokens.push(ApaToken::Plain(format!("({})", df.join(", "))));
        }
        tokens.push(ApaToken::Plain(format!(" = {value}")));
        self.part(tokens)
    }

    /// Adds `p = .043` or `p < .001`.
    #[must_use]
    pub fn p(self, p: f64) -> Self {
        let (relation, value) = p_value_parts(p);
        self.part(vec![
            ApaToken::Italic("p".into()),
            ApaToken::Plain(format!(" {relation} {value}")),
        ])
    }

    /// Adds an interval, `95% CI [lo, hi]`, with two decimals.
    #[must_use]
    pub fn ci(self, ci: &Interval, bounded_stat: bool) -> Self {
        self.part(vec![ApaToken::Plain(ci_text(
            ci.level,
            ci.lo,
            ci.hi,
            2,
            bounded_stat,
        ))])
    }

    /// Adds an effect size, `d = −1.48`, and its interval when given.
    #[must_use]
    pub fn effect(self, sym: &str, value: f64, ci: Option<&Interval>) -> Self {
        let line = self.stat(sym, &[], &number(value, 2));
        match ci {
            Some(ci) => line.ci(ci, false),
            None => line,
        }
    }

    /// The tokens, with `, ` between parts.
    #[must_use]
    pub fn tokens(&self) -> Vec<ApaToken> {
        let mut out = Vec::new();
        for (i, part) in self.parts.iter().enumerate() {
            if i > 0 {
                out.push(ApaToken::Plain(", ".into()));
            }
            out.extend(part.iter().cloned());
        }
        out
    }

    /// The line as plain text, italics dropped.
    #[must_use]
    pub fn plain(&self) -> String {
        self.tokens()
            .into_iter()
            .map(|t| match t {
                ApaToken::Plain(s) | ApaToken::Italic(s) => s,
            })
            .collect()
    }

    /// The line as markdown, italics as `*t*`.
    #[must_use]
    pub fn markdown(&self) -> String {
        self.tokens()
            .into_iter()
            .map(|t| match t {
                ApaToken::Plain(s) => s,
                ApaToken::Italic(s) => format!("*{s}*"),
            })
            .collect()
    }
}

/// `t(df) = t, p = p`.
#[must_use]
pub fn t_test(t: f64, df: f64, p: f64) -> ApaLine {
    ApaLine::new().stat("t", &[df], &number(t, 2)).p(p)
}

/// `F(df1, df2) = F, p = p`.
#[must_use]
pub fn f_test(f: f64, df1: f64, df2: f64, p: f64) -> ApaLine {
    ApaLine::new().stat("F", &[df1, df2], &number(f, 2)).p(p)
}

/// `χ²(df, N = n) = χ², p = p`.
#[must_use]
pub fn chi_square(chi2: f64, df: f64, n: u64, p: f64) -> ApaLine {
    ApaLine::new()
        .part(vec![
            ApaToken::Plain(format!("χ²({}, ", df_text(df))),
            ApaToken::Italic("N".into()),
            ApaToken::Plain(format!(" = {n}) = {}", number(chi2, 2))),
        ])
        .p(p)
}

/// A correlation, `r(df) = .45, p = .010`; `symbol` is `r`, `rₛ` or `τ`.
#[must_use]
pub fn correlation(sym: &str, r: f64, df: f64, p: f64) -> ApaLine {
    ApaLine::new().stat(sym, &[df], &bounded(r, 2)).p(p)
}

/// A statistic without df, e.g. `U = 23.50, p = .021`.
#[must_use]
pub fn statistic(sym: &str, value: f64, p: f64) -> ApaLine {
    ApaLine::new().stat(sym, &[], &number(value, 2)).p(p)
}

/// `M = 3.40, SD = 1.20` with `decimals` decimals (one for integer scales,
/// two otherwise).
#[must_use]
pub fn mean_sd(mean: f64, sd: f64, decimals: u32) -> ApaLine {
    ApaLine::new()
        .stat("M", &[], &number(mean, decimals))
        .stat("SD", &[], &number(sd, decimals))
}

/// `R² = .76`.
#[must_use]
pub fn r_squared(r2: f64) -> ApaLine {
    ApaLine::new().stat("R²", &[], &bounded(r2, 2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p_values_follow_apa() {
        assert_eq!(p_value_text(0.000_4), "< .001");
        assert_eq!(p_value_text(0.001_374), ".001");
        assert_eq!(p_value_text(0.043_2), ".043");
        assert_eq!(p_value_text(0.38), ".38");
        assert_eq!(p_value_text(0.123_4), ".12");
        assert_eq!(p_value_text(0.999), "> .99");
        assert_eq!(p_value_text(1.0), "> .99");
        assert_eq!(p_value_parts(0.02), ("=", ".020".into()));
    }

    #[test]
    fn numbers_use_a_true_minus_and_no_negative_zero() {
        assert_eq!(number(-3.767, 2), "−3.77");
        assert_eq!(number(-0.001, 2), "0.00");
        assert_eq!(number(12.0, 0), "12");
        assert_eq!(with_minus("1e-4"), "1e−4");
    }

    #[test]
    fn bounded_statistics_drop_the_leading_zero() {
        assert_eq!(bounded(0.456, 2), ".46");
        assert_eq!(bounded(-0.07, 2), "−.07");
        assert_eq!(bounded(1.0, 2), "1.00");
        assert_eq!(bounded(-0.001, 2), ".00");
    }

    #[test]
    fn significant_digits() {
        assert_eq!(significant(-0.012_345, 3), "−0.0123");
        assert_eq!(significant(1.234_56, 3), "1.23");
        assert_eq!(significant(12_345.0, 3), "12300");
        assert_eq!(significant(0.0, 3), "0.00");
    }

    #[test]
    fn df_is_whole_or_two_decimals_without_separator() {
        assert_eq!(df_text(30.0), "30");
        assert_eq!(df_text(18.333), "18.33");
        assert_eq!(df_text(12_345.0), "12345");
    }

    #[test]
    fn confidence_intervals() {
        assert_eq!(
            ci_text(0.95, -11.28, -3.21, 2, false),
            "95% CI [−11.28, −3.21]"
        );
        assert_eq!(ci_text(0.995, 0.12, 0.5, 2, true), "99.5% CI [.12, .50]");
    }

    #[test]
    fn t_test_line_has_italic_symbols() {
        let ci = Interval {
            level: 0.95,
            lo: -2.27,
            hi: -0.67,
            method: None,
        };
        let line = t_test(-3.767, 18.33, 0.001_374).effect("d", -1.48, Some(&ci));
        assert_eq!(
            line.plain(),
            "t(18.33) = −3.77, p = .001, d = −1.48, 95% CI [−2.27, −0.67]"
        );
        assert_eq!(
            line.markdown(),
            "*t*(18.33) = −3.77, *p* = .001, *d* = −1.48, 95% CI [−2.27, −0.67]"
        );
        assert_eq!(line.tokens()[0], ApaToken::Italic("t".into()));
    }

    #[test]
    fn other_lines() {
        assert_eq!(
            chi_square(4.123, 1.0, 1500, 0.0423).markdown(),
            "χ²(1, *N* = 1500) = 4.12, *p* = .042"
        );
        assert_eq!(
            f_test(45.1, 2.0, 29.0, 0.000_01).plain(),
            "F(2, 29) = 45.10, p < .001"
        );
        assert_eq!(
            correlation("r", 0.451, 30.0, 0.0101).plain(),
            "r(30) = .45, p = .010"
        );
        assert_eq!(
            correlation("τ", -0.3, 18.0, 0.2).markdown(),
            "τ(18) = −.30, *p* = .20"
        );
        assert_eq!(statistic("U", 23.5, 0.021).plain(), "U = 23.50, p = .021");
        assert_eq!(mean_sd(3.4, 1.21, 1).plain(), "M = 3.4, SD = 1.2");
        assert_eq!(r_squared(0.7612).markdown(), "*R²* = .76");
    }
}
