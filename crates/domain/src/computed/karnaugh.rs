//! Body of the Karnaugh-map card: a Boolean function of up to six
//! variables (its minterms and don't-cares), its prime implicants, and a
//! minimal sum of products and product of sums.
//!
//! Minterm `m` sets variable `i` (0 = the first, most significant) to bit
//! `n − 1 − i` of `m`, so with variables A, B, C minterm 6 is A·B·C'. An
//! [`Implicant`] fixes the variables in its `care` mask to the bits of
//! `value`; the rest are free. The map layout ([`KarnaughBody::maps`])
//! uses Gray-code order on both axes; five and six variables are drawn as
//! two or four 4×4 maps side by side.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ComputedError;

/// Most variables a map takes.
pub const MAX_VARIABLES: usize = 6;

/// How the cover was chosen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CoverMethod {
    /// Essential primes, then Petrick's method: a cover with the fewest
    /// terms, then the fewest literals.
    #[default]
    Exact,
    /// Essential primes, then the prime covering most uncovered minterms
    /// (Petrick's expansion grew too large): minimal not guaranteed.
    Greedy,
}

/// A product term (or, in a POS, the sum term of a group of zeros).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Implicant {
    /// Bit `n − 1 − i` set when variable `i` appears.
    pub care: u32,
    /// The values of the variables that appear.
    pub value: u32,
    /// The term as written (`AB'`, or `(A' + B)` in a POS).
    pub term: String,
    /// The minterms (or maxterms) it covers, ascending.
    pub covers: Vec<u32>,
    /// Whether it is essential (the only prime covering some minterm).
    #[serde(default)]
    pub essential: bool,
}

impl Implicant {
    /// Number of literals.
    #[must_use]
    pub fn literals(&self) -> u32 {
        self.care.count_ones()
    }

    /// Whether it covers minterm `m`.
    #[must_use]
    pub fn covers_minterm(&self, m: u32) -> bool {
        m & self.care == self.value & self.care
    }
}

/// What one cell of the map holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellValue {
    /// F = 1.
    One,
    /// F = 0.
    Zero,
    /// Don't care.
    DontCare,
}

/// One drawn map: row and column labels (Gray order) and the minterm of
/// each cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KMap {
    /// Caption: the values of the map-selecting variables (`A = 1`), or
    /// empty for a single map.
    pub label: String,
    /// Variables along the rows (`AB`) and columns (`CD`).
    pub row_vars: String,
    /// Variables along the columns.
    pub col_vars: String,
    /// Row labels (`00`, `01`, `11`, `10`).
    pub rows: Vec<String>,
    /// Column labels.
    pub cols: Vec<String>,
    /// `cells[r][c]` = minterm index.
    pub cells: Vec<Vec<u32>>,
}

/// `bits`-bit Gray code in order.
#[must_use]
pub fn gray(bits: u32) -> Vec<u32> {
    (0..(1u32 << bits)).map(|i| i ^ (i >> 1)).collect()
}

fn binary(value: u32, bits: u32) -> String {
    (0..bits)
        .rev()
        .map(|b| if value >> b & 1 == 1 { '1' } else { '0' })
        .collect()
}

/// Body of a Karnaugh-map card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct KarnaughBody {
    /// Variable names, most significant first.
    pub variables: Vec<String>,
    /// Minterms where F = 1, ascending.
    pub minterms: Vec<u32>,
    /// Don't-care minterms, ascending.
    #[serde(default)]
    pub dont_cares: Vec<u32>,
    /// Every prime implicant of F (ones and don't-cares).
    #[serde(default)]
    pub prime_implicants: Vec<Implicant>,
    /// The chosen product terms.
    pub sop: Vec<Implicant>,
    /// The minimal sum of products, e.g. `AB' + C`.
    pub sop_expression: String,
    /// The chosen sum terms (groups of zeros).
    pub pos: Vec<Implicant>,
    /// The minimal product of sums, e.g. `(A + C)(B' + C)`.
    pub pos_expression: String,
    /// How the covers were chosen.
    pub method: CoverMethod,
}

impl KarnaughBody {
    /// Checks the variable count and that terms are in range and
    /// disjoint between ones and don't-cares.
    ///
    /// # Errors
    ///
    /// [`ComputedError::Body`] otherwise.
    pub fn validate(&self) -> Result<(), ComputedError> {
        let n = self.variables.len();
        if !(1..=MAX_VARIABLES).contains(&n) {
            return Err(ComputedError::Body(format!(
                "a Karnaugh map takes 1 to {MAX_VARIABLES} variables"
            )));
        }
        let size = 1u32 << n;
        if self
            .minterms
            .iter()
            .chain(&self.dont_cares)
            .any(|m| *m >= size)
        {
            return Err(ComputedError::Body(format!(
                "terms must be below {size} for {n} variables"
            )));
        }
        let ascending = |v: &[u32]| v.windows(2).all(|w| w[0] < w[1]);
        if !(ascending(&self.minterms) && ascending(&self.dont_cares)) {
            return Err(ComputedError::Body(
                "minterms and don't-cares must ascend without repeats".into(),
            ));
        }
        if self.minterms.iter().any(|m| self.dont_cares.contains(m)) {
            return Err(ComputedError::Body(
                "a minterm cannot also be a don't-care".into(),
            ));
        }
        Ok(())
    }

    /// Rows of the truth table.
    #[must_use]
    pub fn size(&self) -> u32 {
        1u32 << self.variables.len()
    }

