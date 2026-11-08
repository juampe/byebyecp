//! Configuration loaded from `bbcp.toml`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Absolute or config-relative path of the consolidated album.
    pub album: PathBuf,
    /// Where the inventory and every reversible artefact live.
    #[serde(default)]
    pub paths: Paths,
    /// Roots to inventory. At least one must have `kind = "album"`.
    #[serde(default)]
    pub roots: Vec<Root>,
    #[serde(default)]
    pub rules: Rules,
    #[serde(default)]
    pub google: Google,
    /// Directory that receives the `.json` archives before external media is
    /// purged, so Google ids and metadata survive the cleanup.
    #[serde(default = "default_metadata_archive")]
    pub metadata_archive: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Paths {
    #[serde(default = "default_db")]
    pub db: PathBuf,
    #[serde(default = "default_quarantine")]
    pub quarantine: PathBuf,
    #[serde(default = "default_staging")]
    pub staging: PathBuf,
    #[serde(default = "default_logs")]
    pub logs: PathBuf,
    #[serde(default = "default_manifests")]
    pub manifests: PathBuf,
}

impl Default for Paths {
    fn default() -> Self {
        Self {
            db: default_db(),
            quarantine: default_quarantine(),
            staging: default_staging(),
            logs: default_logs(),
            manifests: default_manifests(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Root {
    pub label: String,
    pub path: PathBuf,
    /// `album` (protected, never purged), `source` (Takeout, old dumps) or
    /// `quarantine` (digiKam `.dtrash`, recycle bins).
    #[serde(default = "default_kind")]
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rules {
    /// Reject perceptual hashes with fewer set bits: flat images collide.
    #[serde(default = "default_popcount")]
    pub min_popcount: u32,
    /// Maximum `max_aspect / min_aspect` inside an exact group.
    #[serde(default = "default_aspect")]
    pub aspect_tolerance: f64,
    /// Mean absolute difference (0-255) that still counts as the same image.
    #[serde(default = "default_mad")]
    pub mad_threshold: f64,
    /// Size or bitrate ratio that makes a copy "materially better".
    #[serde(default = "default_upgrade_ratio")]
    pub material_upgrade_ratio: f64,
    /// Sum of pHash+dHash distance that identifies the same video frame.
    #[serde(default = "default_frame_distance")]
    pub frame_distance: u32,
    /// Allowed duration difference when pairing re-encoded videos.
    #[serde(default = "default_duration_tolerance")]
    pub duration_tolerance: f64,
    /// Sentinel dates that mean "unknown", never a real capture date.
    #[serde(default = "default_ignore_dates")]
    pub ignore_dates: Vec<String>,
    /// Folder names (YYYY-MM-DD) that are bulk imports, not capture dates.
    #[serde(default = "default_dump_days")]
    pub import_dump_days: Vec<String>,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            min_popcount: default_popcount(),
            aspect_tolerance: default_aspect(),
            mad_threshold: default_mad(),
            material_upgrade_ratio: default_upgrade_ratio(),
            frame_distance: default_frame_distance(),
            duration_tolerance: default_duration_tolerance(),
            ignore_dates: default_ignore_dates(),
            import_dump_days: default_dump_days(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Google {
    /// Items per `google_batches/batch_NNN.tsv` file.
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
}

impl Default for Google {
    fn default() -> Self {
        Self {
            batch_size: default_batch_size(),
        }
    }
}

fn default_db() -> PathBuf {
    PathBuf::from(".bbcp/bbcp.db")
}
fn default_quarantine() -> PathBuf {
    PathBuf::from(".bbcp/quarantine")
}
fn default_staging() -> PathBuf {
    PathBuf::from(".bbcp/staging")
}
fn default_logs() -> PathBuf {
    PathBuf::from(".bbcp/logs")
}
fn default_manifests() -> PathBuf {
    PathBuf::from(".bbcp/manifests")
}
fn default_metadata_archive() -> PathBuf {
    PathBuf::from(".bbcp/metadata-archive")
}
fn default_kind() -> String {
    "source".into()
}
fn default_popcount() -> u32 {
    12
}
fn default_aspect() -> f64 {
    1.05
}
fn default_mad() -> f64 {
    1.5
}
fn default_upgrade_ratio() -> f64 {
    1.25
}
fn default_frame_distance() -> u32 {
    16
}
fn default_duration_tolerance() -> f64 {
    0.5
}
fn default_ignore_dates() -> Vec<String> {
    vec!["1970-01-01".into(), "1980-01-01".into()]
}
fn default_dump_days() -> Vec<String> {
    Vec::new()
}
fn default_batch_size() -> usize {
    1000
}

impl Config {
    /// Load a config file and resolve every relative path against its folder.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("leyendo config {}", path.display()))?;
        let mut cfg: Config =
            toml::from_str(&text).with_context(|| format!("parseando {}", path.display()))?;
        let base = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        cfg.album = resolve(&base, &cfg.album);
        cfg.paths.db = resolve(&base, &cfg.paths.db);
        cfg.paths.quarantine = resolve(&base, &cfg.paths.quarantine);
        cfg.paths.staging = resolve(&base, &cfg.paths.staging);
        cfg.paths.logs = resolve(&base, &cfg.paths.logs);
        cfg.paths.manifests = resolve(&base, &cfg.paths.manifests);
        cfg.metadata_archive = resolve(&base, &cfg.metadata_archive);
        for root in &mut cfg.roots {
            root.path = resolve(&base, &root.path);
        }
        Ok(cfg)
    }

    pub fn album_root(&self) -> Option<&Root> {
        self.roots.iter().find(|r| r.kind == "album")
    }

    pub fn root(&self, label: &str) -> Option<&Root> {
        self.roots.iter().find(|r| r.label == label)
    }

    /// Fail early with a readable message instead of a confusing error later.
    pub fn validate(&self) -> Result<()> {
        if self.album_root().is_none() {
            anyhow::bail!("la configuración necesita una raíz con kind = \"album\"");
        }
        for r in &self.roots {
            if !matches!(r.kind.as_str(), "album" | "source" | "quarantine") {
                anyhow::bail!(
                    "raíz {}: kind desconocido {:?} (usa album, source o quarantine)",
                    r.label,
                    r.kind
                );
            }
        }
        Ok(())
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        for dir in [
            self.paths.quarantine.clone(),
            self.paths.staging.clone(),
            self.paths.logs.clone(),
            self.paths.manifests.clone(),
            self.metadata_archive.clone(),
        ] {
            std::fs::create_dir_all(&dir).with_context(|| format!("creando {}", dir.display()))?;
        }
        if let Some(parent) = self.paths.db.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creando {}", parent.display()))?;
        }
        Ok(())
    }
}

fn resolve(base: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

/// Write a commented starter configuration.
pub fn example_config(album: &Path) -> String {
    format!(
        r#"# Bye Bye Cloud Photos — configuración
# Todas las rutas relativas se resuelven contra la carpeta de este archivo.

album = "{album}"

# Los JSON de Takeout se archivan aquí antes de purgar una raíz externa.
metadata_archive = ".bbcp/metadata-archive"

[paths]
db = ".bbcp/bbcp.db"
quarantine = ".bbcp/quarantine"
staging = ".bbcp/staging"
logs = ".bbcp/logs"
manifests = ".bbcp/manifests"

[[roots]]
label = "album"
path = "{album}"
kind = "album"

# Añade una entrada por cada export de Google Takeout o carpeta antigua.
# [[roots]]
# label = "takeout-2024"
# path = "/ruta/al/Takeout/Google Fotos"
# kind = "source"

[rules]
min_popcount = 12
aspect_tolerance = 1.05
mad_threshold = 1.5
material_upgrade_ratio = 1.25
frame_distance = 16
duration_tolerance = 0.5
ignore_dates = ["1970-01-01", "1980-01-01"]
# Carpetas de volcado masivo que no representan la fecha de captura.
import_dump_days = []

[google]
batch_size = 1000
"#,
        album = album.display()
    )
}
