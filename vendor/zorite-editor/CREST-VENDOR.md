# zorite-editor, vendored

zorite-editor 0.11.3 from crates.io (repo https://github.com/packetThrower/zorite),
MIT (see `LICENSE`), vendored on 2026-11-17. `Cargo.toml.orig`, `Cargo.lock`,
`.cargo_vcs_info.json` and `.cargo-ok` were dropped. The crate is not a workspace
member (root `Cargo.toml`: `exclude` + `[patch.crates-io]`).

## Local changes

1. **Spaces at the edge of a table cell are no longer swallowed.**
   `markdown_syntax::table_cell_extents` (new: trimmed start to the next pipe),
   `TableRow::cell_extents` (new field, filled in `element.rs`),
   `tables::table_cell_at`, `EditorState::caret_table_cell_pos` (pick the cell
   "before the next pipe"), `tables::table_caret_pos` (draws the caret at the
   content end plus the advance of the extra spaces),
   `EditorState::table_move_horizontal` (Left/Right step through the spaces),
   `EditorState::caret_pos_for_cell` (may sit before the pipe when the cell has
   trailing spaces), selection edge in `element.rs` (`last_end`). The source is
   never trimmed on edit. Tests: `table_cell_space_tests`, `cell_extent_tests`.
2. **Hover handles clipped and hidden when scrolled out.** `element.rs` prepaint
   table interaction + paint: `TableAffordance::{clip, show_pill}`; the outline
   (and the caret cell outline) paint under a content mask of
   `table_visible_band`; the row pill is hidden once the table is scrolled off
   its gutter edge; the column pill is centred on the visible part of its column
   and hidden when that part is narrower than the pill; column hover only inside
   the band.
3. **Fit a table wider than the editor.** `tables::table_column_widths` (new
   `fit_avail` argument), `tables::fit_column_widths` (new), `TABLE_FIT_MIN`,
   `EditorState::set_table_fit(bool)` (default true, field `table_fit`),
   `shape_document` (new `table_fit` argument; `fit_avail` = wrap width minus
   `TABLE_GUTTER`, part of the column cache key). Not applied when the table has
   `cols=`; nothing is written to the document. Each column keeps
   `min(natural, max(72, widest unbreakable piece))` (words split at whitespace
   and `/`), the rest is shared in proportion to natural width; horizontal scroll
   remains only when the minimums do not fit. Test: `fitting_shrinks_...`.
4. **"Module" header wrapped as "Modul / e".** Root cause: `table_column_widths`
   sized a column to the exact float width of an unwrapped `shape_line`, while
   the paint and `table_row_wrap_rows` wrap at `column - 2 * pad`; with zero slack
   a rounding difference wraps the last glyph (the `autofit` path already
   worked around it with `ceil`). Fix: measure through `shape_cell` (the shaper
   the paint uses, same font, bold header), round up and add `TABLE_CELL_SLACK`
   (1 px).
5. **Corner-grip resize deleted the link of an image whose alt has a `)`.**
   `set_image_width` (lib.rs) ended the image body at the first `)` of the line,
   so `![image (14)](assets/a.png)` became `![image (14){width=727}`. Now it finds
   `](` after the list marker, then the first `)` after that; if either is
   missing the line is returned unchanged. Audited and left as is:
   `markdown_syntax::image_line` / `image_row` (already `](` first),
   `inline_image_spans` (`]` then `(` then `)`, fine for `)` in alt),
   `inline_image_at` (`rfind('(')` / `rfind(')')`, fine). Tests:
   `set_image_width_alt_with_parentheses*`, `set_image_width_list_item_alt_with_parentheses`.

Note: `cargo test` standalone needs the host's `Cargo.lock` copied next to this
manifest (a fresh resolve picks a newer gpui-pre) and `--lib`: the `demo` example
imports `os_spellcheck`, which the manifest does not declare.
