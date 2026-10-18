//! `app.http`: HTTP requests to the hosts a plugin declared with
//! `network:` capabilities.
//!
//! ```lua
//! local res = app.http.request{
//!   method = "POST", url = "https://jira.example.com/rest/api/2/search",
//!   headers = { Authorization = "Bearer " .. app.secret("token") },
//!   query = { maxResults = 10 }, json = { jql = "assignee = currentUser()" },
//!   timeout = 20,
//! }
//! -- res = { status = 200, headers = { ["content-type"] = "..." }, body = "...", json = {...} }
//! local res = app.http.get(url, opts)
//! local res = app.http.post(url, opts)
//! ```
//!
//! Every URL, the first one and each redirect target, must be allowed by a
//! granted capability; at most [`MAX_REDIRECTS`] redirects are followed and
//! `Authorization` and `Cookie` are dropped when one leaves the host. The
//! whole exchange is limited by `timeout` (default
//! [`DEFAULT_TIMEOUT_S`] seconds) and the body by [`MAX_BODY_BYTES`]. Any
//! status comes back as a response; network failures, timeouts and denied
//! hosts raise Lua errors.

use std::sync::Arc;
use std::time::Duration;

use application::ports::plugin::Capability;
use mlua::{Lua, Table, Value};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, Url};

use super::{Context, authorize, context};
use crate::config::api::Origin;
use crate::convert;

/// Default limit of one request, redirects and body included.
pub(crate) const DEFAULT_TIMEOUT_S: f64 = 30.0;
/// Longest `timeout` accepted.
const MAX_TIMEOUT_S: f64 = 600.0;
/// Redirects followed before giving up.
pub(crate) const MAX_REDIRECTS: usize = 5;
/// Largest response body read.
pub(crate) const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

const API: &str = "app.http";

fn error(message: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(format!("{API}: {message}"))
}

/// A request as read from Lua.
#[derive(Debug)]
struct Request {
    method: Method,
    url: Url,
    headers: HeaderMap,
    body: Option<Vec<u8>>,
    timeout: Duration,
}

/// What came back.
#[derive(Debug)]
struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// Builds the `app.http` table for one phase.
pub(crate) fn table(lua: &Lua, origin: &Arc<Origin>) -> mlua::Result<Table> {
    let http = lua.create_table()?;
    let o = Arc::clone(origin);
    http.set(
        "request",
        lua.create_function(move |lua, opts: Table| run(lua, &o, &opts, None, None))?,
    )?;
    for (name, method) in [("get", Method::GET), ("post", Method::POST)] {
        let o = Arc::clone(origin);
        http.set(
            name,
            lua.create_function(move |lua, (url, opts): (String, Option<Table>)| {
                let opts = match opts {
                    Some(opts) => opts,
                    None => lua.create_table()?,
                };
                run(lua, &o, &opts, Some(url), Some(method.clone()))
            })?,
        )?;
    }
    Ok(http)
}

fn run(
    lua: &Lua,
    origin: &Origin,
    opts: &Table,
    url: Option<String>,
    method: Option<Method>,
) -> mlua::Result<Table> {
    let request = read_request(lua, opts, url, method)?;
    let target = request.url.clone();
    let identity = authorize(
        lua,
        origin,
        API,
        Some((
            &format!("network:{}", describe(&target)),
            &|c: &Capability| allows(c, &target),
        )),
    )?;
    let allowed = |url: &Url| identity.capabilities.iter().any(|c| allows(c, url));
    let ctx: Arc<Context> = context(lua)?;
    let response = ctx.block_on(send(request, &allowed))??;
    to_lua(lua, response)
}

/// `host[:port]` or `scheme://host:port`, as a capability would name it.
fn describe(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default();
    match (url.scheme(), url.port()) {
        ("https", None) => host.to_owned(),
        (scheme, Some(port)) => format!("{scheme}://{host}:{port}"),
        (scheme, None) => format!("{scheme}://{host}"),
    }
}

fn allows(capability: &Capability, url: &Url) -> bool {
    let Capability::Network(target) = capability else {
        return false;
    };
    url.host_str()
        .is_some_and(|host| target.allows(url.scheme(), host, url.port()))
}

