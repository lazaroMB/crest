//! `/open @path`: shows a workspace file in a modal.
//!
//! The argument names the file with a leading `@`, which is stripped; the
//! rest is resolved against the workspace root (the directory crest was
//! started in, or `--workspace`). Every read goes through the
//! [`FileSystemPort`], so the same rules as the file tools apply: no path
//! outside the root (`..` or a symlink that leaves it) and no sensitive
//! path. The viewer is chosen from the extension; the contents are only
//! shown, never run or rendered as a page. 3D models (OBJ, STL, PLY) are
//! parsed here ([`model3d`](super::model3d)) and drawn with an orbit camera.

use domain::block::{CustomNode, ImageData, ImageFit};
use domain::modal::{ModalId, ModalPosition, ModalSpec, Percent};
use domain::model3d::ModelSpec;
use domain::workspace::{PathError, WorkspacePath};

use crate::ports::fs::{FileContent, FileSystemPort, FsError, ReadFileRequest};
use crate::usecases::model3d::{self, MAX_MODEL_BYTES, MODEL_EXTENSIONS};

/// Most lines shown; longer files are cut and say so in the title.
pub const MAX_OPEN_LINES: usize = 20_000;
/// Share of the window the modal takes, in percent.
const OPEN_MODAL_PERCENT: f32 = 85.0;
/// Extensions shown as pictures.
const IMAGE_EXTENSIONS: [&str; 8] = ["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico"];

/// Why a file cannot be opened; the message says what to do instead.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpenError {
    /// No argument, or one without the `@`.
    #[error("usage: /open @<path> (type @ to pick a file)")]
    Usage,
    /// The path leaves the workspace root or is malformed.
    #[error("cannot open {path}: it is outside the workspace {root}")]
    Outside {
        /// The path as typed, without the `@`.
        path: String,
        /// The workspace root.
        root: String,
    },
    /// The path is malformed (empty, NUL bytes).
    #[error("cannot open {path}: {reason}")]
    Invalid {
        /// The path as typed.
        path: String,
        /// Why it is invalid.
        reason: String,
    },
    /// Nothing at that path.
    #[error("{path} does not exist; type @ to pick a file")]
    NotFound {
        /// The path.
        path: String,
    },
    /// The path is a directory.
    #[error("{path} is a directory; add / to browse it and pick a file")]
    Directory {
        /// The path.
        path: String,
    },
    /// The file matches a sensitive-path pattern.
    #[error("{path} is denied by the sensitive path pattern `{pattern}`")]
    Sensitive {
        /// The path.
        path: String,
        /// The pattern that denied it.
        pattern: String,
    },
    /// The file is too large to show.
    #[error("{path} is too large to show ({size} bytes, limit {limit})")]
    TooLarge {
        /// The path.
        path: String,
        /// Its size in bytes.
        size: u64,
        /// The limit in bytes.
        limit: u64,
    },
    /// A binary file that is not a picture.
    #[error("{path} is a binary file and cannot be shown")]
    Binary {
        /// The path.
        path: String,
    },
    /// A 3D model file that cannot be shown.
    #[error("cannot show the 3D model {path}: {reason}")]
    Model {
        /// The path.
        path: String,
        /// What is wrong with it.
        reason: String,
    },
    /// Any other failure while reading.
    #[error("cannot open {path}: {message}")]
    Io {
        /// The path.
        path: String,
        /// The error.
        message: String,
    },
}

/// The workspace path named by `arg` (`@src/main.rs`, or quoted when it
/// has spaces: `@"My Docs/a.md"`).
///
/// # Errors
///
/// [`OpenError::Usage`] without an `@` path, [`OpenError::Outside`] or
/// [`OpenError::Invalid`] when the path does not resolve inside `fs`'s
/// root.
pub fn parse_target(fs: &dyn FileSystemPort, arg: &str) -> Result<WorkspacePath, OpenError> {
    let path = arg
        .trim()
        .strip_prefix('@')
        .map(unquote)
        .filter(|path| !path.is_empty())
        .ok_or(OpenError::Usage)?;
    fs.root().resolve(path).map_err(|err| match err {
        PathError::InvalidPath { reason } => OpenError::Invalid {
            path: path.to_owned(),
            reason: reason.to_string(),
        },
        PathError::OutsideWorkspace { .. } | PathError::NotAbsoluteRoot { .. } => {
            OpenError::Outside {
                path: path.to_owned(),
                root: root(fs),
            }
        }
    })
}

/// `path` without its quotes: `"a b/c.md"` and the still open `"a b/c.md`
/// (as completion leaves it) are both `a b/c.md`.
fn unquote(path: &str) -> &str {
    path.strip_prefix('"')
        .map_or(path, |inner| inner.strip_suffix('"').unwrap_or(inner))
}

