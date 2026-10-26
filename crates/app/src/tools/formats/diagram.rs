//! `show_dot` (a Graphviz DOT graph) and `show_er` (an
//! entity-relationship diagram from SQL DDL) on the graph widget.
//!
//! Both draw what a file (or a pasted source) says, unlike
//! `show_mermaid`, which draws what the model writes. The model gets the
//! shape of the graph: counts, clusters, the most connected nodes,
//! sources, sinks and cycles; for DDL, the tables with their keys and
//! the foreign keys.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::devops::diagram::DiagramSpec;
use domain::permission::ToolEffect;
use domain::widget::graph::DRAW_LIMIT;
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::devops::MAX_TOP;
use super::{FormatToolPorts, MAX_LISTED, invalid, one_line, read};
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::usecases::compute::offloaded;

/// Largest inline source, in bytes.
pub const MAX_INLINE_SOURCE: usize = 4 * 1024 * 1024;
/// Tables listed to the model.
pub const MAX_TABLES_LISTED: usize = 40;

/// The text of `path` or the inline source, with a name for messages.
pub(super) async fn source_text(
    ports: &FormatToolPorts,
    path: Option<&str>,
    inline: Option<String>,
) -> Result<String, String> {
    match path {
        Some(path) => {
            let bytes = read(ports.fs.as_ref(), path).await?;
            String::from_utf8(bytes).map_err(|_| format!("`{path}` is not UTF-8 text"))
        }
        None => Ok(inline.unwrap_or_default()),
    }
}

/// Exactly one of `path` and the inline source, or an input error.
pub(super) fn one_of(
    tool: &str,
    path: Option<String>,
    inline: Option<String>,
    inline_name: &str,
) -> Result<(Option<String>, Option<String>), ToolError> {
    let path = path.filter(|p| !p.trim().is_empty());
    let inline = inline.filter(|s| !s.trim().is_empty());
    match (&path, &inline) {
        (Some(_), Some(_)) => Err(invalid(
            tool,
            format!("give `path` or `{inline_name}`, not both"),
        )),
        (None, None) => Err(invalid(tool, format!("give `path` or `{inline_name}`"))),
        (None, Some(s)) if s.len() > MAX_INLINE_SOURCE => Err(invalid(
            tool,
            format!("`{inline_name}` is too large: save it to a file and pass `path`"),
        )),
        _ => Ok((path, inline)),
    }
}

// show_dot

/// Input of `show_dot`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowDot {
    /// A Graphviz `.dot` / `.gv` file of the workspace (from cargo
    /// depgraph, terraform graph, pydeps, Bazel query --output graph...).
    /// Give this or `source`.
    #[serde(default)]
    pub path: Option<String>,
    /// DOT source inline (`digraph { a -> b }`). Give this or `path`.
    #[serde(default)]
    pub source: Option<String>,
    /// Title shown above the graph; defaults to the graph's label or name.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_dot`: a DOT graph on the graph widget.
#[derive(Clone)]
pub struct ShowDotTool {
    ports: FormatToolPorts,
}

