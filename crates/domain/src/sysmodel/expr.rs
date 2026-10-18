//! The expression language of guards, effects and assertions.
//!
//! Integers (`i64`, checked arithmetic), strings in single or double
//! quotes, `true` / `false`, names (`inflight`, `state`, `api.inflight`,
//! `api.state`), `+ - * / %`, comparisons, `&&`, `||`, `!` and
//! parentheses. A hand-written parser, no `eval`: the HTML export can
//! interpret the same grammar. Precedence, lowest first: `||`, `&&`,
//! `== !=`, `< <= > >=`, `+ -`, `* / %`, unary `! -`.

use std::fmt;

/// A value of an expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// An integer.
    Int(i64),
    /// A string (state names).
    Str(String),
    /// A boolean.
    Bool(bool),
}

impl Value {
    /// The type of the value.
    #[must_use]
    pub const fn ty(&self) -> Ty {
        match self {
            Self::Int(_) => Ty::Int,
            Self::Str(_) => Ty::Str,
            Self::Bool(_) => Ty::Bool,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(n) => write!(f, "{n}"),
            Self::Str(s) => write!(f, "'{s}'"),
            Self::Bool(b) => write!(f, "{b}"),
        }
    }
}

/// The type of a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ty {
    /// Integer.
    Int,
    /// String.
    Str,
    /// Boolean.
    Bool,
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Int => "integer",
            Self::Str => "string",
            Self::Bool => "boolean",
        })
    }
}

/// A unary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    /// `!`.
    Not,
    /// `-`.
    Neg,
}

/// A binary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    /// `||`.
    Or,
    /// `&&`.
    And,
    /// `==`.
    Eq,
    /// `!=`.
    Ne,
    /// `<`.
    Lt,
    /// `<=`.
    Le,
    /// `>`.
    Gt,
    /// `>=`.
    Ge,
    /// `+`.
    Add,
    /// `-`.
    Sub,
    /// `*`.
    Mul,
    /// `/` (truncating).
    Div,
    /// `%`.
    Rem,
}

impl BinOp {
    const fn symbol(self) -> &'static str {
        match self {
            Self::Or => "||",
            Self::And => "&&",
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::Rem => "%",
        }
    }

    const fn precedence(self) -> u8 {
        match self {
            Self::Or => 1,
            Self::And => 2,
            Self::Eq | Self::Ne => 3,
            Self::Lt | Self::Le | Self::Gt | Self::Ge => 4,
            Self::Add | Self::Sub => 5,
            Self::Mul | Self::Div | Self::Rem => 6,
        }
    }
}

/// A parsed expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// A literal.
    Lit(Value),
    /// `name` or `scope.name`.
    Name {
        /// Component id, when qualified.
        scope: Option<String>,
        /// Variable, `state` or another built-in.
        name: String,
    },
    /// `!e`, `-e`.
    Unary(UnOp, Box<Expr>),
    /// `a op b`.
    Binary(BinOp, Box<Expr>, Box<Expr>),
}

/// Why a text is not an expression.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message} at column {column} of `{source_text}`")]
pub struct ParseError {
    /// What went wrong.
    pub message: String,
    /// 1-based column.
    pub column: usize,
    /// The whole text.
    pub source_text: String,
}

/// Why an expression could not be evaluated.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EvalError {
    /// A name with no value.
    #[error("unknown name `{0}`")]
    Unknown(String),
    /// An operator applied to the wrong types.
    #[error("type error: {0}")]
    Type(String),
    /// Division or remainder by zero.
    #[error("division by zero")]
    DivisionByZero,
    /// Out of the `i64` range.
    #[error("integer overflow")]
    Overflow,
}

/// Values of the names an expression reads.
pub trait Env {
    /// The value of `scope.name` (or `name` without a scope).
    fn get(&self, scope: Option<&str>, name: &str) -> Option<Value>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Int(i64),
    Str(String),
    Ident(String),
    Dot,
    LParen,
    RParen,
    Op(BinOp),
    Not,
    End,
}

