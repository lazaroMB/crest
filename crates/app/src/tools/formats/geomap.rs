//! `show_map`: GeoJSON features on a pannable map with their CRS, and the
//! box, geodesic area and length the engine measured.
//!
//! The model gets the feature counts by geometry type, the CRS and where
//! it came from, the totals (with the envelope id to cite them by), the
//! largest features and the property keys, never the coordinates.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::permission::ToolEffect;
use domain::science::geomap::{MapSpec, area_text, length_text};
use domain::workspace::WorkspacePath;
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::diagram::one_of;
use super::{FormatToolPorts, invalid, non_blank, one_line, parse_off, shown};
use crate::ports::formats::MapRequest;
use crate::ports::fs::FileSystemPort;
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, TypedTool};

/// Largest GeoJSON file read, in bytes.
pub const MAX_MAP_BYTES: u64 = 64 * 1024 * 1024;
/// Features listed to the model by size.
const LISTED: usize = 8;

/// Input of `show_map`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowMap {
    /// A GeoJSON file of the workspace (`.geojson`, `.json`).
    #[serde(default)]
    pub path: Option<String>,
    /// Inline GeoJSON instead of `path`: a FeatureCollection, Feature or
    /// geometry (an object or its JSON text).
    #[serde(default)]
    pub geojson: Option<Value>,
    /// The CRS the coordinates are in, over the file's own: `EPSG:32632`
    /// or a PROJ string. Leave it out for WGS 84 longitude/latitude
    /// (RFC 7946) or a file with a legacy `crs` member.
    #[serde(default)]
    pub crs: Option<String>,
    /// A property to colour the features by (categories or a numeric
    /// ramp); the user can switch it on the map.
    #[serde(default)]
    pub color_by: Option<String>,
    /// Title shown above the map.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_map`: a GeoJSON map with measures.
#[derive(Clone)]
pub struct ShowMapTool {
    ports: FormatToolPorts,
}

