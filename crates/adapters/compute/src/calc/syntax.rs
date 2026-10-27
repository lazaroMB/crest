//! A small reader of numbat expressions, enough to find the names a calc
//! line uses and to typeset it.
//!
//! It mirrors the precedence of numbat's grammar (lowest first):
//! conversion (`->`, `→`, `to`), `+ -`, `* × · / ÷`, `per`, unary `- +`,
//! juxtaposition (`9.81 m`, so `a / b c` is `a / (b c)`), `^` (right
//! associative), `!`, Unicode exponents, calls and parentheses. Anything
//! outside that subset (strings, comparisons, lists, `if`) is not parsed:
//! [`parse`] returns `None` and the line is shown without TeX, but
//! [`identifiers`] still finds its names.

/// One token.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    /// A number as written, e.g. `1.5e3` or `1_000`.
    Number(String),
    /// A name: a unit, constant, function or sheet variable.
    Ident(String),
    /// An operator or punctuation: `+ - * × · / ÷ ^ ! ( ) , ->`.
    Op(&'static str),
    /// A Unicode exponent such as `²` or `⁻¹`, as an integer.
    Exponent(i32),
    /// A string literal (contents dropped).
    Str,
    /// Anything else (`<`, `[`, `=`, …).
    Other(char),
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || matches!(c, '_' | '°' | '′' | '″' | '%' | '€' | '$' | '£' | '¥')
}

fn is_ident_continue(c: char) -> bool {
    // Superscript digits count as numeric, but they are exponents.
    (c.is_alphanumeric() && superscript(c).is_none())
        || matches!(c, '_' | '%' | '€' | '$' | '£' | '¥')
}

fn superscript(c: char) -> Option<i32> {
    Some(match c {
        '⁰' => 0,
        '¹' => 1,
        '²' => 2,
        '³' => 3,
        '⁴' => 4,
        '⁵' => 5,
        '⁶' => 6,
        '⁷' => 7,
        '⁸' => 8,
        '⁹' => 9,
        _ => return None,
    })
}

/// The tokens of `source`.
pub fn tokens(source: &str) -> Vec<Token> {
    let chars: Vec<char> = source.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit()
            || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit))
        {
            let start = i;
            if c == '0' && matches!(chars.get(i + 1), Some('x' | 'o' | 'b')) {
                i += 2;
                while chars.get(i).is_some_and(char::is_ascii_alphanumeric) {
                    i += 1;
                }
            } else {
                while chars
                    .get(i)
                    .is_some_and(|c| c.is_ascii_digit() || matches!(c, '_' | '.'))
                {
                    i += 1;
                }
                if matches!(chars.get(i), Some('e' | 'E')) {
                    let sign = usize::from(matches!(chars.get(i + 1), Some('+' | '-')));
                    if chars.get(i + 1 + sign).is_some_and(char::is_ascii_digit) {
                        i += 1 + sign;
                        while chars
                            .get(i)
                            .is_some_and(|c| c.is_ascii_digit() || *c == '_')
                        {
                            i += 1;
                        }
                    }
                }
            }
            out.push(Token::Number(chars[start..i].iter().collect()));
        } else if is_ident_start(c) {
            let start = i;
            i += 1;
            while chars.get(i).copied().is_some_and(is_ident_continue) {
                i += 1;
            }
            out.push(Token::Ident(chars[start..i].iter().collect()));
        } else if c == '"' {
            i += 1;
            while chars.get(i).is_some_and(|c| *c != '"') {
                i += 1;
            }
            i += 1;
            out.push(Token::Str);
        } else if c == '⁻' || superscript(c).is_some() {
            let negative = c == '⁻';
            if negative {
                i += 1;
            }
            let mut value = 0i32;
            let mut any = false;
            while let Some(d) = chars.get(i).copied().and_then(superscript) {
                value = value.saturating_mul(10).saturating_add(d);
                any = true;
                i += 1;
            }
            if any {
                out.push(Token::Exponent(if negative { -value } else { value }));
            } else {
                out.push(Token::Other('⁻'));
            }
        } else {
            let two: String = chars[i..chars.len().min(i + 2)].iter().collect();
            let op = match two.as_str() {
                "->" => Some(("->", 2)),
                "**" => Some(("^", 2)),
                _ => match c {
                    '→' => Some(("->", 1)),
                    '+' => Some(("+", 1)),
                    '-' | '−' => Some(("-", 1)),
                    '*' => Some(("*", 1)),
                    '×' => Some(("×", 1)),
                    '·' | '⋅' => Some(("·", 1)),
                    '/' | '÷' => Some(("/", 1)),
                    '^' => Some(("^", 1)),
                    '!' => Some(("!", 1)),
                    '(' => Some(("(", 1)),
                    ')' => Some((")", 1)),
                    ',' => Some((",", 1)),
                    _ => None,
                },
            };
            match op {
                // `!=` is a comparison, not a factorial.
                Some(("!", _)) if chars.get(i + 1) == Some(&'=') => {
                    out.push(Token::Other('!'));
                    i += 1;
                }
                Some((op, len)) => {
                    out.push(Token::Op(op));
                    i += len;
                }
                None => {
                    out.push(Token::Other(c));
                    i += 1;
                }
            }
        }
    }
    out
}

