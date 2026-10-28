//! The expressions of a plot: a small recursive-descent parser and an
//! evaluator, shared by `show_plot` (which checks what the model wrote, so
//! it can fix a typo) and the UI (which evaluates the functions as sliders
//! move).
//!
//! The grammar is ordinary calculator notation:
//!
//! - numbers (`2`, `0.5`, `1e-3`), the constants `pi` (or `π`) and `e`, and
//!   the variables the caller allows (`x`, `t`, `y`, parameter names);
//! - `+ - * /`, `^` (or `**`) for powers, right-associative and binding
//!   tighter than unary minus (`-x^2` is `-(x^2)`, `2^-x` works);
//! - implicit multiplication between a factor and a number, name or
//!   parenthesis (`2x`, `a sin(x)`, `(x + 1)(x - 1)`);
//! - functions of one argument (`sin cos tan asin acos atan sinh cosh tanh
//!   exp ln log log10 sqrt abs floor ceil`, with `log` the natural log) and
//!   of two (`atan2(y, x) min max pow`).
//!
//! Evaluation never fails: a domain error gives NaN, as `f64` does, and the
//! plot leaves a gap there.

use std::fmt;

use thiserror::Error;

/// Functions of one argument.
pub const UNARY_FUNCTIONS: [&str; 17] = [
    "sin", "cos", "tan", "asin", "acos", "atan", "sinh", "cosh", "tanh", "exp", "ln", "log",
    "log10", "sqrt", "abs", "floor", "ceil",
];

/// Functions of two arguments.
pub const BINARY_FUNCTIONS: [&str; 4] = ["atan2", "min", "max", "pow"];

/// Named constants.
pub const CONSTANTS: [&str; 2] = ["pi", "e"];

/// Whether `name` is a function or constant, so it cannot be a variable.
#[must_use]
pub fn is_builtin(name: &str) -> bool {
    UNARY_FUNCTIONS.contains(&name) || BINARY_FUNCTIONS.contains(&name) || CONSTANTS.contains(&name)
}

/// Why an expression does not parse.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message} (column {column})")]
pub struct ExprError {
    /// 1-based character column where the problem is.
    pub column: usize,
    /// What is wrong.
    pub message: String,
}

/// A parsed expression over a fixed list of variables.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    root: Node,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unary {
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Sinh,
    Cosh,
    Tanh,
    Exp,
    Ln,
    Log10,
    Sqrt,
    Abs,
    Floor,
    Ceil,
}

