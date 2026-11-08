//! Quality ranking. The album is not assumed to be the best copy: the
//! ranking is purely evidence-based.

use crate::model::FileRow;

/// Rank used to pick the copy that survives.
///
/// 1. more pixels (resolution);
/// 2. more bits per second (video) or more bytes (images, less compressed);
/// 3. older modification time (derivatives are written later);
/// 4. larger file as a final tie-breaker.
pub fn score(f: &FileRow) -> (u64, u64, i64, u64) {
    let pixels = f.pixels();
    let rich = f.bitrate.unwrap_or(0).max(f.size);
    let older = -f.mtime;
    (pixels, rich, older, f.size)
}

pub fn best(files: &[FileRow]) -> Option<&FileRow> {
    files.iter().max_by_key(|f| score(f))
}

/// Rank a slice of references (groups filtered by root).
pub fn best_of<'a>(files: &[&'a FileRow]) -> Option<&'a FileRow> {
    files.iter().copied().max_by_key(|f| score(f))
}

/// True when `candidate` is better enough to justify replacing `current`.
pub fn materially_better(candidate: &FileRow, current: &FileRow, ratio: f64) -> bool {
    if candidate.pixels() != current.pixels() {
        return candidate.pixels() > current.pixels();
    }
    let a = candidate.bitrate.unwrap_or(0).max(candidate.size) as f64;
    let b = current.bitrate.unwrap_or(0).max(current.size) as f64;
    if b <= 0.0 {
        return a > 0.0;
    }
    a > b * ratio
}

/// Human explanation for the ranking, copied into manifests.
pub fn explain(candidate: &FileRow, current: &FileRow, ratio: f64) -> String {
    if candidate.pixels() > current.pixels() {
        return format!(
            "más resolución: {}x{} > {}x{}",
            candidate.width.unwrap_or(0),
            candidate.height.unwrap_or(0),
            current.width.unwrap_or(0),
            current.height.unwrap_or(0)
        );
    }
    let a = candidate.bitrate.unwrap_or(0).max(candidate.size);
    let b = current.bitrate.unwrap_or(0).max(current.size);
    if b > 0 && a as f64 > b as f64 * ratio {
        return format!("misma resolución y {}x más datos ({a} vs {b})", ratio);
    }
    "mejor puntuación global".into()
}
