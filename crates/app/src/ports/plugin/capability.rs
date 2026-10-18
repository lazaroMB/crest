//! Capabilities a plugin asks for with `app.plugin{ capabilities = ... }`:
//! HTTP requests to some hosts (`network:`) and running some programs
//! (`shell:`).
//!
//! A capability is written as a string and parsed here, so the Lua host,
//! the trust store and the messages shown to the user agree on its meaning
//! and spelling:
//!
//! - `network:jira.example.com`: HTTPS to that host, on port 443;
//! - `network:*.github.com`: HTTPS to any subdomain of `github.com` (not
//!   `github.com` itself);
//! - `network:http://localhost:8080` or `network:https://host:8443`: an
//!   explicit scheme and port;
//! - `shell:jq`: running the program `jq`, looked up on `PATH`.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::ConfigOrigin;

/// URL scheme a network capability allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scheme {
    /// `https` (the default).
    Https,
    /// Plain `http`, only when written explicitly.
    Http,
}

impl Scheme {
    /// The scheme's name, as in a URL.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Https => "https",
            Self::Http => "http",
        }
    }

    /// Port used when a URL names none.
    #[must_use]
    pub fn default_port(self) -> u16 {
        match self {
            Self::Https => 443,
            Self::Http => 80,
        }
    }
}

/// Hosts a network capability allows.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HostPattern {
    /// Exactly this host (lowercase).
    Exact(String),
    /// Any subdomain of this host (`*.github.com` stores `github.com`).
    Subdomains(String),
}

impl HostPattern {
    /// Whether `host` (as in a URL, any case) is allowed.
    #[must_use]
    pub fn matches(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        match self {
            Self::Exact(allowed) => host == *allowed,
            Self::Subdomains(parent) => host
                .strip_suffix(parent.as_str())
                .and_then(|rest| rest.strip_suffix('.'))
                .is_some_and(|sub| !sub.is_empty()),
        }
    }
}

impl fmt::Display for HostPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exact(host) => f.write_str(host),
            Self::Subdomains(parent) => write!(f, "*.{parent}"),
        }
    }
}

/// Where a `network:` capability may connect.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NetworkTarget {
    /// Allowed scheme.
    pub scheme: Scheme,
    /// Allowed hosts.
    pub host: HostPattern,
    /// Allowed port; `None` is the scheme's default port.
    pub port: Option<u16>,
}

impl NetworkTarget {
    /// Whether a URL with these parts is allowed. `port` is the URL's
    /// explicit port, if any.
    #[must_use]
    pub fn allows(&self, scheme: &str, host: &str, port: Option<u16>) -> bool {
        let wanted = self.port.unwrap_or_else(|| self.scheme.default_port());
        scheme.eq_ignore_ascii_case(self.scheme.as_str())
            && self.host.matches(host)
            && port.unwrap_or_else(|| self.scheme.default_port()) == wanted
    }
}

impl fmt::Display for NetworkTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let plain = self.scheme == Scheme::Https && self.port.is_none();
        if plain {
            return write!(f, "{}", self.host);
        }
        write!(f, "{}://{}", self.scheme.as_str(), self.host)?;
        if let Some(port) = self.port {
            write!(f, ":{port}")?;
        }
        Ok(())
    }
}

/// One capability of a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Capability {
    /// `network:<target>`: `app.http` requests to that target.
    Network(NetworkTarget),
    /// `shell:<program>`: `app.shell.run` of that program.
    Shell(String),
}

impl Capability {
    /// Parses `network:<host>`, `network:*.<suffix>`,
    /// `network:<scheme>://<host>[:<port>]` or `shell:<program>`.
    ///
    /// # Errors
    ///
    /// What is wrong with `text`, to show the user.
    pub fn parse(text: &str) -> Result<Self, String> {
        if let Some(target) = text.strip_prefix("network:") {
            return parse_network(target).map(Self::Network);
        }
        if let Some(program) = text.strip_prefix("shell:") {
            return parse_program(program).map(|p| Self::Shell(p.to_owned()));
        }
        Err(format!(
            "unknown capability `{text}`: use `network:<host>` or `shell:<program>`"
        ))
    }

    /// `network` or `shell`, to group capabilities in messages.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Network(_) => "network",
            Self::Shell(_) => "shell",
        }
    }

    /// The part after `kind:`.
    #[must_use]
    pub fn target(&self) -> String {
        match self {
            Self::Network(target) => target.to_string(),
            Self::Shell(program) => program.clone(),
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind(), self.target())
    }
}

impl TryFrom<String> for Capability {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(&text)
    }
}

impl From<Capability> for String {
    fn from(capability: Capability) -> Self {
        capability.to_string()
    }
}

fn parse_network(target: &str) -> Result<NetworkTarget, String> {
    let bad = |why: &str| format!("invalid capability `network:{target}`: {why}");
    let (scheme, rest) = match target.split_once("://") {
        Some(("https", rest)) => (Scheme::Https, rest),
        Some(("http", rest)) => (Scheme::Http, rest),
        Some(_) => return Err(bad("the scheme must be http or https")),
        None => (Scheme::Https, target),
    };
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port)) => {
            let port = port
                .parse::<u16>()
                .ok()
                .filter(|p| *p != 0)
                .ok_or_else(|| bad("the port must be a number from 1 to 65535"))?;
            (host, Some(port))
        }
        None => (rest, None),
    };
    let host = host.to_ascii_lowercase();
    let pattern = match host.strip_prefix("*.") {
        Some(parent) => HostPattern::Subdomains(valid_host(parent).map_err(|w| bad(&w))?),
        None => HostPattern::Exact(valid_host(&host).map_err(|w| bad(&w))?),
    };
    Ok(NetworkTarget {
        scheme,
        host: pattern,
        port,
    })
}