impl Unary {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "sin" => Self::Sin,
            "cos" => Self::Cos,
            "tan" => Self::Tan,
            "asin" => Self::Asin,
            "acos" => Self::Acos,
            "atan" => Self::Atan,
            "sinh" => Self::Sinh,
            "cosh" => Self::Cosh,
            "tanh" => Self::Tanh,
            "exp" => Self::Exp,
            "ln" | "log" => Self::Ln,
            "log10" => Self::Log10,
            "sqrt" => Self::Sqrt,
            "abs" => Self::Abs,
            "floor" => Self::Floor,
            "ceil" => Self::Ceil,
            _ => return None,
        })
    }

    fn apply(self, v: f64) -> f64 {
        match self {
            Self::Sin => v.sin(),
            Self::Cos => v.cos(),
            Self::Tan => v.tan(),
            Self::Asin => v.asin(),
            Self::Acos => v.acos(),
            Self::Atan => v.atan(),
            Self::Sinh => v.sinh(),
            Self::Cosh => v.cosh(),
            Self::Tanh => v.tanh(),
            Self::Exp => v.exp(),
            Self::Ln => v.ln(),
            Self::Log10 => v.log10(),
            Self::Sqrt => v.sqrt(),
            Self::Abs => v.abs(),
            Self::Floor => v.floor(),
            Self::Ceil => v.ceil(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Binary {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Atan2,
    Min,
    Max,
}

impl Binary {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "atan2" => Self::Atan2,
            "min" => Self::Min,
            "max" => Self::Max,
            "pow" => Self::Pow,
            _ => return None,
        })
    }

    fn apply(self, a: f64, b: f64) -> f64 {
        match self {
            Self::Add => a + b,
            Self::Sub => a - b,
            Self::Mul => a * b,
            Self::Div => a / b,
            Self::Pow => a.powf(b),
            Self::Atan2 => a.atan2(b),
            Self::Min => a.min(b),
            Self::Max => a.max(b),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Number(f64),
    Variable(usize),
    Negate(Box<Node>),
    Unary(Unary, Box<Node>),
    Binary(Binary, Box<Node>, Box<Node>),
}

impl Node {
    fn eval(&self, vars: &[f64]) -> f64 {
        match self {
            Self::Number(n) => *n,
            Self::Variable(i) => vars.get(*i).copied().unwrap_or(f64::NAN),
            Self::Negate(a) => -a.eval(vars),
            Self::Unary(f, a) => f.apply(a.eval(vars)),
            Self::Binary(op, a, b) => op.apply(a.eval(vars), b.eval(vars)),
        }
    }
}

impl Expr {
    /// Parses `source`, where the names in `variables` may appear; the
    /// value of `variables[i]` is `values[i]` in [`Expr::eval`].
    ///
    /// # Errors
    ///
    /// [`ExprError`] at the first problem: an unknown name or function, a
    /// wrong number of arguments, a missing parenthesis, a stray character.
    pub fn parse(source: &str, variables: &[&str]) -> Result<Self, ExprError> {
        let tokens = tokenize(source)?;
        let end = source.chars().count() + 1;
        let mut parser = Parser {
            tokens,
            pos: 0,
            variables,
            end,
        };
        let root = parser.sum()?;
        if let Some((token, column)) = parser.tokens.get(parser.pos) {
            return Err(ExprError {
                column: *column,
                message: format!("unexpected {token}"),
            });
        }
        Ok(Self { root })
    }

    /// The value with `values` for the variables, in the order given to
    /// [`Expr::parse`]; a missing one reads as NaN.
    #[must_use]
    pub fn eval(&self, values: &[f64]) -> f64 {
        self.root.eval(values)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Name(String),
    Symbol(char),
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(n) => write!(f, "number {n}"),
            Self::Name(name) => write!(f, "name {name:?}"),
            Self::Symbol(c) => write!(f, "'{c}'"),
        }
    }
}

/// The tokens of `source` with their 1-based columns.
fn tokenize(source: &str) -> Result<Vec<(Token, usize)>, ExprError> {
    let chars: Vec<char> = source.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        let column = i + 1;
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit()
            || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit))
        {
            let start = i;
            while chars
                .get(i)
                .is_some_and(|c| c.is_ascii_digit() || *c == '.')
            {
                i += 1;
            }
            // An exponent only when digits follow, so `2e` is 2 times e.
            if chars.get(i).is_some_and(|c| *c == 'e' || *c == 'E') {
                let sign = usize::from(chars.get(i + 1).is_some_and(|c| *c == '+' || *c == '-'));
                if chars.get(i + 1 + sign).is_some_and(char::is_ascii_digit) {
                    i += 1 + sign;
                    while chars.get(i).is_some_and(char::is_ascii_digit) {
                        i += 1;
                    }
                }
            }
            let text: String = chars[start..i].iter().collect();
            let n = text.parse::<f64>().map_err(|_| ExprError {
                column,
                message: format!("bad number {text:?}"),
            })?;
            tokens.push((Token::Number(n), column));
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while chars
                .get(i)
                .is_some_and(|c| c.is_alphanumeric() || *c == '_')
            {
                i += 1;
            }
            let name: String = chars[start..i].iter().collect();
            let name = if name == "π" { "pi".to_owned() } else { name };
            tokens.push((Token::Name(name), column));
        } else {
            let symbol = match c {
                '*' if chars.get(i + 1) == Some(&'*') => {
                    i += 1;
                    '^'
                }
                '+' | '-' | '*' | '/' | '^' | '(' | ')' | ',' => c,
                '−' => '-',
                '×' | '·' => '*',
                '÷' => '/',
                _ => {
                    return Err(ExprError {
                        column,
                        message: format!("unexpected character '{c}'"),
                    });
                }
            };
            i += 1;
            tokens.push((Token::Symbol(symbol), column));
        }
    }
    Ok(tokens)
}

