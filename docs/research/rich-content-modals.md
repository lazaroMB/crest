# Rich content in plugin modals: research

This note records what was researched before adding plugin modals (`app.modal.*`) to crest: which kinds of developer content a modal can show, how each one can be drawn with the UI stack crest already uses, what was chosen and what it costs. Phase 1 is implemented (see [plugins.md](../plugins.md#appmodal-modal-windows)); phases 2 and 3 are the roadmap at the end.

Versions checked: `gpui-pre 0.3.9` (GPUI snapshot zed@93d7456), its platform crates (`gpui-pre-linux`, `gpui-pre-wgpu`, `gpui-pre-macos`) and `gpui-component 0.7.3` / `gpui-base 0.7.3`, read from their sources in the cargo registry.

## Summary

- Everything crest needs for phase 1 already exists natively: GPUI's `img()`, gpui-component's `TextView` (markdown and a subset of HTML), `DataTable`, charts, the unified diff renderer and crest's own Mermaid painter. A modal is a GPUI overlay that draws a `CustomNode` tree, the same declarative tree Lua already builds for `Custom` blocks.
- Heavier content goes through one pattern: **render to pixels off the UI thread, hand GPUI a `RenderImage`**. That covers PDF pages, 3D scenes and, with a cell grid instead of pixels, a terminal.
- **Web pages cannot be embedded**, on Linux in particular. They are out of scope: links (`app.ui.link`, links inside markdown and HTML) open the user's browser with `cx.open_url`.

## GPUI constraints

### Images: `img()`

`img(source)` takes an `ImageSource`: a path or URI (`Resource`), encoded bytes (`Arc<Image>`), an already decoded `Arc<RenderImage>`, or a custom loader closure. Decoding uses the `image` crate 0.25 (PNG, JPEG, WebP, GIF, BMP, TIFF, ICO, PNM and more) and `resvg`/`usvg` 0.46 for SVG (with text and system fonts). Animated GIF and WebP play: `Img` keeps a frame index per element and advances it with each frame's delay (it stops while the window is inactive or with reduced motion).

Layout follows the image: with no explicit size an `img` takes its natural size and aspect ratio, and `ObjectFit` (`Contain`, `Cover`, `Fill`, `ScaleDown`, `None`) places it inside explicit bounds. The natural size is available through `Image::use_render_image` (inline bytes) or `window.use_asset::<ImgResourceLoader>` (paths), which is what the phase-1 image viewer uses for its 1:1 and zoom levels.

String sources that are not URIs are looked up in the app's `AssetSource` (crest embeds none), and URIs go through GPUI's HTTP client. So `<img src>` inside HTML or markdown cannot read arbitrary files from disk; only an explicit `app.ui.image{ path = ... }` does, and crest checks those paths (see [Security](#security)).

### Atlas size: huge images need tiling

Every decoded frame is uploaded to the GPU atlas. The wgpu atlas (`gpui-pre-wgpu`, `wgpu_atlas.rs`) caps a texture at the device's `max_texture_dimension_2d` (commonly 8192 or 16384 px). A larger image cannot be uploaded in one piece: a viewer for very large images (a 30000 px wide flame graph PNG, a scanned poster) must cut it into tiles and draw only the visible ones, with a downscaled overview when zoomed out. Phase 1 does not tile; images beyond the limit fail to draw and show the fallback.

### No native surfaces on Linux

`Window::paint_surface` (draws a platform video surface into the scene) only exists on macOS and iOS (`#[cfg(any(target_os = "macos", target_os = "ios"))]`) and takes a `CVPixelBuffer`. On Linux (X11 and Wayland, through wgpu) there is no public API to hand GPUI an external texture or a child surface. Anything that is not drawn with GPUI primitives must become CPU pixels (`RenderImage`), which are then uploaded like any image. That is fine for documents and static scenes, and acceptable for interactive 3D at modest sizes; it is not zero-copy.

### Text: markdown and HTML

gpui-component's `TextView::markdown` renders CommonMark + GFM (tables, task lists, code blocks highlighted with Tree-sitter). `TextView::html` parses with `html5ever` and supports a basic subset: `a b blockquote br code del em h1–h6 i img li mark ol s strong table tbody td th thead u ul` and a few attributes (`href`, `src`, `alt`, `width`, `height`, `style` colors). There is no CSS layout, no forms and no scripts; anything else is flattened to text. It is enough for generated reports (coverage summaries, rustdoc fragments, test reports), not for web pages.

Links in both call `cx.open_url` by default (`TextView::on_link_click` can override that).

## Web content: out of scope

- gpui-component 0.7.3 has no webview. The separate `gpui-wry 0.5` embeds `wry` only on macOS and Windows.
- `wry` itself creates child webviews on macOS, Windows and X11 (GTK); on **Wayland a child webview is impossible** (no subsurface support in `wry`), and crest runs natively on Wayland.
- An off-screen browser (CEF, Servo) would mean hundreds of MB of dependencies, its own process model and, on Linux, the same pixel-copy path as above at video rates.

Decision: no embedded web content. `app.ui.link(text, url)` and the links inside markdown and HTML open the user's browser. Only `http`, `https` and `mailto` links are accepted.

## Content types

| Content | Options considered | Chosen | Phase | Cost |
| --- | --- | --- | --- | --- |
| Images (PNG, JPEG, GIF/WebP animated, SVG, BMP, TIFF, ICO) | `img()`; own decoder | `img()` with a viewer: fit, 1:1, zoom, drag to pan | 1 | done |
| Markdown | `TextView::markdown` | `TextView::markdown` | 1 | done |
| Simple HTML | `TextView::html`; a webview | `TextView::html` (basic subset) | 1 | done |
| Tables, CSV | `DataTable` | `DataTable` (virtualized, sortable); `app.ui.csv` parses CSV in Lua into a table | 1 | done |
| Charts | gpui-component charts | existing chart renderer | 1 | done |
| JSON | gpui-component `tree` (virtualized list, needs a fixed height); own view | own collapsible tree that nests inside the scrolling modal | 1 | done |
| Code | `TextView` code blocks | existing code node | 1 | done |
| Mermaid | own painter (rusty-mermaid layout) | existing Mermaid view with pan and zoom | 1 | done |
| Diffs | gpui-component `diff`; own renderer | existing unified diff renderer | 1 | done |
| Links | `cx.open_url` | `cx.open_url` (`http`, `https`, `mailto`) | 1 | done |
| PDF | `hayro` 0.7 (pure Rust); `pdfium-render` (Chromium's PDFium as a shared library); `mupdf` | `hayro` renders pages to `RenderImage` off the UI thread; `pdfium-render` as an optional backend for fidelity; `mupdf` rejected (AGPL) | 2 | M (1–2 weeks) |
| Jupyter `.ipynb` | `nbconvert` to HTML; native | native: the notebook is JSON; markdown cells → `TextView`, code cells → code, outputs → text, images (base64 PNG/SVG) and HTML subset | 2 | S–M (1 week) |
| DOCX / ODT | own parser; `pandoc` CLI | optional `pandoc -t gfm` through an approved `shell:pandoc` capability → markdown | 2 | S (2–3 days) |
| Logs with ANSI colors | `vte` / own SGR parser | own SGR parser into styled runs, virtualized list, follow mode for growing files | 2 | S–M (1 week) |
| Hex viewer | own | own: offset, hex and ASCII columns, virtualized by rows, paged reads | 2 | S (2–3 days) |
| Large CSV / Parquet | load all; paging | paged `DataTable` delegate; Parquet through `arrow`/`parquet` behind a feature | 2 | M (1–2 weeks) |
| 3D models (glTF, OBJ, STL) | `three-d`, `bevy`, `kiss3d`, `rend3`; own wgpu renderer | own small wgpu off-screen renderer (`gltf`, `tobj`, `stl_io` loaders) → `RenderImage`, orbit camera | 3 | L (3–4 weeks) |
| Embedded terminal | `alacritty_terminal` + own grid; Zed's terminal | `alacritty_terminal` (Apache-2.0) for the VT state and PTY, drawn as a GPUI cell grid written for crest | 3 | L (3–4 weeks) |
| Flame graphs | `inferno` SVG; native canvas | native canvas from folded stacks (zoom on click, search) | 3 | M (1–2 weeks) |
| Graphviz `dot` | `graphviz` CLI; `layout-rs` | `layout-rs` (pure Rust) to SVG → `img()` | 3 | S–M (1 week) |
| Video | `gstreamer` appsink → frames; `ffmpeg` | `gstreamer` frames into `RenderImage`, later; no zero-copy on Linux (see above) | later | L |
| Web pages | webview (`wry`, `gpui-wry`), CEF | not embedded: open in the browser | — | — |

### PDF

- **`hayro` 0.7** (chosen): pure Rust (no C, no system library), renders pages to RGBA through its own rasterizer; good coverage of real-world PDFs, fonts included. Rendering runs on a background thread per visible page at the current zoom; pages are cached as `RenderImage`s and evicted by distance.
- **`pdfium-render`**: best fidelity (Chromium's engine), but it loads a `libpdfium` that crest would have to ship or find. Worth it as an optional backend behind a feature.
- **`mupdf`**: excellent, but AGPL; incompatible with crest's MIT/Apache-2.0 license for a linked dependency. Rejected.

### 3D

GPUI exposes no way to share a wgpu device or put a texture in the scene on Linux, so a 3D view must render off-screen and copy pixels out:

- **Own renderer** (chosen): a small wgpu pipeline (one PBR-lite shader, depth buffer, MSAA) rendering into an off-screen texture, read back into a `RenderImage` per frame while the camera moves, idle otherwise. Loaders: `gltf`, `tobj` (OBJ), `stl_io`. An orbit camera driven by drag and wheel. Small, under crest's control, and it shares the wgpu version GPUI already builds.
- **`three-d`**: brings its own window/context assumptions and a GL path; pulling a second rendering stack for one viewer is heavy.
- **`bevy`**: a full engine with its own app loop, ECS and asset system; far too large to embed in a modal.
- **`kiss3d`**: owns the window and event loop; not embeddable.
- **`rend3`**: unmaintained, and tied to older wgpu versions.

### Terminal

`alacritty_terminal` provides the VT parser, the grid with scrollback and the PTY handling; crest draws the grid with GPUI text runs (one shaped line per row, cached while it does not change) and maps keys to escape sequences. Zed's terminal view does the same, but it is GPL-3.0: crest must not copy it; the GPUI drawing is written from scratch. Running a shell from a plugin needs a capability (`shell:`) and the user's approval like `app.shell.run`.

## Security

- Image files named by a plugin (`app.ui.image{ path = ... }`) are checked in the composition root before the UI sees them, with the same rules as the file tools: the path must resolve (following symlinks) inside the workspace and must not match the sensitive-path rules (`.env`, `.git/`, keys...). A rejected image becomes a text node saying why. This applies to modals and to `Custom` blocks.
- Links only open `http`, `https` and `mailto` URLs.
- `TextView` HTML has no scripts and cannot load local files.
- Modal ids are scoped by plugin: a plugin cannot update or close another plugin's modal.

## Roadmap

### Phase 1 (implemented)

`app.modal.open/update/close`, geometry in percent of the window with a fixed 20 px margin and anchors, Esc and a close button, scrolling content, and these nodes: image (fit, 1:1, zoom, pan), markdown, HTML subset, table, CSV (as a table), chart, JSON tree, code, Mermaid, diff, link. Images are checked against the workspace and the sensitive-path rules. Headless mode prints a one-line notice.

### Phase 2 (about 5–7 weeks)

| Item | Approach | Estimate |
| --- | --- | --- |
| PDF | `hayro` page rendering off the UI thread, page cache, zoom; `pdfium-render` optional backend | 1–2 weeks |
| Jupyter `.ipynb` | native cells (markdown, code, text/image/HTML outputs) | 1 week |
| DOCX / ODT | optional `pandoc` CLI to markdown | 2–3 days |
| Log / ANSI viewer | SGR parser, virtualized, follow mode | 1 week |
| Hex viewer | virtualized rows, paged reads | 2–3 days |
| CSV / Parquet paging | paged table delegate; Parquet behind a feature | 1–2 weeks |
| Image tiling | tiles below the atlas limit, overview when zoomed out | 3–4 days |

### Phase 3 (about 7–10 weeks)

| Item | Approach | Estimate |
| --- | --- | --- |
| 3D viewer | glTF/OBJ/STL, own wgpu off-screen renderer → `RenderImage`, orbit camera | 3–4 weeks |
| Embedded terminal | `alacritty_terminal` + own GPUI grid, behind a `shell:` capability | 3–4 weeks |
| Flame graphs | native canvas from folded stacks | 1–2 weeks |
| Graphviz `dot` | `layout-rs` → SVG → `img()` | 1 week |

Video (`gstreamer`) stays after phase 3: without a surface API on Linux every frame is a CPU copy, which needs measuring before committing to it.
