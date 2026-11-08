//! Perceptual fingerprints and pixel verification.
//!
//! Three independent 64-bit hashes are stored for every image:
//!
//! * **pHash** — 2D DCT of a 32x32 grayscale image; robust to recompression
//!   and resizing.
//! * **dHash** — horizontal gradient of a 9x8 image.
//! * **aHash** — threshold against the mean of an 8x8 image.
//!
//! Requiring all three to match makes an accidental collision extremely
//! unlikely. Hashes with very few set bits (flat or black images) are
//! rejected later because they collide with everything.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use image::imageops::FilterType;
use image::{DynamicImage, GrayImage, ImageDecoder};

/// Grid used for the pHash DCT.
const DCT_SIZE: usize = 32;
/// Side of the low-frequency block kept from the DCT.
const HASH_SIZE: usize = 8;

#[derive(Debug, Clone)]
pub struct Fingerprint {
    pub phash: String,
    pub dhash: String,
    pub ahash: String,
    pub width: u32,
    pub height: u32,
}

/// Decode an image applying EXIF orientation. Falls back to ffmpeg for
/// formats the `image` crate cannot read (HEIC, some RAW files).
pub fn load_gray(path: &Path) -> Result<GrayImage> {
    match load_with_image_crate(path) {
        Ok(img) => Ok(img.to_luma8()),
        Err(primary) => match load_with_ffmpeg(path) {
            Ok(img) => Ok(img.to_luma8()),
            Err(fallback) => anyhow::bail!(
                "no se pudo decodificar {}: {} / {}",
                path.display(),
                primary,
                fallback
            ),
        },
    }
}

fn load_with_image_crate(path: &Path) -> Result<DynamicImage> {
    let reader = image::ImageReader::open(path)
        .with_context(|| format!("abriendo {}", path.display()))?
        .with_guessed_format()
        .context("detectando formato")?;
    let mut decoder = reader.into_decoder().context("creando decoder")?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).context("decodificando")?;
    img.apply_orientation(orientation);
    Ok(img)
}

fn load_with_ffmpeg(path: &Path) -> Result<DynamicImage> {
    let output = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-f", "image2pipe", "-vcodec", "png", "-"])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("ejecutando ffmpeg (¿está instalado?)")?;
    if !output.status.success() || output.stdout.is_empty() {
        let err = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("ffmpeg no pudo decodificar: {}", err.trim());
    }
    image::load_from_memory(&output.stdout).context("decodificando PNG de ffmpeg")
}

/// Compute all three hashes plus the source dimensions.
pub fn fingerprint(path: &Path) -> Result<Fingerprint> {
    let gray = load_gray(path)?;
    let (width, height) = gray.dimensions();
    Ok(Fingerprint {
        phash: format!("{:016x}", phash(&gray)),
        dhash: format!("{:016x}", dhash(&gray)),
        ahash: format!("{:016x}", ahash(&gray)),
        width,
        height,
    })
}

/// pHash of an already grayscale image.
#[allow(clippy::needless_range_loop)]
pub fn phash(img: &GrayImage) -> u64 {
    let small =
        image::imageops::resize(img, DCT_SIZE as u32, DCT_SIZE as u32, FilterType::Triangle);
    let m = dct_matrix();

    // Rows: tmp[y][u] = sum_x m[u][x] * pix[y][x]
    let mut tmp = [[0.0f64; DCT_SIZE]; DCT_SIZE];
    for y in 0..DCT_SIZE {
        for u in 0..DCT_SIZE {
            let mut sum = 0.0;
            for x in 0..DCT_SIZE {
                sum += m[u][x] * small.get_pixel(x as u32, y as u32).0[0] as f64;
            }
            tmp[y][u] = sum;
        }
    }

    // Columns: dct[v][u] = sum_y m[v][y] * tmp[y][u]
    let mut dct = [[0.0f64; DCT_SIZE]; DCT_SIZE];
    for v in 0..DCT_SIZE {
        for u in 0..DCT_SIZE {
            let mut sum = 0.0;
            for y in 0..DCT_SIZE {
                sum += m[v][y] * tmp[y][u];
            }
            dct[v][u] = sum;
        }
    }

    // Keep the low-frequency 8x8 block. The DC component carries brightness,
    // so it is zeroed before taking the median; every bit then describes
    // structure rather than exposure.
    let mut values = Vec::with_capacity(HASH_SIZE * HASH_SIZE);
    for v in 0..HASH_SIZE {
        for u in 0..HASH_SIZE {
            values.push(if u == 0 && v == 0 { 0.0 } else { dct[v][u] });
        }
    }
    let median = median(&values);
    let mut bits = 0u64;
    for (i, value) in values.iter().enumerate() {
        if *value > median {
            bits |= 1u64 << (63 - i);
        }
    }
    bits
}

