//! Pure heuristic that rejects problem statements that cannot be measured.
//!
//! A statement is measurable when it is one sentence and has at least one
//! of:
//!
//! - a **magnitude**: a number with a unit (`500 ms`, `3 GB`, `20 %`,
//!   `500 requests per second`);
//! - a **comparison**: a comparator next to a number (`more than 3`,
//!   `< 20`, `rises from 10 to 500`);
//! - a **verifiable condition**: a trigger (`when`, `if`, `after`…) and an
//!   observable outcome (`fails`, `returns`, `panics`…), as in "when
//!   running `cargo test` the `offline` test fails".
//!
//! "at 500 requests per second the response time rises from 10 ms to
//! 500 ms" passes; "the response time depends on the load" does not.
//! The words are English and Spanish, so statements in either language
//! are judged. The model judges the statements that pass as a second
//! opinion.

use std::fmt;

/// Why a statement counts as measurable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Measure {
    /// A number with a unit.
    Magnitude,
    /// A comparator next to a number.
    Comparison,
    /// A trigger and an observable outcome.
    Condition,
}

/// Why a statement is not measurable, written for the model and the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotMeasurable {
    /// What is missing.
    pub reason: String,
}

impl fmt::Display for NotMeasurable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for NotMeasurable {}

/// Units that may follow a number (lowercase, without accents removed).
const UNITS: &[&str] = &[
    // Time.
    "ns",
    "µs",
    "us",
    "ms",
    "s",
    "seg",
    "segundo",
    "segundos",
    "min",
    "minuto",
    "minutos",
    "h",
    "hora",
    "horas",
    "día",
    "días",
    "dia",
    "dias",
    "sec",
    "secs",
    "second",
    "seconds",
    "mins",
    "minute",
    "minutes",
    "hour",
    "hours",
    "day",
    "days",
    // Size.
    "b",
    "kb",
    "mb",
    "gb",
    "tb",
    "kib",
    "mib",
    "gib",
    "byte",
    "bytes",
    // Rates and counts.
    "%",
    "x",
    "rps",
    "qps",
    "rpm",
    "fps",
    "hz",
    "request",
    "requests",
    "peticiones",
    "petición",
    "solicitudes",
    "consultas",
    "veces",
    "usuarios",
    "conexiones",
    "hilos",
    "threads",
    "archivos",
    "ficheros",
    "líneas",
    "lineas",
    "filas",
    "registros",
    "elementos",
    "items",
    "ítems",
    "tokens",
    "errores",
    "fallos",
    "reintentos",
    "intentos",
    "llamadas",
    "commits",
    "tests",
    "pruebas",
    "núcleos",
    "cores",
    "mensajes",
    "eventos",
    "sesiones",
    "clics",
    "pasos",
    "caracteres",
    "procesos",
    "nodos",
    "páginas",
    "iteraciones",
    "queries",
    "times",
    "users",
    "connections",
    "files",
    "lines",
    "rows",
    "records",
    "elements",
    "errors",
    "failures",
    "retries",
    "attempts",
    "calls",
    "messages",
    "events",
    "sessions",
    "clicks",
    "steps",
    "characters",
    "processes",
    "nodes",
    "pages",
    "iterations",
];

/// Comparator phrases that make a number a threshold.
const COMPARATORS: &[&str] = &[
    "más de",
    "mas de",
    "menos de",
    "al menos",
    "como máximo",
    "como mínimo",
    "no más de",
    "hasta",
    "por encima de",
    "por debajo de",
    "mayor que",
    "menor que",
    "mayor a",
    "menor a",
    "supera",
    "superior a",
    "inferior a",
    "igual a",
    "sube de",
    "baja de",
    "pasa de",
    "crece de",
    "cae de",
    "more than",
    "less than",
    "fewer than",
    "at least",
    "at most",
    "no more than",
    "up to",
    "above",
    "below",
    "greater than",
    "exceeds",
    "equal to",
    "rises from",
    "drops from",
    "goes from",
    "grows from",
    "falls from",
];

/// Comparator symbols.
const SYMBOLS: &[char] = &['<', '>', '≤', '≥', '='];

/// Words that open a condition.
const TRIGGERS: &[&str] = &[
    "al", "cuando", "si", "tras", "después", "despues", "ante", "mientras", "cada", "when",
    "whenever", "if", "after", "upon", "while", "every", "each",
];

