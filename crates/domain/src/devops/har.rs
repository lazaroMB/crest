//! An HTTP archive (HAR) as a waterfall, a request list and an
//! inspector (`show_har`).
//!
//! The adapter reads the `log.entries` of a HAR 1.2 file (as browsers'
//! DevTools, proxies and HTTP clients export it) into one [`HarEntry`]
//! per request, in start order, with times in milliseconds from the start
//! of the first request. Bodies are kept as a preview of at most
//! [`MAX_BODY_PREVIEW`] bytes.
//!
//! Requests carry credentials: cookies, `Authorization` headers, tokens
//! in query strings. [`is_sensitive_header`] and [`redact_url`] say what
//! to hide; the tool's summary for the model never has header values and
//! has URLs redacted, and the inspector masks sensitive values until the
//! user reveals them.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::invalid;
use crate::block::BlockError;
use crate::widget::list::{ListItem, ListSpec};
use crate::widget::timeline::{
    Marker, Span, SpanStatus, TimeUnit, TimelineRow, TimelineSpec, format_duration,
};

/// Requests a block keeps.
pub const MAX_ENTRIES: usize = 5_000;
/// Bytes of a body kept for the preview.
pub const MAX_BODY_PREVIEW: usize = 64 * 1024;
/// Headers kept per request or response.
pub const MAX_HEADERS: usize = 200;
/// The value shown instead of a secret.
pub const REDACTED: &str = "<redacted>";

/// The timing phases of a request, in the order they happen.
pub const PHASES: [&str; 7] = [
    "blocked", "dns", "connect", "ssl", "send", "wait", "receive",
];

const BLOCK: &str = "har";

/// Where a request's time went, in ms (`None` when the phase did not
/// apply, `-1` in the file).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HarTimings {
    /// Queued, waiting for a connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<f64>,
    /// DNS lookup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns: Option<f64>,
    /// TCP connect, without the TLS handshake (the file's `connect`
    /// includes `ssl`; the adapter takes it out).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect: Option<f64>,
    /// TLS handshake.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssl: Option<f64>,
    /// Sending the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send: Option<f64>,
    /// Waiting for the first byte (server time plus a round trip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<f64>,
    /// Reading the response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receive: Option<f64>,
}

impl HarTimings {
    /// Each phase with its time, in [`PHASES`] order.
    #[must_use]
    pub fn phases(&self) -> [(&'static str, Option<f64>); 7] {
        [
            ("blocked", self.blocked),
            ("dns", self.dns),
            ("connect", self.connect),
            ("ssl", self.ssl),
            ("send", self.send),
            ("wait", self.wait),
            ("receive", self.receive),
        ]
    }

    /// Sum of the phases.
    #[must_use]
    pub fn total(&self) -> f64 {
        self.phases().iter().filter_map(|(_, t)| *t).sum()
    }
}

/// A header, query parameter or form field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarHeader {
    /// Name as written.
    pub name: String,
    /// Value as written.
    pub value: String,
}

/// A request or response body.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HarBody {
    /// Its media type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Size in bytes, when the file gave it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// The first [`MAX_BODY_PREVIEW`] bytes of the text; `None` when the
    /// file did not keep it or it is binary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The text was cut to the preview size.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// The file stored it base64-encoded (binary); no preview is kept.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub base64: bool,
}

/// One request and its response.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HarEntry {
    /// `GET`, `POST`...
    pub method: String,
    /// The full URL as requested.
    pub url: String,
    /// `HTTP/1.1`, `h2`...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_version: Option<String>,
    /// Response status; 0 when there was no response (blocked, aborted,
    /// failed).
    #[serde(default)]
    pub status: u16,
    /// Reason phrase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_text: Option<String>,
    /// `document`, `script`, `stylesheet`, `image`, `font`, `xhr`,
    /// `fetch`, `media`, `websocket` or `other`.
    pub resource_type: String,
    /// Media type of the response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Start, in ms from the start of the first request.
    pub start: f64,
    /// Total time, in ms.
    pub time: f64,
    /// Where the time went.
    #[serde(default)]
    pub timings: HarTimings,
    /// Request headers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub request_headers: Vec<HarHeader>,
    /// Response headers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response_headers: Vec<HarHeader>,
    /// Query string parameters.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query: Vec<HarHeader>,
    /// The posted body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body: Option<HarBody>,
    /// The response body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_body: Option<HarBody>,
    /// Bytes on the wire (headers and compressed body).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_size: Option<u64>,
    /// Size of the decoded response body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_size: Option<u64>,
    /// IP address of the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_ip: Option<String>,
    /// Target of a redirect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_url: Option<String>,
    /// Why the request failed (`net::ERR_...`), when the file says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Id of the page it belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
}

