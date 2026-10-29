//! Entity-relationship diagrams from SQL DDL, on `sqlparser`.
//!
//! The text is split into statements (on `;` outside quotes, comments and
//! `$$` bodies) and each is parsed on its own, so a function body or a
//! dialect extension the parser does not know skips one statement, not
//! the file. `CREATE TABLE` gives a table with its columns (type, `NOT
//! NULL`, primary key, unique, inline `REFERENCES`) and its table
//! constraints; `ALTER TABLE ... ADD CONSTRAINT` / `ADD COLUMN` add to it.
//! Each foreign key becomes an edge from the referencing table to the
//! referenced one, labelled with the columns.

use domain::devops::diagram::{
    DiagramSource, DiagramSpec, ErColumn, ErTable, MAX_TABLES, MAX_WARNINGS,
};
use domain::widget::graph::{FlowDirection, GraphEdge, GraphNode, GraphSpec, LayoutKind};
use serde_json::{Map, Value};
use sqlparser::ast::{
    AlterTableOperation, ColumnDef, ColumnOption, Expr, ForeignKeyConstraint, ObjectName,
    ObjectNamePart, Statement, TableConstraint,
};
use sqlparser::dialect::{Dialect, GenericDialect, dialect_from_str};
use sqlparser::parser::Parser;

/// Dialects accepted by name, for messages.
pub const DIALECTS: &str = "generic, postgresql, mysql, sqlite, mssql, snowflake, bigquery, duckdb, clickhouse, hive, redshift, ansi";

/// A foreign key found in the DDL: the table, its columns, the target
/// table and columns.
struct ForeignKey {
    table: String,
    columns: Vec<String>,
    target: String,
    target_columns: Vec<String>,
    name: Option<String>,
}