fn tokenize(src: &str) -> Result<Vec<(Tok, usize)>, ParseError> {
    let err = |message: String, at: usize| ParseError {
        message,
        column: at + 1,
        source_text: src.to_owned(),
    };
    let chars: Vec<(usize, char)> = src.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (at, c) = chars[i];
        let next = chars.get(i + 1).map(|&(_, c)| c);
        match c {
            c if c.is_whitespace() => i += 1,
            '0'..='9' => {
                let start = i;
                while i < chars.len() && chars[i].1.is_ascii_digit() {
                    i += 1;
                }
                let text: String = chars[start..i].iter().map(|&(_, c)| c).collect();
                let n = text
                    .parse::<i64>()
                    .map_err(|_| err(format!("number {text} is too large"), at))?;
                out.push((Tok::Int(n), at));
            }
            '\'' | '"' => {
                let quote = c;
                let mut text = String::new();
                i += 1;
                loop {
                    match chars.get(i) {
                        Some(&(_, c)) if c == quote => break,
                        Some(&(_, c)) => text.push(c),
                        None => return Err(err("unterminated string".into(), at)),
                    }
                    i += 1;
                }
                i += 1;
                out.push((Tok::Str(text), at));
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].1.is_ascii_alphanumeric() || chars[i].1 == '_') {
                    i += 1;
                }
                let text: String = chars[start..i].iter().map(|&(_, c)| c).collect();
                out.push((Tok::Ident(text), at));
            }
            '.' => {
                out.push((Tok::Dot, at));
                i += 1;
            }
            '(' => {
                out.push((Tok::LParen, at));
                i += 1;
            }
            ')' => {
                out.push((Tok::RParen, at));
                i += 1;
            }
            _ => {
                let pair = match (c, next) {
                    ('|', Some('|')) => Some(BinOp::Or),
                    ('&', Some('&')) => Some(BinOp::And),
                    ('=', Some('=')) => Some(BinOp::Eq),
                    ('!', Some('=')) => Some(BinOp::Ne),
                    ('<', Some('=')) => Some(BinOp::Le),
                    ('>', Some('=')) => Some(BinOp::Ge),
                    _ => None,
                };
                let tok = if let Some(op) = pair {
                    i += 2;
                    Tok::Op(op)
                } else {
                    i += 1;
                    match c {
                        '<' => Tok::Op(BinOp::Lt),
                        '>' => Tok::Op(BinOp::Gt),
                        '+' => Tok::Op(BinOp::Add),
                        '-' => Tok::Op(BinOp::Sub),
                        '*' => Tok::Op(BinOp::Mul),
                        '/' => Tok::Op(BinOp::Div),
                        '%' => Tok::Op(BinOp::Rem),
                        '!' => Tok::Not,
                        '=' => return Err(err("`=` is not an operator; use `==`".into(), at)),
                        c => return Err(err(format!("unexpected character `{c}`"), at)),
                    }
                };
                out.push((tok, at));
            }
        }
    }
    out.push((Tok::End, src.len()));
    Ok(out)
}

