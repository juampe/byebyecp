//! Capture dates.
//!
//! Sources, in order:
//!
//! 1. the file's own Google Takeout sidecar (`photoTakenTime`);
//! 2. the sidecar of an **exact copy** in another root (metadata bridge);
//! 3. EXIF `DateTimeOriginal`;
//! 4. the file name (`2021-08-07 08.18.41`, `IMG_20210807_081841`, ...);
//! 5. the folder path (`2024/2024-05-01`).
//!
//! Sentinel dates (`1970-01-01`, `1980-01-01`) and configured bulk-import
//! folders are never used: they describe the export, not the capture.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{Local, NaiveDate, TimeZone};
use regex::Regex;
use serde::Deserialize;
use walkdir::WalkDir;

use crate::config::Config;
use crate::model::Sidecar;
use crate::store::Store;

#[derive(Debug, Default)]
pub struct DateStats {
    pub sidecar: usize,
    pub bridge: usize,
    pub exif: usize,
    pub filename: usize,
    pub folder: usize,
    pub none: usize,
    pub rejected: usize,
}

#[derive(Deserialize)]
struct RawSidecar {
    title: Option<String>,
    #[serde(rename = "photoTakenTime")]
    photo_taken: Option<RawTime>,
    #[serde(rename = "creationTime")]
    creation: Option<RawTime>,
    description: Option<String>,
    #[serde(rename = "googleId")]
    google_id: Option<String>,
    #[serde(rename = "googlePhotosOrigin")]
    _origin: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct RawTime {
    timestamp: Option<String>,
}

/// Read every Takeout JSON under the configured roots and match it to a media
/// file. Returns the number of sidecars stored.
pub fn import_sidecars(store: &mut Store, cfg: &Config) -> Result<usize> {
    let mut stored = 0;
    for root in &cfg.roots {
        if !root.path.exists() {
            continue;
        }
        let root_id = match store.root_id(&root.label) {
            Ok(id) => id,
            Err(_) => continue,
        };
        let files = store.by_root(&root.label)?;
        let mut by_path: HashMap<String, i64> = HashMap::new();
        let mut by_name: HashMap<String, i64> = HashMap::new();
        for f in &files {
            by_path.insert(f.path.to_string_lossy().to_string(), f.id);
            by_name
                .entry(
                    f.path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default(),
                )
                .or_insert(f.id);
        }

        let mut json_paths = Vec::new();
        for entry in WalkDir::new(&root.path)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .map(|e| e.eq_ignore_ascii_case("json"))
                    .unwrap_or(false)
            {
                json_paths.push(entry.path().to_path_buf());
            }
        }

        for path in json_paths {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            // Cheap check before paying for a full JSON parse.
            if !text.contains("photoTakenTime") && !text.contains("creationTime") {
                continue;
            }
            let Ok(raw) = serde_json::from_str::<RawSidecar>(&text) else {
                continue;
            };
            let taken = raw
                .photo_taken
                .as_ref()
                .and_then(|t| t.timestamp.as_deref())
                .and_then(ts_to_string);
            let creation = raw
                .creation
                .as_ref()
                .and_then(|t| t.timestamp.as_deref())
                .and_then(ts_to_string);
            if taken.is_none() && creation.is_none() {
                continue;
            }
            let matched = match_media(&path, &by_path, &by_name);
            let sidecar = Sidecar {
                id: 0,
                root: root.label.clone(),
                path: path.clone(),
                google_id: raw.google_id,
                title: raw.title,
                taken_ts: taken,
                creation_ts: creation,
                description: raw.description,
                matched_file_id: matched,
            };
            store.set_sidecar(&sidecar, root_id)?;
            if matched.is_some() {
                stored += 1;
            }
        }
    }
    Ok(stored)
}

/// `photo.jpg.json` and `photo.jpg.supplemental-metadata.json` both map to
/// `photo.jpg`. Old Takeouts truncated long names, so a unique prefix match is
/// accepted as a last resort.
fn match_media(
    sidecar: &Path,
    by_path: &HashMap<String, i64>,
    by_name: &HashMap<String, i64>,
) -> Option<i64> {
    let raw = sidecar.to_string_lossy();
    let without_json = raw.strip_suffix(".json").unwrap_or(&raw);
    let candidates = [
        without_json.to_string(),
        without_json
            .strip_suffix(".supplemental-metadata")
            .unwrap_or(without_json)
            .to_string(),
    ];
    for c in &candidates {
        if let Some(id) = by_path.get(c) {
            return Some(*id);
        }
    }
    for c in &candidates {
        let name = Path::new(c)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())?;
        if let Some(id) = by_name.get(&name) {
            return Some(*id);
        }
        // Truncated Takeout names: the media name starts with the sidecar
        // stem. Only accept a unique match.
        let stem = name.trim_end_matches(".json");
        let mut found = None;
        for (media_name, id) in by_name {
            if media_name.starts_with(stem) {
                if found.is_some() {
                    found = None;
                    break;
                }
                found = Some(*id);
            }
        }
        if let Some(id) = found {
            return Some(id);
        }
    }
    None
}

