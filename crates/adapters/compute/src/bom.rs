//! The BOM costing card: quantity × unit cost per line in `rust_decimal`
//! arithmetic, totals per category, per unit and for every unit built,
//! and the lines with no price or no MPN.
//!
//! Columns are found by their header (case and punctuation ignored)
//! unless the request names them. Prices written as text are read with
//! their currency signs and thousands separators stripped (`$1,234.50`,
//! `1.234,50 €`). A line without a quantity takes the count of its
//! reference designators (`R1, R2, R5-R8` is 6). A line without a price
//! is costed at nothing and listed, never guessed. When the file has its
//! own extended-cost column, lines where it disagrees with quantity ×
//! unit cost are reported.

use std::collections::BTreeMap;
use std::str::FromStr as _;

use application::ports::compute::{BomRequest, ColumnValues, ComputeError, DataColumn, DataSet};
use domain::computed::{
    Assumption, AssumptionStatus, BomBody, BomCategory, BomLine, Computed, ComputedBody,
    ComputedValue, Narrative, UncertaintyKind, WarningSeverity,
};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive as _;
use serde_json::Value;

use crate::data::key_part;
use crate::envelope::{self, count, warning};

/// Most BOM lines.
pub const MAX_LINES: usize = 20_000;
/// Lines listed in a warning.
const LISTED: usize = 10;

fn invalid(text: impl Into<String>) -> ComputeError {
    ComputeError::Invalid(text.into())
}

/// A header reduced to letters and digits, lower case.
fn squash(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The column named `given`, or the first whose header is one of
/// `candidates` (squashed).
fn find<'a>(
    data: &'a DataSet,
    given: Option<&str>,
    candidates: &[&str],
    what: &str,
) -> Result<Option<&'a DataColumn>, ComputeError> {
    if let Some(name) = given {
        return data
            .column(name)
            .map(Some)
            .ok_or_else(|| invalid(format!("no column `{name}` for the {what}")));
    }
    Ok(candidates
        .iter()
        .find_map(|c| data.columns.iter().find(|col| squash(&col.name) == *c)))
}

/// Cell `row` of `column` as text.
fn text(column: Option<&DataColumn>, row: usize) -> Option<String> {
    match &column?.values {
        ColumnValues::Numeric(v) => v.get(row).copied().flatten().map(|x| x.to_string()),
        ColumnValues::Categorical(v) => v
            .get(row)
            .cloned()
            .flatten()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty()),
    }
}

/// A decimal from a number's shortest text.
fn decimal(x: f64) -> Option<Decimal> {
    if x.is_finite() {
        Decimal::from_str(&x.to_string()).ok()
    } else {
        None
    }
}