impl ShowDotTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_dot`.
pub const SHOW_DOT: &str = "show_dot";

#[async_trait]
impl TypedTool for ShowDotTool {
    type Input = ShowDot;

    fn name(&self) -> &str {
        SHOW_DOT
    }

    fn description(&self) -> &str {
        "Shows a Graphviz DOT graph (a workspace file in `path`, or `source`) \
         as an interactive node-link graph in the user's timeline: layered \
         layout in the graph's rankdir, clusters as boxes, nodes colored by \
         their DOT color, edge labels, pan, zoom and a click to focus a \
         node's neighbourhood. Returns the node, edge and cluster counts, \
         the most connected nodes, sources, sinks and whether it has \
         cycles: do not repeat the graph in your reply."
    }

    fn effect(&self, _input: &ShowDot) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(&self, input: ShowDot, _ctx: &ToolContext) -> Result<PreparedCall, ToolError> {
        let (path, inline) = one_of(SHOW_DOT, input.path, input.source, "source")?;
        let subject = path.clone().unwrap_or_else(|| "DOT source".to_owned());
        let ports = self.ports.clone();
        let title = input.title.filter(|t| !t.trim().is_empty());
        let run = async move {
            let text = match source_text(&ports, path.as_deref(), inline).await {
                Ok(text) => text,
                Err(message) => return Ok(ToolOutput::error(message)),
            };
            let formats = Arc::clone(&ports.formats);
            let at = path.clone();
            let parsed = offloaded(&ports.offload, move || {
                formats.parse_dot(&text, at.as_deref())
            })
            .await;
            let mut spec = match parsed {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => return Ok(ToolOutput::error(err.to_string())),
                None => return Ok(ToolOutput::error("the DOT parser did not run")),
            };
            spec.title = title;
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(err.to_string()));
            }
            let summary = dot_summary(path.as_deref(), &spec);
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::Diagram(Box::new(spec))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// `1 table`, `3 tables`.
fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

fn names(spec: &DiagramSpec, ixs: impl Iterator<Item = usize>) -> Vec<String> {
    ixs.map(|ix| one_line(spec.graph.nodes[ix].text(), 80))
        .collect()
}

/// What the model gets back from `show_dot`.
#[must_use]
pub fn dot_summary(path: Option<&str>, spec: &DiagramSpec) -> String {
    let g = &spec.graph;
    let mut out = path.map_or_else(|| "DOT graph".to_owned(), |p| format!("DOT graph `{p}`"));
    if let Some(title) = spec.title.as_ref().or(g.title.as_ref()) {
        let _ = write!(out, " ({})", one_line(title, 80));
    }
    let _ = write!(
        out,
        " shown to the user: {}, {}, {}",
        if g.directed { "directed" } else { "undirected" },
        count(g.nodes.len(), "node", "nodes"),
        count(g.edges.len(), "edge", "edges")
    );
    let clusters = g.groups_in_use();
    if !clusters.is_empty() {
        let labels: Vec<String> = clusters
            .iter()
            .take(MAX_LISTED)
            .map(|id| one_line(g.group_label(id), 60))
            .collect();
        let _ = write!(
            out,
            ", {} ({})",
            count(clusters.len(), "cluster", "clusters"),
            labels.join(", ")
        );
    }
    if g.directed {
        out.push_str(if spec.has_cycle() {
            ", with cycles"
        } else {
            ", acyclic"
        });
    }
    out.push('.');
    if g.nodes.len() > DRAW_LIMIT {
        let _ = write!(
            out,
            " It is too large to draw (more than {DRAW_LIMIT} nodes): the user sees this summary."
        );
    }
    let degrees = spec.degrees();
    let top: Vec<String> = degrees
        .iter()
        .filter(|d| d.incoming + d.outgoing > 0)
        .take(MAX_TOP)
        .map(|d| {
            let name = one_line(g.nodes[d.node].text(), 80);
            if g.directed {
                format!("{name} (in {}, out {})", d.incoming, d.outgoing)
            } else {
                format!("{name} ({})", d.incoming + d.outgoing)
            }
        })
        .collect();
    if !top.is_empty() {
        let _ = write!(out, "\nMost connected: {}", top.join(", "));
    }
    if g.directed {
        let mut by_node = degrees.clone();
        by_node.sort_by_key(|d| d.node);
        let sources = names(
            spec,
            by_node
                .iter()
                .filter(|d| d.incoming == 0 && d.outgoing > 0)
                .map(|d| d.node)
                .take(MAX_LISTED),
        );
        let sinks = names(
            spec,
            by_node
                .iter()
                .filter(|d| d.outgoing == 0 && d.incoming > 0)
                .map(|d| d.node)
                .take(MAX_LISTED),
        );
        if !sources.is_empty() {
            let _ = write!(out, "\nSources (no incoming edge): {}", sources.join(", "));
        }
        if !sinks.is_empty() {
            let _ = write!(out, "\nSinks (no outgoing edge): {}", sinks.join(", "));
        }
    }
    let isolated = degrees
        .iter()
        .filter(|d| d.incoming + d.outgoing == 0)
        .count();
    if isolated > 0 {
        let _ = write!(
            out,
            "\n{} without an edge.",
            count(isolated, "node", "nodes")
        );
    }
    for warning in &spec.warnings {
        let _ = write!(out, "\nNote: {}", one_line(warning, 200));
    }
    out
}

// show_er

/// Input of `show_er`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowEr {
    /// A SQL file of the workspace with `CREATE TABLE` (and `ALTER TABLE
    /// ... ADD CONSTRAINT`) statements: a schema dump (`pg_dump
    /// --schema-only`, `mysqldump --no-data`, `sqlite3 .schema`) or
    /// migrations. Give this or `sql`.
    #[serde(default)]
    pub path: Option<String>,
    /// The DDL inline. Give this or `path`.
    #[serde(default)]
    pub sql: Option<String>,
    /// SQL dialect: generic (default), postgresql, mysql, sqlite, mssql,
    /// snowflake, bigquery, duckdb, clickhouse, hive, redshift or ansi.
    #[serde(default)]
    pub dialect: Option<String>,
    /// Title shown above the diagram; defaults to the file name.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_er`: an ER diagram from DDL on the graph widget.
