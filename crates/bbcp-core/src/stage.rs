//! Move unique media to a staging area so a human can review it before it is
//! merged into the album.
//!
//! Staging uses `rename` inside the same filesystem (instant, atomic). The
//! inventory is updated, so a later `bbcp apply --only import` reads the files
//! from their new location.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Local;
use rayon::prelude::*;

use crate::config::{Config, Root};
use crate::model::Action;
use crate::store::Store;

#[derive(Debug, Default)]
pub struct StageStats {
    pub moved: usize,
    pub bytes: u64,
    pub missing: usize,
    pub collisions: usize,
    pub errors: usize,
}

pub fn stage(
    store: &mut Store,
    cfg: &Config,
    roots: &[String],
    execute: bool,
) -> Result<StageStats> {
    let day = Local::now().format("%Y-%m-%d").to_string();
    let stage_dir = cfg.paths.staging.join(&day);
    let label = format!("staging-{day}");
    let mut stats = StageStats::default();

    let wanted: HashSet<&str> = roots.iter().map(String::as_str).collect();
    let plan = store.plan()?;
    let mut rows: Vec<(i64, PathBuf, String, String, u64)> = Vec::new();
    for (action, file) in &plan {
        if action.action != Action::Import {
            continue;
        }
        if !wanted.is_empty() && !wanted.contains(file.root.as_str()) {
            continue;
        }
        rows.push((
            file.id,
            file.path.clone(),
            file.root.clone(),
            file.rel_path.clone(),
            file.size,
        ));
    }

    println!("staging {} files into {}", rows.len(), stage_dir.display());
    if !execute {
        for r in rows.iter().take(8) {
            println!(
                "  {} -> {}",
                r.1.display(),
                stage_dir.join(&r.2).join(&r.3).display()
            );
        }
        return Ok(stats);
    }

    // Register the staging folder as a root so the database stays coherent.
    store.sync_roots(&[Root {
        label: label.clone(),
        path: stage_dir.clone(),
        kind: "source".into(),
    }])?;
    let stage_root_id = store.root_id(&label)?;
    let mut used: HashSet<PathBuf> = HashSet::new();

    struct Move {
        id: i64,
        source: PathBuf,
        dest: PathBuf,
        size: u64,
    }
    let mut moves = Vec::new();
    for (id, path, root, rel, size) in rows {
        let mut dest = stage_dir.join(&root).join(&rel);
        if used.contains(&dest) {
            stats.collisions += 1;
            let stem = dest
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let ext = dest
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            let mut n = 2;
            loop {
                let candidate = dest.with_file_name(format!("{stem}__{n}{ext}"));
                if !used.contains(&candidate) {
                    dest = candidate;
                    break;
                }
                n += 1;
            }
        }
        used.insert(dest.clone());
        moves.push(Move {
            id,
            source: path,
            dest,
            size,
        });
    }

    let results: Vec<Result<(usize, u64)>> = moves
        .par_iter()
        .map(|m| {
            if !m.source.exists() {
                return Ok((1usize, 0u64)); // missing
            }
            if let Some(parent) = m.dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(&m.source, &m.dest)
                .with_context(|| format!("moving {}", m.source.display()))?;
            Ok((0usize, m.size))
        })
        .collect();

    let mut journal = Vec::new();
    for (result, m) in results.into_iter().zip(moves.iter()) {
        match result {
            Ok((1, _)) => stats.missing += 1,
            Ok((_, size)) => {
                stats.moved += 1;
                stats.bytes += size;
                journal.push((m.id, m.dest.clone()));
            }
            Err(error) => {
                stats.errors += 1;
                eprintln!("ERROR {}: {error}", m.source.display());
            }
        }
    }

    // Update the inventory in one transaction.
    let tx = store.con.transaction()?;
    {
        let mut stmt = tx.prepare("UPDATE files SET root_id=?, path=?, rel_path=? WHERE id=?")?;
        for (id, dest) in &journal {
            let rel = dest
                .strip_prefix(&stage_dir)
                .unwrap_or(dest)
                .to_string_lossy()
                .replace('\\', "/");
            stmt.execute(rusqlite::params![
                stage_root_id,
                dest.to_string_lossy(),
                rel,
                id
            ])?;
        }
    }
    tx.commit()?;

    // Journal mirrors the database so a crash can be reconciled.
    std::fs::create_dir_all(&cfg.paths.logs)?;
    let journal_path = cfg.paths.logs.join(format!("stage-{day}.tsv"));
    let mut text = String::from("file_id\tpath\n");
    for (id, dest) in &journal {
        text.push_str(&format!("{id}\t{}\n", dest.display()));
    }
    std::fs::write(&journal_path, text).context("writing stage journal")?;

    Ok(stats)
}

/// List what is currently in a staging directory.
pub fn listing(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() {
            out.push(entry.path().to_path_buf());
        }
    }
    Ok(out)
}
