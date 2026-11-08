//! Video metadata extraction through ffprobe.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Default)]
pub struct VideoMeta {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration: Option<f64>,
    pub bitrate: Option<u64>,
    pub codec: Option<String>,
}

#[derive(Deserialize)]
struct Probe {
    #[serde(default)]
    streams: Vec<Stream>,
    format: Option<Format>,
}

#[derive(Deserialize)]
struct Stream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    duration: Option<String>,
    bit_rate: Option<String>,
    tags: Option<Tags>,
}

#[derive(Deserialize)]
struct Tags {
    #[serde(default)]
    rotate: Option<String>,
}

#[derive(Deserialize)]
struct Format {
    duration: Option<String>,
    bit_rate: Option<String>,
}

/// Probe one video. Rotation metadata is applied to width/height so a phone
/// video and its rotated copy compare equal.
pub fn probe(path: &Path) -> Result<VideoMeta> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("ejecutando ffprobe (¿está instalado?)")?;
    if !output.status.success() {
        anyhow::bail!(
            "ffprobe falló: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let probe: Probe =
        serde_json::from_slice(&output.stdout).context("parseando JSON de ffprobe")?;
    let stream = probe
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"))
        .context("el archivo no tiene pista de vídeo")?;

    let mut width = stream.width;
    let mut height = stream.height;
    // A 90/270 degree rotation swaps the visible dimensions.
    if let Some(tags) = &stream.tags {
        if let Some(rotate) = tags
            .rotate
            .as_deref()
            .and_then(|r| r.trim().parse::<i32>().ok())
        {
            if rotate.rem_euclid(180) == 90 {
                std::mem::swap(&mut width, &mut height);
            }
        }
    }

    let duration = stream
        .duration
        .as_deref()
        .and_then(|d| d.parse::<f64>().ok())
        .or_else(|| {
            probe
                .format
                .as_ref()
                .and_then(|f| f.duration.as_deref())
                .and_then(|d| d.parse::<f64>().ok())
        })
        .filter(|d| d.is_finite() && *d > 0.0);

    let bitrate = stream
        .bit_rate
        .as_deref()
        .and_then(|b| b.parse::<u64>().ok())
        .or_else(|| {
            probe
                .format
                .as_ref()
                .and_then(|f| f.bit_rate.as_deref())
                .and_then(|b| b.parse::<u64>().ok())
        });

    Ok(VideoMeta {
        width,
        height,
        duration,
        bitrate,
        codec: stream.codec_name.clone(),
    })
}

pub fn ffprobe_available() -> bool {
    Command::new("ffprobe")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