#[derive(Clone)]
pub struct ShowErTool {
    ports: FormatToolPorts,
}

impl ShowErTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_er`.
pub const SHOW_ER: &str = "show_er";

#[async_trait]
impl TypedTool for ShowErTool {
    type Input = ShowEr;

    fn name(&self) -> &str {
        SHOW_ER
    }

    fn description(&self) -> &str {
        "Shows an entity-relationship diagram drawn from SQL DDL (a schema \
         dump or migrations in `path`, or `sql`) in the user's timeline: a \
         node per table, an edge per foreign key labelled with its \
         columns, and a table list with column types and PK / FK badges; \
         focus a table to see its columns. Statements that do not parse \
         are skipped and reported. Returns the tables with their keys and \
         the foreign keys: do not repeat the schema in your reply."
    }

    fn effect(&self, _input: &ShowEr) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(&self, input: ShowEr, _ctx: &ToolContext) -> Result<PreparedCall, ToolError> {
        let (path, inline) = one_of(SHOW_ER, input.path, input.sql, "sql")?;
        let subject = path.clone().unwrap_or_else(|| "SQL DDL".to_owned());
        let ports = self.ports.clone();
        let title = input.title.filter(|t| !t.trim().is_empty());
        let dialect = input.dialect.filter(|d| !d.trim().is_empty());
        let run = async move {
            let text = match source_text(&ports, path.as_deref(), inline).await {
                Ok(text) => text,
                Err(message) => return Ok(ToolOutput::error(message)),
            };
            let formats = Arc::clone(&ports.formats);
            let at = path.clone();
            let parsed = offloaded(&ports.offload, move || {
                formats.parse_ddl(&text, dialect.as_deref(), at.as_deref())
            })
            .await;
            let mut spec = match parsed {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => return Ok(ToolOutput::error(err.to_string())),
                None => return Ok(ToolOutput::error("the SQL parser did not run")),
            };
            spec.title = title;
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(err.to_string()));
            }
            let summary = er_summary(path.as_deref(), &spec);
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::Diagram(Box::new(spec))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// What the model gets back from `show_er`.
#[must_use]
pub fn er_summary(path: Option<&str>, spec: &DiagramSpec) -> String {
    let mut out = path.map_or_else(
        || "ER diagram".to_owned(),
        |p| format!("ER diagram of `{p}`"),
    );
    if let Some(dialect) = &spec.dialect {
        let _ = write!(out, " ({dialect})");
    }
    let _ = write!(
        out,
        " shown to the user: {}, {}.",
        count(spec.tables.len(), "table", "tables"),
        count(spec.graph.edges.len(), "foreign key", "foreign keys")
    );
    let tables: Vec<String> = spec
        .tables
        .iter()
        .take(MAX_TABLES_LISTED)
        .map(|t| {
            let pk = t.primary_key();
            let key = if pk.is_empty() {
                "no primary key".to_owned()
            } else {
                format!("PK {}", pk.join(", "))
            };
            format!(
                "{} ({}, {key})",
                t.name,
                count(t.columns.len(), "column", "columns")
            )
        })
        .collect();
    if !tables.is_empty() {
        let _ = write!(out, "\nTables: {}", tables.join("; "));
        if spec.tables.len() > MAX_TABLES_LISTED {
            let _ = write!(out, "; … {} more", spec.tables.len() - MAX_TABLES_LISTED);
        }
    }
    let fks: Vec<String> = spec
        .tables
        .iter()
        .flat_map(|t| {
            t.columns.iter().filter_map(move |c| {
                Some(format!(
                    "{}.{} → {}",
                    t.name,
                    c.name,
                    c.references.as_ref()?
                ))
            })
        })
        .collect();
    if !fks.is_empty() {
        let shown: Vec<&str> = fks
            .iter()
            .take(MAX_TABLES_LISTED)
            .map(String::as_str)
            .collect();
        let _ = write!(out, "\nForeign keys: {}", shown.join("; "));
        if fks.len() > MAX_TABLES_LISTED {
            let _ = write!(out, "; … {} more", fks.len() - MAX_TABLES_LISTED);
        }
    }
    let no_pk: Vec<&str> = spec
        .tables
        .iter()
        .filter(|t| t.primary_key().is_empty())
        .map(|t| t.name.as_str())
        .take(MAX_LISTED)
        .collect();
    if !no_pk.is_empty() {
        let _ = write!(out, "\nWithout a primary key: {}", no_pk.join(", "));
    }
    let isolated: Vec<&str> = spec
        .isolated_tables()
        .into_iter()
        .take(MAX_LISTED)
        .collect();
    if !isolated.is_empty() && spec.tables.len() > 1 {
        let _ = write!(
            out,
            "\nNot related to another table: {}",
            isolated.join(", ")
        );
    }
    for warning in &spec.warnings {
        let _ = write!(out, "\nNote: {}", one_line(warning, 200));
    }
    out
}

#[cfg(test)]
mod tests {
    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use serde_json::{Value, json};

    use super::*;
    use crate::testing::formats::FakeFormats;
    use crate::testing::fs::{InMemoryFs, test_root};
    use crate::tools::registry::{Tool, Typed};
    use crate::usecases::compute::inline_offload;

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"))
    }

    fn ports(formats: Arc<FakeFormats>) -> FormatToolPorts {
        let fs = InMemoryFs::new(test_root())
            .with_file("ci.dot", "digraph {}")
            .with_file("db/schema.sql", "create table x (id int);");
        FormatToolPorts {
            formats,
            fs: Arc::new(fs),
            offload: inline_offload(),
        }
    }

    async fn call(tool: &dyn Tool, input: Value) -> Result<ToolOutput, ToolError> {
        let prepared = tool.prepare(input, &ctx()).await?;
        prepared.run.await
    }

    fn definition<T: TypedTool + 'static>(tool: T) -> Value {
        let tool = Typed(tool);
        json!({
            "name": tool.name(),
            "description": tool.description(),
            "input_schema": tool.input_schema(),
        })
    }

    #[test]
    fn definitions() {
        let ports = ports(Arc::new(FakeFormats::new()));
        insta::assert_json_snapshot!(
            "show_dot_definition",
            definition(ShowDotTool::new(ports.clone()))
        );
        insta::assert_json_snapshot!("show_er_definition", definition(ShowErTool::new(ports)));
    }

    #[tokio::test]
    async fn a_dot_file_reports_its_shape() {
        let formats = Arc::new(FakeFormats::new());
        let tool = Typed(ShowDotTool::new(ports(Arc::clone(&formats))));
        let out = call(&tool, json!({ "path": "ci.dot" })).await.unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(
            out.text_content(),
            "DOT graph `ci.dot` (pipeline) shown to the user: directed, 4 nodes, 3 edges, 1 \
             cluster (CI), acyclic.\n\
             Most connected: test (in 2, out 1), build (in 0, out 1), deploy (in 1, out 0), \
             lint (in 0, out 1)\n\
             Sources (no incoming edge): build, lint\n\
             Sinks (no outgoing edge): deploy"
        );
        assert!(matches!(out.block_update, Some(BlockKind::Diagram(_))));
        let inline = call(&tool, json!({ "source": "digraph { a -> b }" }))
            .await
            .unwrap();
        assert!(inline.text_content().starts_with("DOT graph (pipeline)"));
        assert_eq!(formats.calls()[1], ("parse_dot".to_owned(), "-".to_owned()));
        let both = tool
            .prepare(json!({ "path": "a", "source": "b" }), &ctx())
            .await;
        assert!(matches!(both, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn ddl_reports_tables_and_keys() {
        let formats = Arc::new(FakeFormats::new());
        let tool = Typed(ShowErTool::new(ports(Arc::clone(&formats))));
        let out = call(
            &tool,
            json!({ "path": "db/schema.sql", "dialect": "postgresql" }),
        )
        .await
        .unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(
            out.text_content(),
            "ER diagram of `db/schema.sql` (postgresql) shown to the user: 2 tables, 1 foreign \
             key.\n\
             Tables: users (1 column, PK id); orders (2 columns, PK id)\n\
             Foreign keys: orders.user_id → users.id"
        );
        let neither = tool.prepare(json!({ "dialect": "mysql" }), &ctx()).await;
        assert!(matches!(neither, Err(ToolError::InvalidInput { .. })));
    }
}