impl HarEntry {
    /// Failed: no response, or a 4xx/5xx status.
    #[must_use]
    pub fn failed(&self) -> bool {
        self.status == 0 || self.status >= 400
    }

    /// `2xx`, `3xx`... or `failed` without a response.
    #[must_use]
    pub fn status_class(&self) -> String {
        if self.status == 0 {
            "failed".to_owned()
        } else {
            format!("{}xx", self.status / 100)
        }
    }

    /// The host of the URL.
    #[must_use]
    pub fn host(&self) -> &str {
        let rest = self
            .url
            .split_once("://")
            .map_or(self.url.as_str(), |(_, r)| r);
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let host = &rest[..end];
        host.rsplit_once('@').map_or(host, |(_, h)| h)
    }

    /// The path and query of the URL (`/` when empty), query redacted.
    #[must_use]
    pub fn path(&self) -> String {
        let url = redact_url(&self.url);
        let rest = url.split_once("://").map_or(url.as_str(), |(_, r)| r);
        match rest.find(['/', '?']) {
            Some(at) => rest[at..].to_owned(),
            None => "/".to_owned(),
        }
    }

    /// Bytes to show: on the wire, else the body size.
    #[must_use]
    pub fn size(&self) -> Option<u64> {
        self.transfer_size.or(self.content_size)
    }

    /// Whether any header carries a credential.
    #[must_use]
    pub fn has_credentials(&self) -> bool {
        self.request_headers
            .iter()
            .chain(&self.response_headers)
            .any(|h| is_sensitive_header(&h.name))
    }

    /// The request as a `curl` command; sensitive header values are
    /// replaced by [`REDACTED`] unless `reveal`.
    #[must_use]
    pub fn to_curl(&self, reveal: bool) -> String {
        let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
        let mut out = format!("curl {}", quote(&self.url));
        if self.method != "GET" {
            out.push_str(&format!(" -X {}", self.method));
        }
        for header in &self.request_headers {
            // HTTP/2 pseudo-headers and what curl computes itself.
            let name = header.name.to_ascii_lowercase();
            if name.starts_with(':') || name == "content-length" || name == "host" {
                continue;
            }
            let value = if !reveal && is_sensitive_header(&header.name) {
                REDACTED
            } else {
                header.value.as_str()
            };
            out.push_str(&format!(
                " \\\n  -H {}",
                quote(&format!("{}: {value}", header.name))
            ));
        }
        if let Some(text) = self.request_body.as_ref().and_then(|b| b.text.as_deref()) {
            out.push_str(&format!(" \\\n  --data-raw {}", quote(text)));
        }
        out
    }
}

/// A page load the requests belong to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HarPage {
    /// Id the entries refer to.
    pub id: String,
    /// Title (often the URL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Start, in ms from the first request.
    pub start: f64,
    /// `DOMContentLoaded`, in ms from the page start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_content_load: Option<f64>,
    /// `load`, in ms from the page start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_load: Option<f64>,
}

/// The requests of a HAR file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HarSpec {
    /// Caption; the file name when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Workspace path of the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// What wrote it (`WebInspector 537.36`, `Firefox 131.0`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator: Option<String>,
    /// Wall clock of the first request, in seconds since the epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<f64>,
    /// Page loads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<HarPage>,
    /// Requests in start order.
    pub entries: Vec<HarEntry>,
    /// Requests in the file; more than `entries` when it was cut at
    /// [`MAX_ENTRIES`].
    pub total_entries: usize,
}

