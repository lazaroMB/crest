//! Running one HTTP request with `curl`.
//!
//! curl is spawned directly (no shell) with `-q`, so no `.curlrc` changes
//! its behaviour, and with only the environment it needs (`PATH`, locale,
//! proxies, CA bundles). Headers go in a config read from stdin (`-K -`),
//! so a key never appears in the process arguments. After the body, curl
//! writes a trailer with the status, the content type and the final URL
//! (`-w`), which [`parse_output`] splits off.

use std::ffi::OsString;
use std::process::Stdio;

use application::ports::web::WebError;
use tokio::io::AsyncWriteExt as _;
use tokio::process::Command;

/// Largest body read, in bytes (`--max-filesize`).
pub const MAX_BODY_BYTES: u64 = 5 * 1024 * 1024;
/// Redirects followed.
const MAX_REDIRECTS: u32 = 5;
/// Marks the trailer `-w` writes after the body.
const TRAILER: &str = "\n\u{1f}crest\u{1f}";
/// Environment variables passed to curl; everything else is dropped.
const KEPT_ENV: [&str; 16] = [
    "PATH",
    "HOME",
    "LANG",
    "LC_ALL",
    "SYSTEMROOT",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "CURL_CA_BUNDLE",
];
/// What curl says it is.
pub const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) crest";

/// One request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The URL.
    pub url: String,
    /// Form fields sent as a POST (`--data-urlencode name=value`).
    pub form: Vec<(String, String)>,
    /// Headers, written to the stdin config.
    pub headers: Vec<String>,
}

impl Request {
    /// A GET of `url`.
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            form: Vec::new(),
            headers: Vec::new(),
        }
    }

    /// Adds a header (`Name: value`).
    #[must_use]
    pub fn header(mut self, header: impl Into<String>) -> Self {
        self.headers.push(header.into());
        self
    }

    /// Adds a form field, making the request a POST.
    #[must_use]
    pub fn form(mut self, name: &str, value: &str) -> Self {
        self.form.push((name.to_owned(), value.to_owned()));
        self
    }
}

/// What came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// HTTP status.
    pub status: u16,
    /// `Content-Type`, lower-cased (empty when none).
    pub content_type: String,
    /// URL after redirects.
    pub url: String,
    /// The body.
    pub body: Vec<u8>,
}

/// The arguments of curl for `request`, with a `timeout_s` limit.
pub fn args(request: &Request, timeout_s: u64) -> Vec<String> {
    let mut args: Vec<String> = [
        "-q",
        "--silent",
        "--show-error",
        "--location",
        "--compressed",
        "--proto",
        "=http,https",
        "--proto-redir",
        "=http,https",
    ]
    .map(str::to_owned)
    .into();
    args.extend([
        "--max-redirs".to_owned(),
        MAX_REDIRECTS.to_string(),
        "--max-time".to_owned(),
        timeout_s.to_string(),
        "--max-filesize".to_owned(),
        MAX_BODY_BYTES.to_string(),
        "--user-agent".to_owned(),
        USER_AGENT.to_owned(),
        "--config".to_owned(),
        "-".to_owned(),
        "--write-out".to_owned(),
        format!("{TRAILER}%{{http_code}}\u{1f}%{{content_type}}\u{1f}%{{url_effective}}"),
    ]);
    for (name, value) in &request.form {
        args.push("--data-urlencode".to_owned());
        args.push(format!("{name}={value}"));
    }
    args.push("--url".to_owned());
    args.push(request.url.clone());
    args
}

/// The config curl reads from stdin: the headers.
pub fn config(request: &Request) -> String {
    request
        .headers
        .iter()
        .map(|header| {
            let escaped = header.replace('\\', "\\\\").replace('"', "\\\"");
            format!("header = \"{escaped}\"\n")
        })
        .collect()
}

/// Splits curl's stdout into the body and the `-w` trailer.
pub fn parse_output(stdout: &[u8]) -> Option<Response> {
    let marker = TRAILER.as_bytes();
    let at = stdout
        .windows(marker.len())
        .rposition(|window| window == marker)?;
    let trailer = String::from_utf8_lossy(&stdout[at + marker.len()..]).into_owned();
    let mut fields = trailer.split('\u{1f}');
    let status = fields.next()?.trim().parse().ok()?;
    let content_type = fields.next()?.trim().to_ascii_lowercase();
    let url = fields.next()?.trim().to_owned();
    Some(Response {
        status,
        content_type,
        url,
        body: stdout[..at].to_vec(),
    })
}

