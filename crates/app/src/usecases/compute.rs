//! Running the compute engine for the statistics tools, Lua and the
//! Recompute button: loading data files of the workspace, keeping the CPU
//! work off the async executor, and re-running an envelope against the
//! current contents of its file.
//!
//! The engine ([`ComputePort`]) is synchronous. The application has no
//! runtime of its own, so whoever wires it passes an [`Offload`] that runs
//! a job on a blocking thread (the binary uses `spawn_blocking`);
//! [`inline_offload`] runs it in place, for tests and small inputs.

use std::sync::Arc;

use domain::computed::{
    Computed, ComputedBody, ComputedStatus, ComputedWarning, InputRef, WarningSeverity,
};
use domain::workspace::WorkspacePath;

use crate::csv;
use crate::ports::compute::{ComputeError, ComputePort, ComputeRequest, DataSet, REQUEST_PARAM};
use crate::ports::fs::FileSystemPort;

/// Largest data file the tools read, in bytes.
pub const MAX_DATA_BYTES: u64 = 32 * 1024 * 1024;

/// Code of the warning a recompute adds when the data file changed.
pub const INPUT_CHANGED: &str = "input_changed";

/// Key of `method.params` that remembers the CSV delimiter the tool was
/// given, so a recompute reads the file the same way.
pub const DELIMITER_PARAM: &str = "delimiter";

/// Runs a job somewhere it may block (a blocking thread pool).
pub type Offload = Arc<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>;

/// An [`Offload`] that runs the job at once on the calling thread.
#[must_use]
pub fn inline_offload() -> Offload {
    Arc::new(|job| job())
}

/// Runs `work` through `offload` and waits for its result without
/// blocking; `None` when the job was dropped without running.
pub async fn offloaded<R: Send + 'static>(
    offload: &Offload,
    work: impl FnOnce() -> R + Send + 'static,
) -> Option<R> {
    let (tx, rx) = futures::channel::oneshot::channel();
    offload(Box::new(move || {
        // The caller may have stopped waiting; nothing to do then.
        let _ = tx.send(work());
    }));
    rx.await.ok()
}

/// Reads the CSV/TSV file `path` of the workspace through `fs` (with its
/// workspace and sensitive-path guard) as named columns. The delimiter
/// defaults to the one the extension implies.
///
/// # Errors
///
/// A message for the caller to show: bad path, unreadable or binary file,
/// malformed CSV.
pub async fn load_file(
    fs: &dyn FileSystemPort,
    path: &str,
    delimiter: Option<char>,
) -> Result<DataSet, String> {
    let parsed = WorkspacePath::parse(path).map_err(|e| format!("data path `{path}`: {e}"))?;
    let bytes = fs
        .read_bytes(&parsed, MAX_DATA_BYTES)
        .await
        .map_err(|e| format!("cannot read `{path}`: {e}"))?;
    let text = String::from_utf8(bytes).map_err(|_| format!("`{path}` is not UTF-8 text"))?;
    let source = InputRef {
        name: parsed.file_name().unwrap_or(path).to_owned(),
        role: Some("data".into()),
        path: Some(parsed.to_string()),
        hash: None,
        rows: None,
        columns: Vec::new(),
        filter: None,
    };
    let delimiter = delimiter.unwrap_or_else(|| csv::delimiter_for(path));
    csv::data_set(&text, delimiter, source).map_err(|e| format!("`{path}`: {e}"))
}

/// Who asked for a computation, written into the envelope's provenance.
pub fn set_provenance(computed: &mut Computed, agent: &str, requested_by: &str) {
    computed.provenance.agent = Some(agent.to_owned());
    computed.provenance.requested_by = Some(requested_by.to_owned());
}