/// A request header that carries a credential: cookies, authorization,
/// API keys, tokens and session ids.
#[must_use]
pub fn is_sensitive_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(
        name.as_str(),
        "cookie" | "set-cookie" | "authorization" | "proxy-authorization" | "x-api-key"
    ) || [
        "token", "secret", "api-key", "apikey", "session", "auth", "csrf", "xsrf",
    ]
    .iter()
    .any(|part| name.contains(part))
}

/// Whether a query parameter is a credential (`token`, `key`, `sig`...).
#[must_use]
pub fn is_sensitive_param(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(
        name.as_str(),
        "key" | "code" | "sig" | "signature" | "password" | "pass" | "pwd" | "jwt" | "auth"
    ) || [
        "token",
        "secret",
        "session",
        "api_key",
        "apikey",
        "credential",
        "x-amz-",
    ]
    .iter()
    .any(|part| name.contains(part))
}

/// `url` with the values of credential query parameters
/// ([`is_sensitive_param`]) and any user info replaced by [`REDACTED`].
#[must_use]
pub fn redact_url(url: &str) -> String {
    let (base, fragment) = match url.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (url, None),
    };
    let (head, query) = match base.split_once('?') {
        Some((h, q)) => (h, Some(q)),
        None => (base, None),
    };
    let mut out = match head.split_once("://") {
        Some((scheme, rest)) => {
            let end = rest.find('/').unwrap_or(rest.len());
            match rest[..end].rsplit_once('@') {
                Some((_, host)) => format!("{scheme}://{REDACTED}@{host}{}", &rest[end..]),
                None => head.to_owned(),
            }
        }
        None => head.to_owned(),
    };
    if let Some(query) = query {
        let parts: Vec<String> = query
            .split('&')
            .map(|pair| match pair.split_once('=') {
                Some((name, _)) if is_sensitive_param(name) => format!("{name}={REDACTED}"),
                _ => pair.to_owned(),
            })
            .collect();
        out.push('?');
        out.push_str(&parts.join("&"));
    }
    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(fragment);
    }
    out
}

/// `bytes` as `1.2 kB`, `3.4 MB` (powers of 1000, as DevTools).
#[must_use]
pub fn format_bytes(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let b = bytes as f64;
    match bytes {
        0..1000 => format!("{bytes} B"),
        1000..1_000_000 => format!("{:.1} kB", b / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1} MB", b / 1e6),
        _ => format!("{:.2} GB", b / 1e9),
    }
}

