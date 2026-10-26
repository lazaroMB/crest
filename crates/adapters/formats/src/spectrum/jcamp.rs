//! Our own JCAMP-DX reader (4.24 and 5.01 labelled data records).
//!
//! Records are `##LABEL=value` with labels compared without spaces,
//! dashes, slashes and underscores, in upper case; `$$` starts a
//! comment. `##TITLE=` opens a block and `##END=` closes it; a link
//! block (`##BLOCKS=`) holds others, and every block with data is kept.
//! Data come from `##XYDATA=(X++(Y..Y))` with AFFN or ASDF compression
//! (SQZ, DIF, DUP, with the DIF y-check value at the start of the next
//! line dropped), `##XYPOINTS=(XY..XY)` or `##PEAK TABLE=(XY..XY)` /
//! `(XYW..XYW)`. `XFACTOR`/`YFACTOR` scale the values; `FIRSTX`,
//! `LASTX` and the decoded count place the abscissae. NMR in Hz is turned
//! into ppm with `.OBSERVE FREQUENCY`. NTUPLES (2D NMR, complex data) are
//! not read.

/// One data block: its records and data.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Block {
    /// Records by normalised label, in file order.
    pub labels: Vec<(String, String)>,
    /// Abscissae.
    pub x: Vec<f64>,
    /// Ordinates.
    pub y: Vec<f64>,
    /// Whether the data are a peak table (sticks).
    pub sticks: bool,
    /// Reader notes.
    pub warnings: Vec<String>,
}

impl Block {
    /// The value of normalised label `key`.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.labels
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.trim())
            .filter(|v| !v.is_empty())
    }

    /// The number under `key`.
    #[must_use]
    pub fn number(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(|v| v.split_whitespace().next()?.parse().ok())
    }
}

/// A label as compared: upper case, without spaces, `-`, `/` and `_`.
#[must_use]
pub fn normalise(label: &str) -> String {
    label
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '/' | '_' | '\t'))
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// A token of a compressed data line.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Token {
    /// An absolute value (AFFN or SQZ).
    Abs(f64),
    /// A difference from the previous value (DIF).
    Dif(f64),
    /// Repeat the previous token this many times in all (DUP).
    Dup(u32),
    /// A missing value (`?`).
    Missing,
}

fn sqz(c: char) -> Option<i32> {
    match c {
        '@' => Some(0),
        'A'..='I' => Some(c as i32 - 'A' as i32 + 1),
        'a'..='i' => Some(-(c as i32 - 'a' as i32 + 1)),
        _ => None,
    }
}

fn dif(c: char) -> Option<i32> {
    match c {
        '%' => Some(0),
        'J'..='R' => Some(c as i32 - 'J' as i32 + 1),
        'j'..='r' => Some(-(c as i32 - 'j' as i32 + 1)),
        _ => None,
    }
}

fn dup(c: char) -> Option<u32> {
    match c {
        'S'..='Z' => Some(c as u32 - 'S' as u32 + 1),
        's' => Some(9),
        _ => None,
    }
}

