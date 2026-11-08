# AGENTS.md

Instructions for agents (human or AI) working **on this repository** or using
it to consolidate a real photo library. The full operational guide is in
[`docs/playbook.md`](docs/playbook.md); the canonical OpenCode skill is in
[`.opencode/skills/bye-bye-cloud-photos/SKILL.md`](.opencode/skills/bye-bye-cloud-photos/SKILL.md).

## 1. What this project is

`bbcp` (Bye Bye Cloud Photos) is a Rust CLI that:

1. inventories one or more roots (album + Takeout exports + old dumps +
   recycle bins) into SQLite;
2. groups exact copies by content (perceptual hashes for images, SHA-256 for
   videos);
3. decides which copy to keep, what to import, what to upgrade and what to
   move to quarantine;
4. verifies by pixels/frames before deleting;
5. performs atomic, checksummed copies and keeps a resumable journal;
6. builds Google Photos deletion batches only for items with a verified local
   copy.

The goal is not "delete fast" but **being able to prove nothing was lost**.

## 2. Non-negotiable invariants

Any code change must preserve them:

1. **Nothing is removed without a verified copy.** A `delete` or
   `external_covered` action exists only when the group has an album member
   that survives the plan.
2. **Everything is reversible by default.** Album duplicates move to
   quarantine; `unlink` is never used unless explicitly requested.
3. **Writes are atomic.** Temp file in the destination directory, `fsync`,
   SHA-256 of source and destination, `rename`.
4. **Destructive actions are opt-in.** `apply` is a dry run without
   `--execute`. `external_covered`/`external_dup` are not in the default
   `--only` set.
5. **The album is not automatically the best copy.** Ranking is based on
   pixels, bitrate/size, age and file size.
6. **No transitive closure.** A group is the exact hash triple; chaining
   similar images merges bursts and different photos.
7. **When in doubt, keep.** If verification fails or a file cannot be decoded,
   the action becomes `keep_distinct`.
8. **The database never lives on 9p/NFS/FUSE.** SQLite plus high latency plus
   unreliable locking is a dangerous combination.
9. **Two independent backups are the operator's responsibility.** Remind the
   user and point at `DISCLAIMER.md`, but do not build or add runtime checks
   that block `apply`: the tool cannot verify that a copy is real, and
   enforcing it would turn advice into a false guarantee. Quarantine is not a
   backup.

If a task seems to require breaking an invariant, stop and ask.

