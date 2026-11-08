//! Pixel verification for image deletions and upgrades.
//!
//! A perceptual hash triple proves "almost certainly the same picture", but
//! burst frames of a moving subject can hash alike. Before deleting anything
//! the two images are decoded and compared at 64x64: a mean absolute
//! difference above the configured threshold turns the action into
//! `keep_distinct`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;
use rayon::prelude::*;

use crate::config::Config;
use crate::imagehash::{load_gray, mad};
use crate::model::{Action, MediaKind};
use crate::store::Store;

#[derive(Debug, Default)]
pub struct VerifyStats {
    pub checked: usize,
    pub confirmed: usize,
    pub kept_distinct: usize,
    pub errors: usize,
}

struct Task {
    file_id: i64,
    left: PathBuf,
    right: PathBuf,
    reference_id: i64,
}

pub fn verify_images(store: &mut Store, cfg: &Config) -> Result<VerifyStats> {
    let plan = store.plan()?;
    let mut tasks = Vec::new();
    for (action, file) in &plan {
        if file.media != MediaKind::Image {
            continue;
        }
        if !matches!(action.action, Action::Delete | Action::Upgrade) {
            continue;
        }
        let Some(reference_id) = action.reference_id else {
            continue;
        };
        let Some(reference) = store.file(reference_id)? else {
            continue;
        };
        tasks.push(Task {
            file_id: file.id,
            left: file.path.clone(),
            right: reference.path.clone(),
            reference_id,
        });
    }

    let errors = AtomicUsize::new(0);
    let results: Vec<(Task, Option<f64>)> = tasks
        .into_par_iter()
        .map(|t| {
            let value = match (load_gray(&t.left), load_gray(&t.right)) {
                (Ok(a), Ok(b)) => Some(mad(&a, &b)),
                _ => {
                    errors.fetch_add(1, Ordering::Relaxed);
                    None
                }
            };
            (t, value)
        })
        .collect();

    let mut stats = VerifyStats::default();
    for (task, value) in results {
        stats.checked += 1;
        match value {
            None => {
                stats.errors += 1;
                // Without evidence, keep the file.
                store.set_action(
                    task.file_id,
                    Action::KeepDistinct,
                    "verification failed: image could not be decoded",
                )?;
                stats.kept_distinct += 1;
            }
            Some(mad) => {
                store.set_mad(task.file_id, mad)?;
                if mad > cfg.rules.mad_threshold {
                    store.set_action(
                        task.file_id,
                        Action::KeepDistinct,
                        &format!(
                            "MAD {mad:.3} > {:.3}: different content, kept (ref {})",
                            cfg.rules.mad_threshold, task.reference_id
                        ),
                    )?;
                    stats.kept_distinct += 1;
                } else {
                    stats.confirmed += 1;
                }
            }
        }
    }
    Ok(stats)
}

/// Compare two images directly; used by the `bbcp compare` diagnostics command.
pub fn compare(left: &std::path::Path, right: &std::path::Path) -> Result<f64> {
    let a = load_gray(left)?;
    let b = load_gray(right)?;
    Ok(compare_images(&a, &b))
}

/// MAD between two already-decoded grayscale images.
pub fn compare_images(a: &image::GrayImage, b: &image::GrayImage) -> f64 {
    mad(a, b)
}
