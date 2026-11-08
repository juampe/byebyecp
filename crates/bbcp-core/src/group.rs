//! Exact duplicate grouping.
//!
//! Images are keyed by the exact triple `(pHash, dHash, aHash)`. There is
//! deliberately **no transitive closure**: chaining near-matches merges
//! different frames of a burst into one giant group.
//!
//! Videos are keyed by SHA-256, which is exact by definition.

use std::collections::HashMap;

use anyhow::Result;

use crate::config::Rules;
use crate::imagehash::popcount;
use crate::model::{FileRow, GroupKind};
use crate::quality;
use crate::store::Store;

#[derive(Debug, Default)]
pub struct GroupStats {
    pub groups: usize,
    pub members: usize,
    pub skipped_flat: usize,
    pub skipped_aspect: usize,
}

/// Group images by their perceptual triple.
pub fn group_images(store: &mut Store, rules: &Rules) -> Result<GroupStats> {
    let images = store.images()?;
    let mut buckets: HashMap<String, Vec<FileRow>> = HashMap::new();
    let mut stats = GroupStats::default();

    for f in images {
        let (Some(ph), Some(dh), Some(ah)) = (&f.phash, &f.dhash, &f.ahash) else {
            continue;
        };
        let min = popcount(ph).min(popcount(dh)).min(popcount(ah));
        if min < rules.min_popcount {
            stats.skipped_flat += 1;
            continue;
        }
        let key = format!("{ph}:{dh}:{ah}");
        buckets.entry(key).or_default().push(f);
    }

    for (key, group) in buckets {
        if group.len() < 2 {
            continue;
        }
        // Reject groups whose members have incompatible aspect ratios: the
        // hash triple matched but the shapes are not the same photograph.
        let mut min_aspect = f64::INFINITY;
        let mut max_aspect = 0.0f64;
        for f in &group {
            let a = f.aspect();
            min_aspect = min_aspect.min(a);
            max_aspect = max_aspect.max(a);
        }
        if min_aspect > 0.0 && max_aspect / min_aspect > rules.aspect_tolerance {
            stats.skipped_aspect += 1;
            continue;
        }

        let best = quality::best(&group).expect("grupo no vacío");
        let members: Vec<(i64, &str)> = group
            .iter()
            .map(|f| {
                let role = if f.id == best.id { "best" } else { "duplicate" };
                (f.id, role)
            })
            .collect();
        store.insert_group(GroupKind::ImageExact, &key, &members)?;
        stats.groups += 1;
        stats.members += group.len();
    }
    Ok(stats)
}

/// Group videos by exact SHA-256.
pub fn group_videos(store: &mut Store) -> Result<GroupStats> {
    let videos = store.videos()?;
    let mut buckets: HashMap<String, Vec<FileRow>> = HashMap::new();
    let mut stats = GroupStats::default();
    for f in videos {
        let Some(sha) = &f.sha256 else {
            continue;
        };
        buckets.entry(sha.clone()).or_default().push(f);
    }
    for (key, group) in buckets {
        if group.len() < 2 {
            continue;
        }
        let best = quality::best(&group).expect("grupo no vacío");
        let members: Vec<(i64, &str)> = group
            .iter()
            .map(|f| {
                let role = if f.id == best.id { "best" } else { "duplicate" };
                (f.id, role)
            })
            .collect();
        store.insert_group(GroupKind::VideoExact, &key, &members)?;
        stats.groups += 1;
        stats.members += group.len();
    }
    Ok(stats)
}