/// Stems of observable outcomes.
const OUTCOMES: &[&str] = &[
    "falla",
    "fallan",
    "falló",
    "devuelve",
    "devuelven",
    "retorna",
    "responde",
    "lanza",
    "muestra",
    "imprime",
    "termina",
    "aborta",
    "pánico",
    "panic",
    "cuelga",
    "bloquea",
    "compila",
    "rechaza",
    "pierde",
    "duplica",
    "corrompe",
    "crashea",
    "cierra",
    "error",
    "excepción",
    "timeout",
    "deadlock",
    "ignora",
    "omite",
    "borra",
    "sobrescribe",
    "fail",
    "return",
    "throw",
    "raise",
    "show",
    "print",
    "exit",
    "crash",
    "hang",
    "reject",
    "lose",
    "loses",
    "duplicate",
    "corrupt",
    "close",
    "exception",
    "ignore",
    "omit",
    "delete",
    "overwrite",
];

/// Vague phrases named in the reason when present.
const VAGUE: &[&str] = &[
    "depende de",
    "a veces",
    "lento",
    "lenta",
    "rápido",
    "rápida",
    "mejor",
    "peor",
    "mucho",
    "poco",
    "bastante",
    "más o menos",
    "optimizar",
    "mejorar",
    "depends on",
    "sometimes",
    "slow",
    "fast",
    "better",
    "worse",
    "a lot",
    "much",
    "little",
    "quite",
    "more or less",
    "optimize",
    "improve",
];

