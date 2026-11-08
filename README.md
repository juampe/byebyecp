<p align="center">
  <img src="assets/icon.png" alt="Bye Bye Cloud Photos icon" width="144">
</p>

<h1 align="center">Bye Bye Cloud Photos</h1>

<p align="center"><strong>Consolidate a cloud photo library, keep the best copy of everything, and leave Google Photos without losing a picture.</strong></p>

`bbcp` is a Rust toolkit for people who have their photos spread across Google
Takeout exports, old hard drives, phone dumps and a half-organised album. It
builds a SQLite inventory, detects duplicates by content, verifies them, and
then applies a **reversible, journaled** plan.

The project was extracted from a real cleanup of a 400 GB library (≈317k
images, ≈17k videos) and keeps the rules that made that run safe.

> [!WARNING]
> **Your data, your risk.** `bbcp` moves, quarantines and can delete files. It
> is provided **as-is, without warranty, and the authors accept zero liability**
> for lost, corrupted or deleted data — including anything you delete in Google
> Photos. Before any destructive run you must have **at least two independent
> backups** on separate media; `bbcp apply --execute` refuses to run without
> them. Quarantine is not a backup. Read
> [`DISCLAIMER.md`](DISCLAIMER.md) before using this on a real library.

* **Content-based deduplication** — perceptual hashes for images, SHA-256 for
  videos, pixel and frame verification before anything is removed.
* **Best copy wins** — resolution, then bitrate/size, then age. The album is
  not assumed to be the best copy.
* **Nothing disappears** — duplicates move to a quarantine tree; source
  deletions require an explicit `--only` flag; every operation is journaled.
* **Metadata survives** — Google Takeout sidecars are parsed, matched and
  archived before a Takeout folder is purged.
* **Dates that make sense** — sidecar → exact-copy bridge → EXIF → filename →
  folder, with sentinel dates and bulk-import folders excluded.
* **Agent-ready** — [`AGENTS.md`](AGENTS.md) and a
  [skill](.opencode/skills/bye-bye-cloud-photos/SKILL.md) describe the whole
  procedure so an assistant can run it without inventing steps.

> For the complete operational procedure, see [`docs/playbook.md`](docs/playbook.md).

## Guarantees

| Invariant | How it is enforced |
|---|---|
| No file is removed without another verified copy | `plan` only emits `delete`/`external_*` when an exact group has a surviving member; `apply` re-checks that the reference exists |
| Every destructive action is reversible | Album duplicates are renamed into `.bbcp/quarantine/<date>/…`; source purges default to quarantine too |
| Writes are atomic and verified | Copy to `.bbcp-tmp-*`, `fsync`, SHA-256 compare, `rename` |
| Runs are resumable | TSV journals in `.bbcp/logs`; the SQLite plan is the source of truth |
| Nothing is deleted by accident | `apply` is a dry run unless `--execute`; `external_covered`/`external_dup` need their own `--only` |
| Different content is kept | Pixel MAD for images, frame distance for videos, aspect-ratio and popcount gates |
| Destructive runs need real backups | `apply --execute` aborts unless `[safety] backups` lists two existing locations outside the album and on another filesystem |

## Install

Requirements: Rust ≥ 1.88, `ffmpeg` and `ffprobe` on `PATH`.

```bash
git clone https://github.com/juampe/byebyecp
cd byebyecp
cargo build --release
./target/release/bbcp --help
```

Or:

```bash
./scripts/install.sh          # builds and installs to ~/.local/bin
```

## Quickstart

```bash
mkdir -p ~/photos/Album
bbcp init --album ~/photos/Album
$EDITOR bbcp.toml             # add one [[roots]] block per Takeout export
bbcp doctor
```

Before the first destructive run, declare two independent backups in
`[safety]` (they must already exist, outside the album and on another disk):

```toml
[safety]
backups = ["/mnt/backup-1/Album", "/mnt/backup-2/Album"]
```

`apply --execute` will refuse to run until `doctor` reports them as usable. If
you truly have no backups, `allow_without_backups = true` bypasses the check —
and you accept the risk described in [`DISCLAIMER.md`](DISCLAIMER.md).

Then run the pipeline. Every stage is idempotent; run it again after adding a
root.

```bash
bbcp scan                     # inventory every root
bbcp sidecars                 # parse Google Takeout JSON files
bbcp probe                    # ffprobe metadata for videos
bbcp fingerprint              # perceptual hashes for images
bbcp hash                     # SHA-256 for videos that share a size
bbcp group                    # exact duplicate groups
bbcp dates                    # fill capture dates
bbcp plan                     # decide keep / delete / upgrade / import
bbcp verify                   # pixel MAD before deleting images
bbcp apply                    # dry run
bbcp apply --execute          # do it (imports, upgrades, quarantine)
bbcp report                   # Markdown summary
```