fn ts_to_string(ts: &str) -> Option<String> {
    let secs: i64 = ts.trim().parse().ok()?;
    let dt = Local.timestamp_opt(secs, 0).single()?;
    Some(dt.format("%Y-%m-%d %H:%M:%S").to_string())
}

/// Fill `capture_ts` for every media file that still lacks a valid date.
pub fn fill_dates(store: &mut Store, cfg: &Config) -> Result<DateStats> {
    let media = store.media()?;
    let mut by_file: HashMap<i64, String> = HashMap::new();
    for s in store.sidecars()? {
        let Some(fid) = s.matched_file_id else {
            continue;
        };
        if let Some(ts) = s.taken_ts.or(s.creation_ts) {
            if valid_date(&ts, cfg) {
                by_file.entry(fid).or_insert(ts);
            }
        }
    }

    // Bridge: an exact copy in another root knows the date.
    let mut peers: HashMap<i64, Vec<i64>> = HashMap::new();
    {
        let mut stmt = store.con.prepare(
            "SELECT a.file_id, b.file_id
             FROM group_members a JOIN group_members b ON a.group_id=b.group_id
             WHERE a.file_id<>b.file_id",
        )?;
        let it = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
        for row in it {
            let (a, b) = row?;
            peers.entry(a).or_default().push(b);
        }
    }

    let mut stats = DateStats::default();
    for f in media {
        if let Some(existing) = &f.capture_ts {
            if valid_date(existing, cfg) {
                continue;
            }
        }
        let own = by_file.get(&f.id).cloned();
        if let Some(ts) = own {
            store.set_date(f.id, &ts, "sidecar")?;
            stats.sidecar += 1;
            continue;
        }
        let bridge = peers
            .get(&f.id)
            .and_then(|list| list.iter().find_map(|p| by_file.get(p).cloned()));
        if let Some(ts) = bridge {
            store.set_date(f.id, &ts, "sidecar-copy")?;
            stats.bridge += 1;
            continue;
        }
        if let Some(ts) = exif_date(&f.path) {
            if valid_date(&ts, cfg) {
                store.set_date(f.id, &ts, "exif")?;
                stats.exif += 1;
                continue;
            }
            stats.rejected += 1;
        }
        if let Some(ts) = date_from_filename(&f.path) {
            if valid_date(&ts, cfg) {
                store.set_date(f.id, &ts, "filename")?;
                stats.filename += 1;
                continue;
            }
            stats.rejected += 1;
        }
        if let Some(ts) = date_from_relpath(&f.rel_path, cfg) {
            store.set_date(f.id, &ts, "folder")?;
            stats.folder += 1;
            continue;
        }
        stats.none += 1;
    }
    Ok(stats)
}

pub fn valid_date(ts: &str, cfg: &Config) -> bool {
    let day = &ts[..ts.len().min(10)];
    if cfg.rules.ignore_dates.iter().any(|d| day == d) {
        return false;
    }
    let year: i32 = match day.get(0..4).and_then(|y| y.parse().ok()) {
        Some(y) => y,
        None => return false,
    };
    let now = Local::now().date_naive().format("%Y").to_string();
    let max: i32 = now.parse().unwrap_or(2100);
    year >= 1900 && year <= max + 1
}

/// Read `DateTimeOriginal` (falling back to digitised/created) from EXIF.
pub fn exif_date(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut buf = std::io::BufReader::new(file);
    let exif = exif::Reader::new().read_from_container(&mut buf).ok()?;
    for tag in [
        exif::Tag::DateTimeOriginal,
        exif::Tag::DateTimeDigitized,
        exif::Tag::DateTime,
    ] {
        if let Some(field) = exif.get_field(tag, exif::In::PRIMARY) {
            let s = field.display_value().to_string();
            if let Some(ts) = exif_to_rfc(&s) {
                return Some(ts);
            }
        }
    }
    None
}

