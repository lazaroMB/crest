//! The Karnaugh-map card: Quine–McCluskey prime implicants and a minimal
//! cover, for the ones (sum of products) and for the zeros (product of
//! sums), with don't-cares used in both.
//!
//! Primes come from merging implicants that differ in one cared bit until
//! nothing merges. Essential primes are taken first; the rest of the
//! minterms are covered by Petrick's method (the product of sums of the
//! primes covering each minterm, expanded with absorption), keeping a
//! cover with the fewest terms and then the fewest literals. If the
//! expansion passes [`MAX_PETRICK`] products, the rest is covered
//! greedily and the card says the result may not be minimal. Every cover
//! is checked against the truth table before it is returned.
//!
//! An expression is evaluated on all 2^n assignments to get the minterms.

use std::collections::{BTreeSet, HashSet};

use application::ports::compute::{ComputeError, KarnaughRequest};
use domain::computed::karnaugh::{MAX_VARIABLES, product_term, sum_term};
use domain::computed::{
    Assumption, AssumptionStatus, Computed, ComputedBody, CoverMethod, Implicant, KarnaughBody,
    Narrative, UncertaintyKind, WarningSeverity,
};
use serde_json::Value;

use crate::envelope::{self, count, warning};

/// Products Petrick's expansion may hold before the greedy fallback.
pub const MAX_PETRICK: usize = 2_000;
/// Longest expression accepted.
const MAX_EXPRESSION: usize = 2_000;

fn invalid(text: impl Into<String>) -> ComputeError {
    ComputeError::Invalid(text.into())
}

// --- Expressions ---------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Var(usize),
    Const(bool),
    Not,
    Prime,
    And,
    Or,
    Xor,
    Open,
    Close,
}

#[derive(Debug, Clone, PartialEq)]
enum Expr {
    Var(usize),
    Const(bool),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Xor(Box<Expr>, Box<Expr>),
}

impl Expr {
    fn eval(&self, m: u32, n: usize) -> bool {
        match self {
            Self::Var(i) => m >> (n - 1 - i) & 1 == 1,
            Self::Const(b) => *b,
            Self::Not(e) => !e.eval(m, n),
            Self::And(a, b) => a.eval(m, n) && b.eval(m, n),
            Self::Or(a, b) => a.eval(m, n) || b.eval(m, n),
            Self::Xor(a, b) => a.eval(m, n) ^ b.eval(m, n),
        }
    }
}

/// The variable names of `text`: one letter with optional digits each,
/// sorted.
fn names_in(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut names = BTreeSet::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_alphabetic() {
            let mut name = chars[i].to_string();
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '_') {
                name.push(chars[i]);
                i += 1;
            }
            names.insert(name);
        } else {
            i += 1;
        }
    }
    names.into_iter().collect()
}