/// Reads the file `arg` names and builds the modal that shows it.
///
/// # Errors
///
/// An [`OpenError`] when the path is missing, outside the workspace,
/// denied, a directory, too large, a binary that is not a picture or a
/// broken 3D model.
pub async fn open_file(fs: &dyn FileSystemPort, arg: &str) -> Result<ModalSpec, OpenError> {
    let path = parse_target(fs, arg)?;
    let shown = path.as_str().to_owned();
    if path.is_root() {
        return Err(OpenError::Directory { path: shown });
    }
    let ext = extension(&path);
    let (content, truncated) = if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        (image(fs, &path).await?, false)
    } else if MODEL_EXTENSIONS.contains(&ext.as_str()) {
        (model(fs, &path, &ext).await?, false)
    } else {
        let file = read_text(fs, &path).await?;
        let truncated = file.truncated;
        (viewer(&ext, file.lines.join("\n")), truncated)
    };
    let title = if truncated {
        format!("{shown} (first {MAX_OPEN_LINES} lines)")
    } else {
        shown.clone()
    };
    let size = Percent::new(OPEN_MODAL_PERCENT).unwrap_or_default();
    Ok(ModalSpec {
        id: ModalId::new(format!("open:{shown}")),
        plugin: "crest".into(),
        title,
        width: size,
        height: size,
        position: ModalPosition::CENTER,
        content,
    })
}

/// The whole text of `path` (cut at [`MAX_OPEN_LINES`]); files over
/// [`MAX_READ_BYTES`](crate::ports::fs::MAX_READ_BYTES) are refused.
async fn read_text(
    fs: &dyn FileSystemPort,
    path: &WorkspacePath,
) -> Result<FileContent, OpenError> {
    let first = fs
        .read_file(ReadFileRequest::new(path.clone()))
        .await
        .map_err(|err| open_error(&root(fs), path, err))?;
    if !first.truncated {
        return Ok(first);
    }
    // Longer than the default line limit but within the size limit (the
    // first read checked it): read again with the larger one.
    let request = ReadFileRequest {
        path: path.clone(),
        offset: Some(1),
        limit: Some(MAX_OPEN_LINES),
    };
    fs.read_file(request)
        .await
        .map_err(|err| open_error(&root(fs), path, err))
}

/// A picture node for `path`, after checking through the port that it may
/// be read (it exists, is a file, inside the root and not sensitive).
async fn image(fs: &dyn FileSystemPort, path: &WorkspacePath) -> Result<CustomNode, OpenError> {
    let probe = ReadFileRequest {
        path: path.clone(),
        offset: Some(1),
        limit: Some(1),
    };
    match fs.read_file(probe).await {
        // An SVG is text; any other picture is binary.
        Ok(_) | Err(FsError::Binary { .. }) => Ok(CustomNode::Image {
            source: ImageData::Path {
                path: path.as_str().to_owned(),
            },
            media_type: None,
            alt: Some(path.as_str().to_owned()),
            fit: ImageFit::Contain,
        }),
        Err(err) => Err(open_error(&root(fs), path, err)),
    }
}

/// A 3D model node for `path`, read whole through the port (up to
/// [`MAX_MODEL_BYTES`]) and parsed as `ext`.
async fn model(
    fs: &dyn FileSystemPort,
    path: &WorkspacePath,
    ext: &str,
) -> Result<CustomNode, OpenError> {
    let bytes = fs
        .read_bytes(path, MAX_MODEL_BYTES)
        .await
        .map_err(|err| open_error(&root(fs), path, err))?;
    let mesh = model3d::parse(ext, &bytes).map_err(|err| OpenError::Model {
        path: path.as_str().to_owned(),
        reason: err.to_string(),
    })?;
    Ok(CustomNode::Model3d(ModelSpec {
        name: path.file_name().unwrap_or(path.as_str()).to_owned(),
        mesh,
    }))
}

/// The node that shows a text file with extension `ext`.
fn viewer(ext: &str, text: String) -> CustomNode {
    match ext {
        "md" | "markdown" => CustomNode::Markdown { source: text },
        "json" => match serde_json::from_str(&text) {
            Ok(value) => CustomNode::Json {
                value,
                expand_depth: 2,
            },
            Err(_) => code(ext, text),
        },
        "mmd" | "mermaid" => CustomNode::Mermaid { source: text },
        "diff" | "patch" => CustomNode::Diff { unified: text },
        _ => code(ext, text),
    }
}

fn code(ext: &str, source: String) -> CustomNode {
    CustomNode::Code {
        lang: (!ext.is_empty()).then(|| ext.to_owned()),
        source,
    }
}

fn root(fs: &dyn FileSystemPort) -> String {
    fs.root().as_path().display().to_string()
}