fn exif_to_rfc(s: &str) -> Option<String> {
    // 2021-08-07 08:18:41 or 2021:08:07 08:18:41
    let s = s.trim();
    let mut parts = s.split_whitespace();
    let date = parts.next()?;
    let time = parts.next()?;
    let date = date.replace(':', "-");
    let d = NaiveDate::parse_from_str(&date, "%Y-%m-%d").ok()?;
    let time = time.split('.').next().unwrap_or(time);
    Some(format!("{} {}", d.format("%Y-%m-%d"), time))
}

/// Extract `YYYY-MM-DD HH:MM:SS` from common camera and messenger names.
pub fn date_from_filename(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy().to_string();
    // 2021-08-07 08.18.41 / 2021-08-07_08-18-41 / 2021.08.07 08:18:41
    let re_full = Regex::new(
        r"(?P<y>\d{4})[-_.](?P<m>\d{2})[-_.](?P<d>\d{2})[ _T-]+(?P<H>\d{2})[.:-](?P<M>\d{2})[.:-](?P<S>\d{2})",
    )
    .ok()?;
    // IMG_20210807_081841 / PXL_20210807_081841123
    let re_compact =
        Regex::new(r"(?P<y>\d{4})(?P<m>\d{2})(?P<d>\d{2})[_-](?P<H>\d{2})(?P<M>\d{2})(?P<S>\d{2})")
            .ok()?;
    let re_date = Regex::new(r"(?P<y>\d{4})[-_.](?P<m>\d{2})[-_.](?P<d>\d{2})").ok()?;

    for re in [&re_full, &re_compact] {
        if let Some(c) = re.captures(&name) {
            if let Some(ts) = build(c) {
                return Some(ts);
            }
        }
    }
    if let Some(c) = re_date.captures(&name) {
        if let Some(ts) = build(c) {
            return Some(format!("{} 00:00:00", &ts[..10]));
        }
    }
    None
}

/// Look for `YYYY-MM-DD` (or `YYYY/YYYY-MM-DD`) in the relative path.
pub fn date_from_relpath(rel: &str, cfg: &Config) -> Option<String> {
    let parts: Vec<&str> = rel.split('/').collect();
    let re = Regex::new(r"^(?P<y>\d{4})-(?P<m>\d{2})-(?P<d>\d{2})$").ok()?;
    for part in &parts {
        if cfg.rules.import_dump_days.iter().any(|d| part == d) {
            continue;
        }
        if let Some(c) = re.captures(part) {
            if let Some(ts) = build(c) {
                return Some(format!("{} 12:00:00", &ts[..10]));
            }
        }
    }
    None
}

fn build(c: regex::Captures<'_>) -> Option<String> {
    let y: i32 = c.name("y")?.as_str().parse().ok()?;
    let m: u32 = c.name("m")?.as_str().parse().ok()?;
    let d: u32 = c.name("d")?.as_str().parse().ok()?;
    let date = NaiveDate::from_ymd_opt(y, m, d)?;
    let (h, mi, s) = match (c.name("H"), c.name("M"), c.name("S")) {
        (Some(h), Some(mi), Some(s)) => (
            h.as_str().parse::<u32>().ok()?,
            mi.as_str().parse::<u32>().ok()?,
            s.as_str().parse::<u32>().ok()?,
        ),
        _ => (0, 0, 0),
    };
    if h > 23 || mi > 59 || s > 60 {
        return None;
    }
    Some(format!(
        "{} {:02}:{:02}:{:02}",
        date.format("%Y-%m-%d"),
        h,
        mi,
        s
    ))
}

/// Year/month/day path used when importing into the album.
pub fn album_relative(ts: &str, filename: &str) -> PathBuf {
    let day = &ts[..10];
    let year = &ts[..4];
    PathBuf::from(year).join(day).join(filename)
}

/// Build a collision-free album target from a capture date or a fallback name.
pub fn album_target(album: &Path, ts: Option<&str>, id: i64, ext: &str) -> PathBuf {
    match ts {
        Some(ts) if ts.len() >= 19 => {
            let day = &ts[..10];
            let year = &ts[..4];
            let time = ts[11..19].replace(':', ".");
            let name = format!("{day}_{time}_{id}{ext}");
            album.join(year).join(day).join(name)
        }
        _ => album.join("_sin-fecha").join(format!("{id}{ext}")),
    }
}

/// Context for an error message.
pub fn hint_missing_dates(store: &Store) -> Result<usize> {
    Ok(store.con.query_row(
        "SELECT COUNT(*) FROM files WHERE media IN ('image','video')
             AND (capture_ts IS NULL OR capture_ts='')",
        [],
        |r| r.get::<_, i64>(0),
    )? as usize)
}

#[allow(dead_code)]
fn _assert_context<T>(r: Result<T>) -> Result<T> {
    r.context("fechas")
}
