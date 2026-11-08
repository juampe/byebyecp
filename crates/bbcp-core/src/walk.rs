//! Filesystem discovery.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use walkdir::WalkDir;

use crate::config::Root;
use crate::model::{classify_extension, MediaKind};
use crate::store::ScanEntry;

/// Directories that never contain user media and only slow the walk down.
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".bbcp",
    "node_modules",
    "@eaDir",
    "#recycle",
    "$RECYCLE.BIN",
    "System Volume Information",
    ".Trash-1000",
];

/// Files that are noise in a photo library.
fn is_junk(name: &str) -> bool {
    name.starts_with("._")
        || matches!(
            name,
            ".DS_Store" | "Thumbs.db" | "thumbs.db" | "desktop.ini" | "Desktop.ini"
        )
}

#[derive(Debug, Default)]
pub struct ScanResult {
    pub entries: Vec<ScanEntry>,
    pub errors: Vec<String>,
}

/// Walk a root and return every regular file that looks like media or that we
/// might need for sidecars and documents.
pub fn scan_root(root: &Root) -> Result<ScanResult> {
    let base = &root.path;
    if !base.exists() {
        anyhow::bail!("la raíz {} no existe: {}", root.label, base.display());
    }
    let mut out = ScanResult::default();
    let walker = WalkDir::new(base)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if e.file_type().is_dir() {
                let name = e.file_name().to_string_lossy();
                if e.depth() > 0 && SKIP_DIRS.iter().any(|d| d.eq_ignore_ascii_case(&name)) {
                    return false;
                }
            }
            true
        });

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                out.errors.push(format!("{err}"));
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if is_junk(&name) {
            continue;
        }
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let media = classify_extension(&ext);
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(err) => {
                out.errors.push(format!("{}: {err}", path.display()));
                continue;
            }
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let rel_path = path
            .strip_prefix(base)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        out.entries.push(ScanEntry {
            path: path.to_path_buf(),
            rel_path,
            media,
            size: meta.len(),
            mtime,
        });
    }
    Ok(out)
}

/// Resolve a root-relative path without escaping the root.
pub fn safe_join(base: &Path, rel: &str) -> PathBuf {
    let mut out = base.to_path_buf();
    for part in rel.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            continue;
        }
        out.push(part);
    }
    out
}

/// Keep only paths that exist, for pruning after a scan.
pub fn existing_paths(entries: &[ScanEntry]) -> HashSet<PathBuf> {
    entries.iter().map(|e| e.path.clone()).collect()
}

/// Human-readable media counts for a scan result.
pub fn summarise(entries: &[ScanEntry]) -> (usize, usize, usize, u64) {
    let mut images = 0;
    let mut videos = 0;
    let mut other = 0;
    let mut bytes = 0;
    for e in entries {
        match e.media {
            MediaKind::Image => images += 1,
            MediaKind::Video => videos += 1,
            MediaKind::Other => other += 1,
        }
        bytes += e.size;
    }
    (images, videos, other, bytes)
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.2} {}", UNITS[i])
    }
}

pub fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Fail if two roots nest inside each other, which would make the same file
/// appear under two labels and confuse every later stage.
pub fn check_overlap(roots: &[Root]) -> Result<()> {
    for (i, a) in roots.iter().enumerate() {
        for b in roots.iter().skip(i + 1) {
            let pa = canonical(&a.path);
            let pb = canonical(&b.path);
            if pa == pb || pa.starts_with(&pb) || pb.starts_with(&pa) {
                anyhow::bail!(
                    "las raíces {} y {} se solapan ({} / {})",
                    a.label,
                    b.label,
                    pa.display(),
                    pb.display()
                );
            }
        }
    }
    Ok(())
}

/// Count files quickly for a preflight message.
pub fn count_files(path: &Path) -> usize {
    WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .count()
}

pub fn ensure_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).with_context(|| format!("creando directorio {}", path.display()))
}
