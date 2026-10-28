//! Parquet datasets through the `parquet` crate's record API (no arrow).
//!
//! The row count comes from the footer; the statistics cover the first
//! [`MAX_PROFILE_ROWS`] rows. Types are named from the schema (logical
//! type first, then the physical one). Dates and timestamps are written
//! as ISO text here rather than with the crate's formatter, which panics
//! on values outside chrono's range; nested values become compact
//! JSON-like text.

use std::fmt::Write as _;

use bytes::Bytes;
use parquet::basic::{ConvertedType, LogicalType, TimeUnit, Type as Physical};
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::Field;
use parquet::schema::types::Type;

use super::profile::Value;
use super::{MAX_PROFILE_ROWS, Table};

/// Reads the schema, the row count and the profiled rows of a Parquet
/// file.
///
/// # Errors
///
/// The bytes are not a Parquet file or a row cannot be decoded (for
/// example a codec this build lacks).
pub fn read(bytes: &[u8]) -> Result<Table, String> {
    let reader = SerializedFileReader::new(Bytes::copy_from_slice(bytes))
        .map_err(|e| format!("not a Parquet file: {e}"))?;
    let meta = reader.metadata().file_metadata();
    let total = u64::try_from(meta.num_rows()).unwrap_or(0);
    let fields = meta.schema().get_fields();
    let names: Vec<String> = fields.iter().map(|f| f.name().to_owned()).collect();
    let dtypes: Vec<String> = fields.iter().map(|f| dtype(f)).collect();
    let mut columns: Vec<Vec<Value>> = vec![Vec::new(); names.len()];
    let rows = reader
        .get_row_iter(None)
        .map_err(|e| format!("cannot read the rows: {e}"))?;
    for row in rows.take(MAX_PROFILE_ROWS) {
        let row = row.map_err(|e| format!("cannot read a row: {e}"))?;
        for (ix, (_, field)) in row.get_column_iter().enumerate() {
            if let Some(column) = columns.get_mut(ix) {
                column.push(value(field));
            }
        }
    }
    let profiled = columns.first().map_or(0, Vec::len) as u64;
    Ok(Table {
        names,
        dtypes,
        columns,
        total_rows: Some(total),
        sampled: total > profiled,
    })
}

/// The type of a top-level field, e.g. `int64`, `utf8`, `date32`,
/// `timestamp[ms]`, `decimal(10,2)`, `list`, `struct`.
fn dtype(field: &Type) -> String {
    let info = field.get_basic_info();
    if let Some(logical) = info.logical_type_ref() {
        let name = match logical {
            LogicalType::String => "utf8".to_owned(),
            LogicalType::Enum => "enum".to_owned(),
            LogicalType::Json => "json".to_owned(),
            LogicalType::Uuid => "uuid".to_owned(),
            LogicalType::Date => "date32".to_owned(),
            LogicalType::Float16 => "float16".to_owned(),
            LogicalType::List => "list".to_owned(),
            LogicalType::Map => "map".to_owned(),
            LogicalType::Decimal(d) => format!("decimal({},{})", d.precision, d.scale),
            LogicalType::Integer(i) => {
                format!("{}int{}", if i.is_signed { "" } else { "u" }, i.bit_width)
            }
            LogicalType::Timestamp(t) => format!("timestamp[{}]", unit(&t.unit)),
            LogicalType::Time(t) => format!("time[{}]", unit(&t.unit)),
            _ => String::new(),
        };
        if !name.is_empty() {
            return name;
        }
    }
    // Older writers set only the converted type.
    match info.converted_type() {
        ConvertedType::UTF8 => return "utf8".into(),
        ConvertedType::ENUM => return "enum".into(),
        ConvertedType::JSON => return "json".into(),
        ConvertedType::DATE => return "date32".into(),
        ConvertedType::TIMESTAMP_MILLIS => return "timestamp[ms]".into(),
        ConvertedType::TIMESTAMP_MICROS => return "timestamp[us]".into(),
        ConvertedType::LIST => return "list".into(),
        ConvertedType::MAP | ConvertedType::MAP_KEY_VALUE => return "map".into(),
        _ => {}
    }
    if !field.is_primitive() {
        return "struct".into();
    }
    match field.get_physical_type() {
        Physical::BOOLEAN => "bool",
        Physical::INT32 => "int32",
        Physical::INT64 => "int64",
        Physical::INT96 => "int96",
        Physical::FLOAT => "float32",
        Physical::DOUBLE => "float64",
        Physical::BYTE_ARRAY => "binary",
        Physical::FIXED_LEN_BYTE_ARRAY => "fixed_binary",
    }
    .into()
}

fn unit(unit: &TimeUnit) -> &'static str {
    match unit {
        TimeUnit::MILLIS => "ms",
        TimeUnit::MICROS => "us",
        TimeUnit::NANOS => "ns",
    }
}