fn tokenize(text: &str, variables: &[String]) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = text.chars().collect();
    // Longest names first, so `x10` is not read as `x1` then `0`.
    let mut order: Vec<(usize, Vec<char>)> = variables
        .iter()
        .enumerate()
        .map(|(i, v)| (i, v.chars().collect()))
        .collect();
    order.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if let Some((ix, name)) = order
            .iter()
            .find(|(_, v)| !v.is_empty() && chars[i..].starts_with(v))
        {
            out.push(Token::Var(*ix));
            i += name.len();
            continue;
        }
        let token = match c {
            '0' => Token::Const(false),
            '1' => Token::Const(true),
            '!' | '~' | '¬' => Token::Not,
            '\'' | '’' => Token::Prime,
            '&' | '*' | '·' | '.' | '∧' => Token::And,
            '+' | '|' | '∨' => Token::Or,
            '^' | '⊕' => Token::Xor,
            '(' => Token::Open,
            ')' => Token::Close,
            other => return Err(format!("unexpected `{other}` in the expression")),
        };
        // `&&` and `||` are one operator.
        if matches!(c, '&' | '|') && chars.get(i + 1) == Some(&c) {
            i += 1;
        }
        out.push(token);
        i += 1;
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }
    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.at).cloned();
        self.at += 1;
        t
    }
    fn or(&mut self) -> Result<Expr, String> {
        let mut left = self.xor()?;
        while self.peek() == Some(&Token::Or) {
            self.next();
            left = Expr::Or(Box::new(left), Box::new(self.xor()?));
        }
        Ok(left)
    }
    fn xor(&mut self) -> Result<Expr, String> {
        let mut left = self.and()?;
        while self.peek() == Some(&Token::Xor) {
            self.next();
            left = Expr::Xor(Box::new(left), Box::new(self.and()?));
        }
        Ok(left)
    }
    fn and(&mut self) -> Result<Expr, String> {
        let mut left = self.unary()?;
        loop {
            match self.peek() {
                Some(Token::And) => {
                    self.next();
                }
                // Juxtaposition: `AB`, `A(B + C)`, `A!B`.
                Some(Token::Var(_) | Token::Const(_) | Token::Open | Token::Not) => {}
                _ => break,
            }
            left = Expr::And(Box::new(left), Box::new(self.unary()?));
        }
        Ok(left)
    }
    fn unary(&mut self) -> Result<Expr, String> {
        let mut e = match self.next() {
            Some(Token::Not) => Expr::Not(Box::new(self.unary()?)),
            Some(Token::Var(i)) => Expr::Var(i),
            Some(Token::Const(b)) => Expr::Const(b),
            Some(Token::Open) => {
                let e = self.or()?;
                if self.next() != Some(Token::Close) {
                    return Err("missing `)`".into());
                }
                e
            }
            Some(other) => return Err(format!("unexpected {other:?} in the expression")),
            None => return Err("the expression ends too early".into()),
        };
        while self.peek() == Some(&Token::Prime) {
            self.next();
            e = Expr::Not(Box::new(e));
        }
        Ok(e)
    }
}

/// The minterms of `text` over `variables`.
fn minterms_of(text: &str, variables: &[String]) -> Result<Vec<u32>, String> {
    if text.len() > MAX_EXPRESSION {
        return Err(format!(
            "the expression is longer than {MAX_EXPRESSION} characters"
        ));
    }
    let mut parser = Parser {
        tokens: tokenize(text, variables)?,
        at: 0,
    };
    let expr = parser.or()?;
    if parser.at < parser.tokens.len() {
        return Err("unexpected text after the expression".into());
    }
    let n = variables.len();
    Ok((0..1u32 << n).filter(|m| expr.eval(*m, n)).collect())
}

// --- Quine–McCluskey ---------------------------------------------------------

/// A cube: the cared bits and their values.
type Cube = (u32, u32);

/// The prime implicants of the `on` and `dc` terms of `n` variables.
pub fn primes(on: &[u32], dc: &[u32], n: usize) -> Vec<Cube> {
    let full = (1u32 << n) - 1;
    let mut current: BTreeSet<Cube> = on.iter().chain(dc).map(|m| (full, *m)).collect();
    let mut primes: BTreeSet<Cube> = BTreeSet::new();
    while !current.is_empty() {
        let list: Vec<Cube> = current.iter().copied().collect();
        let mut merged: HashSet<Cube> = HashSet::new();
        let mut next: BTreeSet<Cube> = BTreeSet::new();
        for (i, a) in list.iter().enumerate() {
            for b in &list[i + 1..] {
                if a.0 != b.0 {
                    continue;
                }
                let diff = (a.1 ^ b.1) & a.0;
                if diff.count_ones() == 1 {
                    next.insert((a.0 & !diff, a.1 & !diff));
                    merged.insert(*a);
                    merged.insert(*b);
                }
            }
        }
        for c in &list {
            if !merged.contains(c) {
                primes.insert(*c);
            }
        }
        current = next;
    }
    primes.into_iter().collect()
}

fn covers(cube: Cube, m: u32) -> bool {
    m & cube.0 == cube.1 & cube.0
}

