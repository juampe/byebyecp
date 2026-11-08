---
name: bye-bye-cloud-photos
description: Consolidate a cloud photo and video library (Google Takeout, old dumps, local album) with `bbcp`: SQLite inventory, content-based deduplication, best-copy selection, pixel/frame verification, reversible quarantine, journals, and Google Photos deletion batches. Use it when freeing space or merging libraries without losing files.
---

# Bye Bye Cloud Photos — operating skill

Tool: `bbcp` (Rust). Repository: <https://github.com/juampe/byebyecp>.
All configuration lives in `bbcp.toml`; all state lives in `.bbcp/`.

This skill is self-sufficient. It does not require previous documents or
external scripts.

## When to use it

* Consolidate several Google Takeout exports into a single local album.
* Deduplicate a library while keeping the best copy.
* Free space reversibly (quarantine) before deleting in the cloud.
* Prepare Google Photos deletion batches with proof of a local copy.
* Investigate why two files are considered the same or different.

## Safety rules (read before running anything)

1. **Never delete without a verified copy.** The plan only proposes deletions
   when another member of the group survives; still, review the dry run.
2. **`apply` is a dry run without `--execute`.** Show the summary to the user
   and ask for confirmation before executing.
3. **Album deletions go to quarantine** (`--album-remove quarantine`, the
   default). Do not use `unlink` unless the user asks for it and space/backup
   has been verified.
4. **External sources are never purged by default.** `external_covered` and
   `external_dup` require an explicit `--only`.
5. **Before purging a Takeout root, archive its JSON files** (metadata and
   `googleId`).
6. **Stop when verification fails.** `keep_distinct` means "review manually";
   do not force it.
7. **Do not touch documents, ROMs or anything that is not image/video.**
8. **The database must be on a local filesystem.** Never on 9p/NFS/FUSE.
9. **Stop file sync** (ownCloud, Syncthing, Drive) while moving album files.

## 0. Preflight

```bash
bbcp doctor
df -h /path/to/Album      # free space where the album lives
df -h .                   # free space where .bbcp/ lives
```

`doctor` checks `ffmpeg`, `ffprobe`, SQLite, album permissions and that every
root exists. If ffmpeg/ffprobe are missing, install them before continuing.

Minimal configuration (`bbcp.toml`):

```toml
album = "/path/Album"
metadata_archive = ".bbcp/metadata-archive"

[paths]
db = ".bbcp/bbcp.db"
quarantine = ".bbcp/quarantine"
staging = ".bbcp/staging"
logs = ".bbcp/logs"
manifests = ".bbcp/manifests"

[[roots]]
label = "album"
path = "/path/Album"
kind = "album"

[[roots]]
label = "takeout-2026"
path = "/path/Takeout/Takeout/Google Fotos"
kind = "source"

[rules]
min_popcount = 12
aspect_tolerance = 1.05
mad_threshold = 1.5
material_upgrade_ratio = 1.25
frame_distance = 16
duration_tolerance = 0.5
ignore_dates = ["1970-01-01", "1980-01-01"]
import_dump_days = []   # e.g. ["2026-01-05"] if Takeout dumped everything there
```

Register new roots:

```bash
bbcp scan --root takeout-2026
```

`scan` automatically syncs the roots in the TOML with the database.

## 1. Inventory and fingerprints

```bash
bbcp scan                 # all roots; --prune removes disappeared rows
bbcp sidecars             # Takeout JSON -> sidecars table
bbcp probe --workers 16   # video metadata (ffprobe)
bbcp fingerprint --workers 16
bbcp hash --workers 16    # SHA-256 only for videos with a repeated size
bbcp group --reset
bbcp dates
```

Order matters: `group` **before** `dates`. This lets a copy without metadata
inherit the date from its exact twin (`sidecar-copy` bridge).

Useful checks:

```bash
bbcp status
sqlite3 .bbcp/bbcp.db "SELECT media, COUNT(*) FROM files GROUP BY media;"
```

## 2. Plan and verification

```bash
bbcp plan
bbcp verify --workers 8
```

Possible actions:

| Action | Meaning |
|---|---|
| `keep` | Stays where it is |
| `keep_distinct` | Looked like a duplicate but verification separated it: review |
| `delete` | Album duplicate → quarantine |
| `upgrade` | Better copy in a source → replaces the album copy |
| `import` | Unique source media → copy into the album |
| `external_covered` | Duplicate of a protected album copy → purge the source |
| `external_dup` | Duplicate inside a source → purge after importing the best |

Review the summary (counts and bytes). If something looks wrong, fix the
configuration and run `plan` again; nothing has touched the disk yet.

## 3. Execution

```bash
bbcp apply                                    # dry run
bbcp apply --execute --only upgrade,import    # additive and verified
bbcp apply --execute --only delete --album-remove quarantine
```

What `apply` does:

