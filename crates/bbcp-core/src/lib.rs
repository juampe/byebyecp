//! # bbcp-core — engine of **Bye Bye Cloud Photos**
//!
//! A safe, reversible pipeline to consolidate a cloud photo library:
//!
//! 1. `walk` — inventory every root into SQLite;
//! 2. `video` / `imagehash` — probe videos and fingerprint images;
//! 3. `hash` — SHA-256 of candidate videos;
//! 4. `group` — exact duplicate groups (no transitive closure);
//! 5. `dates` — recover capture dates from sidecars, EXIF, names and folders;
//! 6. `plan` — decide keep / delete / upgrade / import per file;
//! 7. `verify` — pixel MAD check before deleting images;
//! 8. `apply` — atomic copies, quarantine, resumable journal;
//! 9. `stage` — park unique files for human review;
//! 10. `google` — deletion batches with a verified local reference;
//! 11. `report` — Markdown summary.
//!
//! The design favours being slow and provable over being fast and
//! irreversible: every destructive step has a separate, explicit switch.

pub mod apply;
pub mod config;
pub mod dates;
pub mod google;
pub mod group;
pub mod hash;
pub mod imagehash;
pub mod model;
pub mod plan;
pub mod quality;
pub mod report;
pub mod stage;
pub mod store;
pub mod verify;
pub mod video;
pub mod walk;

pub use config::{Config, Rules};
pub use model::{Action, FileRow, GroupKind, MediaKind};
pub use store::Store;

use anyhow::Result;

/// Environment facts an agent should check before starting.
#[derive(Debug, Default)]
pub struct DoctorReport {
    pub ffmpeg: bool,
    pub ffprobe: bool,
    pub sqlite: String,
    pub album_exists: bool,
    pub album_writable: bool,
    pub free_bytes: Option<u64>,
    pub roots_ok: Vec<(String, bool, usize)>,
}

pub fn doctor(cfg: &Config) -> Result<DoctorReport> {
    let mut report = DoctorReport {
        ffmpeg: imagehash::ffmpeg_available(),
        ffprobe: video::ffprobe_available(),
        sqlite: rusqlite::version().to_string(),
        ..Default::default()
    };
    report.album_exists = cfg.album.exists();
    report.album_writable = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(cfg.album.join(".bbcp-write-test"))
        .and_then(|f| {
            drop(f);
            std::fs::remove_file(cfg.album.join(".bbcp-write-test"))
        })
        .is_ok();
    for root in &cfg.roots {
        let exists = root.path.exists();
        let count = if exists {
            walk::count_files(&root.path)
        } else {
            0
        };
        report.roots_ok.push((root.label.clone(), exists, count));
    }
    Ok(report)
}
