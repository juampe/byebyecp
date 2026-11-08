# Third-party notices

Bye Bye Cloud Photos is distributed under the **GNU General Public License,
version 3 only** (`GPL-3.0-only`). See [LICENSE](LICENSE).

## Provenance: UMDF

The hashing, fingerprinting, grouping and "most likely original" ideas were
adapted from [UMDF — Universal Media Duplicate Finder](https://github.com/juampe/umdf),
Copyright (C) juampe and UMDF contributors, also licensed GPL-3.0-only.

The Rust implementation in this repository is a rewrite for photo-library
consolidation: new data model, new plan/apply pipeline, new safety rules and a
different grouping strategy (three 64-bit hashes, no transitive closure).

## FFmpeg

`bbcp` invokes the external `ffmpeg` and `ffprobe` binaries. They are **not**
linked into the binary and are not distributed with this repository. Install
them from your distribution or from <https://ffmpeg.org>. FFmpeg is licensed
under the LGPL/GPL depending on the build options; comply with the license of
the build you install.

## Rust crates

The binary is built from the crates listed in `Cargo.lock`. Most are dual
licensed MIT/Apache-2.0. The main direct dependencies are:

| Crate | License |
|---|---|
| `anyhow` | MIT OR Apache-2.0 |
| `chrono` | MIT OR Apache-2.0 |
| `clap` | MIT OR Apache-2.0 |
| `hex` | MIT OR Apache-2.0 |
| `image` | MIT OR Apache-2.0 |
| `kamadak-exif` | BSD-2-Clause |
| `rayon` | MIT OR Apache-2.0 |
| `regex` | MIT OR Apache-2.0 |
| `rusqlite` | MIT |
| `serde`, `serde_json` | MIT OR Apache-2.0 |
| `sha2` | MIT OR Apache-2.0 |
| `thiserror` | MIT OR Apache-2.0 |
| `toml` | MIT OR Apache-2.0 |
| `walkdir` | Unlicense OR MIT |

`rusqlite` uses the `bundled` feature, which compiles SQLite from source.
SQLite is in the public domain. See <https://sqlite.org/copyright.html>.

Generate the full dependency tree and license metadata with:

```bash
cargo tree
cargo metadata --format-version 1
```

## Media

No test images, videos or user data are distributed with this repository.
