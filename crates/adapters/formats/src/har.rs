//! HAR 1.2 files into a [`HarSpec`].
//!
//! Own serde structs rather than the `har` crate: that one needs
//! `serde_with` (not otherwise in the tree) and rejects files missing
//! fields the spec calls required, which real exports often omit (no
//! `cache`, no `cookies`, `-1` sizes). Here every field is optional and
//! unknown ones are ignored. Chrome's extensions are read when present:
//! `_resourceType`, `_transferSize`, `_error`.
//!
//! Entries are sorted by start time; times become milliseconds from the
//! first start. The file's `connect` includes `ssl`; the spec's does not.

use domain::devops::har::{
    HarBody, HarEntry, HarHeader, HarPage, HarSpec, HarTimings, MAX_BODY_PREVIEW, MAX_ENTRIES,
    MAX_HEADERS,
};
use serde::Deserialize;

use crate::logs::parse_time;

#[derive(Debug, Deserialize)]
struct File {
    log: Log,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Log {
    creator: Option<Creator>,
    browser: Option<Creator>,
    pages: Vec<Page>,
    entries: Vec<Entry>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Creator {
    name: String,
    version: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Page {
    id: String,
    title: Option<String>,
    started_date_time: Option<String>,
    page_timings: PageTimings,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct PageTimings {
    on_content_load: Option<f64>,
    on_load: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Entry {
    pageref: Option<String>,
    started_date_time: Option<String>,
    time: Option<f64>,
    request: Request,
    response: Response,
    timings: Timings,
    #[serde(rename = "serverIPAddress")]
    server_ip_address: Option<String>,
    #[serde(rename = "_resourceType")]
    resource_type: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Request {
    method: String,
    url: String,
    http_version: Option<String>,
    headers: Vec<Pair>,
    query_string: Vec<Pair>,
    post_data: Option<PostData>,
    body_size: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Response {
    status: Option<i64>,
    status_text: Option<String>,
    http_version: Option<String>,
    headers: Vec<Pair>,
    content: Content,
    #[serde(rename = "redirectURL")]
    redirect_url: Option<String>,
    headers_size: Option<i64>,
    body_size: Option<i64>,
    #[serde(rename = "_transferSize")]
    transfer_size: Option<i64>,
    #[serde(rename = "_error")]
    error: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Content {
    size: Option<i64>,
    mime_type: Option<String>,
    text: Option<String>,
    encoding: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct PostData {
    mime_type: Option<String>,
    text: Option<String>,
    params: Vec<Pair>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Pair {
    name: String,
    value: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Timings {
    blocked: Option<f64>,
    dns: Option<f64>,
    connect: Option<f64>,
    ssl: Option<f64>,
    send: Option<f64>,
    wait: Option<f64>,
    receive: Option<f64>,
}

/// Parses a HAR file.
///
/// # Errors
///
/// A message for malformed JSON or a file without `log.entries`.
pub fn parse(bytes: &[u8], path: &str) -> Result<HarSpec, String> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let file: File = serde_json::from_slice(bytes).map_err(|e| {
        format!("`{path}` is not a HAR file (expected {{\"log\": {{\"entries\": [...]}}}}): {e}")
    })?;
    let log = file.log;
    let total_entries = log.entries.len();
    let mut raw: Vec<(f64, Entry)> = log
        .entries
        .into_iter()
        .enumerate()
        .map(|(ix, e)| {
            let at = e
                .started_date_time
                .as_deref()
                .and_then(parse_time)
                // Without a time, keep file order after the timed ones.
                .unwrap_or(f64::MAX / 2.0 + ix as f64);
            (at, e)
        })
        .collect();
    raw.sort_by(|a, b| a.0.total_cmp(&b.0));
    raw.truncate(MAX_ENTRIES);
    let first = raw
        .first()
        .map(|(at, _)| *at)
        .filter(|at| *at < f64::MAX / 4.0);
    let entries = raw
        .into_iter()
        .map(|(at, e)| {
            let start = match first {
                Some(first) if at < f64::MAX / 4.0 => ((at - first) * 1000.0).max(0.0),
                _ => 0.0,
            };
            entry(e, start)
        })
        .collect();
    let pages = log
        .pages
        .into_iter()
        .map(|p| HarPage {
            start: match (first, p.started_date_time.as_deref().and_then(parse_time)) {
                (Some(first), Some(at)) => ((at - first) * 1000.0).max(0.0),
                _ => 0.0,
            },
            id: p.id,
            title: p.title.filter(|t| !t.is_empty()),
            on_content_load: phase(p.page_timings.on_content_load),
            on_load: phase(p.page_timings.on_load),
        })
        .collect();
    let creator = log
        .browser
        .or(log.creator)
        .map(|c| format!("{} {}", c.name, c.version).trim().to_owned())
        .filter(|c| !c.is_empty());
    Ok(HarSpec {
        title: None,
        path: Some(path.to_owned()),
        creator,
        started_at: first,
        pages,
        entries,
        total_entries,
    })
}

/// A phase time: `-1` (or any negative, or non-finite) is "did not
/// apply".
fn phase(t: Option<f64>) -> Option<f64> {
    t.filter(|t| t.is_finite() && *t >= 0.0)
}

fn size(n: Option<i64>) -> Option<u64> {
    n.and_then(|n| u64::try_from(n).ok())
}

fn pairs(pairs: Vec<Pair>) -> Vec<HarHeader> {
    pairs
        .into_iter()
        .take(MAX_HEADERS)
        .map(|p| HarHeader {
            name: p.name,
            value: p.value.unwrap_or_default(),
        })
        .collect()
}

/// The first [`MAX_BODY_PREVIEW`] bytes of `text`, on a char boundary,
/// and whether it was cut.
fn preview(text: String) -> (String, bool) {
    if text.len() <= MAX_BODY_PREVIEW {
        return (text, false);
    }
    let mut end = MAX_BODY_PREVIEW;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), true)
}

fn entry(e: Entry, start: f64) -> HarEntry {
    let t = &e.timings;
    let ssl = phase(t.ssl);
    // HAR's `connect` includes the TLS handshake.
    let connect = phase(t.connect).map(|c| (c - ssl.unwrap_or(0.0)).max(0.0));
    let timings = HarTimings {
        blocked: phase(t.blocked),
        dns: phase(t.dns),
        connect,
        ssl,
        send: phase(t.send),
        wait: phase(t.wait),
        receive: phase(t.receive),
    };
    let time = phase(e.time).unwrap_or_else(|| timings.total());
    let response = e.response;
    let mime_type = response.content.mime_type.clone().filter(|m| !m.is_empty());
    let resource_type = e
        .resource_type
        .filter(|t| !t.is_empty())
        .map(|t| t.to_ascii_lowercase())
        .unwrap_or_else(|| guess_type(mime_type.as_deref(), &e.request.url));
    let transfer_size = size(response.transfer_size).or_else(|| {
        match (size(response.headers_size), size(response.body_size)) {
            (Some(h), Some(b)) => Some(h + b),
            (None, Some(b)) => Some(b),
            _ => None,
        }
    });
    let base64 = response.content.encoding.as_deref() == Some("base64");
    let response_body = {
        let content = response.content;
        let (text, truncated) = match content.text.filter(|_| !base64) {
            Some(text) => {
                let (text, cut) = preview(text);
                (Some(text), cut)
            }
            None => (None, false),
        };
        (text.is_some() || content.size.is_some_and(|s| s > 0)).then(|| HarBody {
            mime_type: content.mime_type.filter(|m| !m.is_empty()),
            size: size(content.size),
            text,
            truncated,
            base64,
        })
    };
    let content_size = response_body.as_ref().and_then(|b| b.size);
    let request = e.request;
    let request_body = request.post_data.map(|post| {
        let text = post.text.or_else(|| {
            (!post.params.is_empty()).then(|| {
                post.params
                    .iter()
                    .map(|p| format!("{}={}", p.name, p.value.as_deref().unwrap_or("")))
                    .collect::<Vec<_>>()
                    .join("&")
            })
        });
        let (text, truncated) = match text {
            Some(text) => {
                let (text, cut) = preview(text);
                (Some(text), cut)
            }
            None => (None, false),
        };
        HarBody {
            mime_type: post.mime_type.filter(|m| !m.is_empty()),
            size: size(request.body_size),
            text,
            truncated,
            base64: false,
        }
    });
    let status = response
        .status
        .and_then(|s| u16::try_from(s).ok())
        .unwrap_or(0);
    HarEntry {
        method: if request.method.is_empty() {
            "GET".to_owned()
        } else {
            request.method
        },
        url: request.url,
        http_version: request
            .http_version
            .or(response.http_version)
            .filter(|v| !v.is_empty()),
        status,
        status_text: response.status_text.filter(|s| !s.is_empty()),
        resource_type,
        mime_type,
        start,
        time,
        timings,
        request_headers: pairs(request.headers),
        response_headers: pairs(response.headers),
        query: pairs(request.query_string),
        request_body,
        response_body,
        transfer_size,
        content_size,
        server_ip: e.server_ip_address.filter(|s| !s.is_empty()),
        redirect_url: response.redirect_url.filter(|s| !s.is_empty()),
        error: response.error.filter(|s| !s.is_empty()),
        page: e.pageref.filter(|p| !p.is_empty()),
    }
}

/// A resource type from the media type, else the URL's extension.
fn guess_type(mime: Option<&str>, url: &str) -> String {
    let mime = mime.unwrap_or("").to_ascii_lowercase();
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    let ext = path
        .rsplit('/')
        .next()
        .and_then(|last| last.rsplit_once('.'))
        .map(|(_, e)| e);
    let kind = if mime.contains("html") {
        "document"
    } else if mime.contains("javascript") || mime.contains("ecmascript") || ext == Some("js") {
        "script"
    } else if mime.contains("css") || ext == Some("css") {
        "stylesheet"
    } else if mime.starts_with("image/")
        || matches!(
            ext,
            Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "avif")
        )
    {
        "image"
    } else if mime.starts_with("font/")
        || mime.contains("font")
        || matches!(ext, Some("woff" | "woff2" | "ttf" | "otf"))
    {
        "font"
    } else if mime.starts_with("video/") || mime.starts_with("audio/") {
        "media"
    } else if mime.contains("json") || mime.contains("xml") || mime.contains("grpc") {
        "fetch"
    } else {
        "other"
    };
    kind.to_owned()
}