## 3. Development commands

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release
```

CI runs exactly that on Linux and macOS. Clippy warnings are not accepted.

Minimal manual test (no real library required):

```bash
cargo run -q -p bbcp-cli -- init --album /tmp/bbcp-demo/Album --force
cargo run -q -p bbcp-cli -- doctor
cargo run -q -p bbcp-cli -- scan
cargo run -q -p bbcp-cli -- fingerprint
cargo run -q -p bbcp-cli -- group --reset
cargo run -q -p bbcp-cli -- plan
```

To generate test media: `ffmpeg -f lavfi -i testsrc=size=320x240 -t 2 out.mp4`
and any JPEG already on the system.

## 4. Code map

| File | Responsibility |
|---|---|
| `crates/bbcp-core/src/model.rs` | `FileRow`, `Action`, `MediaKind`, extension → type |
| `config.rs` | `bbcp.toml`, defaults, validation, example config |
| `store.rs` | SQLite schema, upserts, plan, groups, sidecars, relocation |
| `walk.rs` | Root traversal, junk filters, overlap checks |
| `hash.rs` | Streaming SHA-256 |
| `imagehash.rs` | pHash/dHash/aHash, MAD, video frame extraction |
| `video.rs` | `ffprobe` (rotation-aware resolution, duration, bitrate) |
| `group.rs` | Exact image and video groups |
| `dates.rs` | Sidecars, copy bridge, EXIF, filename, folder |
| `quality.rs` | Ranking and material-upgrade rule |
| `plan.rs` | Building `plan_actions` |
| `verify.rs` | MAD before deletion; `compare` |
| `apply.rs` | Atomic copies, quarantine, journal |
| `stage.rs` | Staging of unique files and inventory update |
| `google.rs` | Deletion candidates and batches |
| `report.rs` | Markdown report |
| `lib.rs` | Modules and `doctor` |
| `crates/bbcp-cli/src/main.rs` | Subcommands and console output |

Style rules:

* Rust 2021, `anyhow::Result` in the library, user-facing error messages in
  Spanish (the CLI prints them verbatim).
* No `unwrap()`/`expect()` outside tests and proven invariants (for example a
  non-empty group).
* Never assume user paths: always `strip_prefix` with a fallback.
* Schema changes go into `SCHEMA` with `CREATE TABLE IF NOT EXISTS`; if a
  column of an existing table changes, document the migration.

## 5. Workflow when changing the engine

1. Read `docs/decisions.md` to understand why the design is what it is.
2. Change the module and add a test in
   `crates/bbcp-core/tests/core.rs` (no ffmpeg) or a documented manual test.
3. Run `cargo fmt`, `clippy -D warnings` and `cargo test`.
4. Run the full pipeline in `/tmp` with synthetic images **before** touching a
   real library.
5. If the change affects safety (deletion, quarantine, journal, references),
   update `docs/safety.md` and `AGENTS.md`.

## 6. Workflow on a real library

1. **Preflight**: `./scripts/preflight.sh`, `bbcp doctor`, stop
   ownCloud/Syncthing, and confirm with the user that two backups exist.
2. **Inventory**: `scan`, `sidecars`, `probe`, `fingerprint`, `hash`, `group`,
   `dates`.
3. **Plan**: `plan`; review counts and bytes. If something looks wrong, fix
   the configuration and run `plan` again.
4. **Verification**: `verify`.
5. **Dry run**: `apply` and `apply --only ...` for each subset.
6. **Additive execution**: `apply --execute --only upgrade,import`.
7. **Quarantine**: `apply --execute --only delete --album-remove quarantine`.
8. **Stage review**: `stage`, review, `apply --execute --only import`.
9. **External roots**: archive the JSON sidecars, then
   `--only external_covered` and `--only external_dup` with
   `--external-remove quarantine` or `unlink`.
10. **Google**: `google`, review `google_delete_candidates.tsv`, delete in
    batches.
11. **Close**: `report`, rescan, check totals and free space.

Never run steps 6–9 without first showing the user the dry-run summary and
getting explicit confirmation.

**Backups are the user's call.** Ask about them, say plainly what is at risk
if there is only one copy, and point at [`DISCLAIMER.md`](DISCLAIMER.md). It is
documentation, not a gate: `bbcp` does not check backups and must not be made
to.

## 7. Known pitfalls

* **9p**: every `stat`/`rename` is slow. Parallelize and avoid per-file
  `fsync` (the journal syncs every 100 lines).
* **Degenerate hashes**: flat images produce hashes with few set bits.
  `min_popcount = 12` filters them out.
* **Bursts**: two frames can share the hash triple. MAD verification separates
  them; never skip it.
* **"Approx" videos**: same duration and resolution does not mean the same
  video (in one real library ~85% were different). Verify by frame.
* **Sentinel dates**: `1970-01-01` and `1980-01-01` are not dates.
* **Dump folders**: `2026/2026-01-05` is the import date; add it to
  `import_dump_days`.
* **Quarantine collisions**: two roots can share the same relative path. They
  are errors, not something to overwrite silently.

## 8. Documentation and skill

* `docs/playbook.md` — full operational flow with reference numbers.
* `docs/safety.md` — safety model and failure modes.
* `docs/decisions.md` — design decisions and rejected alternatives.
* `docs/google-takeout.md` — sidecars, dates and Google batches.
* `.opencode/skills/bye-bye-cloud-photos/SKILL.md` — canonical OpenCode skill.
  If you change the flow, update it in the same commit.

## 9. License and liability

GPL-3.0-only. When reusing code from another GPL project, keep the attribution
in `THIRD_PARTY.md`.

The software ships **as-is, without warranty**, and the authors accept **no
liability** for data loss. Every safety or documentation change must keep
[`DISCLAIMER.md`](DISCLAIMER.md) and the README warning true, including the
advice to keep at least two independent backups. Do not weaken the wording
without an explicit decision from the maintainer, and do not turn the advice
into a runtime restriction.