/// A minimal cover of `on` by `primes`: the chosen indices, whether the
/// essential ones, and whether it is exact.
fn cover(primes: &[Cube], on: &[u32]) -> (Vec<usize>, Vec<bool>, bool) {
    let mut chosen: BTreeSet<usize> = BTreeSet::new();
    let mut essential = vec![false; primes.len()];
    for m in on {
        let by: Vec<usize> = (0..primes.len())
            .filter(|p| covers(primes[*p], *m))
            .collect();
        if let [only] = by.as_slice() {
            chosen.insert(*only);
            essential[*only] = true;
        }
    }
    let rest: Vec<u32> = on
        .iter()
        .copied()
        .filter(|m| !chosen.iter().any(|p| covers(primes[*p], *m)))
        .collect();
    if rest.is_empty() {
        return (chosen.into_iter().collect(), essential, true);
    }
    let literals =
        |set: &BTreeSet<usize>| -> u32 { set.iter().map(|p| primes[*p].0.count_ones()).sum() };
    // Petrick: expand Π_m (Σ_{p covers m} p) with absorption.
    let mut products: Vec<BTreeSet<usize>> = vec![BTreeSet::new()];
    let mut exact = true;
    for m in &rest {
        let by: Vec<usize> = (0..primes.len())
            .filter(|p| covers(primes[*p], *m))
            .collect();
        let mut next: Vec<BTreeSet<usize>> = Vec::new();
        for product in &products {
            if by.iter().any(|p| product.contains(p)) {
                next.push(product.clone());
                continue;
            }
            for p in &by {
                let mut grown = product.clone();
                grown.insert(*p);
                next.push(grown);
            }
        }
        if next.len() > 4 * MAX_PETRICK {
            exact = false;
            break;
        }
        // Absorption: drop products that contain another.
        next.sort_by_key(BTreeSet::len);
        next.dedup();
        let mut kept: Vec<BTreeSet<usize>> = Vec::new();
        for product in next {
            if !kept.iter().any(|k| k.is_subset(&product)) {
                kept.push(product);
            }
        }
        products = kept;
        if products.len() > MAX_PETRICK {
            exact = false;
            break;
        }
    }
    let extra = if exact {
        products
            .into_iter()
            .min_by(|a, b| {
                a.len()
                    .cmp(&b.len())
                    .then(literals(a).cmp(&literals(b)))
                    .then(a.cmp(b))
            })
            .unwrap_or_default()
    } else {
        // Greedy: the prime covering most uncovered minterms, fewer
        // literals on ties.
        let mut uncovered: BTreeSet<u32> = rest.iter().copied().collect();
        let mut picked = BTreeSet::new();
        while !uncovered.is_empty() {
            let best = (0..primes.len()).max_by(|a, b| {
                let ca = uncovered.iter().filter(|m| covers(primes[*a], **m)).count();
                let cb = uncovered.iter().filter(|m| covers(primes[*b], **m)).count();
                ca.cmp(&cb)
                    .then(primes[*b].0.count_ones().cmp(&primes[*a].0.count_ones()))
                    .then(b.cmp(a))
            });
            let Some(best) = best else { break };
            uncovered.retain(|m| !covers(primes[best], *m));
            picked.insert(best);
        }
        picked
    };
    chosen.extend(extra);
    (chosen.into_iter().collect(), essential, exact)
}

fn sorted_unique(v: &[u32]) -> Vec<u32> {
    let set: BTreeSet<u32> = v.iter().copied().collect();
    set.into_iter().collect()
}

