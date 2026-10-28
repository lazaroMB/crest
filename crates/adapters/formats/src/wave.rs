//! WaveDrom timing diagrams: WaveJSON to SVG with the `wavedrom` crate.
//!
//! WaveJSON is usually written JSON5-style, as on wavedrom.com: keys
//! without quotes, single-quoted strings, comments and trailing commas.
//! [`relax`] turns that into strict JSON first, so the crate does not need
//! its `json5` feature (and pest).

/// Renders `source` to an SVG document.
///
/// # Errors
///
/// The source is not WaveJSON the renderer understands.
pub fn render(source: &str) -> Result<String, String> {
    let json = relax(source);
    let mut svg = Vec::new();
    wavedrom::render_json(&json, &mut svg).map_err(|e| format!("invalid WaveJSON: {e}"))?;
    let svg = String::from_utf8(svg).map_err(|e| format!("WaveDrom wrote invalid UTF-8: {e}"))?;
    // Helvetica is rarely installed outside macOS: let the rasterizer fall
    // back to a sans-serif face of similar widths.
    Ok(svg.replace(
        r#"font-family="Helvetica""#,
        r#"font-family="Helvetica, Arial, Liberation Sans, sans-serif""#,
    ))
}

/// JSON5-style text as strict JSON: quotes bare keys, turns single-quoted
/// strings into double-quoted ones, drops comments and trailing commas.
/// Strict JSON passes through unchanged.
#[must_use]
pub fn relax(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len() + 16);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' | '\'' => {
                i = string(&chars, i, &mut out);
                continue;
            }
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i += 2;
                continue;
            }
            ',' => {
                // A comma before a closing bracket is dropped.
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                if !matches!(chars.get(j), Some('}' | ']')) {
                    out.push(',');
                }
            }
            c if c.is_alphabetic() || c == '_' || c == '$' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$')
                {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                let mut j = i;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                if chars.get(j) == Some(&':') {
                    out.push('"');
                    out.push_str(&word);
                    out.push('"');
                } else {
                    out.push_str(&word);
                }
                continue;
            }
            c => out.push(c),
        }
        i += 1;
    }
    out
}

/// Copies the string starting at `chars[start]` as a double-quoted JSON
/// string; returns the index after it.
fn string(chars: &[char], start: usize, out: &mut String) -> usize {
    let quote = chars[start];
    out.push('"');
    let mut i = start + 1;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                match chars.get(i + 1) {
                    // `\'` needs no escape in a double-quoted string.
                    Some('\'') => out.push('\''),
                    Some(&next) => {
                        out.push('\\');
                        out.push(next);
                    }
                    None => {}
                }
                i += 2;
                continue;
            }
            c if c == quote => {
                out.push('"');
                return i + 1;
            }
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
        i += 1;
    }
    out.push('"');
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relaxes_json5_style_wavejson() {
        let source = "{ signal: [\n  // the clock\n  { name: 'clk', wave: 'p...' },\n  { name: \"it's\", wave: '=.', data: ['a b',], },\n] /* end */ }";
        let json = relax(source);
        let value: serde_json::Value =
            serde_json::from_str(&json).unwrap_or_else(|e| panic!("{e}: {json}"));
        assert_eq!(value["signal"][0]["name"], "clk");
        assert_eq!(value["signal"][1]["name"], "it's");
        assert_eq!(value["signal"][1]["data"][0], "a b");
        let strict = r#"{"signal": [{"name": "x", "wave": "01"}]}"#;
        assert_eq!(relax(strict), strict);
        assert_eq!(
            relax(r#"{a: 'say \'hi\' \"x\"'}"#),
            r#"{"a": "say 'hi' \"x\""}"#
        );
        assert_eq!(relax("{a: true, b: null}"), r#"{"a": true, "b": null}"#);
    }
}