struct Parser<'a> {
    tokens: Vec<(Token, usize)>,
    pos: usize,
    variables: &'a [&'a str],
    /// Column just past the end, for "unexpected end" errors.
    end: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos).map(|(t, _)| t)
    }

    fn column(&self) -> usize {
        self.tokens.get(self.pos).map_or(self.end, |(_, c)| *c)
    }

    fn peek_symbol(&self, symbol: char) -> bool {
        self.peek() == Some(&Token::Symbol(symbol))
    }

    fn error(&self, message: impl Into<String>) -> ExprError {
        ExprError {
            column: self.column(),
            message: message.into(),
        }
    }

    fn expect(&mut self, symbol: char) -> Result<(), ExprError> {
        if self.peek_symbol(symbol) {
            self.pos += 1;
            Ok(())
        } else {
            let found = self
                .peek()
                .map_or_else(|| "the end".to_owned(), ToString::to_string);
            Err(self.error(format!("expected '{symbol}', found {found}")))
        }
    }

    /// `product (('+' | '-') product)*`
    fn sum(&mut self) -> Result<Node, ExprError> {
        let mut lhs = self.product()?;
        loop {
            let op = if self.peek_symbol('+') {
                Binary::Add
            } else if self.peek_symbol('-') {
                Binary::Sub
            } else {
                return Ok(lhs);
            };
            self.pos += 1;
            let rhs = self.product()?;
            lhs = Node::Binary(op, Box::new(lhs), Box::new(rhs));
        }
    }

    /// `unary (('*' | '/')? unary)*`, the bare form being implicit
    /// multiplication.
    fn product(&mut self) -> Result<Node, ExprError> {
        let mut lhs = self.unary()?;
        loop {
            let op = if self.peek_symbol('*') {
                self.pos += 1;
                Binary::Mul
            } else if self.peek_symbol('/') {
                self.pos += 1;
                Binary::Div
            } else if matches!(
                self.peek(),
                Some(Token::Number(_) | Token::Name(_) | Token::Symbol('('))
            ) {
                Binary::Mul
            } else {
                return Ok(lhs);
            };
            let rhs = self.unary()?;
            lhs = Node::Binary(op, Box::new(lhs), Box::new(rhs));
        }
    }

    /// `('-' | '+') unary | power`
    fn unary(&mut self) -> Result<Node, ExprError> {
        if self.peek_symbol('-') {
            self.pos += 1;
            return Ok(Node::Negate(Box::new(self.unary()?)));
        }
        if self.peek_symbol('+') {
            self.pos += 1;
            return self.unary();
        }
        self.power()
    }

    /// `primary ('^' unary)?`, so powers chain to the right.
    fn power(&mut self) -> Result<Node, ExprError> {
        let base = self.primary()?;
        if self.peek_symbol('^') {
            self.pos += 1;
            let exponent = self.unary()?;
            return Ok(Node::Binary(
                Binary::Pow,
                Box::new(base),
                Box::new(exponent),
            ));
        }
        Ok(base)
    }

    /// A number, a variable, a constant, a call or a parenthesized sum.
    fn primary(&mut self) -> Result<Node, ExprError> {
        let column = self.column();
        let Some((token, _)) = self.tokens.get(self.pos).cloned() else {
            return Err(self.error("unexpected end of the expression"));
        };
        self.pos += 1;
        match token {
            Token::Number(n) => Ok(Node::Number(n)),
            Token::Symbol('(') => {
                let inner = self.sum()?;
                self.expect(')')?;
                Ok(inner)
            }
            Token::Symbol(c) => Err(ExprError {
                column,
                message: format!("expected a number, a name or '(', found '{c}'"),
            }),
            Token::Name(name) => self.name(&name, column),
        }
    }

    fn name(&mut self, name: &str, column: usize) -> Result<Node, ExprError> {
        let call = self.peek_symbol('(');
        let is_function = UNARY_FUNCTIONS.contains(&name) || BINARY_FUNCTIONS.contains(&name);
        if is_function && !call {
            return Err(ExprError {
                column,
                message: format!("function {name} needs parentheses, as in {name}(x)"),
            });
        }
        if is_function {
            self.pos += 1;
            let mut args = vec![self.sum()?];
            while self.peek_symbol(',') {
                self.pos += 1;
                args.push(self.sum()?);
            }
            self.expect(')')?;
            let wanted = if UNARY_FUNCTIONS.contains(&name) {
                1
            } else {
                2
            };
            if args.len() != wanted {
                return Err(ExprError {
                    column,
                    message: format!(
                        "{name} takes {wanted} argument{}, got {}",
                        if wanted == 1 { "" } else { "s" },
                        args.len()
                    ),
                });
            }
            let mut args = args.into_iter().map(Box::new);
            return match (
                Unary::from_name(name),
                Binary::from_name(name),
                args.next(),
                args.next(),
            ) {
                (Some(f), _, Some(a), None) => Ok(Node::Unary(f, a)),
                (_, Some(op), Some(a), Some(b)) => Ok(Node::Binary(op, a, b)),
                _ => Err(ExprError {
                    column,
                    message: format!("bad call of {name}"),
                }),
            };
        }
        if let Some(index) = self.variables.iter().position(|v| *v == name) {
            return Ok(Node::Variable(index));
        }
        match name {
            "pi" => return Ok(Node::Number(std::f64::consts::PI)),
            "e" => return Ok(Node::Number(std::f64::consts::E)),
            _ => {}
        }
        let known = if self.variables.is_empty() {
            "no variables here".to_owned()
        } else {
            format!("variables here: {}", self.variables.join(", "))
        };
        let what = if call { "function" } else { "name" };
        Err(ExprError {
            column,
            message: format!("unknown {what} {name:?} ({known})"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(source: &str, x: f64) -> f64 {
        Expr::parse(source, &["x"])
            .unwrap_or_else(|e| panic!("{source}: {e}"))
            .eval(&[x])
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-12
    }

    #[test]
    fn precedence_and_associativity() {
        assert_eq!(eval("1 + 2 * 3", 0.0), 7.0);
        assert_eq!(eval("(1 + 2) * 3", 0.0), 9.0);
        assert_eq!(eval("8 / 4 / 2", 0.0), 1.0);
        assert_eq!(eval("10 - 4 - 3", 0.0), 3.0);
        assert_eq!(eval("2 ^ 3 ^ 2", 0.0), 512.0);
        assert_eq!(eval("2 ** 3", 0.0), 8.0);
        assert_eq!(eval("2 * x ^ 2", 3.0), 18.0);
    }

    #[test]
    fn unary_minus_binds_looser_than_powers() {
        assert_eq!(eval("-x^2", 3.0), -9.0);
        assert_eq!(eval("(-x)^2", 3.0), 9.0);
        assert_eq!(eval("2^-1", 0.0), 0.5);
        assert_eq!(eval("--x", 2.0), 2.0);
        assert_eq!(eval("3 - -x", 2.0), 5.0);
        assert_eq!(eval("+x", 2.0), 2.0);
        assert_eq!(eval("−x", 2.0), -2.0);
    }

    #[test]
    fn implicit_multiplication() {
        assert_eq!(eval("2x", 3.0), 6.0);
        assert_eq!(eval("2 x + 1", 3.0), 7.0);
        assert_eq!(eval("(x + 1)(x - 1)", 3.0), 8.0);
        assert!(close(eval("2pi", 0.0), 2.0 * std::f64::consts::PI));
        assert!(close(eval("2e", 0.0), 2.0 * std::f64::consts::E));
        assert_eq!(eval("2e3", 0.0), 2000.0);
        assert_eq!(eval("1.5E-1", 0.0), 0.15);
        assert_eq!(eval("x 2 ^ 2", 3.0), 12.0);
    }

    #[test]
    fn functions_and_constants() {
        assert!(close(eval("sin(pi / 2)", 0.0), 1.0));
        assert!(close(eval("cos(π)", 0.0), -1.0));
        assert!(close(eval("ln(e)", 0.0), 1.0));
        assert!(close(eval("log(e^2)", 0.0), 2.0));
        assert!(close(eval("log10(1000)", 0.0), 3.0));
        assert_eq!(eval("sqrt(16) + abs(-2)", 0.0), 6.0);
        assert_eq!(eval("floor(2.7) + ceil(2.1)", 0.0), 5.0);
        assert_eq!(eval("min(x, 2) + max(x, 2)", 5.0), 7.0);
        assert_eq!(eval("pow(2, 10)", 0.0), 1024.0);
        assert!(close(eval("atan2(1, 1)", 0.0), std::f64::consts::FRAC_PI_4));
        assert!(close(eval("tanh(0) + sinh(0) + cosh(0)", 0.0), 1.0));
        assert!(close(eval("asin(1) - acos(0) + atan(0)", 0.0), 0.0));
        assert!(eval("sqrt(x)", -1.0).is_nan());
        assert!(eval("1 / x", 0.0).is_infinite());
    }

    #[test]
    fn variables_follow_the_given_order() {
        let expr = Expr::parse("a * exp(-k * x) + y", &["x", "y", "a", "k"]).unwrap();
        assert!(close(expr.eval(&[0.0, 1.0, 2.0, 0.5]), 3.0));
        // A variable shadows a constant of the same name.
        let shadow = Expr::parse("e", &["e"]).unwrap();
        assert_eq!(shadow.eval(&[4.0]), 4.0);
        assert!(shadow.eval(&[]).is_nan());
    }

    fn error(source: &str) -> ExprError {
        match Expr::parse(source, &["x", "a"]) {
            Err(e) => e,
            Ok(expr) => panic!("{source} parsed as {expr:?}"),
        }
    }

    #[test]
    fn unknown_names_and_functions_are_errors() {
        let e = error("a * z");
        assert_eq!(e.column, 5);
        assert_eq!(e.message, r#"unknown name "z" (variables here: x, a)"#);
        assert_eq!(
            e.to_string(),
            r#"unknown name "z" (variables here: x, a) (column 5)"#
        );
        assert_eq!(
            error("sinx").message,
            r#"unknown name "sinx" (variables here: x, a)"#
        );
        assert_eq!(
            error("foo(x)").message,
            r#"unknown function "foo" (variables here: x, a)"#
        );
        assert_eq!(
            error("sin x").message,
            "function sin needs parentheses, as in sin(x)"
        );
        assert_eq!(error("pow(x)").message, "pow takes 2 arguments, got 1");
        assert_eq!(error("sin(x, 1)").message, "sin takes 1 argument, got 2");
    }

    #[test]
    fn syntax_errors_name_the_column() {
        assert_eq!(error("(x + 1").message, "expected ')', found the end");
        assert_eq!(error("(x + 1").column, 7);
        assert_eq!(error("x +").message, "unexpected end of the expression");
        assert_eq!(error("x + * 2").column, 5);
        assert_eq!(error("x $ 2").message, "unexpected character '$'");
        assert_eq!(error("x)").message, "unexpected ')'");
        assert_eq!(error("").message, "unexpected end of the expression");
    }

    #[test]
    fn builtins_are_reserved() {
        assert!(is_builtin("sin"));
        assert!(is_builtin("pi"));
        assert!(!is_builtin("a"));
    }
}