/// A record field as a profiled value.
fn value(field: &Field) -> Value {
    match field {
        Field::Null => Value::Null,
        Field::Bool(b) => Value::Bool(*b),
        Field::Byte(i) => Value::Int(i64::from(*i)),
        Field::Short(i) => Value::Int(i64::from(*i)),
        Field::Int(i) => Value::Int(i64::from(*i)),
        Field::Long(i) => Value::Int(*i),
        Field::UByte(i) => Value::Int(i64::from(*i)),
        Field::UShort(i) => Value::Int(i64::from(*i)),
        Field::UInt(i) => Value::Int(i64::from(*i)),
        #[allow(clippy::cast_precision_loss)]
        Field::ULong(i) => i64::try_from(*i).map_or(Value::Float(*i as f64), Value::Int),
        Field::Float16(f) => Value::Float(f64::from(f32::from(*f))),
        Field::Float(f) => Value::Float(f64::from(*f)),
        Field::Double(f) => Value::Float(*f),
        Field::Decimal(d) => decimal(d.data(), d.scale()).map_or(Value::Null, Value::Float),
        Field::Str(s) => Value::Text(s.clone()),
        Field::Date(days) => Value::Text(date(i64::from(*days))),
        Field::TimestampMillis(ms) => Value::Text(timestamp(*ms, 1_000)),
        Field::TimestampMicros(us) => Value::Text(timestamp(*us, 1_000_000)),
        _ => Value::Text(text(field)),
    }
}

/// A big-endian two's-complement unscaled decimal as a float.
fn decimal(bytes: &[u8], scale: i32) -> Option<f64> {
    if bytes.is_empty() || bytes.len() > 16 {
        return None;
    }
    let fill = if bytes[0] & 0x80 == 0 { 0 } else { 0xff };
    let mut buf = [fill; 16];
    buf[16 - bytes.len()..].copy_from_slice(bytes);
    #[allow(clippy::cast_precision_loss)]
    let unscaled = i128::from_be_bytes(buf) as f64;
    Some(unscaled / 10f64.powi(scale))
}

/// Days since 1970-01-01 as `YYYY-MM-DD` (proleptic Gregorian; Howard
/// Hinnant's `civil_from_days`).
#[must_use]
pub fn date(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// `ticks` per second since the epoch as `YYYY-MM-DD HH:MM:SS[.fff]` (UTC).
fn timestamp(ticks: i64, per_second: i64) -> String {
    let seconds = ticks.div_euclid(per_second);
    let fraction = ticks.rem_euclid(per_second);
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    let mut out = format!(
        "{} {:02}:{:02}:{:02}",
        date(days),
        rest / 3_600,
        rest / 60 % 60,
        rest % 60
    );
    if fraction != 0 {
        let width = if per_second == 1_000 { 3 } else { 6 };
        let _ = write!(out, ".{fraction:0width$}");
    }
    out
}

/// A field as text: scalars plainly, nested values JSON-like.
fn text(field: &Field) -> String {
    match field {
        Field::Group(row) => {
            let parts: Vec<String> = row
                .get_column_iter()
                .map(|(name, f)| format!("{name}: {}", nested(f)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        Field::ListInternal(list) => {
            let parts: Vec<String> = list.elements().iter().map(nested).collect();
            format!("[{}]", parts.join(", "))
        }
        Field::MapInternal(map) => {
            let parts: Vec<String> = map
                .entries()
                .iter()
                .map(|(k, v)| format!("{}: {}", nested(k), nested(v)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        Field::Bytes(b) => format!("<{} bytes>", b.len()),
        Field::TimeMillis(ms) => timestamp(i64::from(*ms), 1_000)[11..].to_owned(),
        Field::TimeMicros(us) => timestamp(*us, 1_000_000)[11..].to_owned(),
        other => match value(other) {
            Value::Null => "null".into(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Text(t) => t,
        },
    }
}

/// A value inside a nested one; strings are quoted.
fn nested(field: &Field) -> String {
    match field {
        Field::Str(s) => format!("{s:?}"),
        other => text(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_days_and_ticks_become_iso_text() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(19_783), "2024-03-01");
        assert_eq!(date(-1), "1969-12-31");
        assert_eq!(
            timestamp(1_709_294_400_123, 1_000),
            "2024-03-01 12:00:00.123"
        );
        assert_eq!(timestamp(0, 1_000_000), "1970-01-01 00:00:00");
    }

    #[test]
    fn decimals_are_scaled() {
        assert_eq!(decimal(&[0x04, 0xd2], 2), Some(12.34));
        assert_eq!(decimal(&[0xff, 0x38], 1), Some(-20.0));
        assert_eq!(decimal(&[], 1), None);
    }
}
