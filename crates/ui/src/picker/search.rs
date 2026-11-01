//! The searches and preview reads the picker runs off the UI thread.

use std::sync::Arc;

use application_core::ports::fs::{FileSystemPort, FsError};
use application_core::ports::search::{
    ContentSearchPort, FileSearchPort, ListFilesRequest, SearchError,
};
use application_core::usecases::model3d;
use application_core::usecases::open_file::extension;
use domain::model3d::ModelSpec;
use domain::workspace::WorkspacePath;
use gpui::AppContext as _;

use super::preview::{
    PREVIEW_IMAGE_MAX_BYTES, PREVIEW_MODEL_MAX_BYTES, Preview, RichPreview, human_size,
    preview_request,
};
use super::{
    EMPTY_QUERY_LIMIT, Found, GrepOptions, PickerItem, PickerMode, files_request, grep_request,
    items_for_empty_query, items_from_files, items_from_grep,
};

/// What `Enter` and `/open` do with a file the preview cannot draw.
pub(super) const UNSUPPORTED_HINT: &str = "Enter inserts the path · /open @path shows it";

/// The text preview of `item`: the lines around its match, or an
/// "unsupported" card for a binary file.
pub(super) async fn read_text(
    fs: &Arc<dyn FileSystemPort>,
    item: &PickerItem,
    cx: &mut gpui::AsyncApp,
) -> Preview {
    let fs = Arc::clone(fs);
    let request = preview_request(item);
    let read = cx
        .background_spawn(async move { fs.read_file(request).await })
        .await;
    match read {
        Ok(content) => Preview::read(item, content),
        Err(FsError::Binary { .. }) => Preview::unsupported(item, "binary file"),
        Err(err) => Preview::failed(item, err.to_string()),
    }
}

/// The picture preview of `item`: its absolute path, once the port says
/// the file may be read (inside the root, not sensitive) and its size is
/// within [`PREVIEW_IMAGE_MAX_BYTES`]. GPUI decodes it when drawn.
pub(super) async fn read_image(
    fs: &Arc<dyn FileSystemPort>,
    item: &PickerItem,
    cx: &mut gpui::AsyncApp,
) -> Preview {
    let absolute = fs.root().join(&item.path).to_string_lossy().into_owned();
    let (fs, path) = (Arc::clone(fs), item.path.clone());
    let probe = cx
        .background_spawn(async move { fs.read_head(&path, 16).await })
        .await;
    match probe {
        Ok((_, size)) if size > PREVIEW_IMAGE_MAX_BYTES => Preview::unsupported(
            item,
            format!(
                "picture of {} (over {})",
                human_size(size),
                human_size(PREVIEW_IMAGE_MAX_BYTES)
            ),
        ),
        Ok(_) => Preview::rich(item, RichPreview::Image { path: absolute }),
        Err(err) => Preview::failed(item, err.to_string()),
    }
}

/// The 3D preview of `item`: the file read and parsed off the UI thread,
/// when it is under [`PREVIEW_MODEL_MAX_BYTES`].
pub(super) async fn read_model(
    fs: &Arc<dyn FileSystemPort>,
    item: &PickerItem,
    cx: &mut gpui::AsyncApp,
) -> Preview {
    let (fs, path) = (Arc::clone(fs), item.path.clone());
    let ext = extension(&item.path);
    let name = item.path.file_name().unwrap_or("model").to_owned();
    let parsed = cx
        .background_spawn(async move {
            let bytes = fs.read_bytes(&path, PREVIEW_MODEL_MAX_BYTES).await?;
            Ok::<_, FsError>(model3d::parse(&ext, &bytes).map(|mesh| ModelSpec {
                name,
                mesh,
                unit: None,
            }))
        })
        .await;
    match parsed {
        Ok(Ok(spec)) => Preview::rich(item, RichPreview::Model(Arc::new(spec))),
        Ok(Err(err)) => Preview::unsupported(item, format!("3D model: {err}")),
        Err(FsError::TooLarge { size, limit }) => Preview::unsupported(
            item,
            format!(
                "3D model of {} (over {})",
                human_size(size),
                human_size(limit)
            ),
        ),
        Err(err) => Preview::failed(item, err.to_string()),
    }
}

pub(super) async fn run_search(
    mode: PickerMode,
    query: &str,
    options: GrepOptions,
    recent: &[WorkspacePath],
    files: Arc<dyn FileSearchPort>,
    content: Arc<dyn ContentSearchPort>,
) -> Result<Found, SearchError> {
    match mode {
        PickerMode::Files if query.trim().is_empty() => {
            let list = files
                .list_files(ListFilesRequest {
                    glob: None,
                    limit: EMPTY_QUERY_LIMIT,
                })
                .await?;
            let items = items_for_empty_query(recent, &list);
            Ok(Found {
                total: list.total.max(items.len()),
                truncated: list.truncated,
                items,
            })
        }
        PickerMode::Files => files
            .find_file(files_request(query))
            .await
            .map(|list| Found {
                items: items_from_files(&list),
                total: list.total,
                truncated: list.truncated,
            }),
        PickerMode::Grep => content
            .grep(grep_request(query, options))
            .await
            .map(|result| Found {
                items: items_from_grep(&result),
                total: result.total,
                truncated: result.truncated,
            }),
    }
}