struct Parser<'a> {
    src: &'a str,
    toks: Vec<(Tok, usize)>,
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> &Tok {
        self.toks.get(self.pos).map_or(&Tok::End, |(t, _)| t)
    }

    fn at(&self) -> usize {
        self.toks
            .get(self.pos)
            .map_or(self.src.len(), |&(_, at)| at)
    }

    fn bump(&mut self) -> Tok {
        let tok = self.peek().clone();
        self.pos += 1;
        tok
    }

    fn error(&self, message: impl Into<String>) -> ParseError {
        ParseError {
            message: message.into(),
            column: self.at() + 1,
            source_text: self.src.to_owned(),
        }
    }

    fn expr(&mut self, min: u8) -> Result<Expr, ParseError> {
        let mut lhs = self.unary()?;
        while let Tok::Op(op) = *self.peek() {
            if op.precedence() < min {
                break;
            }
            self.bump();
            let rhs = self.expr(op.precedence() + 1)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr, ParseError> {
        match self.peek() {
            Tok::Not => {
                self.bump();
                Ok(Expr::Unary(UnOp::Not, Box::new(self.unary()?)))
            }
            Tok::Op(BinOp::Sub) => {
                self.bump();
                Ok(Expr::Unary(UnOp::Neg, Box::new(self.unary()?)))
            }
            _ => self.atom(),
        }
    }

    fn atom(&mut self) -> Result<Expr, ParseError> {
        match self.bump() {
            Tok::Int(n) => Ok(Expr::Lit(Value::Int(n))),
            Tok::Str(s) => Ok(Expr::Lit(Value::Str(s))),
            Tok::Ident(name) if name == "true" => Ok(Expr::Lit(Value::Bool(true))),
            Tok::Ident(name) if name == "false" => Ok(Expr::Lit(Value::Bool(false))),
            Tok::Ident(first) => {
                if *self.peek() == Tok::Dot {
                    self.bump();
                    match self.bump() {
                        Tok::Ident(name) => Ok(Expr::Name {
                            scope: Some(first),
                            name,
                        }),
                        _ => {
                            self.pos -= 1;
                            Err(self.error(format!("expected a name after `{first}.`")))
                        }
                    }
                } else {
                    Ok(Expr::Name {
                        scope: None,
                        name: first,
                    })
                }
            }
            Tok::LParen => {
                let inner = self.expr(0)?;
                if self.bump() != Tok::RParen {
                    self.pos -= 1;
                    return Err(self.error("expected `)`"));
                }
                Ok(inner)
            }
            Tok::End => {
                self.pos -= 1;
                Err(self.error("unexpected end of expression"))
            }
            _ => {
                self.pos -= 1;
                Err(self.error("expected a value, a name or `(`"))
            }
        }
    }
}

/// Parses `src`.
///
/// # Errors
///
/// A [`ParseError`] with the column of the problem.
pub fn parse(src: &str) -> Result<Expr, ParseError> {
    let toks = tokenize(src)?;
    let mut parser = Parser { src, toks, pos: 0 };
    let expr = parser.expr(0)?;
    if *parser.peek() != Tok::End {
        return Err(parser.error("unexpected text after the expression"));
    }
    Ok(expr)
}

fn name_text(scope: Option<&str>, name: &str) -> String {
    scope.map_or_else(|| name.to_owned(), |s| format!("{s}.{name}"))
}

impl Expr {
    /// Evaluates the expression.
    ///
    /// # Errors
    ///
    /// Unknown names, type errors, division by zero and overflow.
    pub fn eval(&self, env: &dyn Env) -> Result<Value, EvalError> {
        match self {
            Self::Lit(v) => Ok(v.clone()),
            Self::Name { scope, name } => env
                .get(scope.as_deref(), name)
                .ok_or_else(|| EvalError::Unknown(name_text(scope.as_deref(), name))),
            Self::Unary(UnOp::Not, e) => match e.eval(env)? {
                Value::Bool(b) => Ok(Value::Bool(!b)),
                v => Err(EvalError::Type(format!("`!` needs a boolean, got {v}"))),
            },
            Self::Unary(UnOp::Neg, e) => match e.eval(env)? {
                Value::Int(n) => n.checked_neg().map(Value::Int).ok_or(EvalError::Overflow),
                v => Err(EvalError::Type(format!("`-` needs an integer, got {v}"))),
            },
            Self::Binary(op @ (BinOp::And | BinOp::Or), a, b) => {
                let Value::Bool(left) = a.eval(env)? else {
                    return Err(EvalError::Type(format!("`{}` needs booleans", op.symbol())));
                };
                if (*op == BinOp::And && !left) || (*op == BinOp::Or && left) {
                    return Ok(Value::Bool(left));
                }
                match b.eval(env)? {
                    Value::Bool(right) => Ok(Value::Bool(right)),
                    _ => Err(EvalError::Type(format!("`{}` needs booleans", op.symbol()))),
                }
            }
            Self::Binary(op, a, b) => binary(*op, a.eval(env)?, b.eval(env)?),
        }
    }

    /// Evaluates to a boolean.
    ///
    /// # Errors
    ///
    /// As [`Expr::eval`], and a type error for another type.
    pub fn eval_bool(&self, env: &dyn Env) -> Result<bool, EvalError> {
        match self.eval(env)? {
            Value::Bool(b) => Ok(b),
            v => Err(EvalError::Type(format!("expected a boolean, got {v}"))),
        }
    }

    /// Evaluates to an integer.
    ///
    /// # Errors
    ///
    /// As [`Expr::eval`], and a type error for another type.
    pub fn eval_int(&self, env: &dyn Env) -> Result<i64, EvalError> {
        match self.eval(env)? {
            Value::Int(n) => Ok(n),
            v => Err(EvalError::Type(format!("expected an integer, got {v}"))),
        }
    }

    /// Every name it reads, as `(scope, name)`, in order, with repeats.
    #[must_use]
    pub fn names(&self) -> Vec<(Option<&str>, &str)> {
        let mut out = Vec::new();
        self.collect_names(&mut out);
        out
    }

    fn collect_names<'a>(&'a self, out: &mut Vec<(Option<&'a str>, &'a str)>) {
        match self {
            Self::Lit(_) => {}
            Self::Name { scope, name } => out.push((scope.as_deref(), name)),
            Self::Unary(_, e) => e.collect_names(out),
            Self::Binary(_, a, b) => {
                a.collect_names(out);
                b.collect_names(out);
            }
        }
    }

    /// The type of the expression given the type of each name.
    ///
    /// # Errors
    ///
    /// A message for an unknown name or a type mismatch.
    pub fn infer(&self, types: &dyn Fn(Option<&str>, &str) -> Option<Ty>) -> Result<Ty, String> {
        match self {
            Self::Lit(v) => Ok(v.ty()),
            Self::Name { scope, name } => types(scope.as_deref(), name)
                .ok_or_else(|| format!("unknown name `{}`", name_text(scope.as_deref(), name))),
            Self::Unary(UnOp::Not, e) => match e.infer(types)? {
                Ty::Bool => Ok(Ty::Bool),
                t => Err(format!("`!` needs a boolean, got {t}")),
            },
            Self::Unary(UnOp::Neg, e) => match e.infer(types)? {
                Ty::Int => Ok(Ty::Int),
                t => Err(format!("`-` needs an integer, got {t}")),
            },
            Self::Binary(op, a, b) => {
                let (ta, tb) = (a.infer(types)?, b.infer(types)?);
                let sym = op.symbol();
                match op {
                    BinOp::And | BinOp::Or if ta == Ty::Bool && tb == Ty::Bool => Ok(Ty::Bool),
                    BinOp::And | BinOp::Or => Err(format!("`{sym}` needs booleans")),
                    BinOp::Eq | BinOp::Ne if ta == tb => Ok(Ty::Bool),
                    BinOp::Eq | BinOp::Ne => Err(format!("`{sym}` compares a {ta} with a {tb}")),
                    BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
                        if ta == Ty::Int && tb == Ty::Int =>
                    {
                        Ok(Ty::Bool)
                    }
                    _ if ta == Ty::Int && tb == Ty::Int => Ok(Ty::Int),
                    _ => Err(format!("`{sym}` needs integers, got {ta} and {tb}")),
                }
            }
        }
    }
}

