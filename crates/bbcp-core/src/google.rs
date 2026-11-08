//! Google Photos deletion assistance.
//!
//! The tool never talks to Google. It produces TSV batches of items that have
//! a verified local copy in the album, so a human (or a browser-automation
//! agent) can delete them in Google Photos without guessing.
//!
//! Matching goes through the Takeout sidecars: each sidecar knows the Google
//! title and capture date, and is matched to a local Takeout file. That file
//! belongs to an exact group whose album member is the safe local copy.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Local;

use crate::config::Config;
use crate::model::GroupKind;
use crate::store::Store;

#[derive(Debug, Default)]
pub struct GoogleStats {
    pub candidates: usize,
    pub kept: usize,
    pub bytes: u64,
    pub batches: usize,
}

struct Candidate {
    google_id: String,
    title: String,
    taken: String,
    bytes: u64,
    takeout_path: PathBuf,
    album_ref: PathBuf,
    match_kind: String,
}

pub fn plan(store: &Store, cfg: &Config) -> Result<GoogleStats> {
    let album = cfg
        .album_root()
        .ok_or_else(|| anyhow::anyhow!("no album root in config"))?
        .label
        .clone();

    // Album references that will still exist after applying the plan. A file
    // marked `delete` is not a valid reference.
    let plan = store.plan()?;
    let surviving: HashMap<i64, (PathBuf, String)> = plan
        .iter()
        .filter(|(a, _)| {
            matches!(
                a.action,
                crate::model::Action::Keep
                    | crate::model::Action::KeepDistinct
                    | crate::model::Action::Upgrade
                    | crate::model::Action::Import
            )
        })
        .map(|(_, f)| (f.id, (f.path.clone(), f.root.clone())))
        .collect();

    // file_id -> (album file id, album path, match kind), for every group.
    let mut album_ref: HashMap<i64, (i64, PathBuf, String)> = HashMap::new();
    for group in store.groups_with_members()? {
        let kind = if group.kind == GroupKind::ImageExact.as_str() {
            "image-exact"
        } else {
            "video-exact"
        };
        let mut album_members: Vec<crate::model::FileRow> = Vec::new();
        for (id, _) in &group.members {
            if let Some(f) = store.file(*id)? {
                if f.root == album && surviving.contains_key(id) {
                    album_members.push(f);
                }
            }
        }
        let refs: Vec<&crate::model::FileRow> = album_members.iter().collect();
        let Some(best) = crate::quality::best_of(&refs) else {
            continue;
        };
        for (id, _) in &group.members {
            album_ref
                .entry(*id)
                .or_insert((best.id, best.path.clone(), kind.to_string()));
        }
    }

    let mut candidates = Vec::new();
    let mut kept = 0usize;
    for sidecar in store.sidecars()? {
        let Some(file_id) = sidecar.matched_file_id else {
            kept += 1;
            continue;
        };
        let Some((ref_id, ref_path, kind)) = album_ref.get(&file_id) else {
            kept += 1;
            continue;
        };
        // Is the reference the surviving copy?
        if !surviving.contains_key(ref_id) || !ref_path.exists() {
            kept += 1;
            continue;
        }
        let Some(file) = store.file(file_id)? else {
            kept += 1;
            continue;
        };
        let title = sidecar.title.clone().unwrap_or_else(|| {
            file.path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        });
        candidates.push(Candidate {
            google_id: sidecar.google_id.clone().unwrap_or_default(),
            title,
            taken: sidecar
                .taken_ts
                .clone()
                .or_else(|| file.capture_ts.clone())
                .unwrap_or_default(),
            bytes: file.size,
            takeout_path: file.path.clone(),
            album_ref: ref_path.clone(),
            match_kind: kind.clone(),
        });
    }

    let dir = cfg
        .paths
        .manifests
        .join(Local::now().format("%Y-%m-%d").to_string());
    fs::create_dir_all(&dir)?;
    let batch_dir = dir.join("google_batches");
    fs::create_dir_all(&batch_dir)?;

    let mut stats = GoogleStats {
        candidates: candidates.len(),
        kept,
        bytes: candidates.iter().map(|c| c.bytes).sum(),
        batches: 0,
    };

    write_tsv(
        &dir.join("google_delete_candidates.tsv"),
        candidates.iter().map(|c| {
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                c.google_id,
                c.title.replace('\t', " "),
                c.taken,
                c.bytes,
                c.takeout_path.display(),
                c.album_ref.display(),
                c.match_kind
            )
        }),
    )?;

    // Batch files: a stable order (oldest capture first) keeps runs resumable.
    let mut sorted = candidates;
    sorted.sort_by(|a, b| a.taken.cmp(&b.taken).then(a.title.cmp(&b.title)));
    let size = cfg.google.batch_size.max(1);
    for (n, chunk) in sorted.chunks(size).enumerate() {
        let path = batch_dir.join(format!("batch_{:03}.tsv", n + 1));
        write_tsv(
            &path,
            chunk.iter().map(|c| {
                format!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    c.google_id,
                    c.title.replace('\t', " "),
                    c.taken,
                    c.bytes,
                    c.takeout_path.display(),
                    c.album_ref.display(),
                    c.match_kind
                )
            }),
        )?;
        stats.batches += 1;
    }

    Ok(stats)
}

fn write_tsv(path: &Path, rows: impl Iterator<Item = String>) -> Result<()> {
    let mut text = String::from("google_id\ttitle\ttaken\tbytes\ttakeout_path\talbum_ref\tmatch\n");
    for row in rows {
        text.push_str(&row);
        text.push('\n');
    }
    fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
