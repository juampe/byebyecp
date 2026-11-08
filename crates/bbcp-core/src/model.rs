//! Core data types shared by every stage.

use std::path::PathBuf;

/// What a file is, judged by its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Image,
    Video,
    Other,
}

impl MediaKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MediaKind::Image => "image",
            MediaKind::Video => "video",
            MediaKind::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "image" => MediaKind::Image,
            "video" => MediaKind::Video,
            _ => MediaKind::Other,
        }
    }

    pub fn is_media(self) -> bool {
        matches!(self, MediaKind::Image | MediaKind::Video)
    }
}

/// Image extensions we try to fingerprint. RAW formats are decoded through
/// ffmpeg when the `image` crate cannot open them.
pub const IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "bmp", "webp", "tif", "tiff", "heic", "heif", "avif", "jxl",
    "dng", "cr2", "cr3", "nef", "arw", "raf", "orf", "rw2", "pef", "srw",
];

/// Video extensions handled by ffprobe/ffmpeg.
pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "m4v", "mov", "avi", "mkv", "webm", "3gp", "3g2", "mts", "m2ts", "ts", "wmv", "mpg",
    "mpeg", "flv", "ogv", "vob",
];

pub fn classify_extension(ext: &str) -> MediaKind {
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    if IMAGE_EXTS.contains(&ext.as_str()) {
        MediaKind::Image
    } else if VIDEO_EXTS.contains(&ext.as_str()) {
        MediaKind::Video
    } else {
        MediaKind::Other
    }
}

/// Planned action for one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Keep the file where it is.
    Keep,
    /// Keep it even though it looked like a duplicate (verification failed).
    KeepDistinct,
    /// Album duplicate: move to quarantine.
    Delete,
    /// Better copy found in a source root: copy over the album copy.
    Upgrade,
    /// New media from a source root: copy into the album.
    Import,
    /// Source duplicate of a protected album copy: safe to purge.
    ExternalCovered,
    /// Duplicate inside a source root: safe to purge after the best is imported.
    ExternalDup,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Keep => "keep",
            Action::KeepDistinct => "keep_distinct",
            Action::Delete => "delete",
            Action::Upgrade => "upgrade",
            Action::Import => "import",
            Action::ExternalCovered => "external_covered",
            Action::ExternalDup => "external_dup",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "keep" => Action::Keep,
            "keep_distinct" => Action::KeepDistinct,
            "delete" => Action::Delete,
            "upgrade" => Action::Upgrade,
            "import" => Action::Import,
            "external_covered" => Action::ExternalCovered,
            "external_dup" => Action::ExternalDup,
            _ => return None,
        })
    }
}

/// One row of the inventory.
#[derive(Debug, Clone)]
pub struct FileRow {
    pub id: i64,
    pub root: String,
    pub path: PathBuf,
    pub rel_path: String,
    pub media: MediaKind,
    pub size: u64,
    pub mtime: i64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration: Option<f64>,
    pub bitrate: Option<u64>,
    pub codec: Option<String>,
    pub sha256: Option<String>,
    pub phash: Option<String>,
    pub dhash: Option<String>,
    pub ahash: Option<String>,
    pub capture_ts: Option<String>,
    pub capture_source: Option<String>,
    pub fingerprint_status: Option<String>,
    pub meta_status: Option<String>,
    pub error: Option<String>,
}

impl FileRow {
    pub fn pixels(&self) -> u64 {
        self.width.unwrap_or(0) as u64 * self.height.unwrap_or(0) as u64
    }

    pub fn aspect(&self) -> f64 {
        match (self.width, self.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => {
                let (lo, hi) = if w < h { (w, h) } else { (h, w) };
                hi as f64 / lo as f64
            }
            _ => 1.0,
        }
    }

    pub fn has_hashes(&self) -> bool {
        self.phash.is_some() && self.dhash.is_some() && self.ahash.is_some()
    }
}

/// A parsed Google Takeout sidecar.
#[derive(Debug, Clone, Default)]
pub struct Sidecar {
    pub id: i64,
    pub root: String,
    pub path: PathBuf,
    pub google_id: Option<String>,
    pub title: Option<String>,
    pub taken_ts: Option<String>,
    pub creation_ts: Option<String>,
    pub description: Option<String>,
    pub matched_file_id: Option<i64>,
}

/// A planned action plus the evidence behind it.
#[derive(Debug, Clone)]
pub struct PlannedAction {
    pub file_id: i64,
    pub action: Action,
    pub group_key: Option<String>,
    pub reference_id: Option<i64>,
    pub target_path: Option<PathBuf>,
    pub bytes: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    ImageExact,
    VideoExact,
}

impl GroupKind {
    pub fn as_str(self) -> &'static str {
        match self {
            GroupKind::ImageExact => "image-exact",
            GroupKind::VideoExact => "video-exact",
        }
    }
}