/// Splits SQL into statements on `;` outside strings, quoted names,
/// comments and dollar-quoted bodies.
fn statements(sql: &str) -> Vec<&str> {
    let bytes = sql.as_bytes();
    let mut out = Vec::new();
    let (mut start, mut i) = (0, 0);
    while i < bytes.len() {
        match bytes[i] {
            b'\'' | b'"' | b'`' => {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 1;
            }
            b'$' => {
                // $tag$ ... $tag$
                let tag_end = bytes[i + 1..]
                    .iter()
                    .position(|b| *b == b'$')
                    .map(|p| i + 1 + p);
                if let Some(end) = tag_end
                    && bytes[i + 1..end]
                        .iter()
                        .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
                {
                    let tag = &sql[i..=end];
                    match sql[end + 1..].find(tag) {
                        Some(close) => i = end + 1 + close + tag.len() - 1,
                        None => i = bytes.len(),
                    }
                }
            }
            b';' => {
                out.push(&sql[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(&sql[start.min(sql.len())..]);
    out.into_iter()
        .filter(|s| {
            s.lines()
                .map(|l| l.split("--").next().unwrap_or("").trim())
                .any(|l| !l.is_empty())
        })
        .collect()
}

/// A name without quotes: `public.users`.
fn object_name(name: &ObjectName) -> String {
    name.0
        .iter()
        .map(|part| match part {
            ObjectNamePart::Identifier(ident) => ident.value.clone(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn expr_name(expr: &Expr) -> String {
    match expr {
        Expr::Identifier(ident) => ident.value.clone(),
        Expr::CompoundIdentifier(parts) => {
            parts.last().map(|p| p.value.clone()).unwrap_or_default()
        }
        other => other.to_string(),
    }
}

fn foreign_key(table: &str, fk: &ForeignKeyConstraint, columns: Vec<String>) -> ForeignKey {
    ForeignKey {
        table: table.to_owned(),
        columns,
        target: object_name(&fk.foreign_table),
        target_columns: fk
            .referred_columns
            .iter()
            .map(|c| c.value.clone())
            .collect(),
        name: fk.name.as_ref().map(|n| n.value.clone()),
    }
}

fn column(table: &str, def: &ColumnDef, fks: &mut Vec<ForeignKey>) -> ErColumn {
    let mut col = ErColumn {
        name: def.name.value.clone(),
        data_type: def.data_type.to_string().to_ascii_lowercase(),
        primary_key: false,
        nullable: true,
        unique: false,
        references: None,
    };
    for option in &def.options {
        match &option.option {
            ColumnOption::NotNull => col.nullable = false,
            ColumnOption::PrimaryKey(_) => {
                col.primary_key = true;
                col.nullable = false;
            }
            ColumnOption::Unique(_) => col.unique = true,
            ColumnOption::ForeignKey(fk) => {
                fks.push(foreign_key(table, fk, vec![col.name.clone()]));
            }
            _ => {}
        }
    }
    col
}

fn constraint(table: &mut ErTable, c: &TableConstraint, fks: &mut Vec<ForeignKey>) {
    match c {
        TableConstraint::PrimaryKey(pk) => {
            for ic in &pk.columns {
                let name = expr_name(&ic.column.expr);
                if let Some(col) = table.columns.iter_mut().find(|c| c.name == name) {
                    col.primary_key = true;
                    col.nullable = false;
                }
            }
        }
        TableConstraint::Unique(u) if u.columns.len() == 1 => {
            let name = expr_name(&u.columns[0].column.expr);
            if let Some(col) = table.columns.iter_mut().find(|c| c.name == name) {
                col.unique = true;
            }
        }
        TableConstraint::ForeignKey(fk) => {
            let columns = fk.columns.iter().map(|c| c.value.clone()).collect();
            fks.push(foreign_key(&table.name, fk, columns));
        }
        _ => {}
    }
}

/// Whether a statement that failed to parse was one we care about.
fn is_table_ddl(statement: &str) -> bool {
    let words: Vec<String> = statement
        .lines()
        .map(|l| l.split("--").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .take(6)
        .map(str::to_ascii_lowercase)
        .collect();
    let has = |w: &str| words.iter().any(|x| x == w);
    (has("create") || has("alter")) && has("table")
}

/// The table `name` refers to: the same name, else the same last part
/// (`public.users` for `users`).
fn resolve(tables: &[ErTable], name: &str) -> Option<usize> {
    tables.iter().position(|t| t.name == name).or_else(|| {
        let last = name.rsplit('.').next().unwrap_or(name);
        tables
            .iter()
            .position(|t| t.name.rsplit('.').next() == Some(last))
    })
}

/// Reads the tables and foreign keys of `sql` in `dialect` (generic when
/// absent).
///
/// # Errors
///
/// A message for an unknown dialect or DDL without any `CREATE TABLE`.
pub fn parse(sql: &str, dialect: Option<&str>, path: Option<&str>) -> Result<DiagramSpec, String> {
    let name = path.unwrap_or("the SQL");
    let dialect_name = dialect.map(str::trim).filter(|d| !d.is_empty());
    let dialect: Box<dyn Dialect> = match dialect_name {
        None => Box::new(GenericDialect {}),
        Some(d) => dialect_from_str(d)
            .ok_or_else(|| format!("unknown SQL dialect {d:?}; one of {DIALECTS}"))?,
    };
    let mut tables: Vec<ErTable> = Vec::new();
    let mut fks: Vec<ForeignKey> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    for text in statements(sql) {
        let parsed = match Parser::parse_sql(dialect.as_ref(), text) {
            Ok(parsed) => parsed,
            Err(err) => {
                // Other statements (functions, grants, inserts) do not
                // matter to the diagram.
                if is_table_ddl(text) && warnings.len() < MAX_WARNINGS {
                    let head: String = text
                        .split_whitespace()
                        .take(4)
                        .collect::<Vec<_>>()
                        .join(" ");
                    warnings.push(format!("skipped `{head} …`: {err}"));
                }
                continue;
            }
        };
        for statement in parsed {
            match statement {
                Statement::CreateTable(create) => {
                    if tables.len() >= MAX_TABLES {
                        continue;
                    }
                    let table_name = object_name(&create.name);
                    let mut table = ErTable {
                        name: table_name.clone(),
                        columns: create
                            .columns
                            .iter()
                            .map(|c| column(&table_name, c, &mut fks))
                            .collect(),
                    };
                    for c in &create.constraints {
                        constraint(&mut table, c, &mut fks);
                    }
                    match tables.iter().position(|t| t.name == table.name) {
                        Some(ix) => tables[ix] = table,
                        None => tables.push(table),
                    }
                }
                Statement::AlterTable(alter) => {
                    let table_name = object_name(&alter.name);
                    let Some(ix) = resolve(&tables, &table_name) else {
                        continue;
                    };
                    for op in &alter.operations {
                        match op {
                            AlterTableOperation::AddConstraint { constraint: c, .. } => {
                                constraint(&mut tables[ix], c, &mut fks);
                            }
                            AlterTableOperation::AddColumn { column_def, .. } => {
                                let col = column(&tables[ix].name.clone(), column_def, &mut fks);
                                tables[ix].columns.push(col);
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if tables.is_empty() {
        let mut message = format!("`{name}` has no CREATE TABLE statement");
        if let Some(first) = warnings.first() {
            message.push_str(&format!(" that parses ({first})"));
        }
        return Err(message);
    }
    // Foreign keys: mark the columns, add the edges.
    let mut edges = Vec::new();
    for fk in &fks {
        let (Some(from), Some(to)) = (resolve(&tables, &fk.table), resolve(&tables, &fk.target))
        else {
            if warnings.len() < MAX_WARNINGS {
                warnings.push(format!(
                    "foreign key of {} references {}, which is not in the file",
                    fk.table, fk.target
                ));
            }
            continue;
        };
        let target_columns = if fk.target_columns.is_empty() {
            tables[to]
                .primary_key()
                .into_iter()
                .map(str::to_owned)
                .collect()
        } else {
            fk.target_columns.clone()
        };
        for (i, c) in fk.columns.iter().enumerate() {
            let target_col = target_columns.get(i).cloned().unwrap_or_default();
            let target = if target_col.is_empty() {
                tables[to].name.clone()
            } else {
                format!("{}.{target_col}", tables[to].name)
            };
            if let Some(col) = tables[from].columns.iter_mut().find(|x| &x.name == c) {
                col.references = Some(target);
            }
        }
        let mut label = format!("{} → {}", fk.columns.join(", "), target_columns.join(", "));
        if let Some(name) = &fk.name {
            label = format!("{label} ({name})");
        }
        edges.push(GraphEdge {
            from: tables[from].name.clone(),
            to: tables[to].name.clone(),
            label: Some(label),
            weight: None,
        });
    }
    let nodes = tables
        .iter()
        .map(|t| {
            let mut attrs = Map::new();
            for c in &t.columns {
                let mut text = c.data_type.clone();
                if c.primary_key {
                    text.push_str(" PK");
                }
                if let Some(r) = &c.references {
                    text.push_str(&format!(" → {r}"));
                }
                if !c.nullable && !c.primary_key {
                    text.push_str(" not null");
                }
                attrs.insert(c.name.clone(), Value::String(text));
            }
            GraphNode {
                id: t.name.clone(),
                label: Some(format!("{} ({})", t.name, t.columns.len())),
                group: t.name.rsplit_once('.').map(|(schema, _)| schema.to_owned()),
                kind: Some("table".to_owned()),
                attrs,
            }
        })
        .collect();
    let graph = GraphSpec {
        title: None,
        directed: true,
        layout: LayoutKind::Layered,
        direction: FlowDirection::Right,
        seed: None,
        nodes,
        edges,
        groups: Vec::new(),
    };
    Ok(DiagramSpec {
        title: None,
        path: path.map(str::to_owned),
        source: DiagramSource::SqlDdl,
        dialect: Some(dialect_name.unwrap_or("generic").to_ascii_lowercase()),
        graph,
        tables,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statements_split_outside_quotes_comments_and_bodies() {
        let sql = "create table a (x text default ';'); -- c;\n\
                   create function f() returns int as $$ select 1; $$ language sql;\n\
                   /* ; */ create table b (y int)";
        let parts = statements(sql);
        assert_eq!(parts.len(), 3, "{parts:?}");
        assert!(parts[1].contains("$$ select 1; $$"));
        assert!(parts[2].trim_start().starts_with("/* ; */"));
    }
}
