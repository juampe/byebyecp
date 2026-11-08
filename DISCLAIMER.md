# Disclaimer, warranty and liability

**Read this before running `bbcp` on anything you care about.**

`bbcp` moves and deletes files. It is designed to be careful, but it is
software, and software fails. The authors give you the code as-is and accept
**no responsibility** for what happens to your data.

If you do not accept these terms, do not use this software.

## 1. No warranty

This program is free software: you can redistribute it and/or modify it under
the terms of the GNU General Public License, version 3 (see [`LICENSE`](LICENSE)).
It is distributed **without any warranty**, to the extent permitted by
applicable law: no warranty of merchantability, fitness for a particular
purpose, or non-infringement.

The full text is in the license, sections 15 ("Disclaimer of Warranty") and 16
("Limitation of Liability"). This page explains in plain language what that
means for a photo library.

## 2. Zero liability

To the maximum extent permitted by applicable law, the authors, contributors
and copyright holders are **not liable** for any claim, damage or other
liability — direct, indirect, incidental, special, exemplary or consequential —
arising from the use of, or inability to use, this software.

That includes, without limitation:

* deleted, quarantined, overwritten, truncated or corrupted files;
* loss of photos, videos, metadata, albums, descriptions, Google ids or dates;
* storage costs, lost time, or a failed migration;
* damage to a disk, filesystem, sync client or cloud account;
* anything you delete **in Google Photos or any other service** with the help of
  this tool.

This limitation does not apply where the law does not allow it to apply, for
example in cases of wilful misconduct, gross negligence, death or personal
injury, or mandatory consumer rights.

## 3. You are the operator

`bbcp` proposes and executes a plan; **you** decide to run it. In particular:

* `apply` is a dry run unless you pass `--execute`.
* `--album-remove unlink` and `--external-remove unlink` are irreversible.
* `apply --only external_covered,external_dup` purges source roots.
* Deleting items in Google Photos is done by you, outside this tool, and is
  permanent once the service's trash is emptied.

Review the dry run, the journals and the manifests. If a number looks wrong,
stop.

## 4. Keep at least two backups

Before any destructive run, have **at least two independent backups** of the
data you are about to touch:

1. two copies on **different physical media**,
2. outside the album and outside the quarantine,
3. and at least one of them **offline or off-site**.

Somewhere sensible:

```text
/mnt/backup-1/Album     # external disk
/mnt/backup-2/Album     # off-site or cloud, synced
```

`bbcp` does **not** check, block or verify this. It cannot tell whether a
directory really holds a complete copy, whether it is up to date, or whether it
is restorable — and pretending otherwise would be misleading. It is a
recommendation, and it is on you.

Rules of thumb:

* **Quarantine is not a backup.** `.bbcp/quarantine` lives next to the album;
  a bad command, a failing disk or a sync client can take both.
* **A second folder on the same disk is not a backup.**
* **A sync folder is not a backup.** Deleting a file usually deletes it
  everywhere.
* **An untested backup is a hope.** Restore a few files before you trust it.
* **Cloud photos count only if you have a verified local copy** — that is
  exactly what this tool tries to produce.

The 3-2-1 rule is the minimum: **3** copies, on **2** different media, with
**1** off-site.

## 5. No guarantee of correctness

Duplicate detection is probabilistic. `bbcp` uses perceptual hashes with gates
and pixel/frame verification to reduce false positives, but it cannot promise
that no two different photos will ever be treated as equal, nor that every
duplicate will be found. Dates, dimensions, bitrates and codecs come from
EXIF, sidecars and `ffprobe`, and may be wrong or missing.

`keep_distinct` means "the tool refused to decide": review it manually.

## 6. Google, Takeout and other services

This project is not affiliated with, endorsed by or supported by Google. Google
Photos, Google Takeout and related marks belong to Google LLC. Your use of
those services is governed by their own terms, and automated interaction with
them may violate those terms or local law. You are responsible for complying.

The same applies to any other provider, sync client or storage service you
combine with `bbcp`.

## 7. Data you do not own

Use this tool only on media you own or are authorised to manage. Consolidating
or deleting someone else's library without permission may be illegal.

## 8. No support obligation

There is no SLA, no guaranteed response time and no promise that an issue will
be fixed. The project is maintained on a best-effort basis. Security reports
and reproducible bugs are welcome; see [`CONTRIBUTING`](AGENTS.md) guidance in
`AGENTS.md`.

## 9. Third-party components

`bbcp` depends on third-party crates and on `ffmpeg`/`ffprobe`. Their licenses
and their behaviour are their authors' responsibility. See
[`THIRD_PARTY.md`](THIRD_PARTY.md).

---

**Summary:** back up twice, verify the dry run, keep the quarantine, and accept
that you run this at your own risk. The tool will not stop you — it only makes
"wrong" recoverable as far as it can.
