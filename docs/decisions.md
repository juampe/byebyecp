# Design decisions

Short records of the choices that shape the tool and the alternatives that
were rejected. They explain *why*, so a future change does not silently undo a
safety property.

## D1 — Rust CLI, SQLite inventory, no Python

**Decision.** Ship a single `bbcp` binary with an SQLite database as the only
state.

**Why.** The original workflow was a set of Python scripts driven by hand.
Every run had to remember paths, ordering and thresholds. A single binary can
be installed anywhere, keeps the state in one file, and lets an agent run a
stage idempotently.

**Rejected.** Keeping the Python scripts as the engine: dependency drift,
`imagehash`/Pillow/numpy versions, and no safe packaging. SQLite on a network
mount: locking and latency problems. JSON files as state: no transactions.

## D2 — Workspace split: `bbcp-core` + `bbcp-cli`

**Decision.** All logic lives in `bbcp-core`; `bbcp-cli` is a thin `clap`
wrapper. `bbcp-core` is usable as a library and testable without a terminal.

**Why.** Integration tests and future front-ends (TUI, web, batch runner) share
the engine. The CLI stays small and mirrors the pipeline.

## D3 — Three perceptual hashes, exact triple, no transitive closure

**Decision.** Group images by `(pHash, dHash, aHash)` equality, all 64-bit.

**Why.**

* pHash (DCT 32×32) survives recompression and resizing.
* dHash captures horizontal gradients; aHash captures global brightness.
* Requiring all three makes accidental collisions unlikely without decoding
  every pair.

**Transitive closure was explicitly rejected.** Chaining "A resembles B, B
resembles C" merges burst frames, screenshots and edited versions into one huge
group. That is how a deduplicator deletes a photo nobody has a copy of. Groups
are equivalence classes of the exact key, no chaining.

**Also rejected.** A single hash with a distance threshold: more false
positives, and the threshold becomes a guessing game.

## D4 — Popcount and aspect-ratio gates

**Decision.** Reject hashes with fewer than 12 set bits; reject groups whose
max/min aspect ratio exceeds 1.05.

**Why.** Flat, black, or nearly uniform images produce hashes like
`8000000000000000/0` that match thousands of unrelated files. Aspect ratio
catches pairs that hash alike but are not the same photograph (crops,
screenshots, panoramas).

## D5 — Pixel MAD before deleting an image

**Decision.** Decode both files, resize to 64×64 grayscale and compute the mean
absolute difference. Above 1.5 the action becomes `keep_distinct`.

**Why.** Hash equality is probabilistic. Burst frames of a moving subject can
share a triple. MAD is cheap (two decodes), has measured separation in real
libraries (identical ≈ 0–0.5, recompressed ≈ 0.5–1.2, different frames ≥ 2.7)
and is explainable in the journal.

**Rejected.** SSIM or a model embedding: slower, harder to explain, and not
needed for "same content or not".

## D6 — Videos: SHA-256 for exact duplicates, frame check for re-encodes

**Decision.** Group videos only by SHA-256. Hash only videos whose size
collides with another video. For suspected re-encodes, compare one extracted
frame with `pHash + dHash` (distance ≤ 16).

**Why.** Hashing every video reads hundreds of GB for little gain: byte-identical
copies necessarily share size, and the SHA-256 confirms them. Duration +
resolution was measured to be unreliable: in one real library ~85 % of "approx"
pairs were different videos.

**Rejected.** Treating duration+resolution as proof. Treating a frame match as
proof without a second signal.

## D7 — Quality ranking is evidence-based, not album-biased

**Decision.** Rank by `(pixels, bitrate or size, older mtime, size)`. A source
copy replaces an album copy only when it is *materially* better: more pixels, or
the same resolution with > 1.25× the data.

**Why.** The album can contain a downscaled copy from an old phone while the
Takeout folder has the original. A 2 % size difference is noise, not an upgrade;
rewriting the album for noise risks breaking external references and wastes IO.

## D8 — Date precedence and exclusions

**Decision.** sidecar → sidecar of an exact copy → EXIF → filename → folder.
Reject `1970-01-01`, `1980-01-01`, and folders listed in `import_dump_days`.

**Why.** Takeout JSON is the most reliable source, but it is often lost when the
export is reorganised. An exact twin in another root may still have it, so the
date is bridged through the group (this requires `group` before `dates`). EXIF
survives copies made by cameras. File and folder names are a last resort.

Sentinel dates and export-day folders are worse than "unknown": they create
thousands of photos in 1970 or on the import day, which is harder to fix later
than an empty field.

## D9 — Action taxonomy

**Decision.** Seven actions: `keep`, `keep_distinct`, `delete`, `upgrade`,
`import`, `external_covered`, `external_dup`.