    /// F at minterm `m`.
    #[must_use]
    pub fn value(&self, m: u32) -> CellValue {
        if self.minterms.binary_search(&m).is_ok() {
            CellValue::One
        } else if self.dont_cares.binary_search(&m).is_ok() {
            CellValue::DontCare
        } else {
            CellValue::Zero
        }
    }

    /// The maps to draw.
    #[must_use]
    pub fn maps(&self) -> Vec<KMap> {
        let n = u32::try_from(self.variables.len()).unwrap_or(0);
        let (map_bits, row_bits) = match n {
            0 => return Vec::new(),
            1..=4 => (0, n / 2),
            5 => (1, 2),
            _ => (2, 2),
        };
        let col_bits = n - map_bits - row_bits;
        let names = |from: u32, count: u32| -> String {
            self.variables[from as usize..(from + count) as usize].concat()
        };
        let row_vars = names(map_bits, row_bits);
        let col_vars = names(map_bits + row_bits, col_bits);
        gray(map_bits)
            .into_iter()
            .map(|map| {
                let label = if map_bits == 0 {
                    String::new()
                } else {
                    format!("{} = {}", names(0, map_bits), binary(map, map_bits))
                };
                let rows = gray(row_bits);
                let cols = gray(col_bits);
                let cells = rows
                    .iter()
                    .map(|r| {
                        cols.iter()
                            .map(|c| (map << (row_bits + col_bits)) | (r << col_bits) | c)
                            .collect()
                    })
                    .collect();
                KMap {
                    label,
                    row_vars: row_vars.clone(),
                    col_vars: col_vars.clone(),
                    rows: rows.iter().map(|r| binary(*r, row_bits)).collect(),
                    cols: cols.iter().map(|c| binary(*c, col_bits)).collect(),
                    cells,
                }
            })
            .collect()
    }

    /// The truth table as CSV: the variables, then `F` (`1`, `0` or `x`).
    #[must_use]
    pub fn truth_table_csv(&self) -> String {
        let n = u32::try_from(self.variables.len()).unwrap_or(0);
        let mut out = self.variables.join(",");
        out.push_str(",F\n");
        for m in 0..self.size() {
            for b in (0..n).rev() {
                out.push_str(if m >> b & 1 == 1 { "1," } else { "0," });
            }
            out.push_str(match self.value(m) {
                CellValue::One => "1\n",
                CellValue::Zero => "0\n",
                CellValue::DontCare => "x\n",
            });
        }
        out
    }
}

/// The product term of `care`/`value` over `variables` (`AB'`; `1` when
/// empty). Names longer than one character are joined with `·`.
#[must_use]
pub fn product_term(care: u32, value: u32, variables: &[String]) -> String {
    let n = variables.len();
    let parts: Vec<String> = (0..n)
        .filter(|i| care >> (n - 1 - i) & 1 == 1)
        .map(|i| {
            let set = value >> (n - 1 - i) & 1 == 1;
            if set {
                variables[i].clone()
            } else {
                format!("{}'", variables[i])
            }
        })
        .collect();
    if parts.is_empty() {
        "1".to_owned()
    } else if variables.iter().all(|v| v.chars().count() == 1) {
        parts.concat()
    } else {
        parts.join("·")
    }
}

/// The sum term of a group of zeros (`(A' + B)`; `0` when empty): a
/// variable that is 1 throughout the group appears complemented.
#[must_use]
pub fn sum_term(care: u32, value: u32, variables: &[String]) -> String {
    let n = variables.len();
    let parts: Vec<String> = (0..n)
        .filter(|i| care >> (n - 1 - i) & 1 == 1)
        .map(|i| {
            let set = value >> (n - 1 - i) & 1 == 1;
            if set {
                format!("{}'", variables[i])
            } else {
                variables[i].clone()
            }
        })
        .collect();
    match parts.len() {
        0 => "0".to_owned(),
        1 => parts[0].clone(),
        _ => format!("({})", parts.join(" + ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn terms_read_like_the_textbook() {
        let v = vars(&["A", "B", "C"]);
        assert_eq!(product_term(0b110, 0b100, &v), "AB'");
        assert_eq!(product_term(0, 0, &v), "1");
        assert_eq!(sum_term(0b101, 0b100, &v), "(A' + C)");
        assert_eq!(sum_term(0b001, 0b000, &v), "C");
        let long = vars(&["en", "q"]);
        assert_eq!(product_term(0b11, 0b01, &long), "en'·q");
        assert_eq!(gray(2), [0, 1, 3, 2]);
    }

    #[test]
    fn maps_lay_out_gray_order_and_split_five_variables() {
        let body = KarnaughBody {
            variables: vars(&["A", "B", "C", "D"]),
            minterms: vec![0, 5],
            dont_cares: vec![15],
            prime_implicants: Vec::new(),
            sop: Vec::new(),
            sop_expression: String::new(),
            pos: Vec::new(),
            pos_expression: String::new(),
            method: CoverMethod::Exact,
        };
        let maps = body.maps();
        assert_eq!(maps.len(), 1);
        assert_eq!(maps[0].rows, ["00", "01", "11", "10"]);
        // Row AB = 11, column CD = 10 is minterm 14.
        assert_eq!(maps[0].cells[2][3], 14);
        assert_eq!(body.value(15), CellValue::DontCare);
        assert!(
            body.truth_table_csv()
                .starts_with("A,B,C,D,F\n0,0,0,0,1\n0,0,0,1,0\n")
        );
        let five = KarnaughBody {
            variables: vars(&["A", "B", "C", "D", "E"]),
            ..body
        };
        let maps = five.maps();
        assert_eq!(maps.len(), 2);
        assert_eq!(maps[1].label, "A = 1");
        assert_eq!(maps[1].cells[0][0], 16);
    }
}
