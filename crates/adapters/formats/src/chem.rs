//! What the chemistry and spectra envelopes share: the engine record,
//! input hashes and deterministic ids.

use domain::computed::{Engine, InputRef};

/// Engine name of the chemistry checks (reaction balance, descriptors,
/// peak picking).
pub const CHEM_ENGINE_NAME: &str = "crest-chem";
/// The cheminformatics library underneath.
pub const CHEMATIC: &str = "chematic 1.2.1";

/// The engine record, with `backend` (the library used).
#[must_use]
pub fn engine(backend: &str) -> Engine {
    Engine {
        name: CHEM_ENGINE_NAME.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        backend: Some(backend.to_owned()),
        platform: Some(format!(
            "{}-{}",
            std::env::consts::ARCH,
            std::env::consts::OS
        )),
    }
}

/// `blake3:<hex>` of `bytes`.
#[must_use]
pub fn hash(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

/// A deterministic envelope id: `ch_` and 12 hex digits of the hash of
/// `kind` and `bytes`.
#[must_use]
pub fn derive_id(kind: &str, bytes: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(kind.as_bytes());
    hasher.update(bytes);
    let hex = hasher.finalize().to_hex();
    format!("ch_{}", &hex.as_str()[..12])
}

/// The input record of inline text `bytes` named `name`.
#[must_use]
pub fn input(name: &str, role: &str, bytes: &[u8], path: Option<&str>) -> InputRef {
    InputRef {
        name: name.to_owned(),
        role: Some(role.to_owned()),
        path: path.map(str::to_owned),
        hash: Some(hash(bytes)),
        rows: None,
        columns: Vec::new(),
        filter: None,
    }
}