**Why.** The plan must express *where* the surviving copy is, not only *what* to
delete.

* `delete` is an album duplicate; the survivor is another album member.
* `external_covered` is a source duplicate of a protected album member.
* `external_dup` is a duplicate inside a source root; the best member is
  imported first.
* `upgrade` replaces an album member with a better source member.
* `import` adds unique source media.

Separating album from source actions means the destructive source operations can
require a different flag and a different confirmation.

## D10 — Album root is protected; plan never trusts the `best` role

**Decision.** Only roots with `kind = "album"` are eligible as the surviving
copy. `plan` and `google` recompute the best album member from the current
rows instead of trusting the `best` role written by `group`.

**Why.** `group` may pick a source copy as `best` (it can genuinely be better).
That is fine for ranking, but it cannot be the survivor until it has been
imported. Recomputing keeps the plan correct after a partial apply or a rescan.

## D11 — Additive before destructive; quarantine by default

**Decision.** `apply` runs three phases: imports/upgrades, album duplicates,
source duplicates. Album duplicates move to quarantine; source removal defaults
to unlink but only runs when the action is explicitly included in `--only`.

**Why.** If the process dies mid-run, the replacement copies exist before
anything is removed. Quarantine makes the common case reversible; unlinking is
reserved for source roots where the whole point is freeing space.

## D12 — Atomic copy with checksum

**Decision.** Write to `.bbcp-tmp-<pid>-<name>` in the destination directory,
`fsync`, compare SHA-256 with the source, then `rename`. Refuse to write outside
the album.

**Why.** `rename` within a directory is atomic; a partially written file is
never visible under the final name. The checksum catches a truncated read,
bad media and the classic "same size, different bytes" assumption.

## D13 — Staging for human review

**Decision.** `stage` moves `import` rows to `.bbcp/staging/<date>/<root>/<rel>`
with `rename`, updates the database, and keeps a journal.

**Why.** Importing thousands of unique files straight into the album is a
one-way door: naming collisions, duplicates between source roots and unwanted
screenshots become permanent. A staging tree lets the user inspect first, then
`apply --only import` moves them in. Because the database is updated, the import
reads from the new path.

## D14 — Google batches only for verified local copies

**Decision.** A Google deletion candidate must have: a parsed sidecar matched to
a local file, that file in an exact group, an album member that survives the
current plan, and that path existing on disk.

**Why.** `google_id` alone is not proof of a local copy. The sidecar-to-file
match is the weakest link, so the chain is validated end to end and the album
reference is written into every TSV row for auditing.

**Not done.** Talking to the Google Photos API. It is unnecessary for deletion
batches and would add OAuth, quotas and a new failure surface. The TSV is
consumed by a human or a browser agent.

## D15 — Deliverable documentation is self-sufficient

**Decision.** `AGENTS.md`, the skill and `docs/` contain the entire procedure,
including thresholds, commands, traps and rollback. The repository does not
reference `MigrationPlan.md` or any external note.

**Why.** The original knowledge lived in a chat transcript and several local
documents. A public repository that says "see the private plan" is not
reproducible. Every rule needed to run the tool is in the repo.

## D16 — License: GPL-3.0-only

**Decision.** GPL-3.0-only.

**Why.** Ideas and code patterns were reused from UMDF (GPL-3.0). Keeping the
same license preserves compatibility and keeps the project honest about its
provenance. See `THIRD_PARTY.md`.

## D17 — Two independent backups are a hard precondition

**Decision.** `apply --execute` refuses to run unless `[safety] backups` lists
at least two existing directories: outside the album, on a different
filesystem, and distinct from each other. `bbcp doctor` reports the same check.
`allow_without_backups = true` is the only escape hatch and it must be set by
the operator.

**Why.** Quarantine is on the same disk as the album. It protects against a bad
pattern in the plan, not against a dead disk, a mistaken `rm`, a sync client or
a bug in `apply`. The tool cannot verify that a backup is restorable, but it can
refuse to make the situation worse when there is no second copy.

**Rejected.** A warning only: warnings are ignored exactly when they matter. A
CLI flag: a flag can be copy-pasted from a tutorial, while a config entry forces
the user to name two real paths.

## D18 — As-is, no warranty, no liability

**Decision.** Keep the GPL-3.0 warranty disclaimer, and state it in plain
language in `DISCLAIMER.md` and in a warning box at the top of the README.

**Why.** The tool deletes files. A user who skips the dry run and the backups
must not be able to claim they were not warned, and the maintainers must not
carry the consequences of a run they did not perform. The disclaimer also names
the non-limitable cases (wilful misconduct, gross negligence, mandatory
consumer rights) so it is honest rather than theatrical.