fn read_request(
    lua: &Lua,
    opts: &Table,
    url: Option<String>,
    method: Option<Method>,
) -> mlua::Result<Request> {
    let url = match url {
        Some(url) => url,
        None => opts
            .get::<Option<String>>("url")?
            .ok_or_else(|| error("`url` is required"))?,
    };
    let mut url = Url::parse(&url).map_err(|e| error(format!("invalid url `{url}`: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(error(format!("`{url}` is not an http(s) URL")));
    }
    let method = match method {
        Some(method) => method,
        None => match opts.get::<Option<String>>("method")? {
            Some(name) => Method::from_bytes(name.to_ascii_uppercase().as_bytes())
                .map_err(|_| error(format!("invalid method `{name}`")))?,
            None => Method::GET,
        },
    };
    if let Some(query) = opts.get::<Option<Table>>("query")? {
        let mut pairs: Vec<(String, String)> = Vec::new();
        for pair in query.pairs::<String, Value>() {
            let (key, value) = pair?;
            pairs.push((key, scalar(&value, "query")?));
        }
        pairs.sort();
        let mut query = url.query_pairs_mut();
        for (key, value) in &pairs {
            query.append_pair(key, value);
        }
    }
    let mut headers = HeaderMap::new();
    if let Some(table) = opts.get::<Option<Table>>("headers")? {
        for pair in table.pairs::<String, Value>() {
            let (key, value) = pair?;
            let name = HeaderName::from_bytes(key.as_bytes())
                .map_err(|_| error(format!("invalid header name `{key}`")))?;
            let value = HeaderValue::from_str(&scalar(&value, "headers")?)
                .map_err(|_| error(format!("invalid value for header `{key}`")))?;
            headers.insert(name, value);
        }
    }
    let body = match (opts.get::<Value>("body")?, opts.get::<Value>("json")?) {
        (Value::Nil, Value::Nil) => None,
        (Value::String(s), Value::Nil) => Some(s.as_bytes().to_vec()),
        (Value::Nil, json) => {
            let json = convert::to_json(lua, json)?;
            if !headers.contains_key(reqwest::header::CONTENT_TYPE) {
                headers.insert(
                    reqwest::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                );
            }
            Some(serde_json::to_vec(&json).map_err(error)?)
        }
        (Value::String(_), _) => return Err(error("give either `body` or `json`, not both")),
        (other, _) => {
            return Err(error(format!(
                "`body` must be a string, got {}",
                other.type_name()
            )));
        }
    };
    let timeout = match opts.get::<Option<f64>>("timeout")? {
        None => DEFAULT_TIMEOUT_S,
        Some(t) if t > 0.0 && t <= MAX_TIMEOUT_S => t,
        Some(t) => {
            return Err(error(format!(
                "`timeout` must be between 0 and {MAX_TIMEOUT_S} seconds, got {t}"
            )));
        }
    };
    Ok(Request {
        method,
        url,
        headers,
        body,
        timeout: Duration::from_secs_f64(timeout),
    })
}

/// A query or header value: a string, number or boolean.
fn scalar(value: &Value, field: &str) -> mlua::Result<String> {
    match value {
        Value::String(s) => Ok(s.to_str()?.to_owned()),
        Value::Integer(n) => Ok(n.to_string()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Boolean(b) => Ok(b.to_string()),
        other => Err(error(format!(
            "`{field}` values must be strings, numbers or booleans, got {}",
            other.type_name()
        ))),
    }
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("crest/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("cannot create the HTTP client: {e}"))
}

/// Sends `request`, following redirects to allowed URLs, within its
/// timeout.
async fn send(request: Request, allowed: &dyn Fn(&Url) -> bool) -> mlua::Result<Response> {
    let limit = request.timeout;
    let shown = request.url.clone();
    let exchange = async move {
        let client = client().map_err(error)?;
        follow(&client, request, allowed).await
    };
    match tokio::time::timeout(limit, exchange).await {
        Ok(result) => result,
        Err(_) => Err(error(format!(
            "request to {shown} timed out after {}s",
            limit.as_secs_f64()
        ))),
    }
}

async fn follow(
    client: &reqwest::Client,
    mut request: Request,
    allowed: &dyn Fn(&Url) -> bool,
) -> mlua::Result<Response> {
    for hop in 0..=MAX_REDIRECTS {
        let mut builder = client
            .request(request.method.clone(), request.url.clone())
            .headers(request.headers.clone());
        if let Some(body) = &request.body {
            builder = builder.body(body.clone());
        }
        let mut response = builder
            .send()
            .await
            .map_err(|e| error(format!("request to {} failed: {e}", request.url)))?;
        let status = response.status();
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        if let (true, Some(location)) = (status.is_redirection(), location) {
            if hop == MAX_REDIRECTS {
                return Err(error(format!(
                    "too many redirects (more than {MAX_REDIRECTS}) from {}",
                    request.url
                )));
            }
            let next = request
                .url
                .join(&location)
                .map_err(|e| error(format!("invalid redirect `{location}`: {e}")))?;
            if !allowed(&next) {
                return Err(error(format!(
                    "redirect from {} to {next} denied: no granted capability allows network:{}",
                    request.url,
                    describe(&next)
                )));
            }
            let code = status.as_u16();
            if code == 303 || (matches!(code, 301 | 302) && request.method == Method::POST) {
                request.method = Method::GET;
                request.body = None;
                request.headers.remove(reqwest::header::CONTENT_TYPE);
            }
            if next.host_str() != request.url.host_str() {
                request.headers.remove(reqwest::header::AUTHORIZATION);
                request.headers.remove(reqwest::header::COOKIE);
            }
            request.url = next;
            continue;
        }
        let headers = response
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_owned(),
                    String::from_utf8_lossy(v.as_bytes()).into_owned(),
                )
            })
            .collect();
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| error(format!("reading the response of {}: {e}", request.url)))?
        {
            if body.len() + chunk.len() > MAX_BODY_BYTES {
                return Err(error(format!(
                    "the response of {} is larger than {} MiB",
                    request.url,
                    MAX_BODY_BYTES / (1024 * 1024)
                )));
            }
            body.extend_from_slice(&chunk);
        }
        return Ok(Response {
            status: status.as_u16(),
            headers,
            body,
        });
    }
    Err(error("too many redirects"))
}

fn to_lua(lua: &Lua, response: Response) -> mlua::Result<Table> {
    let out = lua.create_table()?;
    out.set("status", response.status)?;
    let headers = lua.create_table()?;
    let mut is_json = false;
    for (name, value) in response.headers {
        if name == "content-type" && value.contains("json") {
            is_json = true;
        }
        let joined = match headers.get::<Option<String>>(name.as_str())? {
            Some(previous) => format!("{previous}, {value}"),
            None => value,
        };
        headers.set(name, joined)?;
    }
    out.set("headers", headers)?;
    if is_json && let Ok(json) = serde_json::from_slice::<serde_json::Value>(&response.body) {
        out.set("json", convert::to_lua(lua, &json)?)?;
    }
    out.set("body", lua.create_string(&response.body)?)?;
    Ok(out)
}