Order matters: run `group` before `dates` so a copy without metadata can
inherit the date from its exact twin.

Review the dry run before `--execute`. If the numbers look wrong, fix the
configuration and re-run `plan`; nothing has touched the disk yet.

### Typical output

```text
$ bbcp plan
keep                  5  0 B
keep_distinct         0
delete                1  46.67 KB
upgrade               0  0 B
import                2  68.01 KB
external              3  98.46 KB
```

## Safety model

```text
source roots ──scan──▶ SQLite inventory ──fingerprint/hash──▶ groups
                                                              │
                                                     plan ────┤
                                                              ▼
                                   verify (MAD) ──▶ apply ──▶ album
                                                      │
                                                      ├─▶ quarantine (reversible)
                                                      └─▶ journal (resumable)
```

* `apply` handles `upgrade`, `import` and `delete` by default.
* `external_covered` (source copy of a protected album file) and
  `external_dup` (duplicate inside a source root) are **never** in the default
  set: add them explicitly, e.g.
  `bbcp apply --execute --only external_covered --external-remove quarantine`.
* `--consume-sources` removes the source after a verified import/upgrade. Only
  use it when the album is on a different, trusted disk.
* `--album-remove unlink` frees space immediately but is irreversible. The
  default is `quarantine`.

See [`docs/safety.md`](docs/safety.md) for the failure modes this design
addresses.

## Google Photos

`bbcp google` writes deletion candidates that have a verified local copy:

```text
.bbcp/manifests/<date>/
├── google_delete_candidates.tsv
└── google_batches/
    ├── batch_001.tsv
    └── batch_002.tsv
```

Each row carries `google_id`, title, capture date, size, the Takeout path, the
album reference that proves the copy exists, and the match kind. Items without
a surviving album copy are counted as `kept` and never written to a batch.

The tool does not talk to Google. A human or a browser-automation agent deletes
the batches from Google Photos; see
[`docs/google-takeout.md`](docs/google-takeout.md).

## Commands

| Command | Purpose |
|---|---|
| `init` | Write a starter `bbcp.toml` and create the database |
| `doctor` | Check ffmpeg/ffprobe, album permissions, roots and backup locations |
| `scan` | Inventory roots (optionally `--prune`) |
| `sidecars` | Parse Takeout JSON and match it to media |
| `probe` | ffprobe metadata for videos |
| `fingerprint` | pHash/dHash/aHash for images |
| `hash` | SHA-256 for video size-collision candidates (`--all` for every video) |
| `group` | Build exact groups (`--reset` to rebuild) |
| `dates` | Fill `capture_ts` from sidecars/copies/EXIF/names/folders |
| `plan` | Build `plan_actions` |
| `verify` | MAD check for image deletes/upgrades |
| `apply` | Dry run, or `--execute` the plan (refused without two backups) |
| `stage` | Move unique imports to `.bbcp/staging/<date>` for review |
| `google` | Write Google deletion candidates and batches |
| `report` | Markdown report in `.bbcp/manifests/<date>` |
| `compare A B` | Print the MAD between two images |
| `status` | Quick totals from the database |

All commands accept `--config <file>` (default `bbcp.toml`) and `--db <file>`.

## Layout

```text
crates/bbcp-core   engine: walk, hashes, groups, plan, verify, apply, report
crates/bbcp-cli    `bbcp` command-line interface
docs/              playbook, safety model, decisions, Takeout notes
assets/            icon and social preview
.opencode/skills/  canonical agent skill
AGENTS.md          instructions for coding agents working on the repo
DISCLAIMER.md      warranty, liability and the two-backup requirement
```

## Disclaimer

This software is provided **as-is**, without warranty of any kind, and the
authors are **not liable** for data loss, deletions, corruptions, costs or any
other damage resulting from its use. Running `bbcp` requires **at least two
independent backups** of the data you touch; the tool enforces this before
`apply --execute`. Deleting items in Google Photos or any other service is done
by you, outside this tool, and is your responsibility.

The complete terms are in [`DISCLAIMER.md`](DISCLAIMER.md) and in the GNU GPL
sections 15–16 ([`LICENSE`](LICENSE)).

## License

GPL-3.0-only. See [`LICENSE`](LICENSE) and [`THIRD_PARTY.md`](THIRD_PARTY.md).