/// Re-runs `envelope` on `engine`. When its data came from a file, the
/// file is read again first: if its contents changed, the new envelope
/// gets an [`INPUT_CHANGED`] warning naming the old and new hashes (and
/// most likely a mismatch in `verification`). The new envelope keeps the
/// old provenance, with `requested_by` set to `requested_by`.
///
/// # Errors
///
/// A message for the user: the envelope cannot be re-run, or its file can
/// no longer be read.
pub async fn recompute(
    engine: Arc<dyn ComputePort>,
    fs: Option<&dyn FileSystemPort>,
    offload: &Offload,
    envelope: &Computed,
    requested_by: &str,
) -> Result<(Computed, ComputedBody), String> {
    let mut request = ComputeRequest::from_envelope(envelope).map_err(|e| e.to_string())?;
    let mut stored = envelope.clone();
    if let (Some(fs), Some(path)) = (fs, request.data().and_then(|d| d.source.path.clone())) {
        let delimiter = envelope
            .method
            .params
            .get(DELIMITER_PARAM)
            .and_then(serde_json::Value::as_str)
            .and_then(|d| d.chars().next());
        let fresh = load_file(fs, &path, delimiter).await?;
        if let Some(data) = request.data_mut() {
            *data = fresh;
        }
        stored.method.params.insert(
            REQUEST_PARAM.into(),
            serde_json::to_value(&request).map_err(|e| e.to_string())?,
        );
    }
    let job = {
        let engine = Arc::clone(&engine);
        move || engine.recompute(&stored)
    };
    let (mut computed, body) = offloaded(offload, job)
        .await
        .ok_or_else(|| "the engine stopped before answering".to_owned())?
        .map_err(|e: ComputeError| e.to_string())?;
    let old_hashes: Vec<Option<&String>> =
        envelope.inputs.iter().map(|i| i.hash.as_ref()).collect();
    let new_hashes: Vec<Option<&String>> =
        computed.inputs.iter().map(|i| i.hash.as_ref()).collect();
    if old_hashes != new_hashes {
        let show = |h: &[Option<&String>]| {
            h.iter()
                .map(|h| h.map_or("?", |s| short_hash(s)))
                .collect::<Vec<_>>()
                .join(", ")
        };
        computed.warnings.push(ComputedWarning {
            code: INPUT_CHANGED.into(),
            severity: WarningSeverity::Warn,
            text: format!(
                "the data changed since it was computed (input hash {} → {})",
                show(&old_hashes),
                show(&new_hashes)
            ),
        });
        if computed.status == ComputedStatus::Ok {
            computed.status = ComputedStatus::Warning;
        }
    }
    if let Some(delimiter) = envelope.method.params.get(DELIMITER_PARAM) {
        computed
            .method
            .params
            .insert(DELIMITER_PARAM.into(), delimiter.clone());
    }
    computed.provenance = envelope.provenance.clone();
    computed.provenance.requested_by = Some(requested_by.to_owned());
    Ok((computed, body))
}

/// `blake3:9c1e3a4b…` shortened to its first 8 hex digits.
fn short_hash(hash: &str) -> &str {
    let (prefix, hex) = hash.split_once(':').unwrap_or(("", hash));
    let end = prefix.len() + usize::from(!prefix.is_empty()) + hex.len().min(8);
    &hash[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::compute::FakeCompute;
    use crate::testing::fs::{InMemoryFs, test_root};

    #[tokio::test]
    async fn loads_a_workspace_file_with_the_guard() {
        let fs = InMemoryFs::new(test_root())
            .with_file("data/cars.tsv", "mpg\tam\n21\tm\n22\ta\n")
            .with_file(".env", "a,b\n1,2\n");
        let data = load_file(&fs, "data/cars.tsv", None).await.unwrap();
        assert_eq!(data.columns.len(), 2);
        assert_eq!(data.source.path.as_deref(), Some("data/cars.tsv"));
        assert_eq!(data.source.name, "cars.tsv");
        assert!(load_file(&fs, ".env", None).await.is_err());
        assert!(load_file(&fs, "../x.csv", None).await.is_err());
        assert!(
            load_file(&fs, "missing.csv", None)
                .await
                .unwrap_err()
                .contains("missing.csv")
        );
    }

    #[tokio::test]
    async fn offloaded_work_returns_its_result() {
        assert_eq!(offloaded(&inline_offload(), || 2 + 2).await, Some(4));
        let dropping: Offload = Arc::new(|_job| {});
        assert_eq!(offloaded(&dropping, || 1).await, None);
    }

    #[tokio::test]
    async fn recompute_reloads_a_changed_file_and_says_so() {
        let fs = InMemoryFs::new(test_root()).with_file("d.csv", "x\n1\n2\n3\n");
        let engine = Arc::new(FakeCompute::new());
        let data = load_file(&fs, "d.csv", None).await.unwrap();
        let request = crate::ports::compute::DescribeRequest {
            data,
            variables: Vec::new(),
            by: None,
        };
        let (envelope, _) = engine.describe(&request).unwrap();
        let offload = inline_offload();

        let (same, _) = recompute(engine.clone(), Some(&fs), &offload, &envelope, "user")
            .await
            .unwrap();
        assert_eq!(same.verification.matched, Some(true));
        assert!(same.warnings.iter().all(|w| w.code != INPUT_CHANGED));
        assert_eq!(same.provenance.requested_by.as_deref(), Some("user"));

        fs.set_file("d.csv", "x\n1\n2\n30\n");
        let (changed, _) = recompute(engine, Some(&fs), &offload, &envelope, "user")
            .await
            .unwrap();
        assert_eq!(changed.verification.matched, Some(false));
        let warning = changed
            .warnings
            .iter()
            .find(|w| w.code == INPUT_CHANGED)
            .unwrap();
        assert!(warning.text.contains("fake:"), "{}", warning.text);
    }

    #[test]
    fn hashes_are_shortened() {
        assert_eq!(short_hash("blake3:0123456789abcdef"), "blake3:01234567");
        assert_eq!(short_hash("abc"), "abc");
    }
}
