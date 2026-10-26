//! The engine record of the science B envelopes (map measures, CPM
//! schedules, array previews, network metrics, training curves), each
//! under its own engine name.

use domain::computed::Engine;

/// The engine record `name` with `backend` (the libraries used).
#[must_use]
pub fn engine(name: &str, backend: &str) -> Engine {
    Engine {
        name: name.to_owned(),
        version: env!("CARGO_PKG_VERSION").into(),
        backend: Some(backend.to_owned()),
        platform: Some(format!(
            "{}-{}",
            std::env::consts::ARCH,
            std::env::consts::OS
        )),
    }
}

/// A deterministic envelope id: `prefix`, `_` and 12 hex digits of the
/// hash of `parts`.
#[must_use]
pub fn derive_id(prefix: &str, parts: &[&[u8]]) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(&(part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    let hex = hasher.finalize().to_hex();
    format!("{prefix}_{}", &hex.as_str()[..12])
}
