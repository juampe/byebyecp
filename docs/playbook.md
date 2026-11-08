# Operational playbook

Complete guide to consolidating a real library with `bbcp`. It is written so a
person or an agent can follow it without prior knowledge of the project.

If you only want to try the tool, start with the
[README](../README.md#quickstart). This page documents a serious run.

## 0. Goal and principles

Consolidate every photo and video scattered across Google Photos exports, old
dumps and recycle bins into a single local album, keeping the **best copy** of
each piece of content, and prepare batched deletion in Google Photos without
losing anything.

Principles:

1. **Nothing is deleted without a verified copy.**
2. **Everything is reversible.** Deletions move files to quarantine on the same
   filesystem (atomic rename) and are recorded in a journal.
3. **Do not assume the album is the best copy.** Higher-resolution versions may
   exist in temporary folders.
4. **Record before acting.** Every phase leaves TSV manifests and journals.
5. **Stop sync** (ownCloud, Syncthing, Drive) while moving album files.
6. **The database lives on a local filesystem**, never on 9p, NFS or FUSE.

## 1. Environment

Example layout (adapt the paths):

| Item | Path |
|---|---|
| Album | `/mnt/b/ownCloud/Album` |
| New Takeout | `/mnt/b/GPhotos/Takeout/Takeout/Google Fotos` |
| Quarantine | `/mnt/b/PMM/quarantine` |
| Database | `/home/juampe/.local/share/bbcp/bbcp.db` |
| Logs / journals | `/mnt/b/PMM/logs` |
| Manifests | `/mnt/b/PMM/manifests/<YYYY-MM-DD>` |
| Metadata archive | `/mnt/b/PMM/metadata-archive` |
| Review staging | `/mnt/b/PMM/staging-revision/<YYYY-MM-DD>` |
| Configuration | `/home/juampe/Develop/byebyecp/bbcp.toml` |
| Backup 1 (off-site) | `/mnt/backup-1/Album` |
| Backup 2 (other disk) | `/mnt/backup-2/Album` |

The two backups are declared in `[safety] backups` and verified by
`bbcp doctor`. Without them, `apply --execute` refuses to run.

Dependencies: `ffmpeg`, `ffprobe` and a local database. Nothing else.

`/mnt/b` may be 9p: every metadata operation has high latency. Therefore:

* the database lives on ext4;
* hashes and fingerprints are parallelized;
* the journal syncs every 100 lines, not per file;
* renames and deletions run in parallel.

## 2. Architecture

### 2.1 Database

`.bbcp/bbcp.db` (SQLite, WAL) contains:

* **roots** — inventoried roots (`album`, `takeout-2026`, `old-staging`,
  `album-dtrash`, `todo`, `sync-delete`, `gdrive-delete`…).
* **files** — one row per file: `path` (unique), `rel_path`, `media`
  (`image`/`video`/`other`), `size`, `mtime`, `width`, `height`, `duration`,
  `bitrate`, `codec`, `sha256`, `phash`, `dhash`, `ahash`, `capture_ts`,
  `capture_source`, `fingerprint_status`, `meta_status`.
* **sidecars** — Takeout JSON: `google_id`, `title`, `taken_ts`,
  `creation_ts`, `description`, `matched_file_id`.
* **groups** / **group_members** — exact groups; `kind` =
  `image-exact`/`video-exact`; `role` = `best`/`duplicate`.
* **plan_actions** — action per file: `keep`, `keep_distinct`, `delete`,
  `upgrade`, `import`, `external_covered`, `external_dup`, with `reference_id`,
  `target_path`, `bytes`, `mad`, `reason`.

### 2.2 Modules

| Module | Purpose |
|---|---|
| `walk` | root inventory |
| `dates` | sidecars, copy bridge, EXIF, filename, folder |
| `video` | ffprobe (rotation-aware resolution, duration, bitrate) |
| `imagehash` | pHash/dHash/aHash, MAD, video frame |
| `hash` | streaming SHA-256 |
| `group` | exact groups |
| `quality` | ranking and material upgrade |
| `plan` | `plan_actions` |
| `verify` | MAD before deleting images |
| `apply` | atomic copies, quarantine, journal |
| `stage` | staging of unique files |
| `google` | Google candidates and batches |
| `report` | Markdown report |

## 3. Decision rules

### 3.1 Images: perceptual exact duplicate

Key: the `(pHash, dHash, aHash)` triple, 64 bits each.

Mandatory guards:

* `popcount` of **each** hash ≥ 12. This discards flat, black or screenshot-like
  images that collide.
* **Aspect-ratio** variation inside the group ≤ 5%. Otherwise the group is
  discarded.
* **No transitive closure.** Groups are not chained: a burst could otherwise
  end up with hundreds of different photos in one group.
* Final pixel verification (MAD) before deleting.

**MAD** = mean absolute difference of two 64×64 grayscale thumbnails.

| MAD | Interpretation | Action |
|---|---|---|
| 0 – 0.5 | identical copies | delete the worse one |
| 0.5 – 1.2 | recompressed copies | delete the worse one |
| ≥ 2.7 | different burst frames | keep both |

Default threshold: `mad_threshold = 1.5`. Above it, the action becomes
`keep_distinct`.

### 3.2 Videos: exact duplicate and approx

* **Exact**: same SHA-256. Only videos that share a size with another video are
  hashed (saves reading hundreds of GB).
* **Approx** (same content, different encoding): same resolution and
  `|Δduration| ≤ 0.5 s`, plus frame verification: extract a frame at the
  midpoint and compare `pHash + dHash`.
  * distance ≤ 16 → same video;
  * distance > 16 → different videos (do not delete).

Do not trust duration + resolution alone: in one real library, ~85% of
"approx" pairs were different videos.

The current implementation groups videos by SHA-256. Frame comparison
(`frame_distance`) remains a diagnostic tool (`bbcp compare` for images; frame
extraction lives in `imagehash::video_frame`) and the basis for future
extensions.

### 3.3 Quality ranking

Order: `(pixels, bitrate or size, age, size)`.

* Pixels come first: a 12 MP copy beats a 2 MP copy.
* At equal resolution, higher bitrate (video) or more bytes (less compressed
  image) wins.
* Otherwise the older file wins (derivatives are written later).
* File size is the final tie-breaker.

**Material upgrade** (to replace a copy): more resolution, or the same
resolution with bitrate/size > 1.25×. A 2% difference is not an upgrade.

### 3.4 Dates

Source order:

1. the file's own `sidecar` (`photoTakenTime`).
2. the `sidecar` of an **exact copy** in another root (metadata bridge).
3. EXIF `DateTimeOriginal` (or digitized/created).
4. file name (`2021-08-07 08.18.41`, `IMG_20210807_081841`, …).
5. folder `YYYY/YYYY-MM-DD`.

Exclusions:

* sentinels `1970-01-01` and `1980-01-01`.
* bulk-import folders configured in `import_dump_days`: the name/folder date
  is the **import** date, not the capture date.

## 4. Full pipeline

### Phase 0 — Preflight

```bash
./scripts/preflight.sh
bbcp doctor
# two independent backups configured and present
# ownCloud/Syncthing/Drive stopped
df -h /mnt/b | tail -1
sqlite3 .bbcp/bbcp.db "PRAGMA journal_mode;"   # wal
```

### Phase 1 — Inventory

```bash
bbcp scan
# or per root:
bbcp scan --root album
bbcp scan --root takeout-2026
```

### Phase 2 — Google sidecars

```bash
bbcp sidecars
```

Matches `x.jpg.json` and `x.jpg.supplemental-metadata.json` to `x.jpg`,
including names truncated by older Takeout exports (unique prefix match).

### Phase 3 — Metadata

```bash
bbcp probe --workers 16
```

Videos only. Images get their dimensions from `fingerprint`.

### Phase 4 — Image fingerprints

```bash
bbcp fingerprint --workers 16
```

### Phase 5 — SHA-256 for candidate videos

```bash
bbcp hash --workers 16
# every video:
bbcp hash --all --workers 16
```

Only videos that share a size with another video are hashed. Unique sizes
cannot be confirmed by SHA-256.

### Phase 6 — Group duplicates

```bash
bbcp group --reset
```

### Phase 7 — Dates

```bash
bbcp dates
```

Must run **after** `group`.

### Phase 8 — Plan

```bash
bbcp plan
```

Produces `keep`, `keep_distinct`, `delete`, `upgrade`, `import`,
`external_covered`, `external_dup` in the database. It does not touch the disk.

### Phase 9 — Verification

```bash
bbcp verify --workers 8
```

Turns deletions whose MAD exceeds the threshold (or whose images cannot be
decoded) into `keep_distinct`.

### Phase 10 — Upgrade and import (additive)

```bash
bbcp apply                                      # dry run
bbcp apply --execute --only upgrade,import
```

* Atomic copy: temp file in the same directory, `fsync`, `os.replace`.
* SHA-256 of source and destination is verified.
* The old target file moves to `quarantine/<date>/upgrade/album/`.
* Resumable journal in `.bbcp/logs/apply-<date>.tsv`.
* The database is updated: an `import` row points at its album path, and a row
  moved to quarantine or unlinked disappears from the inventory.

### Phase 11 — Delete duplicates (to quarantine)

```bash
bbcp apply --execute --only delete --album-remove quarantine --workers 32
```

Parallel renames to `quarantine/<date>/album/<relative path>`. Nothing is
unlinked.

### Phase 12 — Report

```bash
bbcp report
```

### Phase 13 — Clean external roots

Before deleting media, **archive the JSON metadata**:

```bash
mkdir -p .bbcp/metadata-archive
ROOT=/path/Takeout/Google\ Fotos
find "$ROOT" -type f -name '*.json' -printf '%P\0' \
  | tar -C "$ROOT" --null -T - -czf ".bbcp/metadata-archive/takeout.tar.gz.part"
gzip -t .bbcp/metadata-archive/takeout.tar.gz.part \
  && mv .bbcp/metadata-archive/takeout.tar.gz.part \
        .bbcp/metadata-archive/takeout.tar.gz
```

Then purge only `external_covered` items (checking that the album copy exists):

```bash
bbcp apply --only external_covered --external-remove quarantine
bbcp apply --execute --only external_covered --external-remove quarantine
```

And `external_dup` items:

```bash
bbcp apply --execute --only external_dup --external-remove quarantine
```

`unlink` frees space immediately but is irreversible. Use it only with a backup
and explicit confirmation.

### Phase 14 — digiKam trash (`.dtrash`)

1. Add `.dtrash/files` as a `source` root (label `album-dtrash`).
2. `bbcp scan --root album-dtrash`.
3. `bbcp fingerprint`, `bbcp hash`, `bbcp group --reset`, `bbcp plan`.
4. Delete **only** duplicates that have an exact copy in the album:
   `bbcp apply --execute --only external_covered --external-remove quarantine`.
5. Keep the rest and review manually.

### Phase 15 — Extra roots (TODO, Sync-DELETE, Drive-DELETE…)

Same pipeline (`scan → probe → fingerprint → hash → group → dates → plan`),
then:

```bash
bbcp plan
# categories:
#  album_dup    -> external_covered (delete)
#  album_better -> upgrade (import before deleting)
#  root_dup     -> external_dup (keep the best)
#  unique       -> import / stage
#  other        -> do not touch
bbcp apply --execute --only upgrade,import
bbcp stage --execute
bbcp apply --execute --only import
bbcp apply --execute --only external_covered,external_dup --external-remove quarantine
```

### Phase 16 — Google Photos batches

```bash
bbcp plan                 # google uses the plan
bbcp google
```

Outputs:

* `google_delete_candidates.tsv` — items with a safe copy in the album.
* `google_batches/batch_NNN.tsv` — chunks of `google.batch_size`.

Strategy to go from ~148 GB to ~50 GB: check the account's real usage, start
with videos and the heaviest items, and delete in batches, checking free space
after each one.

### Phase 17 — Review staging

```bash
bbcp stage                       # dry run
bbcp stage --execute             # move to .bbcp/staging/<date>
bbcp stage --list
bbcp apply --execute --only import
```

Staging exposes duplicates **between** roots: the same photo may be in TODO and
in Sync-DELETE. `group` already merges them; staging only moves `import` rows.

### Phase 18 — Final rescan

```bash
bbcp scan --prune
bbcp status
bbcp report
du -sh /path/Album .bbcp/quarantine
```

## 5. Known pitfalls

### 9p / performance

* SQLite on 9p is extremely slow: keep the database on ext4.
* Per-file `fsync` kills throughput: batch the journal (every 100 lines) and
  parallelize renames/deletes.
* `find | tar` over thousands of small JSON files is slow; write to `.part` and
  rename only after `gzip -t`.

### Fingerprints

* Degenerate hashes (all zeros / one bit) cause massive collisions. Filter with
  `popcount ≥ 12`.
* Transitive closure merges bursts and different photos. **Never** use it.
* Resolution differences between copies of the same photo are normal; the
  aspect ratio must match.

### SQLite

* A connection cannot be shared between threads: resolve paths/IDs on the main
  thread before parallelizing.
* After moving or deleting files, the database can become stale: rescan or use
  the journal as the source of truth.

### Videos

* Same size does not imply same content: confirm with SHA-256.
* Same duration + resolution does **not** imply same video: verify by frame.
* `.nsp`, ROMs and documents are not photos/videos: do not touch them.

### Dates

* `1970-01-01` and `1980-01-01` are sentinels.
* The folder `2026/2026-01-05` is an import dump.
* The sidecar bridge through exact copies recovers thousands of lost dates.

### Operations

* Deduplicate **by destination** in upgrades: two sources may point at the same
  album file and overwrite each other. `apply` aborts if the destination exists
  with different content.
* Verify SHA-256 on every copy; never trust size alone.
* Move to quarantine, do not delete, until final verification.
* `du -sb` includes `.dtrash`, `Thumbs.db` and `.tmp`; database totals may not
  match.

## 6. Reference numbers (one real run)

| Concept | Value |
|---|---|
| Images inventoried | ~317,000 |
| Videos | ~16,700 |
| Initial album | 389.9 GB (138,490 files) |
| Album duplicates quarantined | 42,729 (65.2 GB) |
| Images confirmed by MAD ≤ 1.5 | 41,327 (22.98 GB) |
| Kept because MAD > 1.5 | 1,039 |
| Quality replacements | 311 |
| Imports into the album | 3,425 |
| External items covered | 260.34 GB |
| `.dtrash` confirmed | 31,462 (22.22 GB) |
| Extra roots processed | 339 GB |
| Extra duplicates deleted | 130,994 (275.96 GB) |
| Unique staging | 17,783 (45.69 GB) |
| Safe Google candidates | 86,485 (125.71 GB) |
| Google heavy batch | 9,930 (100 GB) |
| Google videos only | 2,386 (83.60 GB) |

These are references, not guarantees: numbers depend on each library.

## 7. Rollback

Every journal has `source`, `target`, `backup`, `status` columns.

* Album duplicates: `quarantine/<date>/album/<relative path>`.
* Upgrades: `quarantine/<date>/upgrade/album/<relative path>`.
* Sources: `quarantine/<date>/source/<root>/<relative path>`.
* Staging: move back from `.bbcp/staging/<date>/<root>/<path>` and rescan.

```bash
mv ".bbcp/quarantine/2026-11-08/album/2024/2024-05-01/x.jpg" \
   "/mnt/b/ownCloud/Album/2024/2024-05-01/x.jpg"
```

## 8. Safety rules for the agent

0. Two independent backups exist and `bbcp doctor` reports them as usable;
   otherwise stop before any destructive step and show `DISCLAIMER.md`.
1. Before deleting, check that the album reference exists.
2. Do not delete `external_*` or `keep` without explicit authorization.
3. Do not delete Google items without a verified `album_ref`.
4. Do not touch documents or ROMs.
5. Do not restart sync until the user asks.
6. When in doubt, keep the file and mark it for review.
7. Always write a journal and manifest before executing.
8. Simulate and review the summary before executing.
9. Never enable `allow_without_backups` on the user's behalf.

`bbcp` is provided as-is and the authors accept no liability for data loss. The
operator owns the decision to delete; the agent owns reading the dry run
correctly. See [`../DISCLAIMER.md`](../DISCLAIMER.md).

## 9. Quick checklist

- [ ] Sync stopped.
- [ ] Database on ext4 and in WAL mode.
- [ ] Roots registered and scanned.
- [ ] Sidecars parsed.
- [ ] Videos probed; images fingerprinted.
- [ ] SHA-256 for candidate videos.
- [ ] Exact groups (no transitivity, with guards).
- [ ] Dates filled (no sentinels).
- [ ] Plan generated and reviewed.
- [ ] MAD verified.
- [ ] Upgrades/imports executed with SHA-256.
- [ ] Duplicates in quarantine.
- [ ] JSON archived before deleting externals.
- [ ] `.dtrash` verified by content.
- [ ] Extra roots classified; better copies imported; uniques staged.
- [ ] Google batches generated and reviewed.
- [ ] Report and journals saved.
- [ ] Final rescan.

## 10. Glossary

* **pHash/dHash/aHash** — 64-bit perceptual hashes; robust to recompression
  and resizing.
* **MAD** — mean absolute difference between thumbnails; measures whether two
  images are the same content.
* **Approx** — video with the same duration and resolution but a different
  SHA-256; requires frame verification.
* **Staging** — intermediate folder with unique files, reviewed before they are
  imported into the album.
* **Quarantine** — reversible destination for deletions; same filesystem.
* **Journal** — TSV record of every operation, with status and paths; allows
  resuming.
* **Manifest** — TSV listing of the plan (what is kept, deleted, imported or
  upgraded).