/// The tokens of one data line.
fn tokens(line: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    // The digits (and a decimal point) after a lead character.
    let digits = |i: &mut usize| -> String {
        let start = *i;
        while *i < chars.len() && (chars[*i].is_ascii_digit() || chars[*i] == '.') {
            *i += 1;
        }
        chars[start..*i].iter().collect()
    };
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() || c == ',' || c == ';' {
            i += 1;
        } else if c == '?' {
            out.push(Token::Missing);
            i += 1;
        } else if c.is_ascii_digit() || c == '.' || c == '+' || c == '-' {
            // AFFN, possibly with an exponent.
            let start = i;
            i += 1;
            while i < chars.len()
                && (chars[i].is_ascii_digit()
                    || chars[i] == '.'
                    || ((chars[i] == 'E' || chars[i] == 'e')
                        && chars.get(i + 1).is_some_and(|n| {
                            n.is_ascii_digit() || *n == '+' || *n == '-'
                        })))
            {
                if chars[i] == 'E' || chars[i] == 'e' {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            let text: String = chars[start..i].iter().collect();
            let v: f64 = text
                .parse()
                .map_err(|_| format!("`{text}` is not a number"))?;
            out.push(Token::Abs(v));
        } else if let Some(lead) = sqz(c) {
            i += 1;
            let rest = digits(&mut i);
            out.push(Token::Abs(signed(lead, &rest)?));
        } else if let Some(lead) = dif(c) {
            i += 1;
            let rest = digits(&mut i);
            out.push(Token::Dif(signed(lead, &rest)?));
        } else if let Some(lead) = dup(c) {
            i += 1;
            let rest = digits(&mut i);
            let n: u32 = format!("{lead}{rest}")
                .parse()
                .map_err(|_| format!("bad DUP count near `{c}{rest}`"))?;
            out.push(Token::Dup(n));
        } else {
            return Err(format!("unexpected character `{c}` in the data"));
        }
    }
    Ok(out)
}

/// A SQZ/DIF value: the lead digit (with its sign) and the digits after.
fn signed(lead: i32, rest: &str) -> Result<f64, String> {
    let magnitude: f64 = format!("{}{rest}", lead.abs())
        .parse()
        .map_err(|_| format!("bad compressed value `{lead}{rest}`"))?;
    Ok(if lead < 0 { -magnitude } else { magnitude })
}

/// The ordinates of `(X++(Y..Y))` lines (each line's X is returned too,
/// unscaled), dropping the DIF y-check values.
fn xydata(lines: &[&str], warnings: &mut Vec<String>) -> Result<(Vec<f64>, Vec<f64>), String> {
    let mut ys: Vec<f64> = Vec::new();
    let mut line_x: Vec<f64> = Vec::new();
    let mut checks = 0usize;
    // Whether the previous line ended in DIF form (its last y is repeated
    // as the first y of this line).
    let mut ended_in_dif = false;
    for (n, line) in lines.iter().enumerate() {
        let toks = tokens(line).map_err(|e| format!("XYDATA line {}: {e}", n + 1))?;
        let Some((first, rest)) = toks.split_first() else {
            continue;
        };
        let Token::Abs(x) = *first else {
            return Err(format!("XYDATA line {} does not start with an x value", n + 1));
        };
        line_x.push(x);
        let mut values: Vec<f64> = Vec::new();
        let mut prev: Option<Token> = None;
        let mut last_was_dif = false;
        for &t in rest {
            match t {
                Token::Abs(v) => {
                    values.push(v);
                    last_was_dif = false;
                }
                Token::Dif(d) => {
                    let base = values.last().copied().or_else(|| ys.last().copied());
                    let Some(base) = base else {
                        return Err(format!("XYDATA line {}: a difference with no value before it", n + 1));
                    };
                    values.push(base + d);
                    last_was_dif = true;
                }
                Token::Dup(count) => {
                    let Some(p) = prev else {
                        return Err(format!("XYDATA line {}: a repeat with nothing to repeat", n + 1));
                    };
                    for _ in 1..count {
                        match p {
                            Token::Abs(v) => values.push(v),
                            Token::Dif(d) => {
                                let base = values.last().copied().unwrap_or(0.0);
                                values.push(base + d);
                            }
                            Token::Missing => values.push(f64::NAN),
                            Token::Dup(_) => {}
                        }
                    }
                }
                Token::Missing => {
                    values.push(f64::NAN);
                    last_was_dif = false;
                }
            }
            if !matches!(t, Token::Dup(_)) {
                prev = Some(t);
            }
        }
        if ended_in_dif && !values.is_empty() {
            let check = values.remove(0);
            if let Some(&last) = ys.last()
                && (check - last).abs() > 1e-9 * last.abs().max(1.0)
            {
                checks += 1;
            }
        }
        ended_in_dif = last_was_dif;
        ys.extend(values);
    }
    if checks > 0 {
        warnings.push(format!("{checks} y-check values did not match"));
    }
    Ok((line_x, ys))
}

/// The AFFN numbers of `(XY..XY)` lines in groups of `width`.
fn groups(lines: &[&str], width: usize) -> Result<Vec<Vec<f64>>, String> {
    let mut numbers = Vec::new();
    for line in lines {
        for t in tokens(line)? {
            match t {
                Token::Abs(v) => numbers.push(v),
                Token::Missing => numbers.push(f64::NAN),
                _ => return Err("compressed values in an (XY..XY) table".into()),
            }
        }
    }
    Ok(numbers.chunks(width).filter(|c| c.len() >= 2).map(<[f64]>::to_vec).collect())
}

/// The data blocks of a JCAMP-DX file, in file order.
///
/// # Errors
///
/// A message when the text has no `##` records or a data line cannot be
/// read.
pub fn parse(text: &str) -> Result<Vec<Block>, String> {
    if !text.trim_start().starts_with("##") {
        return Err("not JCAMP-DX: no ##LABEL= records".into());
    }
    // (label, value, data lines) per record.
    let mut records: Vec<(String, String, Vec<&str>)> = Vec::new();
    for raw in text.lines() {
        let line = match raw.find("$$") {
            Some(at) => &raw[..at],
            None => raw,
        };
        if let Some(rest) = line.trim_start().strip_prefix("##") {
            let (label, value) = rest.split_once('=').unwrap_or((rest, ""));
            records.push((normalise(label), value.trim().to_owned(), Vec::new()));
        } else if let Some(last) = records.last_mut()
            && !line.trim().is_empty()
        {
            last.2.push(line);
        }
    }
    let mut out = Vec::new();
    let mut stack: Vec<Block> = Vec::new();
    for (label, value, lines) in records {
        match label.as_str() {
            "TITLE" => {
                let mut block = Block::default();
                block.labels.push((label, value));
                stack.push(block);
            }
            "END" => {
                if let Some(mut block) = stack.pop()
                    && !block.x.is_empty()
                {
                    finish(&mut block);
                    out.push(block);
                }
            }
            _ => {
                let Some(block) = stack.last_mut() else {
                    continue;
                };
                let form = value.replace(' ', "").to_ascii_uppercase();
                match label.as_str() {
                    "XYDATA" if form.starts_with("(X++(Y..Y))") => {
                        let (line_x, ys) = xydata(&lines, &mut block.warnings)?;
                        place(block, &line_x, ys);
                    }
                    "XYDATA" => {
                        block
                            .warnings
                            .push(format!("XYDATA form {value} is not read"));
                    }
                    "XYPOINTS" | "PEAKTABLE" => {
                        let width = if form.contains("XYW") || form.contains("XYM") {
                            3
                        } else {
                            2
                        };
                        let rows = groups(&lines, width)?;
                        let (xf, yf) = (factor(block, "XFACTOR"), factor(block, "YFACTOR"));
                        // A peak table next to XYDATA is not drawn over it.
                        if block.x.is_empty() {
                            block.x = rows.iter().map(|r| r[0] * xf).collect();
                            block.y = rows.iter().map(|r| r[1] * yf).collect();
                            block.sticks = label == "PEAKTABLE";
                        }
                    }
                    "NTUPLES" => block
                        .warnings
                        .push("NTUPLES data (2D or complex spectra) are not read".into()),
                    _ => {}
                }
                block.labels.push((label, value));
            }
        }
    }
    // Blocks never closed with ##END still count.
    while let Some(mut block) = stack.pop() {
        if !block.x.is_empty() {
            finish(&mut block);
            out.push(block);
        }
    }
    if out.is_empty() {
        return Err("the JCAMP-DX file has no XYDATA, XYPOINTS or PEAK TABLE data".into());
    }
    Ok(out)
}

fn factor(block: &Block, key: &str) -> f64 {
    block.number(key).filter(|f| *f != 0.0).unwrap_or(1.0)
}

/// Places decoded ordinates on their abscissae.
fn place(block: &mut Block, line_x: &[f64], ys: Vec<f64>) {
    let yf = factor(block, "YFACTOR");
    let xf = factor(block, "XFACTOR");
    let n = ys.len();
    if let Some(expected) = block.number("NPOINTS")
        && (expected - n as f64).abs() > 0.5
    {
        block.warnings.push(format!(
            "NPOINTS says {expected} but {n} values were decoded"
        ));
    }
    let first = block
        .number("FIRSTX")
        .or_else(|| line_x.first().map(|x| x * xf));
    let last = block.number("LASTX");
    let delta = match (first, last) {
        (Some(f), Some(l)) if n > 1 => Some((l - f) / (n as f64 - 1.0)),
        _ => block.number("DELTAX").or_else(|| {
            // Two line starts and the count between them give the step.
            (line_x.len() > 1).then(|| (line_x[1] - line_x[0]) * xf)
        }),
    };
    let first = first.unwrap_or(0.0);
    let delta = delta.unwrap_or(1.0);
    block.x = (0..n).map(|i| first + delta * i as f64).collect();
    block.y = ys.into_iter().map(|y| y * yf).collect();
}

/// Drops points that are not finite (missing values), noting how many.
fn finish(block: &mut Block) {
    let before = block.x.len();
    let (x, y): (Vec<f64>, Vec<f64>) = block
        .x
        .iter()
        .zip(&block.y)
        .filter(|(x, y)| x.is_finite() && y.is_finite())
        .map(|(x, y)| (*x, *y))
        .unzip();
    if x.len() < before {
        block
            .warnings
            .push(format!("{} missing values left out", before - x.len()));
    }
    block.x = x;
    block.y = y;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asdf_tokens_decode() {
        // The JCAMP-DX example: SQZ, DIF and DUP forms of the same values.
        assert_eq!(
            tokens("1A2b3%J5T").unwrap(),
            vec![
                Token::Abs(1.0),
                Token::Abs(12.0),
                Token::Abs(-23.0),
                Token::Dif(0.0),
                Token::Dif(15.0),
                Token::Dup(2)
            ]
        );
        assert_eq!(tokens("-1.5E2 +3").unwrap(), vec![Token::Abs(-150.0), Token::Abs(3.0)]);
        assert!(tokens("1 #").is_err());
    }

    #[test]
    fn xydata_with_dif_dup_and_y_checks() {
        // Values 10 11 12 12 13 on x 1..5; the next line repeats 13 at
        // x 5 as its check, then 15; the last checks 15 and repeats it.
        let lines = ["1 A0JT%J", "5 A3K", "6 A5T"];
        let mut warnings = Vec::new();
        let (line_x, ys) = xydata(&lines, &mut warnings).unwrap();
        assert_eq!(line_x, [1.0, 5.0, 6.0]);
        assert_eq!(ys, [10.0, 11.0, 12.0, 12.0, 13.0, 15.0, 15.0]);
        // A wrong check value is counted.
        let mut bad = Vec::new();
        xydata(&["1 A0J", "2 A5"], &mut bad).unwrap();
        assert_eq!(bad.len(), 1);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn blocks_and_tables() {
        let text = "##TITLE=link\n##BLOCKS=2\n\
                    ##TITLE=ir\n##JCAMP-DX=5.01\n##XFACTOR=1\n##YFACTOR=0.5\n##FIRSTX=10\n\
                    ##LASTX=13\n##NPOINTS=4\n##XYDATA=(X++(Y..Y))\n10 2 4 6 8 $$ comment\n##END=\n\
                    ##TITLE=ms\n##PEAK TABLE=(XY..XY)\n50,10 51,100\n52,5\n##END=\n##END=\n";
        let blocks = parse(text).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].x, [10.0, 11.0, 12.0, 13.0]);
        assert_eq!(blocks[0].y, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(blocks[0].get("TITLE"), Some("ir"));
        assert!(!blocks[0].sticks);
        assert_eq!(blocks[1].x, [50.0, 51.0, 52.0]);
        assert!(blocks[1].sticks);
        assert!(parse("no records").is_err());
        assert!(parse("##TITLE=x\n##END=\n").is_err());
    }
}
