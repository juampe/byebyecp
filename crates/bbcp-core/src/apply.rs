//! Execute the plan with reversible, verifiable operations.
//!
//! Safety model:
//!
//! * `upgrade` and `import` copy through a temporary file in the destination
//!   directory, `fsync` it, verify SHA-256 and then `rename` atomically.
//! * Album duplicates are **moved** to quarantine, never unlinked by default.
//! * Source deletions (`external_covered`, `external_dup`) are only executed
//!   when the caller explicitly adds them to `--only`, and the album reference
//!   is re-checked first.
//! * Every operation is appended to a resumable TSV journal.

use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::Local;
use rayon::prelude::*;

use crate::config::Config;
use crate::hash::sha256_file;
use crate::model::{Action, FileRow};
use crate::store::Store;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveMode {
    /// Rename into the quarantine tree (reversible, frees no space).
    Quarantine,
    /// Unlink after verification (irreversible, frees space).
    Unlink,
}

impl RemoveMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "quarantine" => Some(RemoveMode::Quarantine),
            "unlink" => Some(RemoveMode::Unlink),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ApplyOptions {
    pub only: Vec<Action>,
    pub execute: bool,
    pub workers: usize,
    pub album_remove: RemoveMode,
    pub external_remove: RemoveMode,
    pub consume_sources: bool,
    pub limit: Option<usize>,
}