impl HarSpec {
    /// Checks the size and that times are finite and not negative.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        if self.entries.len() > MAX_ENTRIES {
            return Err(invalid(
                BLOCK,
                format!("{} requests; at most {MAX_ENTRIES}", self.entries.len()),
            ));
        }
        if self.total_entries < self.entries.len() {
            return Err(invalid(BLOCK, "total_entries is less than the requests"));
        }
        for (ix, entry) in self.entries.iter().enumerate() {
            let phases = entry.timings.phases();
            let times = phases
                .iter()
                .filter_map(|(_, t)| *t)
                .chain([entry.start, entry.time]);
            if times.into_iter().any(|t| !t.is_finite() || t < 0.0) {
                return Err(invalid(
                    BLOCK,
                    format!(
                        "request {ix} ({}) has a negative or non-finite time",
                        entry.url
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Indices of failed requests.
    #[must_use]
    pub fn failed(&self) -> Vec<usize> {
        (0..self.entries.len())
            .filter(|&ix| self.entries[ix].failed())
            .collect()
    }

    /// Bytes transferred in all.
    #[must_use]
    pub fn transferred(&self) -> u64 {
        self.entries.iter().filter_map(HarEntry::size).sum()
    }

    /// From the first start to the last end, in ms.
    #[must_use]
    pub fn duration(&self) -> f64 {
        self.entries
            .iter()
            .map(|e| e.start + e.time)
            .fold(0.0, f64::max)
    }

    /// The `n` slowest requests, slowest first.
    #[must_use]
    pub fn slowest(&self, n: usize) -> Vec<usize> {
        let mut ixs: Vec<usize> = (0..self.entries.len()).collect();
        ixs.sort_by(|&a, &b| {
            self.entries[b]
                .time
                .total_cmp(&self.entries[a].time)
                .then(a.cmp(&b))
        });
        ixs.truncate(n);
        ixs
    }

    /// Count and bytes per resource type, most requests first.
    #[must_use]
    pub fn by_type(&self) -> Vec<(String, usize, u64)> {
        let mut out: Vec<(String, usize, u64)> = Vec::new();
        for entry in &self.entries {
            let size = entry.size().unwrap_or(0);
            match out.iter_mut().find(|(t, _, _)| *t == entry.resource_type) {
                Some((_, n, bytes)) => {
                    *n += 1;
                    *bytes += size;
                }
                None => out.push((entry.resource_type.clone(), 1, size)),
            }
        }
        out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        out
    }

    /// Count per host, most first.
    #[must_use]
    pub fn by_host(&self) -> Vec<(String, usize)> {
        let mut out: Vec<(String, usize)> = Vec::new();
        for entry in &self.entries {
            match out.iter_mut().find(|(h, _)| h == entry.host()) {
                Some((_, n)) => *n += 1,
                None => out.push((entry.host().to_owned(), 1)),
            }
        }
        out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        out
    }

    /// Time per phase summed over all requests, in [`PHASES`] order.
    #[must_use]
    pub fn phase_totals(&self) -> Vec<(&'static str, f64)> {
        PHASES
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let total = self
                    .entries
                    .iter()
                    .filter_map(|e| e.timings.phases()[i].1)
                    .sum();
                (*name, total)
            })
            .collect()
    }

    /// Requests sending credentials (cookies, authorization, tokens).
    #[must_use]
    pub fn with_credentials(&self) -> usize {
        self.entries.iter().filter(|e| e.has_credentials()).count()
    }

    /// The waterfall: one row per request (id = its index), one span per
    /// timing phase colored by phase, failed requests in the error color,
    /// page `DOMContentLoaded` / `load` as markers on the page's first
    /// row.
    #[must_use]
    pub fn timeline(&self) -> TimelineSpec {
        let rows = self
            .entries
            .iter()
            .enumerate()
            .map(|(ix, entry)| {
                let status = if entry.failed() {
                    Some(SpanStatus::Error)
                } else {
                    None
                };
                let mut at = entry.start;
                let mut spans: Vec<Span> = entry
                    .timings
                    .phases()
                    .iter()
                    .filter_map(|(name, t)| {
                        let t = t.filter(|t| *t > 0.0)?;
                        let span = Span {
                            start: at,
                            end: at + t,
                            label: None,
                            color_key: Some((*name).to_owned()),
                            status,
                            attrs: phase_attrs(name, t, entry),
                        };
                        at += t;
                        Some(span)
                    })
                    .collect();
                if spans.is_empty() {
                    spans.push(Span {
                        start: entry.start,
                        end: entry.start + entry.time,
                        label: None,
                        color_key: Some("total".to_owned()),
                        status,
                        attrs: phase_attrs("total", entry.time, entry),
                    });
                }
                let mut markers = Vec::new();
                if let Some(page) = self.pages.iter().find(|p| {
                    entry.page.as_deref() == Some(p.id.as_str())
                        && self.entries[..ix]
                            .iter()
                            .all(|e| e.page.as_deref() != Some(p.id.as_str()))
                }) {
                    for (name, at) in [
                        ("DOMContentLoaded", page.on_content_load),
                        ("load", page.on_load),
                    ] {
                        if let Some(at) = at {
                            markers.push(Marker {
                                at: page.start + at,
                                label: Some(name.to_owned()),
                                color_key: Some(name.to_owned()),
                            });
                        }
                    }
                }
                TimelineRow {
                    id: ix.to_string(),
                    label: format!("{} {}", entry.method, short_path(entry)),
                    depth: 0,
                    group: entry.page.clone(),
                    spans,
                    markers,
                }
            })
            .collect();
        TimelineSpec {
            title: None,
            unit: TimeUnit::Ms,
            rows,
            range: None,
        }
    }

    /// The request list: level from the status (`error` for failures and
    /// 5xx, `warn` for 4xx), fields for status, class, type, method, host,
    /// size and time; chips by class, type, method and host.
    #[must_use]
    pub fn list(&self) -> ListSpec {
        let items = self
            .entries
            .iter()
            .map(|entry| {
                let level = match entry.status {
                    0 | 500.. => Some("error".to_owned()),
                    400..500 => Some("warn".to_owned()),
                    _ => None,
                };
                let mut fields = Map::new();
                let status = if entry.status == 0 {
                    "—".to_owned()
                } else {
                    entry.status.to_string()
                };
                fields.insert("status".into(), status.into());
                fields.insert("class".into(), entry.status_class().into());
                fields.insert("type".into(), entry.resource_type.clone().into());
                fields.insert("method".into(), entry.method.clone().into());
                fields.insert("host".into(), entry.host().to_owned().into());
                fields.insert(
                    "size".into(),
                    entry.size().map(format_bytes).unwrap_or_default().into(),
                );
                fields.insert(
                    "time".into(),
                    format_duration(entry.time, TimeUnit::Ms).into(),
                );
                if let Some(error) = &entry.error {
                    fields.insert("error".into(), error.clone().into());
                }
                ListItem {
                    level,
                    ts: None,
                    text: format!("{} {}", entry.method, redact_url(&entry.url)),
                    fields,
                }
            })
            .collect();
        ListSpec {
            title: None,
            columns: ["status", "type", "size", "time"]
                .iter()
                .map(|c| (*c).to_owned())
                .collect(),
            items,
            facets: ["class", "type", "method", "host"]
                .iter()
                .map(|c| (*c).to_owned())
                .collect(),
            follow: false,
        }
    }

    /// One CSV line per request (URLs redacted).
    #[must_use]
    pub fn to_csv(&self) -> String {
        let mut out = String::from("start_ms,method,url,status,type,mime,size_bytes,time_ms");
        for phase in PHASES {
            out.push_str(&format!(",{phase}_ms"));
        }
        out.push('\n');
        let field = |s: &str| {
            if s.contains([',', '"', '\n']) {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.to_owned()
            }
        };
        let num = |v: f64| format!("{}", (v * 1000.0).round() / 1000.0);
        for e in &self.entries {
            out.push_str(&format!(
                "{},{},{},{},{},{},{},{}",
                num(e.start),
                field(&e.method),
                field(&redact_url(&e.url)),
                e.status,
                field(&e.resource_type),
                field(e.mime_type.as_deref().unwrap_or("")),
                e.size().map(|s| s.to_string()).unwrap_or_default(),
                num(e.time),
            ));
            for (_, t) in e.timings.phases() {
                out.push(',');
                if let Some(t) = t {
                    out.push_str(&num(t));
                }
            }
            out.push('\n');
        }
        out
    }
}

fn phase_attrs(phase: &str, time: f64, entry: &HarEntry) -> Map<String, Value> {
    let mut attrs = Map::new();
    attrs.insert("phase".into(), phase.into());
    attrs.insert(
        "phase time".into(),
        format_duration(time, TimeUnit::Ms).into(),
    );
    attrs.insert("url".into(), redact_url(&entry.url).into());
    if entry.status > 0 {
        attrs.insert("status".into(), entry.status.into());
    }
    attrs.insert(
        "total".into(),
        format_duration(entry.time, TimeUnit::Ms).into(),
    );
    attrs
}

/// The last path segment (or the host for `/`), for a row label.
fn short_path(entry: &HarEntry) -> String {
    let path = entry.path();
    let path = path.split('?').next().unwrap_or_default();
    match path.trim_end_matches('/').rsplit('/').next() {
        Some(last) if !last.is_empty() => last.to_owned(),
        _ => entry.host().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(url: &str, status: u16, start: f64, wait: f64) -> HarEntry {
        HarEntry {
            method: "GET".into(),
            url: url.into(),
            status,
            resource_type: "fetch".into(),
            start,
            time: 5.0 + wait,
            timings: HarTimings {
                dns: Some(2.0),
                send: Some(3.0),
                wait: Some(wait),
                ..HarTimings::default()
            },
            transfer_size: Some(1500),
            ..HarEntry::default()
        }
    }

    fn spec() -> HarSpec {
        let mut first = entry("https://app.test/api/orders?id=7&token=abc", 200, 0.0, 40.0);
        first.request_headers = vec![
            HarHeader {
                name: "Authorization".into(),
                value: "Bearer s3cret".into(),
            },
            HarHeader {
                name: "Accept".into(),
                value: "application/json".into(),
            },
        ];
        HarSpec {
            title: None,
            path: Some("net.har".into()),
            creator: Some("WebInspector 537.36".into()),
            started_at: Some(1_792_847_107.0),
            pages: Vec::new(),
            entries: vec![first, entry("https://cdn.test/app.js", 503, 10.0, 100.0)],
            total_entries: 2,
        }
    }

    #[test]
    fn urls_and_headers_are_redacted() {
        assert_eq!(
            redact_url("https://u:p@x.test/a?id=1&access_token=xyz&sig=9#f"),
            "https://<redacted>@x.test/a?id=1&access_token=<redacted>&sig=<redacted>#f"
        );
        assert_eq!(redact_url("https://x.test/"), "https://x.test/");
        assert!(is_sensitive_header("Cookie"));
        assert!(is_sensitive_header("X-CSRF-Token"));
        assert!(!is_sensitive_header("Accept"));
        let spec = spec();
        let curl = spec.entries[0].to_curl(false);
        assert!(curl.contains("'Authorization: <redacted>'"), "{curl}");
        assert!(curl.contains("'Accept: application/json'"), "{curl}");
        assert!(spec.entries[0].to_curl(true).contains("Bearer s3cret"));
        assert_eq!(spec.with_credentials(), 1);
        assert_eq!(spec.entries[0].path(), "/api/orders?id=7&token=<redacted>");
        assert_eq!(spec.entries[0].host(), "app.test");
    }

    #[test]
    fn waterfall_list_and_totals() {
        let spec = spec();
        spec.validate().unwrap();
        assert_eq!(spec.failed(), [1]);
        assert_eq!(spec.transferred(), 3000);
        assert!((spec.duration() - 115.0).abs() < 1e-9);
        assert_eq!(spec.slowest(1), [1]);
        let timeline = spec.timeline();
        timeline.validate().unwrap();
        let spans: Vec<_> = timeline.rows[0]
            .spans
            .iter()
            .map(|s| (s.color_key.clone().unwrap_or_default(), s.start, s.end))
            .collect();
        assert_eq!(
            spans,
            [
                ("dns".to_owned(), 0.0, 2.0),
                ("send".to_owned(), 2.0, 5.0),
                ("wait".to_owned(), 5.0, 45.0)
            ]
        );
        assert_eq!(timeline.rows[0].label, "GET orders");
        assert_eq!(timeline.rows[1].spans[0].status, Some(SpanStatus::Error));
        let list = spec.list();
        list.validate().unwrap();
        assert_eq!(list.items[1].level.as_deref(), Some("error"));
        assert_eq!(list.items[0].field_text("size").as_deref(), Some("1.5 kB"));
        assert_eq!(
            list.items[0].text,
            "GET https://app.test/api/orders?id=7&token=<redacted>"
        );
        let totals = spec.phase_totals();
        assert_eq!(totals[5], ("wait", 140.0));
        assert_eq!(spec.by_type(), [("fetch".to_owned(), 2, 3000)]);
        let csv = spec.to_csv();
        assert!(
            csv.lines()
                .nth(1)
                .unwrap_or_default()
                .contains("token=<redacted>")
        );
    }
}