/// Runs a Karnaugh request.
pub fn run(
    request: &KarnaughRequest,
    id: String,
) -> Result<(Computed, ComputedBody), ComputeError> {
    let given = [
        !request.minterms.is_empty(),
        !request.maxterms.is_empty(),
        request.expression.is_some(),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    if given > 1 {
        return Err(invalid(
            "give one of `minterms`, `maxterms` or `expression`",
        ));
    }
    let dc = sorted_unique(&request.dont_cares);
    let variables: Vec<String> = if !request.variables.is_empty() {
        request.variables.clone()
    } else if let Some(text) = &request.expression {
        names_in(text)
    } else {
        let top = request
            .minterms
            .iter()
            .chain(&request.maxterms)
            .chain(&dc)
            .max()
            .copied()
            .unwrap_or(1);
        let bits = (32 - top.leading_zeros()).max(2) as usize;
        ["A", "B", "C", "D", "E", "F"]
            .iter()
            .take(bits)
            .map(|s| (*s).to_owned())
            .collect()
    };
    let n = variables.len();
    if !(1..=MAX_VARIABLES).contains(&n) {
        return Err(invalid(format!(
            "a Karnaugh map takes 1 to {MAX_VARIABLES} variables, got {n}"
        )));
    }
    if variables.iter().collect::<HashSet<_>>().len() != n {
        return Err(invalid("variable names must be distinct"));
    }
    let size = 1u32 << n;
    let in_range = |v: &[u32]| v.iter().all(|m| *m < size);
    if !(in_range(&request.minterms) && in_range(&request.maxterms) && in_range(&dc)) {
        return Err(invalid(format!(
            "terms must be below {size} for {n} variables"
        )));
    }
    let on: Vec<u32> = match &request.expression {
        Some(text) => minterms_of(text, &variables)
            .map_err(|e| invalid(format!("expression: {e}")))?
            .into_iter()
            .filter(|m| dc.binary_search(m).is_err())
            .collect(),
        None if !request.maxterms.is_empty() => {
            let zeros = sorted_unique(&request.maxterms);
            (0..size)
                .filter(|m| zeros.binary_search(m).is_err() && dc.binary_search(m).is_err())
                .collect()
        }
        None => sorted_unique(&request.minterms),
    };
    if on.iter().any(|m| dc.binary_search(m).is_ok()) {
        return Err(invalid("a term cannot be both a minterm and a don't-care"));
    }
    let off: Vec<u32> = (0..size)
        .filter(|m| on.binary_search(m).is_err() && dc.binary_search(m).is_err())
        .collect();
    // Ones: sum of products.
    let sop_primes = primes(&on, &dc, n);
    let (sop_pick, sop_essential, sop_exact) = cover(&sop_primes, &on);
    // Zeros: product of sums.
    let pos_primes = primes(&off, &dc, n);
    let (pos_pick, pos_essential, pos_exact) = cover(&pos_primes, &off);
    let cover_of = |m: u32, cube: Cube| covers(cube, m);
    let implicant = |cube: Cube, essential: bool, terms: &[u32], sum: bool| Implicant {
        care: cube.0,
        value: cube.1 & cube.0,
        term: if sum {
            sum_term(cube.0, cube.1, &variables)
        } else {
            product_term(cube.0, cube.1, &variables)
        },
        covers: terms
            .iter()
            .copied()
            .filter(|m| cover_of(*m, cube))
            .collect(),
        essential,
    };
    let all: Vec<u32> = (0..size).collect();
    let sop: Vec<Implicant> = if on.is_empty() {
        Vec::new()
    } else {
        sop_pick
            .iter()
            .map(|p| implicant(sop_primes[*p], sop_essential[*p], &all, false))
            .collect()
    };
    let pos: Vec<Implicant> = if off.is_empty() {
        Vec::new()
    } else {
        pos_pick
            .iter()
            .map(|p| implicant(pos_primes[*p], pos_essential[*p], &all, true))
            .collect()
    };
    let sop_expression = if sop.is_empty() {
        "0".to_owned()
    } else {
        sop.iter()
            .map(|i| i.term.as_str())
            .collect::<Vec<_>>()
            .join(" + ")
    };
    let pos_expression = if pos.is_empty() {
        "1".to_owned()
    } else {
        pos.iter().map(|i| i.term.as_str()).collect::<String>()
    };
    // Plug back on the whole truth table.
    let sop_ok = on
        .iter()
        .all(|m| sop.iter().any(|i| cover_of(*m, (i.care, i.value))))
        && off
            .iter()
            .all(|m| !sop.iter().any(|i| cover_of(*m, (i.care, i.value))));
    let pos_ok = off
        .iter()
        .all(|m| pos.iter().any(|i| cover_of(*m, (i.care, i.value))))
        && on
            .iter()
            .all(|m| !pos.iter().any(|i| cover_of(*m, (i.care, i.value))));
    if !(sop_ok && pos_ok) {
        return Err(ComputeError::Invalid(
            "internal check failed: the cover does not reproduce the truth table".into(),
        ));
    }
    let exact = sop_exact && pos_exact;
    let mut engine = envelope::solve_engine();
    engine.backend = Some("crest Quine–McCluskey + Petrick".into());
    let mut computed = Computed::new(id, "solve.karnaugh", "Karnaugh map", engine);
    computed.uncertainty_kind = Some(UncertaintyKind::Exact);
    computed.inputs.push(domain::computed::InputRef {
        name: "truth table".into(),
        role: Some("data".into()),
        path: None,
        hash: Some(envelope::hash(
            &serde_json::to_vec(request).unwrap_or_default(),
        )),
        rows: Some(u64::from(size)),
        columns: variables.clone(),
        filter: None,
    });
    let literals = |v: &[Implicant]| v.iter().map(Implicant::literals).sum::<u32>();
    let values = &mut computed.values;
    values.insert("ones".into(), count(on.len() as u64));
    values.insert("dont_cares".into(), count(dc.len() as u64));
    values.insert("prime_implicants".into(), count(sop_primes.len() as u64));
    values.insert("sop_terms".into(), count(sop.len() as u64));
    values.insert("sop_literals".into(), count(u64::from(literals(&sop))));
    values.insert("pos_terms".into(), count(pos.len() as u64));
    values.insert("pos_literals".into(), count(u64::from(literals(&pos))));
    let params = &mut computed.method.params;
    params.insert("sop".into(), Value::from(sop_expression.clone()));
    params.insert("pos".into(), Value::from(pos_expression.clone()));
    computed.method.why = Some(if exact {
        "prime implicants by Quine–McCluskey, essential primes first, the rest by Petrick's \
         method (fewest terms, then fewest literals)"
            .into()
    } else {
        "prime implicants by Quine–McCluskey, essential primes first, the rest greedily".into()
    });
    computed.method.alternatives = Vec::new();
    if !exact {
        computed.warnings.push(warning(
            "greedy_cover",
            WarningSeverity::Warn,
            format!(
                "Petrick's expansion passed {MAX_PETRICK} products, so the remaining minterms \
                 were covered greedily: the expression is correct but may not be minimal"
            ),
        ));
    }
    computed.assumptions.push(Assumption {
        id: "plug_back".into(),
        check: Some("truth table".into()),
        stat: None,
        p: None,
        status: AssumptionStatus::Pass,
        consequence: None,
        note: Some(format!(
            "both expressions give F on all {size} rows (don't-cares free)"
        )),
    });
    computed.narrative = Some(Narrative {
        template: "karnaugh".into(),
        text_slots: vec!["sop_terms".into(), "sop_literals".into()],
    });
    let prime_implicants = sop_primes
        .iter()
        .enumerate()
        .map(|(i, c)| implicant(*c, sop_essential[i], &all, false))
        .collect();
    let body = KarnaughBody {
        variables,
        minterms: on,
        dont_cares: dc,
        prime_implicants,
        sop,
        sop_expression,
        pos,
        pos_expression,
        method: if exact {
            CoverMethod::Exact
        } else {
            CoverMethod::Greedy
        },
    };
    Ok((computed, ComputedBody::Karnaugh(body)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn expressions_parse_with_primes_juxtaposition_and_precedence() {
        let v = vars(&["A", "B", "C"]);
        assert_eq!(minterms_of("AB' + C", &v), Ok(vec![1, 3, 4, 5, 7]));
        assert_eq!(minterms_of("!(A | B) & C", &v), Ok(vec![1]));
        assert_eq!(minterms_of("A ^ B", &v), Ok(vec![2, 3, 4, 5]));
        assert_eq!(minterms_of("A(B + C)'", &v), Ok(vec![4]));
        assert_eq!(minterms_of("A && B || 0", &v), Ok(vec![6, 7]));
        assert!(minterms_of("A +", &v).is_err());
        assert!(minterms_of("A $ B", &v).is_err());
        assert_eq!(names_in("x1x2' + x10"), vars(&["x1", "x10", "x2"]));
    }

    #[test]
    fn primes_of_a_textbook_function() {
        // f(A,B,C) = Σm(0,1,2,5,6,7): six primes, no essential one.
        let p = primes(&[0, 1, 2, 5, 6, 7], &[], 3);
        assert_eq!(p.len(), 6);
        let (pick, essential, exact) = cover(&p, &[0, 1, 2, 5, 6, 7]);
        assert_eq!(pick.len(), 3);
        assert!(exact);
        assert!(essential.iter().all(|e| !e));
    }
}