/// A price or quantity written as text: currency signs, codes and spaces
/// dropped; `1,234.50` and `1.234,50` both read as 1234.50, `0,25` as
/// 0.25.
pub fn parse_amount(raw: &str) -> Option<Decimal> {
    let kept: String = raw
        .chars()
        .filter(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-'))
        .collect();
    if kept.is_empty() || !kept.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let normal = match (kept.rfind('.'), kept.rfind(',')) {
        (Some(dot), Some(comma)) if comma > dot => kept.replace('.', "").replace(',', "."),
        (Some(_), Some(_)) => kept.replace(',', ""),
        (None, Some(comma)) => {
            let tail = kept.len() - comma - 1;
            if kept.matches(',').count() == 1 && tail != 3 {
                kept.replace(',', ".")
            } else {
                kept.replace(',', "")
            }
        }
        _ => kept,
    };
    Decimal::from_str(&normal).ok()
}

/// The amount in `column` at `row`, numeric or text.
fn amount(column: Option<&DataColumn>, row: usize) -> Option<Decimal> {
    match &column?.values {
        ColumnValues::Numeric(v) => v.get(row).copied().flatten().and_then(decimal),
        ColumnValues::Categorical(v) => {
            v.get(row).cloned().flatten().and_then(|s| parse_amount(&s))
        }
    }
}

/// Count of reference designators: `R1, R2 R3; R5-R8` is 7.
pub fn designator_count(refs: &str) -> u32 {
    refs.split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .map(|t| {
            let Some((a, b)) = t.split_once('-') else {
                return 1;
            };
            let split = |s: &str| {
                let digits = s.trim_start_matches(|c: char| !c.is_ascii_digit());
                (
                    s[..s.len() - digits.len()].to_owned(),
                    digits.parse::<u32>().ok(),
                )
            };
            match (split(a), split(b)) {
                ((pa, Some(na)), (pb, Some(nb))) if (pa == pb || pb.is_empty()) && nb >= na => {
                    nb - na + 1
                }
                _ => 1,
            }
        })
        .sum()
}

/// `d` for display: at least two decimals, at most four.
pub fn money(d: Decimal) -> String {
    let mut shown = d.round_dp(4).normalize();
    if shown.scale() < 2 {
        shown.rescale(2);
    }
    shown.to_string()
}

fn value(d: Decimal, unit: Option<&String>) -> ComputedValue {
    ComputedValue {
        display: Some(money(d)),
        unit: unit.cloned(),
        ..ComputedValue::new(d.to_f64().unwrap_or(f64::NAN))
    }
}

/// The request's lines as a data set (inline items become columns).
fn lines_data(request: &BomRequest) -> DataSet {
    if request.items.is_empty() {
        return request.data.clone();
    }
    let items = &request.items;
    let text = |f: &dyn Fn(&application::ports::compute::BomItem) -> Option<String>| {
        ColumnValues::Categorical(items.iter().map(f).collect())
    };
    let number = |f: &dyn Fn(&application::ports::compute::BomItem) -> Option<f64>| {
        ColumnValues::Numeric(items.iter().map(f).collect())
    };
    let column = |name: &str, values| DataColumn {
        name: name.into(),
        values,
        unit: None,
    };
    DataSet::inline(vec![
        column("part", text(&|i| Some(i.part.clone()))),
        column("quantity", number(&|i| i.quantity)),
        column("unit cost", number(&|i| i.unit_cost)),
        column("category", text(&|i| i.category.clone())),
        column("reference", text(&|i| i.reference.clone())),
        column("mpn", text(&|i| i.mpn.clone())),
        column("description", text(&|i| i.description.clone())),
    ])
}

/// Runs a BOM request.
#[allow(clippy::too_many_lines)]
pub fn run(request: &BomRequest, id: String) -> Result<(Computed, ComputedBody), ComputeError> {
    let data = lines_data(request);
    let rows = data.columns.first().map_or(0, DataColumn::len);
    if rows == 0 {
        return Err(invalid("the BOM has no lines"));
    }
    if rows > MAX_LINES {
        return Err(invalid(format!("at most {MAX_LINES} BOM lines")));
    }
    let part = find(
        &data,
        Option::as_deref(&request.part),
        &[
            "part",
            "partnumber",
            "pn",
            "partno",
            "item",
            "name",
            "component",
            "value",
            "comment",
        ],
        "part",
    )?;
    let quantity = find(
        &data,
        Option::as_deref(&request.quantity),
        &["qty", "quantity", "count", "qnty", "quan"],
        "quantity",
    )?;
    let unit_cost = find(
        &data,
        Option::as_deref(&request.unit_cost),
        &[
            "unitcost",
            "unitprice",
            "price",
            "cost",
            "priceeach",
            "costeach",
            "priceunit",
        ],
        "unit cost",
    )?;
    let category = find(
        &data,
        Option::as_deref(&request.category),
        &["category", "type", "group", "class", "kind"],
        "category",
    )?;
    let reference = find(
        &data,
        Option::as_deref(&request.reference),
        &[
            "reference",
            "references",
            "ref",
            "refs",
            "designator",
            "designators",
            "refdes",
        ],
        "references",
    )?;
    let mpn = find(
        &data,
        Option::as_deref(&request.mpn),
        &[
            "mpn",
            "manufacturerpartnumber",
            "mfrpartnumber",
            "mfrpart",
            "mfrpn",
            "manufacturerpn",
            "mfgpn",
        ],
        "MPN",
    )?;
    let description = find(&data, None, &["description", "desc"], "description")?;
    let file_extended = find(
        &data,
        None,
        &[
            "extended",
            "extendedprice",
            "extendedcost",
            "extprice",
            "linetotal",
            "total",
            "totalprice",
        ],
        "extended cost",
    )?;
    let part = part.or(mpn).or(description).or(reference).ok_or_else(|| {
        invalid(format!(
            "no part column found among {}; name it with `part`",
            data.columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })?;
    if quantity.is_none() && reference.is_none() {
        return Err(invalid(
            "no quantity or reference column: name it with `quantity`",
        ));
    }
    let builds = decimal(request.builds)
        .filter(|b| *b > Decimal::ZERO)
        .ok_or_else(|| invalid("`builds` must be a positive number"))?;
    let mut lines: Vec<BomLine> = Vec::with_capacity(rows);
    let mut mismatches: Vec<u32> = Vec::new();
    let mut total = Decimal::ZERO;
    for r in 0..rows {
        let row = u32::try_from(r + 1).unwrap_or(u32::MAX);
        let name = text(Some(part), r).unwrap_or_else(|| format!("row {row}"));
        let refs = text(reference, r);
        let (qty, from_refs) = match amount(quantity, r) {
            Some(q) => (q, false),
            None => match &refs {
                Some(refs) => (Decimal::from(designator_count(refs)), true),
                None => (Decimal::ZERO, false),
            },
        };
        if qty < Decimal::ZERO {
            return Err(invalid(format!("row {row}: a negative quantity")));
        }
        let unit = amount(unit_cost, r);
        if unit.is_some_and(|u| u < Decimal::ZERO) {
            return Err(invalid(format!("row {row}: a negative unit cost")));
        }
        let extended = match unit {
            Some(u) => Some(
                qty.checked_mul(u)
                    .ok_or_else(|| invalid(format!("row {row}: the cost overflows")))?,
            ),
            None => None,
        };
        if let (Some(e), Some(f)) = (extended, amount(file_extended, r))
            && (e - f).abs() > Decimal::new(5, 3)
        {
            mismatches.push(row);
        }
        if let Some(e) = extended {
            total += e;
        }
        lines.push(BomLine {
            row,
            part: name,
            reference: refs,
            mpn: text(mpn, r),
            description: text(description, r),
            category: text(category, r).unwrap_or_else(|| "uncategorized".to_owned()),
            quantity: qty.normalize().to_string(),
            unit_cost: unit.map(|u| u.normalize().to_string()),
            extended: extended.map(|e| e.normalize().to_string()),
            quantity_from_references: from_refs,
        });
    }
    // Categories, largest total first.
    let mut by: BTreeMap<String, (u32, Decimal, Decimal, u32)> = BTreeMap::new();
    for l in &lines {
        let entry = by.entry(l.category.clone()).or_default();
        entry.0 += 1;
        entry.1 += Decimal::from_str(&l.quantity).unwrap_or_default();
        match &l.extended {
            Some(e) => entry.2 += Decimal::from_str(e).unwrap_or_default(),
            None => entry.3 += 1,
        }
    }
    let mut categories: Vec<(String, (u32, Decimal, Decimal, u32))> = by.into_iter().collect();
    categories.sort_by(|a, b| b.1.2.cmp(&a.1.2).then(a.0.cmp(&b.0)));
    let total_builds = total
        .checked_mul(builds)
        .ok_or_else(|| invalid("the total overflows"))?;
    // Envelope.
    let mut engine = envelope::solve_engine();
    engine.backend = Some("crest BOM costing (rust_decimal 1.43.0)".into());
    let mut computed = Computed::new(id, "solve.bom", "BOM cost", engine);
    computed.uncertainty_kind = Some(UncertaintyKind::Exact);
    let used: Vec<DataColumn> = [Some(part), quantity, unit_cost, category, reference, mpn]
        .into_iter()
        .flatten()
        .cloned()
        .collect();
    computed.inputs.push(envelope::input(&data, &used, None));
    let currency = request.currency.as_ref();
    let values = &mut computed.values;
    values.insert("total".into(), value(total, currency));
    if builds != Decimal::ONE {
        values.insert("total_builds".into(), value(total_builds, currency));
        values.insert(
            "builds".into(),
            ComputedValue {
                display: Some(builds.normalize().to_string()),
                ..ComputedValue::new(builds.to_f64().unwrap_or(f64::NAN))
            },
        );
    }
    values.insert("lines".into(), count(lines.len() as u64));
    let missing: Vec<&BomLine> = lines.iter().filter(|l| l.unit_cost.is_none()).collect();
    values.insert("missing_prices".into(), count(missing.len() as u64));
    for (name, (_, _, sum, _)) in &categories {
        values.insert(format!("total.{}", key_part(name)), value(*sum, currency));
    }
    let has_mpn = mpn.is_some();
    let no_mpn: Vec<&BomLine> = if has_mpn {
        lines.iter().filter(|l| l.mpn.is_none()).collect()
    } else {
        Vec::new()
    };
    if has_mpn {
        values.insert("missing_mpns".into(), count(no_mpn.len() as u64));
    }
    let params = &mut computed.method.params;
    let mut columns = serde_json::Map::new();
    for (role, col) in [
        ("part", Some(part)),
        ("quantity", quantity),
        ("unit_cost", unit_cost),
        ("category", category),
        ("reference", reference),
        ("mpn", mpn),
    ] {
        if let Some(col) = col {
            columns.insert(role.into(), Value::from(col.name.clone()));
        }
    }
    params.insert("columns".into(), Value::Object(columns));
    computed.method.why = Some(
        "quantity × unit cost per line in decimal arithmetic; unpriced lines count as nothing"
            .into(),
    );
    computed.method.alternatives = Vec::new();
    let names = |ls: &[&BomLine]| {
        let mut text = ls
            .iter()
            .take(LISTED)
            .map(|l| format!("{} (row {})", l.part, l.row))
            .collect::<Vec<_>>()
            .join(", ");
        if ls.len() > LISTED {
            text.push_str(&format!(" and {} more", ls.len() - LISTED));
        }
        text
    };
    if unit_cost.is_none() {
        computed.warnings.push(warning(
            "no_prices",
            WarningSeverity::Warn,
            "no unit cost column: every line is unpriced (name it with `unit_cost`)",
        ));
    } else if !missing.is_empty() {
        computed.warnings.push(warning(
            "missing_prices",
            WarningSeverity::Warn,
            format!(
                "{} lines have no price and are left out of the totals: {}",
                missing.len(),
                names(&missing)
            ),
        ));
    }
    if !no_mpn.is_empty() {
        computed.warnings.push(warning(
            "missing_mpns",
            WarningSeverity::Info,
            format!("{} lines have no MPN: {}", no_mpn.len(), names(&no_mpn)),
        ));
    }
    if !mismatches.is_empty() {
        computed.warnings.push(warning(
            "extended_mismatch",
            WarningSeverity::Warn,
            format!(
                "the file's own extended cost differs from quantity × unit cost on rows {}",
                mismatches
                    .iter()
                    .take(LISTED)
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    let counted = lines.iter().filter(|l| l.quantity_from_references).count();
    if counted > 0 {
        computed.warnings.push(warning(
            "quantity_from_references",
            WarningSeverity::Info,
            format!("{counted} lines have no quantity; it was counted from their references"),
        ));
    }
    computed.assumptions.push(Assumption {
        id: "prices".into(),
        check: None,
        stat: None,
        p: None,
        status: AssumptionStatus::NotChecked,
        consequence: None,
        note: Some("unit costs as given (no price breaks, MOQ, attrition, tax or shipping)".into()),
    });
    computed.narrative = Some(Narrative {
        template: "bom".into(),
        text_slots: vec![
            "total".into(),
            "total_builds".into(),
            "missing_prices".into(),
        ],
    });
    let body = BomBody {
        currency: request.currency.clone(),
        builds: builds.normalize().to_string(),
        lines,
        categories: categories
            .into_iter()
            .map(|(name, (n, q, sum, missing))| BomCategory {
                name,
                lines: n,
                quantity: q.normalize().to_string(),
                total: sum.normalize().to_string(),
                missing_prices: missing,
            })
            .collect(),
        total: total.normalize().to_string(),
        total_builds: total_builds.normalize().to_string(),
        has_mpn,
    };
    Ok((computed, ComputedBody::Bom(body)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_and_designators_read_like_a_buyer_would() {
        let d = |s: &str| Decimal::from_str(s).ok();
        assert_eq!(parse_amount("$1,234.50"), d("1234.50"));
        assert_eq!(parse_amount("1.234,50 €"), d("1234.50"));
        assert_eq!(parse_amount("0,25"), d("0.25"));
        assert_eq!(parse_amount("1,000"), d("1000"));
        assert_eq!(parse_amount("USD 0.0012"), d("0.0012"));
        assert_eq!(parse_amount("n/a"), None);
        assert_eq!(designator_count("R1, R2 R3; R5-R8"), 7);
        assert_eq!(designator_count("C1-3"), 3);
        assert_eq!(designator_count("U1"), 1);
        assert_eq!(money(Decimal::from(12)), "12.00");
        assert_eq!(money(d("0.01234").unwrap_or_default()), "0.0123");
    }
}