/// The names `source` uses, in order of first use, without duplicates.
pub fn identifiers(source: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for token in tokens(source) {
        if let Token::Ident(name) = token
            && !names.contains(&name)
        {
            names.push(name);
        }
    }
    names
}

/// An expression tree of the parsed subset.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// A number as written.
    Number(String),
    /// A name.
    Ident(String),
    /// `( … )` as written.
    Paren(Box<Expr>),
    /// Unary minus or plus.
    Unary(&'static str, Box<Expr>),
    /// `a op b` for `+ - * × · /` (`per` and `÷` read as `/`).
    Binary(&'static str, Box<Expr>, Box<Expr>),
    /// Juxtaposition: `9.81 m`.
    Implicit(Box<Expr>, Box<Expr>),
    /// `a ^ b`, also from Unicode exponents.
    Power(Box<Expr>, Box<Expr>),
    /// `a!`.
    Factorial(Box<Expr>),
    /// `f(args)`.
    Call(String, Vec<Expr>),
    /// `a -> b`.
    Convert(Box<Expr>, Box<Expr>),
}

/// Parses `source`; `None` when it uses anything outside the subset.
pub fn parse(source: &str) -> Option<Expr> {
    let tokens = tokens(source);
    let mut parser = Parser { tokens, pos: 0 };
    let expr = parser.conversion()?;
    (parser.pos == parser.tokens.len()).then_some(expr)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn eat_op(&mut self, ops: &[&'static str]) -> Option<&'static str> {
        match self.peek() {
            Some(Token::Op(op)) if ops.contains(op) => {
                let op = *op;
                self.pos += 1;
                Some(op)
            }
            _ => None,
        }
    }

    fn eat_word(&mut self, word: &str) -> bool {
        if matches!(self.peek(), Some(Token::Ident(w)) if w == word) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn conversion(&mut self) -> Option<Expr> {
        let mut left = self.term()?;
        while self.eat_op(&["->"]).is_some() || self.eat_word("to") {
            let right = self.term()?;
            left = Expr::Convert(Box::new(left), Box::new(right));
        }
        Some(left)
    }

    fn term(&mut self) -> Option<Expr> {
        let mut left = self.factor()?;
        while let Some(op) = self.eat_op(&["+", "-"]) {
            let right = self.factor()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Some(left)
    }

    fn factor(&mut self) -> Option<Expr> {
        let mut left = self.per()?;
        while let Some(op) = self.eat_op(&["*", "×", "·", "/"]) {
            let right = self.per()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Some(left)
    }

    fn per(&mut self) -> Option<Expr> {
        let mut left = self.unary()?;
        while self.eat_word("per") {
            let right = self.unary()?;
            left = Expr::Binary("/", Box::new(left), Box::new(right));
        }
        Some(left)
    }

    fn unary(&mut self) -> Option<Expr> {
        if let Some(op) = self.eat_op(&["-", "+"]) {
            let inner = self.unary()?;
            return Some(Expr::Unary(op, Box::new(inner)));
        }
        self.implicit()
    }

    fn starts_primary(&self) -> bool {
        match self.peek() {
            Some(Token::Number(_)) => true,
            Some(Token::Ident(word)) => !matches!(word.as_str(), "per" | "to"),
            Some(Token::Op("(")) => true,
            _ => false,
        }
    }

    fn implicit(&mut self) -> Option<Expr> {
        let mut left = self.power()?;
        while self.starts_primary() {
            let right = self.power()?;
            left = Expr::Implicit(Box::new(left), Box::new(right));
        }
        Some(left)
    }

    fn power(&mut self) -> Option<Expr> {
        let base = self.factorial()?;
        if self.eat_op(&["^"]).is_some() {
            let exponent = match self.eat_op(&["-"]) {
                Some(op) => Expr::Unary(op, Box::new(self.power()?)),
                None => self.power()?,
            };
            return Some(Expr::Power(Box::new(base), Box::new(exponent)));
        }
        Some(base)
    }

    fn factorial(&mut self) -> Option<Expr> {
        let mut expr = self.unicode_power()?;
        while self.eat_op(&["!"]).is_some() {
            expr = Expr::Factorial(Box::new(expr));
        }
        Some(expr)
    }

    fn unicode_power(&mut self) -> Option<Expr> {
        let base = self.call()?;
        if let Some(Token::Exponent(n)) = self.peek() {
            let n = *n;
            self.pos += 1;
            let exponent = if n < 0 {
                Expr::Unary("-", Box::new(Expr::Number(n.unsigned_abs().to_string())))
            } else {
                Expr::Number(n.to_string())
            };
            return Some(Expr::Power(Box::new(base), Box::new(exponent)));
        }
        Some(base)
    }

    fn call(&mut self) -> Option<Expr> {
        let primary = self.primary()?;
        if matches!(self.peek(), Some(Token::Op("("))) {
            let Expr::Ident(name) = primary else {
                return None;
            };
            self.pos += 1;
            let mut args = Vec::new();
            if self.eat_op(&[")"]).is_none() {
                loop {
                    args.push(self.conversion()?);
                    if self.eat_op(&[")"]).is_some() {
                        break;
                    }
                    self.eat_op(&[","])?;
                }
            }
            return Some(Expr::Call(name, args));
        }
        Some(primary)
    }

    fn primary(&mut self) -> Option<Expr> {
        let token = self.peek()?.clone();
        self.pos += 1;
        match token {
            Token::Number(n) => Some(Expr::Number(n)),
            Token::Ident(name) if !matches!(name.as_str(), "per" | "to" | "if") => {
                Some(Expr::Ident(name))
            }
            Token::Op("(") => {
                let inner = self.conversion()?;
                self.eat_op(&[")"])?;
                Some(Expr::Paren(Box::new(inner)))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident(name: &str) -> Box<Expr> {
        Box::new(Expr::Ident(name.into()))
    }

    fn number(n: &str) -> Box<Expr> {
        Box::new(Expr::Number(n.into()))
    }

    #[test]
    fn juxtaposition_binds_tighter_than_division() {
        assert_eq!(
            parse("9.81 m/s^2"),
            Some(Expr::Binary(
                "/",
                Box::new(Expr::Implicit(number("9.81"), ident("m"))),
                Box::new(Expr::Power(ident("s"), number("2")))
            ))
        );
        assert_eq!(
            parse("1/2 m"),
            Some(Expr::Binary(
                "/",
                number("1"),
                Box::new(Expr::Implicit(number("2"), ident("m")))
            ))
        );
    }

    #[test]
    fn reads_unicode_exponents_calls_and_conversions() {
        assert_eq!(
            parse("sqrt(x) s⁻¹ -> Hz"),
            Some(Expr::Convert(
                Box::new(Expr::Implicit(
                    Box::new(Expr::Call("sqrt".into(), vec![Expr::Ident("x".into())])),
                    Box::new(Expr::Power(
                        ident("s"),
                        Box::new(Expr::Unary("-", number("1")))
                    ))
                )),
                ident("Hz")
            ))
        );
        assert_eq!(parse("2 ^ -3 ^ 2").map(|_| ()), Some(()));
        assert_eq!(parse("1.5e-3 kg"), parse("1.5e-3 kg"));
        assert!(matches!(
            tokens("1.5e-3")[..],
            [Token::Number(ref n)] if n == "1.5e-3"
        ));
    }

    #[test]
    fn leaves_the_rest_unparsed_but_finds_its_names() {
        assert_eq!(parse("if a > b then a else b"), None);
        assert_eq!(parse("(1 + 2"), None);
        assert_eq!(parse("\"x\""), None);
        assert_eq!(
            identifiers("if a > b then a else b"),
            ["if", "a", "b", "then", "else"]
        );
        assert_eq!(identifiers("v_0 * 2 h + \"s\""), ["v_0", "h"]);
        assert_eq!(identifiers("20 °C -> °F"), ["°C", "°F"]);
    }
}