/// Checks that `statement` is one measurable sentence.
///
/// # Errors
///
/// [`NotMeasurable`] with what is missing.
pub fn check(statement: &str) -> Result<Measure, NotMeasurable> {
    let text = statement.trim().to_lowercase();
    if text.is_empty() {
        return Err(NotMeasurable {
            reason: "the definition is empty".into(),
        });
    }
    if sentences(&text) > 1 {
        return Err(NotMeasurable {
            reason: "it must be a single sentence; split each problem into its own definition"
                .into(),
        });
    }
    let numbers = numbers(&text);
    if numbers.iter().any(|unit| unit.is_some()) {
        return Ok(Measure::Magnitude);
    }
    if !numbers.is_empty() && has_comparator(&text) {
        return Ok(Measure::Comparison);
    }
    if has_condition(&text) {
        return Ok(Measure::Condition);
    }
    let mut reason = String::from(
        "it has no magnitude with a unit (e.g. \"500 ms\"), no comparison with a number \
         (e.g. \"more than 3 retries\") and no verifiable condition (e.g. \"when running X, \
         Y fails\")",
    );
    let vague: Vec<&str> = VAGUE
        .iter()
        .copied()
        .filter(|v| contains_phrase(&text, v))
        .collect();
    if !vague.is_empty() {
        reason.push_str("; it uses vague terms: ");
        reason.push_str(
            &vague
                .iter()
                .map(|v| format!("\"{v}\""))
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    Err(NotMeasurable { reason })
}

/// Number of sentences: terminators followed by more text, ignoring
/// decimal points and dots inside code spans.
fn sentences(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut count = 1;
    let mut in_code = false;
    for (i, &c) in chars.iter().enumerate() {
        if c == '`' {
            in_code = !in_code;
        }
        if in_code || !matches!(c, '.' | '!' | '?' | ';') {
            continue;
        }
        let next = chars.get(i + 1).copied();
        if next.is_some_and(char::is_whitespace)
            && chars[i + 1..].iter().any(|c| c.is_alphanumeric())
        {
            count += 1;
        }
    }
    count
}

/// Numbers of the text not glued to a preceding letter (so ids like `E1`
/// or `h2` do not count), each with the unit that follows it, if known.
fn numbers(text: &str) -> Vec<Option<String>> {
    let chars: Vec<char> = text.chars().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let glued = i > 0 && (chars[i - 1].is_alphabetic() || chars[i - 1] == '_');
        if !chars[i].is_ascii_digit() || glued {
            i += 1;
            continue;
        }
        while i < chars.len()
            && (chars[i].is_ascii_digit()
                || (matches!(chars[i], '.' | ',')
                    && chars.get(i + 1).is_some_and(char::is_ascii_digit)))
        {
            i += 1;
        }
        let mut j = i;
        if chars.get(j) == Some(&' ') {
            j += 1;
        }
        let start = j;
        while j < chars.len() && (chars[j].is_alphabetic() || matches!(chars[j], '%' | '/')) {
            j += 1;
        }
        let word: String = chars[start..j].iter().collect();
        found.push(is_unit(&word).then_some(word));
    }
    found
}

/// Whether `word` (e.g. `ms`, `req/s`, `mb/s`) is a unit.
fn is_unit(word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    let base = word.split('/').next().unwrap_or(word);
    UNITS.contains(&word) || UNITS.contains(&base) || base.starts_with("req")
}

fn has_comparator(text: &str) -> bool {
    text.contains(SYMBOLS) || COMPARATORS.iter().any(|c| contains_phrase(text, c))
}

fn has_condition(text: &str) -> bool {
    let words = words(text);
    let trigger = words.iter().any(|w| TRIGGERS.contains(w));
    let outcome = words
        .iter()
        .any(|w| OUTCOMES.iter().any(|o| w.starts_with(o)));
    trigger && outcome
}

fn words(text: &str) -> Vec<&str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Whether `phrase` appears in `text` as whole words.
fn contains_phrase(text: &str, phrase: &str) -> bool {
    let haystack = words(text);
    let target = words(phrase);
    !target.is_empty()
        && haystack
            .windows(target.len())
            .any(|w| w == target.as_slice())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_measurable_statements() {
        let cases = [
            (
                "at 500 requests per second the response time rises from 10 ms to 500 ms",
                Measure::Magnitude,
            ),
            ("login takes 2 s on a cold start", Measure::Magnitude),
            (
                "the retry happens more than 3 times per minute",
                Measure::Magnitude,
            ),
            (
                "startup goes from 1 to 4 on the survey scale",
                Measure::Comparison,
            ),
            ("the queue has > 1000 at peak time", Measure::Comparison),
            (
                "when running `cargo test -p store offline` the test fails",
                Measure::Condition,
            ),
            ("if the network drops, the app panics", Measure::Condition),
            (
                "a 500 requests por segundo el tiempo de respuesta sube de 10 ms a 500 ms",
                Measure::Magnitude,
            ),
            ("el login tarda 2 s en frío", Measure::Magnitude),
            ("el índice ocupa 3,5 GB en disco", Measure::Magnitude),
            (
                "el 20% de las sesiones se pierden al reiniciar",
                Measure::Magnitude,
            ),
            ("el p95 de la búsqueda supera 200ms", Measure::Magnitude),
            (
                "la caché acierta en menos de 3 de cada 10 consultas",
                Measure::Magnitude,
            ),
            (
                "el reintento ocurre más de 3 veces por minuto",
                Measure::Magnitude,
            ),
            (
                "el arranque pasa de 1 a 4 en la escala de la encuesta",
                Measure::Comparison,
            ),
            ("la cola tiene > 1000 en hora pico", Measure::Comparison),
            (
                "al ejecutar `cargo test -p store offline` el test falla",
                Measure::Condition,
            ),
            (
                "cuando el archivo tiene un BOM el parser devuelve un error",
                Measure::Condition,
            ),
            (
                "si la red se corta, la app entra en pánico",
                Measure::Condition,
            ),
        ];
        for (statement, expected) in cases {
            assert_eq!(check(statement), Ok(expected), "{statement}");
        }
    }

    #[test]
    fn rejects_unmeasurable_statements() {
        let cases = [
            (
                "the response time depends on the load",
                Some("\"depends on\""),
            ),
            ("login is slow", Some("\"slow\"")),
            ("the user experience must improve", Some("\"improve\"")),
            ("the parser sometimes fails", Some("\"sometimes\"")),
            ("hypothesis H2 explains the problem [E1]", None),
            ("the system does not scale", None),
            (
                "el tiempo de respuesta depende de la carga",
                Some("\"depende de\""),
            ),
            ("el login es lento", Some("\"lento\"")),
            (
                "hay que mejorar la experiencia de usuario",
                Some("\"mejorar\""),
            ),
            ("el parser a veces falla", Some("\"a veces\"")),
            ("la hipótesis H2 explica el problema [E1]", None),
            ("la versión v2 es peor", Some("\"peor\"")),
            ("el sistema no escala", None),
        ];
        for (statement, vague) in cases {
            let err = check(statement).unwrap_err();
            assert!(err.reason.contains("magnitude with a unit"), "{statement}");
            if let Some(word) = vague {
                assert!(err.reason.contains(word), "{statement}: {err}");
            }
        }
    }

    #[test]
    fn requires_one_sentence() {
        let err = check("login takes 2 s. Search takes 3 s.").unwrap_err();
        assert!(err.reason.contains("a single sentence"));
        assert!(check("el login tarda 2 s. La búsqueda tarda 3 s.").is_err());
        assert!(check("tarda 2.5 s con `a.b. c`").is_ok());
        assert!(check("  ").is_err());
    }
}