impl Default for ApplyOptions {
    fn default() -> Self {
        Self {
            only: vec![Action::Upgrade, Action::Import, Action::Delete],
            execute: false,
            workers: 8,
            album_remove: RemoveMode::Quarantine,
            external_remove: RemoveMode::Unlink,
            consume_sources: false,
            limit: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct ApplyStats {
    pub upgraded: usize,
    pub imported: usize,
    pub deleted: usize,
    pub external: usize,
    pub missing: usize,
    pub skipped: usize,
    pub errors: usize,
    pub bytes_written: u64,
    pub bytes_removed: u64,
}

struct Journal {
    out: BufWriter<File>,
    n: usize,
}

impl Journal {
    fn open(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join(format!("apply-{}.tsv", Local::now().format("%Y-%m-%d")));
        let new = !path.exists();
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening journal {}", path.display()))?;
        let mut out = BufWriter::new(file);
        if new {
            writeln!(
                out,
                "ts\taction\tsource\ttarget\tbackup\tbytes\tstatus\terror"
            )?;
            out.flush()?;
        }
        Ok(Self { out, n: 0 })
    }

    #[allow(clippy::too_many_arguments)]
    fn write(
        &mut self,
        action: Action,
        source: &Path,
        target: Option<&Path>,
        backup: Option<&Path>,
        bytes: u64,
        status: &str,
        error: &str,
    ) -> Result<()> {
        writeln!(
            self.out,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            Local::now().format("%Y-%m-%dT%H:%M:%S"),
            action.as_str(),
            source.display(),
            target.map(|p| p.display().to_string()).unwrap_or_default(),
            backup.map(|p| p.display().to_string()).unwrap_or_default(),
            bytes,
            status,
            error.replace('\t', " "),
        )?;
        self.n += 1;
        if self.n.is_multiple_of(100) {
            self.out.flush()?;
            self.out.get_ref().sync_all()?;
        }
        Ok(())
    }

    fn sync(&mut self) -> Result<()> {
        self.out.flush()?;
        self.out.get_ref().sync_all()?;
        Ok(())
    }
}

pub fn apply(store: &mut Store, cfg: &Config, opts: &ApplyOptions) -> Result<ApplyStats> {
    // Destructive work is refused without two independent backups. A dry run
    // only prints numbers, so it stays available for planning.
    if opts.execute {
        crate::safety::require_backups(cfg)?;
    }
    let plan = store.plan()?;
    let album_dir = cfg.album.clone();
    let album_label = cfg
        .album_root()
        .map(|r| r.label.clone())
        .unwrap_or_else(|| "album".into());
    let album_root_id = store.root_id(&album_label)?;
    let day = Local::now().format("%Y-%m-%d").to_string();
    let mut stats = ApplyStats::default();
    let mut journal = Journal::open(&cfg.paths.logs)?;

    // ---------- Phase 1: upgrades and imports ----------
    let mut additive: Vec<(Action, FileRow, PathBuf)> = plan
        .iter()
        .filter(|(a, _)| matches!(a.action, Action::Upgrade | Action::Import))
        .filter(|(a, _)| opts.only.contains(&a.action))
        .filter_map(|(a, f)| a.target_path.clone().map(|t| (a.action, f.clone(), t)))
        .collect();
    if let Some(limit) = opts.limit {
        additive.truncate(limit);
    }
    println!("phase 1/3: {} upgrade/import operations", additive.len());

    if opts.execute && !additive.is_empty() {
        let quarantine_root = cfg.paths.quarantine.join(&day);
        let results: Vec<_> = additive
            .par_iter()
            .map(|(action, file, target)| {
                apply_copy(
                    *action,
                    file,
                    target,
                    &album_dir,
                    &quarantine_root.join("upgrade").join(&album_label),
                    &quarantine_root.join("source").join(&file.root),
                    opts.consume_sources.then_some(opts.external_remove),
                )
            })
            .collect();

        for (result, (action, file, target)) in results.into_iter().zip(additive.iter()) {
            match result {
                Ok(outcome) => {
                    stats.bytes_written += outcome.bytes;
                    stats.bytes_removed += outcome.removed_bytes.unwrap_or(0);
                    match outcome.status.as_str() {
                        "upgraded" => stats.upgraded += 1,
                        "imported" => stats.imported += 1,
                        "missing" => stats.missing += 1,
                        _ => stats.skipped += 1,
                    }
                    journal.write(
                        *action,
                        &file.path,
                        Some(target),
                        outcome.backup.as_deref(),
                        outcome.bytes,
                        &outcome.status,
                        "",
                    )?;
                    // The imported file now lives in the album: keep the
                    // inventory coherent so later stages see the real path.
                    if *action == Action::Import
                        && matches!(outcome.status.as_str(), "imported" | "already-imported")
                    {
                        let rel = target
                            .strip_prefix(&album_dir)
                            .unwrap_or(target)
                            .to_string_lossy()
                            .replace('\\', "/");
                        store.relocate(file.id, album_root_id, target, &rel)?;
                    }
                }
                Err(error) => {
                    stats.errors += 1;
                    journal.write(
                        *action,
                        &file.path,
                        Some(target),
                        None,
                        file.size,
                        "error",
                        &error.to_string(),
                    )?;
                    eprintln!("ERROR {}: {error}", file.path.display());
                }
            }
        }
    }

    // ---------- Phase 2: album duplicates ----------
    let mut deletes: Vec<RemoveSpec> = Vec::new();
    if opts.only.contains(&Action::Delete) {
        for (action, file) in &plan {
            if action.action != Action::Delete {
                continue;
            }
            let reference_ok = match action.reference_id {
                Some(id) => reference_present(store, id)?.is_some(),
                None => false,
            };
            deletes.push(RemoveSpec {
                action: Action::Delete,
                file: file.clone(),
                quarantine_dir: cfg.paths.quarantine.join(&day).join(&album_label),
                reference_ok,
                mode: opts.album_remove,
            });
        }
    }
    println!("phase 2/3: {} album duplicates", deletes.len());
    if opts.execute && !deletes.is_empty() {
        run_removals(store, &deletes, opts.workers, &mut stats, &mut journal)?;
    }

    // ---------- Phase 3: source duplicates ----------
    let mut external: Vec<RemoveSpec> = Vec::new();
    for (action, file) in &plan {
        if !matches!(action.action, Action::ExternalCovered | Action::ExternalDup) {
            continue;
        }
        if !opts.only.contains(&action.action) {
            continue;
        }
        let reference_ok = match action.reference_id {
            Some(id) => reference_present(store, id)?.is_some(),
            None => false,
        };
        external.push(RemoveSpec {
            action: action.action,
            file: file.clone(),
            quarantine_dir: cfg
                .paths
                .quarantine
                .join(&day)
                .join("source")
                .join(&file.root),
            reference_ok,
            mode: opts.external_remove,
        });
    }
    println!("phase 3/3: {} source duplicates", external.len());
    if opts.execute && !external.is_empty() {
        run_removals(store, &external, opts.workers, &mut stats, &mut journal)?;
    }

    journal.sync()?;
    Ok(stats)
}

fn run_removals(
    store: &mut Store,
    specs: &[RemoveSpec],
    workers: usize,
    stats: &mut ApplyStats,
    journal: &mut Journal,
) -> Result<()> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers.max(1))
        .build()
        .context("creating worker pool")?;
    let results: Vec<Result<RemoveOutcome>> =
        pool.install(|| specs.par_iter().map(remove_one).collect());
    for (result, spec) in results.into_iter().zip(specs.iter()) {
        match result {
            Ok(outcome) => {
                stats.bytes_removed += outcome.bytes;
                if outcome.status == "missing" {
                    stats.missing += 1;
                } else if spec.action == Action::Delete {
                    stats.deleted += 1;
                } else {
                    stats.external += 1;
                }
                journal.write(
                    spec.action,
                    &spec.file.path,
                    None,
                    outcome.backup.as_deref(),
                    outcome.bytes,
                    &outcome.status,
                    "",
                )?;
                if outcome.status == "quarantined" || outcome.status == "purged" {
                    store.forget_file(spec.file.id)?;
                }
            }
            Err(error) => {
                stats.errors += 1;
                journal.write(
                    spec.action,
                    &spec.file.path,
                    None,
                    None,
                    spec.file.size,
                    "error",
                    &error.to_string(),
                )?;
                eprintln!("ERROR {}: {error}", spec.file.path.display());
            }
        }
    }
    Ok(())
}

struct RemoveSpec {
    action: Action,
    file: FileRow,
    quarantine_dir: PathBuf,
    reference_ok: bool,
    mode: RemoveMode,
}

struct RemoveOutcome {
    status: String,
    backup: Option<PathBuf>,
    bytes: u64,
}

fn remove_one(spec: &RemoveSpec) -> Result<RemoveOutcome> {
    if !spec.file.path.exists() {
        return Ok(RemoveOutcome {
            status: "missing".into(),
            backup: None,
            bytes: 0,
        });
    }
    if !spec.reference_ok {
        bail!("reference copy is not present; refusing to remove");
    }
    match spec.mode {
        RemoveMode::Unlink => {
            fs::remove_file(&spec.file.path)
                .with_context(|| format!("removing {}", spec.file.path.display()))?;
            Ok(RemoveOutcome {
                status: "purged".into(),
                backup: None,
                bytes: spec.file.size,
            })
        }
        RemoveMode::Quarantine => {
            let backup =
                move_into_quarantine(&spec.file.path, &spec.quarantine_dir, &spec.file.rel_path)?;
            Ok(RemoveOutcome {
                status: "quarantined".into(),
                backup: Some(backup),
                bytes: spec.file.size,
            })
        }
    }
}

struct CopyOutcome {
    status: String,
    backup: Option<PathBuf>,
    bytes: u64,
    removed_bytes: Option<u64>,
}

#[allow(clippy::too_many_arguments)]
fn apply_copy(
    action: Action,
    file: &FileRow,
    target: &Path,
    album_dir: &Path,
    upgrade_quarantine: &Path,
    source_quarantine: &Path,
    consume: Option<RemoveMode>,
) -> Result<CopyOutcome> {
    let source = &file.path;
    if !source.exists() {
        return Ok(CopyOutcome {
            status: "missing".into(),
            backup: None,
            bytes: 0,
            removed_bytes: None,
        });
    }
    if !target.starts_with(album_dir) {
        bail!("refusing to write outside the album: {}", target.display());
    }
    let target_existed = target.exists();
    if source == target {
        // A previous run already relocated the row; nothing to copy or remove.
        return Ok(CopyOutcome {
            status: "already-imported".into(),
            backup: None,
            bytes: 0,
            removed_bytes: None,
        });
    }

    match action {
        Action::Import if target_existed => {
            // A previous run already imported it. Only remove the source when
            // the existing target is byte-identical.
            if sha256_file(source)? == sha256_file(target)? {
                let removed = if let Some(mode) = consume {
                    Some(remove_source(
                        source,
                        source_quarantine,
                        &file.rel_path,
                        mode,
                    )?)
                } else {
                    None
                };
                return Ok(CopyOutcome {
                    status: "already-imported".into(),
                    backup: None,
                    bytes: 0,
                    removed_bytes: removed,
                });
            }
            bail!("target exists with different content: {}", target.display());
        }
        Action::Upgrade if !target_existed => {
            bail!(
                "upgrade target does not exist (run the plan again): {}",
                target.display()
            );
        }
        _ => {}
    }

    let mut backup = None;
    if action == Action::Upgrade {
        let rel = target
            .strip_prefix(album_dir)
            .unwrap_or_else(|_| Path::new(target.file_name().unwrap_or_default()));
        backup = Some(move_into_quarantine(
            target,
            upgrade_quarantine,
            &rel.to_string_lossy(),
        )?);
    }

    atomic_copy(source, target)?;
    let removed = if let Some(mode) = consume {
        Some(remove_source(
            source,
            source_quarantine,
            &file.rel_path,
            mode,
        )?)
    } else {
        None
    };
    Ok(CopyOutcome {
        status: if action == Action::Upgrade {
            "upgraded".into()
        } else {
            "imported".into()
        },
        backup,
        bytes: file.size,
        removed_bytes: removed,
    })
}

fn remove_source(
    source: &Path,
    quarantine_dir: &Path,
    rel_path: &str,
    mode: RemoveMode,
) -> Result<u64> {
    let size = fs::metadata(source).map(|m| m.len()).unwrap_or(0);
    if source.starts_with(quarantine_dir) {
        bail!(
            "refusing to quarantine the quarantine: {}",
            source.display()
        );
    }
    match mode {
        RemoveMode::Unlink => {
            fs::remove_file(source).with_context(|| format!("removing {}", source.display()))?;
        }
        RemoveMode::Quarantine => {
            move_into_quarantine(source, quarantine_dir, rel_path)?;
        }
    }
    Ok(size)
}

fn move_into_quarantine(path: &Path, quarantine_dir: &Path, rel: &str) -> Result<PathBuf> {
    let dest = quarantine_dir.join(rel);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    if dest.exists() {
        bail!("quarantine collision: {}", dest.display());
    }
    if fs::rename(path, &dest).is_err() {
        // Different filesystem: copy then remove.
        atomic_copy(path, &dest)?;
        fs::remove_file(path)?;
    }
    Ok(dest)
}

fn atomic_copy(source: &Path, target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = target.parent().unwrap_or(Path::new(".")).join(format!(
        ".bbcp-tmp-{}-{}",
        std::process::id(),
        target.file_name().unwrap_or_default().to_string_lossy()
    ));
    let expected = sha256_file(source)?;
    {
        let mut input = File::open(source)?;
        let mut output = File::create(&tmp)?;
        let mut buf = vec![0u8; 8 << 20];
        loop {
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            output.write_all(&buf[..n])?;
        }
        output.flush()?;
        output.sync_all()?;
    }
    let actual = sha256_file(&tmp)?;
    if actual != expected {
        let _ = fs::remove_file(&tmp);
        bail!("checksum mismatch copying {}", source.display());
    }
    if let Err(error) = fs::rename(&tmp, target) {
        let _ = fs::remove_file(&tmp);
        return Err(error).with_context(|| format!("renaming into {}", target.display()));
    }
    Ok(())
}

/// Where a reference lives once the plan has been (partially) applied.
fn reference_present(store: &Store, id: i64) -> Result<Option<PathBuf>> {
    let Some(file) = store.file(id)? else {
        return Ok(None);
    };
    let Some(action) = store.plan_of(id)? else {
        return Ok(Some(file.path).filter(|p| p.exists()));
    };
    if matches!(action.action, Action::Import | Action::Upgrade) {
        if let Some(target) = action.target_path {
            if target.exists() {
                return Ok(Some(target));
            }
        }
    }
    Ok(Some(file.path).filter(|p| p.exists()))
}

/// Counts of what a dry run would do, without touching the disk.
pub fn preview(store: &Store, opts: &ApplyOptions) -> Result<Vec<(Action, usize, u64)>> {
    let plan = store.plan()?;
    let mut map: std::collections::BTreeMap<String, (Action, usize, u64)> =
        std::collections::BTreeMap::new();
    for (action, file) in plan {
        if !opts.only.contains(&action.action) {
            continue;
        }
        let entry = map
            .entry(action.action.as_str().to_string())
            .or_insert((action.action, 0, 0));
        entry.1 += 1;
        entry.2 += file.size;
    }
    Ok(map.into_values().collect())
}
