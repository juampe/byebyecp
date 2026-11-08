# Google Takeout and Google Photos

Practical notes for importing a Takeout export and preparing deletions with
`bbcp`. It is based on the sidecar format Google produces and on what actually
survived a real cleanup.

## 1. What Takeout gives you

A Takeout export of Google Photos is a tree of media files, each one optionally
accompanied by a JSON sidecar:

```text
Takeout/
└── Google Fotos/
    ├── IMG_20210807_081841.jpg
    ├── IMG_20210807_081841.jpg.json
    ├── PXL_20210807_081841123.mp4
    ├── PXL_20210807_081841123.mp4.supplemental-metadata.json
    └── 2024/
        └── IMG_20240212_101010.jpg
```

Two naming conventions exist:

* `<media>.json` — older exports.
* `<media>.supplemental-metadata.json` — newer exports, also used when the
  media has some metadata already.

Long names may be truncated by older exports, so the sidecar stem can be a
prefix of the media name rather than the full name.

`bbcp` parses files containing `photoTakenTime` or `creationTime` and ignores
everything else (album metadata, shared-album lists, print orders).

## 2. What `bbcp sidecars` stores

For every sidecar it finds under a configured root:

| Column | Source | Use |
|---|---|---|
| `google_id` | `googleId` | deletion batch key |
| `title` | `title` | human identification |
| `taken_ts` | `photoTakenTime.timestamp` | preferred capture date |
| `creation_ts` | `creationTime.timestamp` | fallback date |
| `description` | `description` | preserved in the database |
| `matched_file_id` | matching algorithm | joins sidecar to media |

Timestamps are Unix seconds and are converted to local
`YYYY-MM-DD HH:MM:SS`.

Matching order:

1. exact path: `<sidecar path without .json>`;
2. path after removing `.supplemental-metadata`;
3. unique file name inside the same root;
4. unique media name that starts with the sidecar stem (truncated Takeout
   names).

A non-unique prefix match is rejected: a wrong match would attach a Google id
to the wrong photo.

Sidecars that do not match any media are stored with `matched_file_id = NULL`.
They still count as "kept" in the Google plan, never as delete candidates.

## 3. Dates

`photoTakenTime` is in UTC. `bbcp` converts it to local time; the exact hour can
differ from what Google Photos shows if the account timezone changed. For album
organisation this is acceptable; do not use these timestamps for forensic
ordering.

Do not trust:

* `1970-01-01` and `1980-01-01` — sentinels for "unknown";
* the export day (for example `2026/2026-01-05`) when Takeout dumped everything
  into one folder. List it in `import_dump_days`.

When a media file has lost its sidecar but an exact copy in another root still
has one, `bbcp dates` bridges the date through the group. That is why `group`
must run before `dates`.

## 4. Preparing the config

```toml
[[roots]]
label = "album"
path = "/mnt/b/ownCloud/Album"
kind = "album"

[[roots]]
label = "takeout-2026"
path = "/mnt/b/GPhotos/Takeout/Takeout/Google Fotos"
kind = "source"

[rules]
import_dump_days = ["2026-01-05"]
```

Then:

```bash
bbcp scan
bbcp sidecars
bbcp probe
bbcp fingerprint
bbcp hash
bbcp group --reset
bbcp dates
bbcp plan
bbcp verify
```

Check that the number of matched sidecars is plausible:

```bash
sqlite3 .bbcp/bbcp.db \
  "SELECT COUNT(*) AS total,
          SUM(matched_file_id IS NOT NULL) AS matched
   FROM sidecars;"
```

A very low match rate usually means a path or naming change in the export.

## 5. What the plan does with a Takeout root

* Media with an album twin (`image-exact`/`video-exact`) becomes
  `external_covered`: the album copy survives, the Takeout copy is redundant.
* Media without an album twin becomes `import` (moved or copied into the
  album).
