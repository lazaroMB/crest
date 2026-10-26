//! One-line reproduce commands: crest Lua, Python (scipy/statsmodels) and R.
//!
//! The Lua line is the request itself as an `app.compute.<kind>{...}`
//! table (a file-backed data set becomes `data = '<path>'`). The Python and
//! R lines first build a data frame `df`, from the file when the data came
//! from one, else from the values themselves, so the line runs as is.

use application::ports::compute::{ColumnValues, DataColumn, DataSet};
use serde_json::Value;

/// Above this many cells inline data is not written out; the lines read
/// the data by its source name instead.
const MAX_INLINE_CELLS: usize = 2000;

/// `app.compute.<function>{...}` from the request JSON.
pub fn lua(function: &str, request: &Value, data: &DataSet) -> String {
    let mut request = request.clone();
    if let Value::Object(map) = &mut request {
        map.shift_remove("kind");
        if let Some(path) = &data.source.path {
            map.insert("data".into(), Value::String(path.clone()));
        } else if let Some(Value::Object(data)) = map.get_mut("data") {
            data.shift_remove("source");
        }
    }
    format!("app.compute.{function}{}", lua_value(&request))
}

fn lua_value(value: &Value) -> String {
    match value {
        Value::Null => "nil".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => lua_string(s),
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(lua_value).collect();
            format!("{{{}}}", items.join(", "))
        }
        Value::Object(map) => {
            let fields: Vec<String> = map
                .iter()
                .map(|(k, v)| {
                    let key = if is_identifier(k) {
                        k.clone()
                    } else {
                        format!("[{}]", lua_string(k))
                    };
                    format!("{key} = {}", lua_value(v))
                })
                .collect();
            format!("{{{}}}", fields.join(", "))
        }
    }
}

fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn lua_string(s: &str) -> String {
    quoted(s, '\'')
}

fn quoted(s: &str, quote: char) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// A Python string literal.
pub fn py_str(s: &str) -> String {
    quoted(s, '\'')
}

/// An R string literal.
pub fn r_str(s: &str) -> String {
    quoted(s, '"')
}

fn inline(data: &DataSet, used: &[DataColumn]) -> bool {
    data.source.path.is_none()
        && used.iter().map(DataColumn::len).sum::<usize>() <= MAX_INLINE_CELLS
}

fn source_name(data: &DataSet) -> String {
    data.source
        .path
        .clone()
        .unwrap_or_else(|| data.source.name.clone())
}

/// Python that defines `df` with the `used` columns.
pub fn py_frame(data: &DataSet, used: &[DataColumn]) -> String {
    if !inline(data, used) {
        return format!(
            "import pandas as pd; df = pd.read_csv({})",
            py_str(&source_name(data))
        );
    }
    let columns: Vec<String> = used
        .iter()
        .map(|c| {
            let values: Vec<String> = match &c.values {
                ColumnValues::Numeric(v) => v
                    .iter()
                    .map(|x| x.map_or("float('nan')".into(), |x| format!("{x}")))
                    .collect(),
                ColumnValues::Categorical(v) => v
                    .iter()
                    .map(|x| x.as_deref().map_or("None".into(), py_str))
                    .collect(),
            };
            format!("{}: [{}]", py_str(&c.name), values.join(", "))
        })
        .collect();
    format!(
        "import pandas as pd; df = pd.DataFrame({{{}}})",
        columns.join(", ")
    )
}

/// R that defines `df` with the `used` columns.
pub fn r_frame(data: &DataSet, used: &[DataColumn]) -> String {
    if !inline(data, used) {
        return format!("df <- read.csv({})", r_str(&source_name(data)));
    }
    let syntactic = used.iter().all(|c| r_syntactic(&c.name));
    let columns: Vec<String> = used
        .iter()
        .map(|c| {
            let values: Vec<String> = match &c.values {
                ColumnValues::Numeric(v) => v
                    .iter()
                    .map(|x| x.map_or("NA".into(), |x| format!("{x}")))
                    .collect(),
                ColumnValues::Categorical(v) => v
                    .iter()
                    .map(|x| x.as_deref().map_or("NA".into(), r_str))
                    .collect(),
            };
            format!("{} = c({})", r_name(&c.name), values.join(", "))
        })
        .collect();
    let check = if syntactic {
        ""
    } else {
        ", check.names = FALSE"
    };
    format!("df <- data.frame({}{check})", columns.join(", "))
}

fn r_syntactic(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '.')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
}

/// A column name in an R formula or argument list (backticks when not
/// syntactic).
pub fn r_name(name: &str) -> String {
    if r_syntactic(name) {
        name.to_owned()
    } else {
        format!("`{name}`")
    }
}

/// `df$name` or `df[["name"]]`.
pub fn r_col(name: &str) -> String {
    if r_syntactic(name) {
        format!("df${name}")
    } else {
        format!("df[[{}]]", r_str(name))
    }
}

/// `df['name']`.
pub fn py_col(name: &str) -> String {
    format!("df[{}]", py_str(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lua_writes_the_request_as_a_table() {
        let data = DataSet::inline(vec![DataColumn::numeric("x", &[1.0, 2.5])]);
        let request = json!({"kind": "describe", "data": {"source": {"name": "inline"},
            "columns": [{"name": "x", "values": [1.0, null]}]}, "by": null, "my var": "it's"});
        assert_eq!(
            lua("describe", &request, &data),
            "app.compute.describe{data = {columns = {{name = 'x', values = {1.0, nil}}}}, \
             by = nil, ['my var'] = 'it\\'s'}"
        );
    }

    #[test]
    fn frames_embed_inline_data_or_read_the_file() {
        let mut data = DataSet::inline(vec![
            DataColumn::numeric("x", &[1.0, 2.5]),
            DataColumn::categorical("my g", &["a", "b"]),
        ]);
        let used = data.columns.clone();
        assert_eq!(
            py_frame(&data, &used),
            "import pandas as pd; df = pd.DataFrame({'x': [1, 2.5], 'my g': ['a', 'b']})"
        );
        assert_eq!(
            r_frame(&data, &used),
            "df <- data.frame(x = c(1, 2.5), `my g` = c(\"a\", \"b\"), check.names = FALSE)"
        );
        data.source.path = Some("data/m.csv".into());
        assert_eq!(r_frame(&data, &used), "df <- read.csv(\"data/m.csv\")");
        assert_eq!(r_col("my g"), "df[[\"my g\"]]");
    }
}