fn binary(op: BinOp, a: Value, b: Value) -> Result<Value, EvalError> {
    let sym = op.symbol();
    match (op, a, b) {
        (BinOp::Eq, a, b) if a.ty() == b.ty() => Ok(Value::Bool(a == b)),
        (BinOp::Ne, a, b) if a.ty() == b.ty() => Ok(Value::Bool(a != b)),
        (BinOp::Eq | BinOp::Ne, a, b) => Err(EvalError::Type(format!(
            "`{sym}` compares {a} with {b} of another type"
        ))),
        (op, Value::Int(x), Value::Int(y)) => Ok(match op {
            BinOp::Lt => Value::Bool(x < y),
            BinOp::Le => Value::Bool(x <= y),
            BinOp::Gt => Value::Bool(x > y),
            BinOp::Ge => Value::Bool(x >= y),
            BinOp::Add => Value::Int(x.checked_add(y).ok_or(EvalError::Overflow)?),
            BinOp::Sub => Value::Int(x.checked_sub(y).ok_or(EvalError::Overflow)?),
            BinOp::Mul => Value::Int(x.checked_mul(y).ok_or(EvalError::Overflow)?),
            BinOp::Div if y == 0 => return Err(EvalError::DivisionByZero),
            BinOp::Rem if y == 0 => return Err(EvalError::DivisionByZero),
            BinOp::Div => Value::Int(x.checked_div(y).ok_or(EvalError::Overflow)?),
            BinOp::Rem => Value::Int(x.checked_rem(y).ok_or(EvalError::Overflow)?),
            BinOp::Eq | BinOp::Ne | BinOp::And | BinOp::Or => {
                return Err(EvalError::Type(format!("`{sym}` on integers")));
            }
        }),
        (_, a, b) => Err(EvalError::Type(format!(
            "`{sym}` needs integers, got {a} and {b}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    struct Map(BTreeMap<String, Value>);

    impl Env for Map {
        fn get(&self, scope: Option<&str>, name: &str) -> Option<Value> {
            self.0.get(&name_text(scope, name)).cloned()
        }
    }

    fn env() -> Map {
        Map(BTreeMap::from([
            ("inflight".to_owned(), Value::Int(3)),
            ("api.inflight".to_owned(), Value::Int(5)),
            ("api.state".to_owned(), Value::Str("busy".into())),
        ]))
    }

    fn eval(src: &str) -> Result<Value, EvalError> {
        parse(src).unwrap().eval(&env())
    }

    #[test]
    fn precedence_and_associativity() {
        assert_eq!(eval("1 + 2 * 3").unwrap(), Value::Int(7));
        assert_eq!(eval("(1 + 2) * 3").unwrap(), Value::Int(9));
        assert_eq!(eval("10 - 3 - 2").unwrap(), Value::Int(5));
        assert_eq!(eval("7 % 4 / 2").unwrap(), Value::Int(1));
        assert_eq!(eval("-inflight + 1").unwrap(), Value::Int(-2));
        assert_eq!(
            eval("inflight < 4 && api.state == 'busy' || false").unwrap(),
            Value::Bool(true)
        );
        assert_eq!(eval("!(api.inflight <= 4)").unwrap(), Value::Bool(true));
        assert_eq!(eval("api.state != \"idle\"").unwrap(), Value::Bool(true));
    }

    #[test]
    fn short_circuit_skips_the_right_side() {
        assert_eq!(eval("false && nope > 1").unwrap(), Value::Bool(false));
        assert_eq!(eval("true || nope > 1").unwrap(), Value::Bool(true));
        assert_eq!(
            eval("true && nope > 1"),
            Err(EvalError::Unknown("nope".into()))
        );
    }

    #[test]
    fn runtime_errors() {
        assert_eq!(eval("1 / 0"), Err(EvalError::DivisionByZero));
        assert_eq!(eval("1 % 0"), Err(EvalError::DivisionByZero));
        assert_eq!(eval("9223372036854775807 + 1"), Err(EvalError::Overflow));
        assert!(matches!(eval("api.state + 1"), Err(EvalError::Type(_))));
        assert!(matches!(eval("api.state == 1"), Err(EvalError::Type(_))));
        assert!(matches!(eval("!1"), Err(EvalError::Type(_))));
    }

    #[test]
    fn parse_errors_name_the_column() {
        let err = parse("a = 1").unwrap_err();
        assert_eq!(err.column, 3);
        assert!(err.message.contains("=="));
        assert_eq!(parse("(1 + 2").unwrap_err().message, "expected `)`");
        assert_eq!(parse("1 +").unwrap_err().column, 4);
        assert!(parse("a.").is_err());
        assert!(parse("'open").is_err());
        assert!(parse("1 2").is_err());
        assert!(parse("a # b").is_err());
        assert!(parse("99999999999999999999").is_err());
    }

    #[test]
    fn names_and_types() {
        let e = parse("api.inflight + inflight > 2 && state == 'idle'").unwrap();
        assert_eq!(
            e.names(),
            [
                (Some("api"), "inflight"),
                (None, "inflight"),
                (None, "state")
            ]
        );
        let types = |scope: Option<&str>, name: &str| match (scope, name) {
            (_, "state") => Some(Ty::Str),
            (_, "inflight") => Some(Ty::Int),
            _ => None,
        };
        assert_eq!(e.infer(&types), Ok(Ty::Bool));
        assert!(parse("state + 1").unwrap().infer(&types).is_err());
        assert!(parse("inflight && true").unwrap().infer(&types).is_err());
        assert_eq!(
            parse("x > 1").unwrap().infer(&types),
            Err("unknown name `x`".into())
        );
    }
}