* If a Takeout copy is materially better than the album copy, it becomes
  `upgrade` and the old album file goes to quarantine.
* Duplicates inside the Takeout root become `external_dup`: the best member is
  imported, the rest are redundant.

Nothing is removed until `apply` is run with the corresponding `--only`.

## 6. Generating deletion batches

```bash
bbcp plan      # must be current
bbcp google
```

Output:

```text
.bbcp/manifests/2026-11-08/
├── google_delete_candidates.tsv
└── google_batches/
    ├── batch_001.tsv
    ├── batch_002.tsv
    └── ...
```

Columns:

```text
google_id  title  taken  bytes  takeout_path  album_ref  match
```

A row is emitted only when:

1. the sidecar is matched to a local file;
2. the file belongs to an exact group;
3. the group has an album member that survives the plan (`keep`,
   `keep_distinct`, `upgrade` or `import`);
4. that album path exists right now.

Items that fail any condition are counted as `kept` in the command output.

The batches are sorted by capture date and title, so a partial run is stable
and resumable.

## 7. Deleting in Google Photos

`bbcp` does not call Google. Delete by batch, from the browser or with an
automation agent:

1. Look up the item by `google_id`, or by `title` + `taken` when the id is
   empty.
2. Confirm the item is the one in the TSV (date and, for videos, duration).
3. Delete it.
4. After the batch, check the account's storage usage.
5. Keep the TSV batch file and mark it as processed.

Suggested order when storage is the goal:

1. **Videos** — few items, large bytes, easiest to verify by duration.
2. **Heaviest items** — a "heavy" subset up to a target (for example 100 GB).
3. **Everything else** in date order.

Do not delete items that are not in a batch. Do not reuse a batch after the
album or the plan changed; regenerate with `bbcp google`.

## 8. Before you empty the Takeout folder

Once the album has verified copies and the Google batches are generated:

1. Archive the Takeout JSON files (they contain `googleId`, titles, dates and
   descriptions that no longer exist anywhere else):

   ```bash
   OUT=.bbcp/metadata-archive/takeout-2026.tar.gz
   ROOT="/mnt/b/GPhotos/Takeout/Takeout/Google Fotos"
   find "$ROOT" -type f -name '*.json' -printf '%P\0' \
     | tar -C "$ROOT" --null -T - -czf "$OUT.part"
   gzip -t "$OUT.part" && mv "$OUT.part" "$OUT"
   ```

   Write to `.part` and rename only after `gzip -t` passes, so an interrupted
   archive never looks valid.

2. Purge redundant source media into quarantine:

   ```bash
   bbcp apply --execute --only external_covered --external-remove quarantine
   bbcp apply --execute --only external_dup --external-remove quarantine
   ```

3. Delete the remaining Takeout folder only when the album totals match the
   expected counts and the user confirms.

## 9. Common problems

| Symptom | Cause | Fix |
|---|---|---|
| `matched sidecars: 0` | root points to the parent of `Google Fotos` | point it at the folder that contains the media |
| Sidecars matched but no dates | `dates` not run, or sentinel dates | run `bbcp dates`; check `capture_source` |
| Google candidates much lower than expected | plan missing or album copies marked `delete` | run `bbcp plan` and inspect `plan_actions` |
| `google_id` empty | old export without `googleId` in the sidecar | match by title + date; the TSV still has both |
| Duplicate `google_id` rows | the same media appears twice in the export | keep one; inspect the group |
| Wrong dates en masse | export day folder used as capture date | add it to `import_dump_days` and re-run `dates` |

## 10. What is not automated

* Authentication against Google.
* Clicking through the Google Photos UI.
* Verifying Google's trash retention policy. Items in the trash still occupy
  quota until the trash is emptied; that step is manual.
* Shared albums and items owned by someone else: they are not in your storage
  and cannot be deleted by you.

Keep the batch files and the `report.md` next to the journals. They are the
audit trail that proves which items had a local copy when they were deleted.