/// The lowercase extension of `path`, or `""`.
fn extension(path: &WorkspacePath) -> String {
    path.file_name()
        .and_then(|name| name.rsplit_once('.'))
        .filter(|(stem, _)| !stem.is_empty())
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

fn open_error(root: &str, path: &WorkspacePath, err: FsError) -> OpenError {
    let shown = path.as_str().to_owned();
    match err {
        FsError::Path(PathError::InvalidPath { reason }) => OpenError::Invalid {
            path: shown,
            reason: reason.to_string(),
        },
        FsError::Path(_) => OpenError::Outside {
            path: shown,
            root: root.to_owned(),
        },
        FsError::NotFound { .. } => OpenError::NotFound { path: shown },
        FsError::IsDirectory { .. } => OpenError::Directory { path: shown },
        FsError::SensitivePath { pattern, .. } => OpenError::Sensitive {
            path: shown,
            pattern,
        },
        FsError::TooLarge { size, limit } => OpenError::TooLarge {
            path: shown,
            size,
            limit,
        },
        FsError::Binary { .. } => OpenError::Binary { path: shown },
        other => OpenError::Io {
            path: shown,
            message: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use domain::workspace::{SensitivePathRules, WorkspaceRoot};

    use super::*;
    use crate::testing::fs::InMemoryFs;

    fn fs() -> InMemoryFs {
        InMemoryFs::new(WorkspaceRoot::new("/ws").unwrap())
            .with_file("README.md", "# Hi")
            .with_file("src/main.rs", "fn main() {}\n")
            .with_file("data.json", "{\"a\": 1}")
            .with_file("broken.json", "{")
            .with_file("change.patch", "--- a\n+++ b\n")
            .with_file("Makefile", "all:\n")
            .with_file("logo.svg", "<svg/>")
            .with_bytes("shot.png", [0x89, b'P', b'N', b'G', 0, 0])
            .with_bytes("app.bin", [0, 1, 2])
            .with_file(".env", "KEY=1")
            .with_file("My Docs/notes v2.md", "# Notes")
    }

    async fn open(fs: &InMemoryFs, arg: &str) -> Result<ModalSpec, OpenError> {
        open_file(fs, arg).await
    }

    #[tokio::test]
    async fn picks_the_viewer_from_the_extension() {
        let fs = fs();
        let content = |spec: ModalSpec| spec.content;
        assert_eq!(
            content(open(&fs, "@README.md").await.unwrap()),
            CustomNode::Markdown {
                source: "# Hi".into()
            }
        );
        assert!(matches!(
            content(open(&fs, "@data.json").await.unwrap()),
            CustomNode::Json { .. }
        ));
        assert!(matches!(
            content(open(&fs, "@broken.json").await.unwrap()),
            CustomNode::Code { lang: Some(l), .. } if l == "json"
        ));
        assert!(matches!(
            content(open(&fs, "@change.patch").await.unwrap()),
            CustomNode::Diff { .. }
        ));
        assert_eq!(
            content(open(&fs, " @src/main.rs ").await.unwrap()),
            CustomNode::Code {
                lang: Some("rs".into()),
                source: "fn main() {}".into()
            }
        );
        assert!(matches!(
            content(open(&fs, "@Makefile").await.unwrap()),
            CustomNode::Code { lang: None, .. }
        ));
    }

    #[tokio::test]
    async fn pictures_open_as_images_and_other_binaries_are_refused() {
        let fs = fs();
        for name in ["shot.png", "logo.svg"] {
            let spec = open(&fs, &format!("@{name}")).await.unwrap();
            assert!(matches!(
                spec.content,
                CustomNode::Image { source: ImageData::Path { path }, .. } if path == name
            ));
        }
        assert_eq!(
            open(&fs, "@app.bin").await,
            Err(OpenError::Binary {
                path: "app.bin".into()
            })
        );
    }

    #[tokio::test]
    async fn the_modal_is_titled_and_scoped_by_path() {
        let spec = open(&fs(), "@src/../src/main.rs").await.unwrap();
        assert_eq!(spec.id, ModalId::new("open:src/main.rs"));
        assert_eq!(spec.title, "src/main.rs");
        assert_eq!(spec.plugin, "crest");
    }

    #[tokio::test]
    async fn bad_arguments_and_paths_are_explained() {
        let fs = fs();
        for arg in ["", "  ", "@", "README.md", "@\"\"", "@\""] {
            assert_eq!(open(&fs, arg).await, Err(OpenError::Usage), "{arg:?}");
        }
        for arg in ["@../etc/passwd", "@/etc/passwd", "@src/../../x"] {
            assert!(
                matches!(open(&fs, arg).await, Err(OpenError::Outside { .. })),
                "{arg}"
            );
        }
        assert_eq!(
            open(&fs, "@/ws/README.md").await.unwrap().title,
            "README.md"
        );
        assert_eq!(
            open(&fs, "@nope.rs").await,
            Err(OpenError::NotFound {
                path: "nope.rs".into()
            })
        );
        for arg in ["@src", "@src/", "@."] {
            assert!(
                matches!(open(&fs, arg).await, Err(OpenError::Directory { .. })),
                "{arg}"
            );
        }
        assert!(matches!(
            open(&fs, "@.env").await,
            Err(OpenError::Sensitive { pattern, .. }) if pattern == ".env*"
        ));
    }

    #[tokio::test]
    async fn quoted_paths_may_have_spaces() {
        let fs = fs();
        for arg in [
            "@\"My Docs/notes v2.md\"",
            "@\"My Docs/notes v2.md",
            "@My Docs/notes v2.md",
        ] {
            assert_eq!(
                open(&fs, arg).await.unwrap().title,
                "My Docs/notes v2.md",
                "{arg}"
            );
        }
    }

    /// An ASCII STL of a tetrahedron: 4 facets.
    const TETRA_STL: &str = "solid t
facet normal 0 0 0
outer loop
vertex 0 0 0
vertex 0 1 0
vertex 1 0 0
endloop
endfacet
facet normal 0 0 0
outer loop
vertex 0 0 0
vertex 1 0 0
vertex 0 0 1
endloop
endfacet
facet normal 0 0 0
outer loop
vertex 0 0 0
vertex 0 0 1
vertex 0 1 0
endloop
endfacet
facet normal 0 0 0
outer loop
vertex 1 0 0
vertex 0 1 0
vertex 0 0 1
endloop
endfacet
endsolid t
";

    #[tokio::test]
    async fn models_open_in_the_3d_viewer() {
        let fs = fs()
            .with_file("parts/Tetra.STL", TETRA_STL)
            .with_file("tri.obj", "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n");
        let spec = open(&fs, "@parts/Tetra.STL").await.unwrap();
        assert_eq!(spec.title, "parts/Tetra.STL");
        let CustomNode::Model3d(model) = spec.content else {
            panic!("not a model: {:?}", spec.content);
        };
        assert_eq!(model.name, "Tetra.STL");
        assert_eq!(model.mesh.triangles.len(), 4);
        assert_eq!(model.mesh.positions.len(), 4);
        assert!(matches!(
            open(&fs, "@tri.obj").await.unwrap().content,
            CustomNode::Model3d(model) if model.mesh.triangles.len() == 1
        ));
    }

    #[tokio::test]
    async fn broken_and_denied_models_are_explained() {
        let rules = SensitivePathRules::new(["secret/"]).unwrap();
        let fs = fs()
            .with_bytes("broken.stl", [1, 2, 3])
            .with_file("empty.obj", "# nothing\n")
            .with_file("secret/part.stl", TETRA_STL)
            .with_sensitive_rules(rules);
        for name in ["broken.stl", "empty.obj"] {
            assert!(
                matches!(
                    open(&fs, &format!("@{name}")).await,
                    Err(OpenError::Model { ref path, .. }) if path == name
                ),
                "{name}"
            );
        }
        assert!(matches!(
            open(&fs, "@secret/part.stl").await,
            Err(OpenError::Sensitive { .. })
        ));
        assert!(matches!(
            open(&fs, "@missing.ply").await,
            Err(OpenError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn symlinks_out_of_the_root_and_large_files_are_refused() {
        let fs = fs()
            .with_file("link.txt", "outside")
            .with_escaping_link("link.txt")
            .with_file("big.txt", "x".repeat(64))
            .with_max_read_bytes(32);
        assert!(matches!(
            open(&fs, "@link.txt").await,
            Err(OpenError::Outside { .. })
        ));
        assert!(matches!(
            open(&fs, "@big.txt").await,
            Err(OpenError::TooLarge { size: 64, .. })
        ));
    }

    #[tokio::test]
    async fn custom_sensitive_rules_apply() {
        let rules = SensitivePathRules::new(["secrets/"]).unwrap();
        let fs = fs()
            .with_file("secrets/key.txt", "k")
            .with_sensitive_rules(rules);
        assert!(matches!(
            open(&fs, "@secrets/key.txt").await,
            Err(OpenError::Sensitive { .. })
        ));
    }

    #[tokio::test]
    async fn long_files_are_cut_and_say_so() {
        let text = "line\n".repeat(MAX_OPEN_LINES + 5);
        let fs = fs().with_file("long.txt", text);
        let spec = open(&fs, "@long.txt").await.unwrap();
        assert!(spec.title.contains("first 20000 lines"), "{}", spec.title);
        let CustomNode::Code { source, .. } = spec.content else {
            panic!("not code");
        };
        assert_eq!(source.lines().count(), MAX_OPEN_LINES);
    }
}
