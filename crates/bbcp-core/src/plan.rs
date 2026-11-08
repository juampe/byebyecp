//! Build the consolidation plan.
//!
//! Rules, in order of precedence:
//!
//! * An exact group that already has an album member keeps the best album
//!   copy; the other album members become `delete` (quarantine) and every
//!   source member becomes `external_covered`, unless one source member is
//!   materially better, in which case it becomes `upgrade`.
//! * A group with no album member imports its best copy and marks the rest
//!   `external_dup`.
//! * Non-duplicated source media becomes `import`.
//! * Everything else (documents, ROMs, already-correct files) is `keep`.
//!
//! Nothing is deleted here: this only writes `plan_actions`.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;

use crate::config::Config;
use crate::dates::album_target;
use crate::model::{Action, FileRow, PlannedAction};
use crate::quality;
use crate::store::Store;

#[derive(Debug, Default)]
pub struct PlanStats {
    pub keep: usize,
    pub keep_distinct: usize,
    pub delete: usize,
    pub upgrade: usize,
    pub import: usize,
    pub external_covered: usize,
    pub external_dup: usize,
    pub bytes_delete: u64,
    pub bytes_upgrade: u64,
    pub bytes_import: u64,
    pub bytes_external: u64,
}

impl PlanStats {
    fn count(&mut self, action: Action, bytes: u64) {
        match action {
            Action::Keep => self.keep += 1,
            Action::KeepDistinct => self.keep_distinct += 1,
            Action::Delete => {
                self.delete += 1;
                self.bytes_delete += bytes;
            }
            Action::Upgrade => {
                self.upgrade += 1;
                self.bytes_upgrade += bytes;
            }
            Action::Import => {
                self.import += 1;
                self.bytes_import += bytes;
            }
            Action::ExternalCovered => {
                self.external_covered += 1;
                self.bytes_external += bytes;
            }
            Action::ExternalDup => {
                self.external_dup += 1;
                self.bytes_external += bytes;
            }
        }
    }
}

pub fn build_plan(store: &mut Store, cfg: &Config) -> Result<PlanStats> {
    let album = cfg
        .album_root()
        .ok_or_else(|| anyhow::anyhow!("no album root in config"))?
        .label
        .clone();
    let album_dir = cfg.album.clone();
    let ratio = cfg.rules.material_upgrade_ratio;

    let files = store.all()?;
    let by_id: HashMap<i64, FileRow> = files.iter().map(|f| (f.id, f.clone())).collect();
    let mut actions: HashMap<i64, PlannedAction> = HashMap::new();

    let groups = store.groups_with_members()?;
    let mut grouped: std::collections::HashSet<i64> = std::collections::HashSet::new();

    for group in &groups {
        let members: Vec<&FileRow> = group
            .members
            .iter()
            .filter_map(|(id, _)| by_id.get(id))
            .collect();
        if members.len() < 2 {
            continue;
        }
        for m in &members {
            grouped.insert(m.id);
        }

        let mut album_members: Vec<&FileRow> = members
            .iter()
            .copied()
            .filter(|f| f.root == album)
            .collect();
        let mut sources: Vec<&FileRow> = members
            .iter()
            .copied()
            .filter(|f| f.root != album)
            .collect();

        if album_members.is_empty() {
            // No copy in the album yet: import the best, purge the rest.
            let best = quality::best_of(&members).expect("non-empty group");
            let target = album_target(
                &album_dir,
                best.capture_ts.as_deref(),
                best.id,
                &extension(best),
            );
            actions.insert(
                best.id,
                PlannedAction {
                    file_id: best.id,
                    action: Action::Import,
                    group_key: Some(group.key.clone()),
                    reference_id: None,
                    target_path: Some(target.clone()),
                    bytes: best.size,
                    reason: format!("unique best copy of a {}-member group", members.len()),
                },
            );
            for m in members.iter().filter(|m| m.id != best.id) {
                actions.insert(
                    m.id,
                    PlannedAction {
                        file_id: m.id,
                        action: Action::ExternalDup,
                        group_key: Some(group.key.clone()),
                        reference_id: Some(best.id),
                        target_path: None,
                        bytes: m.size,
                        reason: format!("duplicate of {} (best copy imported)", best.id),
                    },
                );
            }
            continue;
        }

        // Album copy exists: keep the best album member.
        let best_album = quality::best_of(&album_members).expect("non-empty album group");
        actions.insert(
            best_album.id,
            PlannedAction {
                file_id: best_album.id,
                action: Action::Keep,
                group_key: Some(group.key.clone()),
                reference_id: None,
                target_path: None,
                bytes: best_album.size,
                reason: "best album copy of its exact group".into(),
            },
        );
        for m in &album_members {
            if m.id != best_album.id {
                actions.insert(
                    m.id,
                    PlannedAction {
                        file_id: m.id,
                        action: Action::Delete,
                        group_key: Some(group.key.clone()),
                        reference_id: Some(best_album.id),
                        target_path: None,
                        bytes: m.size,
                        reason: format!("exact duplicate of {}", best_album.id),
                    },
                );
            }
        }
        album_members.clear();

        // Among source members, the best one may improve the album copy.
        let upgrade = sources
            .iter()
            .copied()
            .filter(|s| quality::materially_better(s, best_album, ratio))
            .max_by_key(|s| quality::score(s));

        if let Some(better) = upgrade {
            actions.insert(
                better.id,
                PlannedAction {
                    file_id: better.id,
                    action: Action::Upgrade,
                    group_key: Some(group.key.clone()),
                    reference_id: Some(best_album.id),
                    target_path: Some(best_album.path.clone()),
                    bytes: better.size,
                    reason: quality::explain(better, best_album, ratio),
                },
            );
            sources.retain(|s| s.id != better.id);
        }

        for m in &sources {
            actions.insert(
                m.id,
                PlannedAction {
                    file_id: m.id,
                    action: Action::ExternalCovered,
                    group_key: Some(group.key.clone()),
                    reference_id: Some(best_album.id),
                    target_path: None,
                    bytes: m.size,
                    reason: format!("exact duplicate of album copy {}", best_album.id),
                },
            );
        }
    }

    // Not part of any exact group: import media from source roots, keep the
    // rest in place.
    for f in &files {
        if grouped.contains(&f.id) {
            continue;
        }
        let action = if f.media.is_media() && f.root != album {
            let target = album_target(&album_dir, f.capture_ts.as_deref(), f.id, &extension(f));
            actions.insert(
                f.id,
                PlannedAction {
                    file_id: f.id,
                    action: Action::Import,
                    group_key: None,
                    reference_id: None,
                    target_path: Some(target),
                    bytes: f.size,
                    reason: "unique media not present in the album".into(),
                },
            );
            continue;
        } else {
            Action::Keep
        };
        actions.insert(
            f.id,
            PlannedAction {
                file_id: f.id,
                action,
                group_key: None,
                reference_id: None,
                target_path: None,
                bytes: f.size,
                reason: if f.media.is_media() {
                    "protected album copy".into()
                } else {
                    "not photo or video".into()
                },
            },
        );
    }

    let mut ordered: Vec<PlannedAction> = actions.into_values().collect();
    ordered.sort_by_key(|a| (a.action.as_str(), a.file_id));
    let mut stats = PlanStats::default();
    for a in &ordered {
        stats.count(a.action, a.bytes);
    }
    store.clear_plan()?;
    store.insert_plan(&ordered)?;
    Ok(stats)
}

fn extension(f: &FileRow) -> String {
    Path::new(&f.path)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_ascii_lowercase()))
        .unwrap_or_default()
}