impl ShowMapTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_map`.
pub const SHOW_MAP: &str = "show_map";

/// Reads a GeoJSON file of the workspace (up to [`MAX_MAP_BYTES`])
/// through the guard.
///
/// # Errors
///
/// A message for the caller: bad path, unreadable or too large.
pub async fn read_map(fs: &dyn FileSystemPort, path: &str) -> Result<Vec<u8>, String> {
    let parsed = WorkspacePath::parse(path).map_err(|e| format!("map path `{path}`: {e}"))?;
    fs.read_bytes(&parsed, MAX_MAP_BYTES)
        .await
        .map_err(|e| format!("cannot read `{path}`: {e}"))
}

/// Inline GeoJSON as text: a string as it is, anything else serialised.
#[must_use]
pub fn inline_geojson(value: Option<Value>) -> Option<String> {
    match value {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s),
        Some(other) => Some(other.to_string()),
    }
}

#[async_trait]
impl TypedTool for ShowMapTool {
    type Input = ShowMap;

    fn name(&self) -> &str {
        SHOW_MAP
    }

    fn description(&self) -> &str {
        "Shows GeoJSON (`path`, or inline `geojson`) in the user's timeline \
         as a map with pan, zoom, a scale bar, feature picking with a \
         properties inspector and colouring by a property. Coordinates are \
         WGS 84 longitude/latitude unless the file's legacy `crs` member or \
         `crs` (an EPSG code or PROJ string) says otherwise; they are \
         reprojected to Web Mercator for display. The engine measures each \
         feature's box, geodesic area and length on the WGS 84 ellipsoid. \
         Returns the counts, the CRS, the totals with their envelope id and \
         the largest features; cite these numbers, do not estimate areas \
         or distances from coordinates yourself."
    }

    fn effect(&self, _input: &ShowMap) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(&self, input: ShowMap, _ctx: &ToolContext) -> Result<PreparedCall, ToolError> {
        let call = normalize_map(input).map_err(|e| invalid(SHOW_MAP, e))?;
        let ports = self.ports.clone();
        let subject = call
            .path
            .clone()
            .unwrap_or_else(|| "inline GeoJSON".to_owned());
        let run = async move {
            shown(build_map(&ports, call).await, map_summary, |spec| {
                BlockKind::GeoMap(Box::new(spec))
            })
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// A `show_map` call, checked: the file or the inline GeoJSON as text.
#[derive(Debug, Clone, PartialEq)]
pub struct MapCall {
    /// The GeoJSON file, when given.
    pub path: Option<String>,
    /// The inline GeoJSON as text, when given instead.
    pub geojson: Option<String>,
    /// An EPSG code or PROJ string over the file's.
    pub crs: Option<String>,
    /// The property features are coloured by.
    pub color_by: Option<String>,
    /// Title above the map.
    pub title: Option<String>,
}

/// Checks a `show_map` input: exactly one of `path` and `geojson` (not
/// too large), and the optional texts when not blank.
///
/// # Errors
///
/// A message for the caller: neither or both given, or `geojson` too
/// large.
pub fn normalize_map(input: ShowMap) -> Result<MapCall, String> {
    let (path, geojson) = one_of(input.path, inline_geojson(input.geojson), "geojson")?;
    Ok(MapCall {
        path,
        geojson,
        crs: non_blank(input.crs),
        color_by: non_blank(input.color_by),
        title: non_blank(input.title),
    })
}

/// The map block of a checked call ([`normalize_map`]): reads the file
/// through the guard (or takes the GeoJSON) and measures it off the
/// executor.
///
/// # Errors
///
/// A message for the caller: the file cannot be read or parsed, or the
/// result is not a valid block.
pub async fn build_map(ports: &FormatToolPorts, call: MapCall) -> Result<MapSpec, String> {
    let bytes = match &call.path {
        Some(path) => read_map(ports.fs.as_ref(), path).await?,
        None => call.geojson.unwrap_or_default().into_bytes(),
    };
    let request = MapRequest {
        path: call.path.clone(),
        crs: call.crs,
        color_by: call.color_by,
    };
    let formats = Arc::clone(&ports.formats);
    let name = call.path.as_deref().unwrap_or("inline GeoJSON").to_owned();
    let mut spec = parse_off(ports, "the GeoJSON reader", move || {
        formats.parse_geojson(&bytes, &request)
    })
    .await?
    .map_err(|e| format!("`{name}`: {e}"))?;
    spec.title = call.title;
    spec.validate().map_err(|e| format!("`{name}`: {e}"))?;
    Ok(spec)
}

/// What the model gets back from `show_map`.
#[must_use]
pub fn map_summary(spec: &MapSpec) -> String {
    let kinds: Vec<String> = spec
        .kind_counts()
        .iter()
        .map(|(k, n)| format!("{n} {}", k.name()))
        .collect();
    let mut out = format!(
        "Map `{}` shown to the user: {} features ({})",
        one_line(&spec.display_title(), 80),
        spec.features.len(),
        kinds.join(", ")
    );
    if spec.without_geometry > 0 {
        let _ = write!(out, ", {} without geometry", spec.without_geometry);
    }
    if spec.total_features > spec.features.len() + spec.without_geometry {
        let _ = write!(out, ", {} in the file", spec.total_features);
    }
    let _ = write!(
        out,
        ".\nCRS: {} ({}); displayed in Web Mercator.",
        spec.crs.label(),
        spec.crs.source_text()
    );
    let m = &spec.measures;
    let _ = write!(out, "\nMeasures (envelope `{}`", m.id);
    let mut keys = Vec::new();
    for key in ["area_m2", "length_m"] {
        if m.values.contains_key(key) {
            keys.push(format!("`{key}`"));
        }
    }
    keys.push("`bbox.west/south/east/north`".into());
    keys.push("`feature.<n>.area_m2|length_m|perimeter_m`".into());
    let _ = write!(out, ", keys {}): ", keys.join(", "));
    let mut parts = Vec::new();
    if let Some(v) = m.values.get("area_m2") {
        parts.push(format!("total area {}", area_text(v.v)));
    }
    if let Some(v) = m.values.get("length_m") {
        parts.push(format!("total line length {}", length_text(v.v)));
    }
    if let (Some(w), Some(s), Some(e), Some(n)) = (
        m.values.get("bbox.west"),
        m.values.get("bbox.south"),
        m.values.get("bbox.east"),
        m.values.get("bbox.north"),
    ) {
        parts.push(format!(
            "box {:.5}, {:.5} to {:.5}, {:.5} (lon, lat)",
            w.v, s.v, e.v, n.v
        ));
    }
    out.push_str(&parts.join("; "));
    out.push_str(". Areas and lengths are geodesic on the WGS 84 ellipsoid.");
    // Areas first (largest first), then lines by length.
    let mut sized: Vec<(usize, bool, f64, String)> = spec
        .features
        .iter()
        .enumerate()
        .filter_map(|(ix, f)| {
            f.area_m2
                .map(|a| (ix, true, a, area_text(a)))
                .or_else(|| f.length_m.map(|l| (ix, false, l, length_text(l))))
        })
        .collect();
    sized.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.total_cmp(&a.2)));
    if !sized.is_empty() {
        out.push_str("\nLargest features:");
        for (ix, _, _, text) in sized.iter().take(LISTED) {
            let f = &spec.features[*ix];
            let _ = write!(
                out,
                "\n{}. {} ({}): {text}",
                ix + 1,
                one_line(&f.label(*ix), 60),
                f.kind.name()
            );
        }
    }
    let keys = spec.property_keys();
    if !keys.is_empty() {
        let shown: Vec<&str> = keys.iter().take(20).map(String::as_str).collect();
        let _ = write!(out, "\nProperties: {}", shown.join(", "));
        if keys.len() > shown.len() {
            let _ = write!(out, " and {} more", keys.len() - shown.len());
        }
        out.push('.');
    }
    if let Some(key) = &spec.color_by {
        let _ = write!(out, " Coloured by `{key}`.");
    }
    for w in spec.warnings.iter().take(5) {
        let _ = write!(out, "\nNote: {}", one_line(w, 200));
    }
    out
}

#[cfg(test)]
mod tests {
    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use serde_json::json;

    use super::*;
    use crate::testing::formats::FakeFormats;
    use crate::testing::fs::{InMemoryFs, test_root};
    use crate::tools::registry::{Tool, ToolOutput, Typed};
    use crate::usecases::compute::inline_offload;

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"))
    }

    fn ports() -> FormatToolPorts {
        let fs = InMemoryFs::new(test_root()).with_file("geo/parks.geojson", "{}");
        FormatToolPorts {
            formats: Arc::new(FakeFormats::new()),
            fs: Arc::new(fs),
            offload: inline_offload(),
        }
    }

    async fn call(tool: &dyn Tool, input: Value) -> Result<ToolOutput, ToolError> {
        let prepared = tool.prepare(input, &ctx()).await?;
        prepared.run.await
    }

    #[test]
    fn definition() {
        let tool = Typed(ShowMapTool::new(ports()));
        insta::assert_json_snapshot!(
            "show_map_definition",
            json!({
                "name": tool.name(),
                "description": tool.description(),
                "input_schema": tool.input_schema(),
            })
        );
        assert_eq!(
            inline_geojson(Some(json!({"a": 1}))).as_deref(),
            Some("{\"a\":1}")
        );
    }

    #[tokio::test]
    async fn the_model_gets_counts_crs_and_measures() {
        let tool = Typed(ShowMapTool::new(ports()));
        let out = call(
            &tool,
            json!({ "path": "geo/parks.geojson", "color_by": "kind" }),
        )
        .await
        .unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(
            out.text_content(),
            "Map `parks.geojson` shown to the user: 2 features (1 LineString, 1 Polygon).\n\
             CRS: EPSG:4326 · WGS 84 (default (RFC 7946)); displayed in Web Mercator.\n\
             Measures (envelope `ge_fake`, keys `area_m2`, `length_m`, \
             `bbox.west/south/east/north`, `feature.<n>.area_m2|length_m|perimeter_m`): total \
             area 12.5 ha; total line length 1.20 km; box 13.40000, 52.50000 to 13.42000, \
             52.52000 (lon, lat). Areas and lengths are geodesic on the WGS 84 ellipsoid.\n\
             Largest features:\n\
             1. Tiergarten (Polygon): 12.5 ha\n\
             2. Trail (LineString): 1.20 km\n\
             Properties: name, kind. Coloured by `kind`."
        );
        assert!(matches!(out.block_update, Some(BlockKind::GeoMap(_))));
        let inline = call(
            &tool,
            json!({ "geojson": {"type": "Point", "coordinates": [0, 0]} }),
        )
        .await
        .unwrap();
        assert!(!inline.is_error);
        assert!(call(&tool, json!({})).await.is_err());
        assert!(
            call(&tool, json!({ "path": "a.geojson", "geojson": "{}" }))
                .await
                .is_err()
        );
        let missing = call(&tool, json!({ "path": "geo/none.geojson" }))
            .await
            .unwrap();
        assert!(missing.is_error);
    }
}
