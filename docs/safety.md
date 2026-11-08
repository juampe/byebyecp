# Safety model

`bbcp` exists because deleting photos is irreversible in the cloud and
expensive to get wrong locally. This page describes the guarantees, the
verification layers and the failure modes the design defends against.

## 1. Threat model

| Risk | Where it comes from | Mitigation |
|---|---|---|
| Two different photos are treated as duplicates | burst frames, flat images, screenshot pairs | triple hash + popcount gate + aspect gate + pixel MAD |
| A video is deleted because it "looks" the same | same duration/resolution, different content | SHA-256 for exact matches; frame comparison for re-encodes; duplicates kept unless verified |
| A copy is deleted before the replacement is written | crash or ordering bug | additive phase first, quarantine second, journal last |
| The album copy is worse than a source copy | import/export history | quality ranking; material-upgrade threshold |
| Metadata is lost when a Takeout folder is purged | sidecars live next to the media | sidecars parsed and archived before purge |
| A crash leaves half-copied files | power loss, full disk | temp file in the destination directory, `fsync`, SHA-256, atomic `rename` |
| The plan becomes stale | files moved by other tools | reference existence re-checked at apply time; failures abort |
| The same file is inventoried twice | nested roots | `check_overlap` rejects nested roots |
| SQLite corrupts or stalls | DB on 9p/NFS | documented requirement: local filesystem, WAL |

Explicit non-goals: defending against a malicious user with write access to the
inventory, encrypting the quarantine, or syncing with the cloud provider's API.

## 2. Layers of verification

Before an image is marked `delete` or `upgrade`:

1. **Exact key** — pHash, dHash and aHash all equal.
2. **Popcount** — each hash has at least `min_popcount` set bits.
3. **Aspect ratio** — max/min inside the group ≤ `aspect_tolerance`.
4. **MAD** — the two files are decoded and compared at 64×64. Above
   `mad_threshold` the action becomes `keep_distinct`.
5. **Reference survival** — at apply time the surviving copy must still exist.

Before a video is marked as an exact duplicate:

1. **SHA-256** of the full file.
2. **Reference survival** at apply time.

Re-encoded videos are compared by an extracted frame (`pHash + dHash`, distance
≤ `frame_distance`). A distance above the threshold means different videos and
nothing is removed.

## 3. Irreversible operations

Only two commands can destroy data:

* `bbcp apply --album-remove unlink` — unlinks album duplicates instead of
  moving them.
* `bbcp apply --external-remove unlink` — unlinks source duplicates.

Both are opt-in and excluded from the default flags. Everything else moves
files into `.bbcp/quarantine/<date>/…`, which is reversible with a `mv`.

The quarantine lives on the same filesystem as the album, so moving a file into
it is an atomic rename. If the quarantine is on another filesystem, `bbcp` falls
back to copy + verify + remove.

## 4. Journal and resume

Every executed operation appends one line to
`.bbcp/logs/apply-<YYYY-MM-DD>.tsv`:

```text
ts  action  source  target  backup  bytes  status  error
```

* `status` is `imported`, `upgraded`, `already-imported`, `quarantined`,
  `purged`, `missing`, `missing-reference` or `error`.
* The file is flushed and `fsync`ed every 100 lines and at the end.
* Re-running `apply` is safe: an `import` whose target already exists is only
  accepted when the SHA-256 matches; otherwise the operation fails loudly.
* The inventory is updated as operations succeed: an imported row is moved to
  its album path (so it is not imported twice and later stages see the real
  file), and a row whose file was quarantined or unlinked is dropped, with its
  sidecar detached rather than deleted.

`stage` writes `.bbcp/logs/stage-<date>.tsv` with `file_id` and the new path,
mirroring the database update.

## 5. Failure recovery

| Situation | What `bbcp` does | What to do |
|---|---|---|
| Source disappeared between plan and apply | reports `missing`, writes journal, continues | rescan and re-plan |
| Upgrade target disappeared | aborts that operation with an error | rescan and re-plan |
| Target exists with different content | aborts, never overwrites | inspect both files; the plan is stale |
| Quarantine collision | aborts that operation | two roots share a relative path; resolve manually |
| Checksum mismatch after copy | removes the temp file, aborts | disk or source problem; retry after checking |
| Reference copy not present | aborts the removal | scan/prune and re-plan; never force |
| Image cannot be decoded in verify | converts to `keep_distinct` | inspect manually |

A non-zero exit code means at least one operation failed; the journal names the
path and the reason. Nothing else is silently rolled back: already-completed
operations stay, and the run can be repeated.

## 6. Operational rules

1. Stop ownCloud/Syncthing/Drive before moving album files.
2. Keep the database and quarantine on a local filesystem.
3. Run `plan` and the dry-run `apply` and read the numbers.
4. Run `verify` before any `delete`.
5. Apply additively first (`upgrade,import`), destructively later.
6. Archive Takeout JSON before purging a source root.
7. Back up the album before the first real run if the data is irreplaceable.
8. When in doubt, keep the file.

## 7. Why quarantine instead of trash

Desktop trash implementations are per-user, may be on another filesystem and
may be emptied automatically. A dedicated tree:

* is predictable (`quarantine/<date>/<root>/<relative path>`);
* keeps the original relative path, so restoring is a single `mv`;
* can be inspected and deleted by the user when they are confident;
* is captured in the journal, so the database can be reconciled.

## 8. Auditing a run

```bash
# What did the plan propose?
sqlite3 .bbcp/bbcp.db \
  "SELECT action, COUNT(*), SUM(bytes) FROM plan_actions GROUP BY action;"

# What was actually done?
column -ts $'\t' .bbcp/logs/apply-*.tsv | less

# What is in quarantine?
find .bbcp/quarantine -type f | wc -l
du -sh .bbcp/quarantine

# Are there unresolved verification cases?
sqlite3 .bbcp/bbcp.db \
  "SELECT COUNT(*) FROM plan_actions WHERE action='keep_distinct';"
```

A `keep_distinct` is not an error: it is a file the tool refused to delete and
a human should review.