/// The error for curl's exit `code` and `stderr`.
fn exit_error(code: Option<i32>, stderr: &str, timeout_s: u64) -> WebError {
    let detail = stderr.trim().trim_start_matches("curl: ").to_owned();
    match code {
        Some(28) => WebError::Timeout { seconds: timeout_s },
        Some(63) => WebError::Request(format!(
            "the page is larger than {} MB",
            MAX_BODY_BYTES / 1024 / 1024
        )),
        Some(1 | 3) => WebError::InvalidUrl(detail),
        _ if detail.is_empty() => WebError::Request(format!("curl exited with {code:?}")),
        _ => WebError::Request(detail),
    }
}

/// The environment curl gets from `inherited`.
pub fn environment(
    inherited: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    inherited
        .into_iter()
        .filter(|(key, _)| KEPT_ENV.iter().any(|kept| key == kept))
        .collect()
}

/// Runs `request` with the curl at `program`.
pub async fn run(
    program: &OsString,
    env: &[(OsString, OsString)],
    request: &Request,
    timeout_s: u64,
) -> Result<Response, WebError> {
    let mut child = Command::new(program)
        .args(args(request, timeout_s))
        .env_clear()
        .envs(env.iter().cloned())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => WebError::CurlMissing,
            _ => WebError::Request(format!("cannot start curl: {err}")),
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(config(request).as_bytes())
            .await
            .map_err(|err| WebError::Request(format!("cannot talk to curl: {err}")))?;
    }
    let output = child
        .wait_with_output()
        .await
        .map_err(|err| WebError::Request(format!("curl failed: {err}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(exit_error(output.status.code(), &stderr, timeout_s));
    }
    parse_output(&output.stdout)
        .ok_or_else(|| WebError::Request("curl gave no status line".to_owned()))
}

/// `text` percent-encoded for a query string.
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// `text` percent-decoded (`+` as a space), lossy.
pub fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if bytes
                .get(i + 1..i + 3)
                .is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit)) =>
            {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00");
                out.push(u8::from_str_radix(hex, 16).unwrap_or(b'?'));
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_goes_to_stdin_not_to_the_arguments() {
        let request = Request::get("https://api.search.brave.com/res/v1/web/search?q=a")
            .header("Accept: application/json")
            .header("X-Subscription-Token: s3cr\"et");
        let args = args(&request, 20);
        assert!(args.iter().all(|arg| !arg.contains("s3cr")), "{args:?}");
        assert_eq!(args.first().map(String::as_str), Some("-q"));
        assert!(args.windows(2).any(|w| w == ["--max-time", "20"]));
        assert!(args.windows(2).any(|w| w == ["--config", "-"]));
        assert_eq!(
            args.last().map(String::as_str),
            Some("https://api.search.brave.com/res/v1/web/search?q=a")
        );
        assert_eq!(
            config(&request),
            "header = \"Accept: application/json\"\nheader = \"X-Subscription-Token: s3cr\\\"et\"\n"
        );
    }

    #[test]
    fn a_form_makes_a_post() {
        let request = Request::get("https://html.duckduckgo.com/html/").form("q", "rust & go");
        let args = args(&request, 5);
        assert!(
            args.windows(2)
                .any(|w| w == ["--data-urlencode", "q=rust & go"]),
            "{args:?}"
        );
    }

    #[test]
    fn the_trailer_is_split_off_the_body() {
        let stdout =
            b"<html>hi</html>\n\x1fcrest\x1f200\x1ftext/HTML; charset=utf-8\x1fhttps://a.org/b";
        assert_eq!(
            parse_output(stdout),
            Some(Response {
                status: 200,
                content_type: "text/html; charset=utf-8".into(),
                url: "https://a.org/b".into(),
                body: b"<html>hi</html>".to_vec(),
            })
        );
        assert_eq!(parse_output(b"no trailer"), None);
    }

    #[test]
    fn exit_codes_become_errors() {
        assert_eq!(
            exit_error(Some(28), "curl: (28) Operation timed out", 20),
            WebError::Timeout { seconds: 20 }
        );
        assert_eq!(
            exit_error(Some(6), "curl: (6) Could not resolve host: nope", 20),
            WebError::Request("(6) Could not resolve host: nope".into())
        );
    }

    #[test]
    fn only_the_needed_environment_is_kept() {
        let env = environment([
            ("PATH".into(), "/usr/bin".into()),
            ("ANTHROPIC_API_KEY".into(), "sk".into()),
            ("BRAVE_API_KEY".into(), "b".into()),
            ("HTTPS_PROXY".into(), "http://proxy".into()),
        ]);
        let keys: Vec<_> = env.iter().map(|(k, _)| k.clone()).collect();
        assert_eq!(
            keys,
            [OsString::from("PATH"), OsString::from("HTTPS_PROXY")]
        );
    }

    #[test]
    fn query_strings_round_trip() {
        assert_eq!(encode("rust 1.99 & más"), "rust+1.99+%26+m%C3%A1s");
        assert_eq!(decode("rust+1.99+%26+m%C3%A1s"), "rust 1.99 & más");
        assert_eq!(decode("100%"), "100%");
    }
}