/// dHash: 9x8 grayscale, one bit per horizontal neighbour comparison.
pub fn dhash(img: &GrayImage) -> u64 {
    let small = image::imageops::resize(img, 9, 8, FilterType::Triangle);
    let mut bits = 0u64;
    let mut i = 0;
    for y in 0..8 {
        for x in 0..8 {
            let left = small.get_pixel(x, y).0[0];
            let right = small.get_pixel(x + 1, y).0[0];
            if left > right {
                bits |= 1u64 << (63 - i);
            }
            i += 1;
        }
    }
    bits
}

/// aHash: 8x8 grayscale thresholded against its mean.
pub fn ahash(img: &GrayImage) -> u64 {
    let small = image::imageops::resize(img, 8, 8, FilterType::Triangle);
    let sum: u64 = small.pixels().map(|p| p.0[0] as u64).sum();
    let mean = sum as f64 / 64.0;
    let mut bits = 0u64;
    for (i, p) in small.pixels().enumerate() {
        if p.0[0] as f64 > mean {
            bits |= 1u64 << (63 - i);
        }
    }
    bits
}

#[allow(clippy::needless_range_loop)]
fn dct_matrix() -> [[f64; DCT_SIZE]; DCT_SIZE] {
    let mut m = [[0.0f64; DCT_SIZE]; DCT_SIZE];
    for k in 0..DCT_SIZE {
        let scale = if k == 0 {
            (1.0 / DCT_SIZE as f64).sqrt()
        } else {
            (2.0 / DCT_SIZE as f64).sqrt()
        };
        for n in 0..DCT_SIZE {
            m[k][n] = scale
                * (std::f64::consts::PI * (2 * n + 1) as f64 * k as f64 / (2.0 * DCT_SIZE as f64))
                    .cos();
        }
    }
    m
}

fn median(values: &[f64]) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[v.len() / 2]
}

/// Median absolute difference of two images resampled to 64x64 grayscale.
///
/// Reference values from real libraries: identical copies 0-0.5,
/// recompressed copies 0.5-1.2, different burst frames >= 2.7.
pub fn mad(a: &GrayImage, b: &GrayImage) -> f64 {
    const N: u32 = 64;
    let ra = image::imageops::resize(a, N, N, FilterType::Triangle);
    let rb = image::imageops::resize(b, N, N, FilterType::Triangle);
    let mut sum = 0.0f64;
    for (x, y) in ra.pixels().zip(rb.pixels()) {
        sum += (x.0[0] as f64 - y.0[0] as f64).abs();
    }
    sum / (N * N) as f64
}

pub fn popcount(hex: &str) -> u32 {
    u64::from_str_radix(hex, 16)
        .map(|v| v.count_ones())
        .unwrap_or(0)
}

pub fn distance_hex(a: &str, b: &str) -> u32 {
    match (u64::from_str_radix(a, 16), u64::from_str_radix(b, 16)) {
        (Ok(x), Ok(y)) => (x ^ y).count_ones(),
        _ => 64,
    }
}

/// Extract one frame at `seconds` and return it as 64x64 grayscale.
pub fn video_frame(path: &Path, seconds: f64) -> Result<GrayImage> {
    let output = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-v",
            "error",
            "-ss",
            &format!("{seconds:.3}"),
            "-i",
        ])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale=64:64",
            "-f",
            "image2pipe",
            "-vcodec",
            "png",
            "-",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("ejecutando ffmpeg")?;
    if !output.status.success() || output.stdout.is_empty() {
        anyhow::bail!(
            "sin fotograma: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let img = image::load_from_memory(&output.stdout)?;
    Ok(image::imageops::resize(
        &img.to_luma8(),
        64,
        64,
        FilterType::Triangle,
    ))
}

/// Run ffmpeg with stdin closed; helper kept for future transcoding commands.
pub fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Write bytes to a child process (used by tests and future exporters).
pub fn write_all(mut child_stdin: impl Write, data: &[u8]) -> std::io::Result<()> {
    child_stdin.write_all(data)?;
    child_stdin.flush()
}
