# Third-party software

crest is licensed under the [GNU General Public License v3.0 or later](LICENSE). It is built on third-party software that keeps its own license:

- **Rust crates.** The binary links the crates listed in `Cargo.lock`, almost all under MIT and/or Apache-2.0 (a few under BSD, ISC, Zlib, Unicode-3.0, CC0 or MPL-2.0). Each crate's license is in its `Cargo.toml` and source; `cargo metadata --format-version 1` lists them all.
- **three.js r160** (MIT, © 2010-2023 three.js authors), bundled unmodified in [`crates/adapters/export/vendor/three/0.160.0/`](crates/adapters/export/vendor/three/0.160.0/) with its [license](crates/adapters/export/vendor/three/0.160.0/LICENSE), and inlined into the `board.html` files crest exports.
