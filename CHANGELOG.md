# Changelog

All notable changes are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [0.1.0] - 2026-11-08

Initial public release.

### Added

- `bbcp-core` engine: filesystem inventory in SQLite, Google Takeout sidecar
  parsing, ffprobe metadata, 64-bit pHash/dHash/aHash fingerprints, SHA-256 for
  video candidates, exact grouping without transitive closure, capture-date
  recovery, evidence-based quality ranking, consolidation plan, pixel MAD
  verification, atomic apply with quarantine and resumable journals, staging,
  Google deletion batches and Markdown reports.
- `bbcp` CLI with `init`, `doctor`, `scan`, `sidecars`, `probe`,
  `fingerprint`, `hash`, `group`, `dates`, `plan`, `verify`, `apply`, `stage`,
  `google`, `report`, `compare` and `status`.
- Safety guarantees: dry-run by default, album root protected, source purges
  require an explicit `--only`, album duplicates moved to quarantine, every
  copy verified by SHA-256 and every operation journaled.
- Agent layer: `AGENTS.md`, an OpenCode skill and a self-sufficient playbook,
  safety model, design decisions and Takeout notes.
- Scripts: `scripts/install.sh` and `scripts/preflight.sh`.
- CI: formatting, clippy with `-D warnings`, tests and release build on Linux
  and macOS.

[0.1.0]: https://github.com/juampe/byebyecp/releases/tag/v0.1.0
