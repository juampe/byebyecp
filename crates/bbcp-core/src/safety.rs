//! Backups: the only guarantee that survives a mistake.
//!
//! `bbcp` verifies copies and quarantines duplicates, but quarantine is not a
//! backup: it lives next to the album and a wrong command or a failing disk can
//! take both away. Every destructive run therefore requires at least
//! [`MIN_BACKUPS`] independent backup locations, declared in `[safety]`.

use std::path::{Path, PathBuf};

use crate::config::Config;

/// Independent copies required before any destructive `apply`.
pub const MIN_BACKUPS: usize = 2;

/// Result of checking the configured backup locations.
#[derive(Debug, Default, Clone)]
pub struct BackupReport {
    /// Number of `[safety] backups` entries.
    pub configured: usize,
    /// Entries that exist, are outside the album and live on another device.
    pub usable: usize,
    /// Human-readable reasons why an entry does not count.
    pub problems: Vec<String>,
}

impl BackupReport {
    /// True when at least [`MIN_BACKUPS`] independent backups are usable.
    pub fn ok(&self) -> bool {
        self.usable >= MIN_BACKUPS && self.problems.is_empty()
    }

    /// Multi-line explanation for CLI errors and `doctor`.
    pub fn describe(&self) -> String {
        let mut text = format!(
            "copias de seguridad: {}/{} utilizables",
            self.usable, MIN_BACKUPS
        );
        for problem in &self.problems {
            text.push_str("\n  - ");
            text.push_str(problem);
        }
        text
    }
}

/// Validate the backup entries in `[safety]`.
///
/// A valid backup is a directory that exists, is not inside the album (and does
/// not contain it) and, on Unix, lives on a different filesystem than the
/// album. Anything else is reported in [`BackupReport::problems`].
pub fn check_backups(cfg: &Config) -> BackupReport {
    let mut report = BackupReport {
        configured: cfg.safety.backups.len(),
        ..BackupReport::default()
    };
    if report.configured < MIN_BACKUPS {
        report.problems.push(format!(
            "hay {} copia(s) configurada(s); se exigen al menos {}",
            report.configured, MIN_BACKUPS
        ));
    }

    let album = normalize(&cfg.album);
    let album_device = device(&album);
    let mut seen: Vec<PathBuf> = Vec::new();

    for raw in &cfg.safety.backups {
        let path = normalize(raw);
        if seen.contains(&path) {
            report.problems.push(format!(
                "{}: ruta duplicada; dos entradas no son dos copias",
                raw.display()
            ));
            continue;
        }
        seen.push(path.clone());

        if !path.is_dir() {
            report.problems.push(format!(
                "{}: no existe o no es un directorio",
                raw.display()
            ));
            continue;
        }
        if path.starts_with(&album) || album.starts_with(&path) {
            report.problems.push(format!(
                "{}: está dentro del álbum o lo contiene; no es una copia independiente",
                raw.display()
            ));
            continue;
        }
        #[cfg(unix)]
        if let (Some(album_device), Some(backup_device)) = (album_device, device(&path)) {
            if album_device == backup_device {
                report.problems.push(format!(
                    "{}: está en el mismo sistema de archivos que el álbum",
                    raw.display()
                ));
                continue;
            }
        }

        report.usable += 1;
    }

    report
}

/// Abort a destructive operation unless two independent backups are available,
/// or the user explicitly accepted the risk with `allow_without_backups`.
pub fn require_backups(cfg: &Config) -> anyhow::Result<BackupReport> {
    let report = check_backups(cfg);
    if report.ok() {
        return Ok(report);
    }
    if cfg.safety.allow_without_backups {
        eprintln!("AVISO: {}", report.describe());
        eprintln!("AVISO: allow_without_backups = true. Si algo falla, no hay segunda copia.");
        return Ok(report);
    }
    anyhow::bail!(
        "se exigen al menos {MIN_BACKUPS} copias de seguridad independientes antes de \
         modificar o mover archivos.\n{}\n\
         Declara dos rutas que ya contengan una copia en [safety] backups = \
         [\"/ruta/copia-1\", \"/ruta/copia-2\"].\n\
         Deben existir, estar fuera del álbum y en otro disco.\n\
         Si de verdad no tienes copias, pon allow_without_backups = true en [safety]: \
         la responsabilidad es exclusivamente tuya.",
        report.describe()
    )
}

fn normalize(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(unix)]
fn device(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|meta| meta.dev())
}

#[cfg(not(unix))]
fn device(_path: &Path) -> Option<u64> {
    None
}