* copies to `.bbcp-tmp-*` in the destination directory, `fsync`, compares
  SHA-256 and `rename`s atomically;
* moves the old target of an `upgrade` to quarantine;
* moves album duplicates to `.bbcp/quarantine/<date>/album/<path>`;
* writes `.bbcp/logs/apply-<date>.tsv` (resumable);
* updates the inventory: imported rows point at their album path, and rows
  moved to quarantine or unlinked are dropped.

`--consume-sources` removes the source after a verified copy. Use it only when
the album is on another disk and the user approves.

If any operation fails, the command exits non-zero and the journal contains the
cause. Fix it and run again: the operation is idempotent.

## 4. Staging for review

```bash
bbcp stage --root takeout-2026           # dry run
bbcp stage --root takeout-2026 --execute
bbcp stage --list
```

Moves `import` files to `.bbcp/staging/<date>/<root>/<path>` and updates the
database. Then:

```bash
bbcp apply --execute --only import
```

Review the staging tree first if there are doubts about names or duplicates
between source roots.

## 5. Cleaning external sources

Only after the album has verified copies and the user approves:

1. Archive the JSON files of the root:

   ```bash
   OUT=".bbcp/metadata-archive/takeout-2026.tar.gz"
   find "$ROOT" -type f -name '*.json' -printf '%P\0' \
     | tar -C "$ROOT" --null -T - -czf "$OUT.part"
   gzip -t "$OUT.part" && mv "$OUT.part" "$OUT"
   ```

2. Purge into quarantine (recommended):

   ```bash
   bbcp apply --execute --only external_covered --external-remove quarantine
   bbcp apply --execute --only external_dup --external-remove quarantine
   ```

3. Only if you really need to free space and have a backup:
   `--external-remove unlink`. This is irreversible.

## 6. Google Photos

```bash
bbcp plan      # mandatory: google uses the plan to know what survives
bbcp google
```

Outputs under `.bbcp/manifests/<date>/`:

* `google_delete_candidates.tsv` — columns `google_id`, `title`, `taken`,
  `bytes`, `takeout_path`, `album_ref`, `match`.
* `google_batches/batch_NNN.tsv` — the same items in chunks of
  `google.batch_size`.

An item is a candidate only when its sidecar is matched to a local file, that
file belongs to an exact group with an album copy that survives the plan, and
that album path exists. Items without a local copy are counted as `kept`.

Never delete a Google item that is not in a batch.

Strategy: check the account's real usage, start with videos or the heaviest
items, and delete in batches, checking free space after each one.

## 7. Report and close

```bash
bbcp report
```

Writes `.bbcp/manifests/<date>/report.md` with the inventory, plan by type,
groups, known dates and verification counts. Keep the journals too.

At the end:

```bash
bbcp scan --prune
bbcp status
du -sh /path/Album .bbcp/quarantine
```

## 8. Rollback

Every journal TSV has `source`, `target`, `backup`, `status`.

* Album duplicates:
  `mv .bbcp/quarantine/<date>/album/<path> /path/Album/<path>`
* Upgrades: the old copy is in
  `.bbcp/quarantine/<date>/upgrade/album/<path>`.
* Sources: `.bbcp/quarantine/<date>/source/<root>/<path>`.
* Staging: `.bbcp/staging/<date>/<root>/<path>`; move the file back and
  rescan (`bbcp scan --root staging-<date>`) or run a full rescan.

## 9. Troubleshooting

| Symptom | Likely cause | Action |
|---|---|---|
| Dates are not filled | `group` did not run before `dates` | `bbcp group && bbcp dates` |
| Huge image group | degenerate hashes | there is no transitive closure; raise `min_popcount` |
| Many `keep_distinct` | bursts or recompressed copies | inspect MAD with `bbcp compare A B` |
| Videos not grouped | sizes differ or `hash` missing | `bbcp hash --all` |
| `apply` says "reference is not present" | stale plan or deleted reference | `bbcp scan --prune && bbcp plan` |
| Quarantine collision | two roots share a relative path | inspect and resolve manually |
| Google has no candidates | unmatched sidecars or plan not built | `bbcp sidecars && bbcp plan` |
| Everything is very slow | database or quarantine on 9p/NFS | move `.bbcp/` to a local disk |

## 10. Final checklist

- [ ] `bbcp doctor` is clean.
- [ ] `scan`, `sidecars`, `probe`, `fingerprint`, `hash`, `group`, `dates`.
- [ ] `plan` reviewed (counts and bytes).
- [ ] `verify` run; `keep_distinct` reviewed.
- [ ] Dry-run `apply` shown to the user.
- [ ] `upgrade,import` executed and verified.
- [ ] Duplicates quarantined.
- [ ] Staging reviewed and imported.
- [ ] JSON archived before purging sources.
- [ ] Google batches reviewed; deletion done in batches.
- [ ] `report` generated and journals kept.
- [ ] Final rescan and free-space check.