/// A host name or IPv4 address: dot-separated labels of letters, digits
/// and `-`.
fn valid_host(host: &str) -> Result<String, String> {
    if host.is_empty() {
        return Err("the host is empty".into());
    }
    let ok = host.split('.').all(|label| {
        !label.is_empty()
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    });
    if ok {
        Ok(host.to_owned())
    } else {
        Err("expected a host name such as `api.example.com` (no path, no `*` except a leading `*.`)".into())
    }
}

fn parse_program(program: &str) -> Result<&str, String> {
    let ok = !program.is_empty()
        && program != "."
        && program != ".."
        && program
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.+".contains(c));
    if ok {
        Ok(program)
    } else {
        Err(format!(
            "invalid capability `shell:{program}`: expected a program name such as `jq` \
             (no path, no arguments)"
        ))
    }
}

/// The capabilities one config file or plugin declared, and who it is.
///
/// Approvals are kept per declaration: the same name from another file, or
/// the same file asking for other capabilities, needs a new approval.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PluginCapabilities {
    /// Plugin name (`app.plugin{ name = ... }`, by default the plugin's
    /// folder), also the namespace of its secrets.
    pub plugin: String,
    /// The `init.lua` that declared them.
    pub source: PathBuf,
    /// Layer of that file; a workspace file comes with the repository.
    #[serde(default)]
    pub origin: ConfigOrigin,
    /// What it asks for, sorted and without duplicates.
    pub capabilities: Vec<Capability>,
}

impl PluginCapabilities {
    /// The capabilities grouped by kind, as shown to the user:
    /// `network: a, b; shell: jq`.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut groups: Vec<(&str, Vec<String>)> = Vec::new();
        for capability in &self.capabilities {
            match groups
                .iter_mut()
                .find(|(kind, _)| *kind == capability.kind())
            {
                Some((_, targets)) => targets.push(capability.target()),
                None => groups.push((capability.kind(), vec![capability.target()])),
            }
        }
        groups
            .iter()
            .map(|(kind, targets)| format!("{kind}: {}", targets.join(", ")))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// Whether `capability` is among the declared ones.
    #[must_use]
    pub fn has(&self, capability: &Capability) -> bool {
        self.capabilities.contains(capability)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(text: &str) -> Capability {
        Capability::parse(text).unwrap()
    }

    #[test]
    fn parses_and_prints_every_form() {
        for text in [
            "network:jira.example.com",
            "network:*.github.com",
            "network:http://localhost:8080",
            "network:https://127.0.0.1:8443",
            "network:http://example.com",
            "shell:jq",
            "shell:git",
        ] {
            assert_eq!(cap(text).to_string(), text);
        }
        assert_eq!(
            cap("network:https://Jira.Example.com").to_string(),
            "network:jira.example.com"
        );
    }

    #[test]
    fn rejects_malformed_capabilities() {
        for text in [
            "disk:/",
            "network:",
            "network:ftp://x.com",
            "network:x.com/path",
            "network:x.com:0",
            "network:x.com:http",
            "network:*",
            "network:a.*.com",
            "network:-x.com",
            "shell:",
            "shell:/bin/sh",
            "shell:git push",
            "shell:..",
        ] {
            assert!(Capability::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn hosts_match_exactly_or_as_subdomains() {
        let Capability::Network(exact) = cap("network:jira.example.com") else {
            unreachable!()
        };
        assert!(exact.allows("https", "jira.example.com", None));
        assert!(exact.allows("https", "JIRA.example.com.", Some(443)));
        assert!(!exact.allows("http", "jira.example.com", None));
        assert!(!exact.allows("https", "jira.example.com", Some(8443)));
        assert!(!exact.allows("https", "evil-jira.example.com", None));
        assert!(!exact.allows("https", "jira.example.com.evil.net", None));

        let Capability::Network(wild) = cap("network:*.github.com") else {
            unreachable!()
        };
        assert!(wild.allows("https", "api.github.com", None));
        assert!(wild.allows("https", "a.b.github.com", None));
        assert!(!wild.allows("https", "github.com", None));
        assert!(!wild.allows("https", "evilgithub.com", None));

        let Capability::Network(local) = cap("network:http://localhost:8080") else {
            unreachable!()
        };
        assert!(local.allows("http", "localhost", Some(8080)));
        assert!(!local.allows("http", "localhost", None));
        assert!(!local.allows("https", "localhost", Some(8080)));
    }

    #[test]
    fn capabilities_round_trip_as_strings() {
        let decl = PluginCapabilities {
            plugin: "jira".into(),
            source: PathBuf::from("/c/plugins/jira/init.lua"),
            origin: ConfigOrigin::Global,
            capabilities: vec![cap("network:jira.example.com"), cap("shell:jq")],
        };
        let json = serde_json::to_value(&decl).unwrap();
        assert_eq!(
            json["capabilities"],
            serde_json::json!(["network:jira.example.com", "shell:jq"])
        );
        let back: PluginCapabilities = serde_json::from_value(json).unwrap();
        assert_eq!(back, decl);
    }

    #[test]
    fn the_summary_groups_by_kind() {
        let decl = PluginCapabilities {
            plugin: "jira".into(),
            source: PathBuf::new(),
            origin: ConfigOrigin::Global,
            capabilities: vec![
                cap("network:*.github.com"),
                cap("network:jira.example.com"),
                cap("shell:git"),
                cap("shell:jq"),
            ],
        };
        assert_eq!(
            decl.summary(),
            "network: *.github.com, jira.example.com; shell: git, jq"
        );
    }
}
