//! Unit and integration tests that do not need ffmpeg or a real photo library.

use std::path::{Path, PathBuf};

use bbcp_core::config::{example_config, Config};
use bbcp_core::dates;
use bbcp_core::imagehash;
use bbcp_core::model::{FileRow, MediaKind};
use bbcp_core::quality;
use bbcp_core::store::{ScanEntry, Store};
use bbcp_core::verify;

fn tmpdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bbcp-test-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn config(dir: &Path) -> Config {
    let album = dir.join("album");
    let path = dir.join("bbcp.toml");
    std::fs::write(&path, example_config(&album)).unwrap();
    Config::load(&path).unwrap()
}

fn row(id: i64, width: u32, height: u32, size: u64, mtime: i64) -> FileRow {
    FileRow {
        id,
        root: "source".into(),
        path: PathBuf::from(format!("/tmp/{id}.jpg")),
        rel_path: format!("{id}.jpg"),
        media: MediaKind::Image,
        size,
        mtime,
        width: Some(width),
        height: Some(height),
        duration: None,
        bitrate: None,
        codec: None,
        sha256: None,
        phash: None,
        dhash: None,
        ahash: None,
        capture_ts: None,
        capture_source: None,
        fingerprint_status: None,
        meta_status: None,
        error: None,
    }
}

#[test]
fn filename_dates_are_parsed() {
    let cases = [
        ("2021-08-07 08.18.41.jpg", "2021-08-07 08:18:41"),
        ("2021-08-07_08-18-41.png", "2021-08-07 08:18:41"),
        ("IMG_20210807_081841.jpg", "2021-08-07 08:18:41"),
        ("PXL_20210807_081841123.mp4", "2021-08-07 08:18:41"),
        ("2021.08.07.jpg", "2021-08-07 00:00:00"),
    ];
    for (name, expected) in cases {
        assert_eq!(
            dates::date_from_filename(Path::new(name)).as_deref(),
            Some(expected),
            "{name}"
        );
    }
    assert!(dates::date_from_filename(Path::new("random.jpg")).is_none());
}

#[test]
fn sentinel_dates_are_rejected() {
    let dir = tmpdir("dates");
    let cfg = config(&dir);
    assert!(!dates::valid_date("1970-01-01 00:00:00", &cfg));
    assert!(!dates::valid_date("1980-01-01 00:00:00", &cfg));
    assert!(dates::valid_date("2021-08-07 08:18:41", &cfg));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn folder_dates_skip_import_dumps() {
    let dir = tmpdir("folder");
    let mut cfg = config(&dir);
    cfg.rules.import_dump_days.push("2026-01-05".into());
    assert_eq!(
        dates::date_from_relpath("2026/2026-01-05/x.jpg", &cfg).as_deref(),
        None
    );
    assert_eq!(
        dates::date_from_relpath("2024/2024-05-01/x.jpg", &cfg).as_deref(),
        Some("2024-05-01 12:00:00")
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn perceptual_hashes_are_stable_and_distinct() {
    use image::{GrayImage, Luma};
    let a = GrayImage::from_fn(64, 64, |x, y| {
        Luma([if (x / 8 + y / 8) % 2 == 0 { 220 } else { 20 }])
    });
    let b = GrayImage::from_fn(64, 64, |x, y| {
        Luma([if (x / 4 + y / 16) % 3 == 0 { 200 } else { 40 }])
    });
    assert_eq!(imagehash::phash(&a), imagehash::phash(&a));
    assert_ne!(imagehash::phash(&a), imagehash::phash(&b));
    assert!(imagehash::popcount(&format!("{:016x}", imagehash::phash(&a))) > 8);
    assert_eq!(
        imagehash::distance_hex("ffffffffffffffff", "ffffffffffffffff"),
        0
    );
    assert_eq!(
        imagehash::distance_hex("0000000000000000", "ffffffffffffffff"),
        64
    );
}

#[test]
fn mad_detects_different_frames() {
    use image::{GrayImage, Luma};
    let a = GrayImage::from_fn(64, 64, |x, y| Luma([((x + y) % 256) as u8]));
    let b = GrayImage::from_fn(64, 64, |x, y| Luma([((x * 3 + y * 2) % 256) as u8]));
    assert_eq!(verify::compare_images(&a, &a), 0.0);
    assert!(verify::compare_images(&a, &b) > 1.5);
    let c = GrayImage::from_pixel(64, 64, Luma([255]));
    assert!(verify::compare_images(&a, &c) > 1.5);
}

#[test]
fn material_upgrade_needs_resolution_or_25_percent() {
    let small = row(1, 800, 600, 100_000, 10);
    let larger = row(2, 1600, 1200, 100_000, 10);
    let same_res_bigger = row(3, 800, 600, 90_000, 10);
    let same_res_margin = row(4, 800, 600, 130_000, 10);
    assert!(quality::materially_better(&larger, &small, 1.25));
    assert!(!quality::materially_better(&same_res_bigger, &small, 1.25));
    assert!(quality::materially_better(&same_res_margin, &small, 1.25));
    assert_eq!(quality::best(&[small, larger]).unwrap().id, 2);
}

#[test]
fn store_roundtrip_and_groups() {
    let dir = tmpdir("store");
    let cfg = config(&dir);
    let mut store = Store::open(&cfg.paths.db).unwrap();
    store.sync_roots(&cfg.roots).unwrap();
    let root_id = store.root_id("album").unwrap();
    let entries = vec![
        ScanEntry {
            path: cfg.album.join("a.jpg"),
            rel_path: "a.jpg".into(),
            media: MediaKind::Image,
            size: 100,
            mtime: 1,
        },
        ScanEntry {
            path: cfg.album.join("b.jpg"),
            rel_path: "b.jpg".into(),
            media: MediaKind::Image,
            size: 200,
            mtime: 2,
        },
    ];
    // The files do not exist; the inventory does not require them yet.
    store.upsert_scan(root_id, &entries).unwrap();
    let all = store.all().unwrap();
    assert_eq!(all.len(), 2);
    let a = store
        .file_by_path(&cfg.album.join("a.jpg"))
        .unwrap()
        .unwrap();
    store
        .set_fingerprint(
            a.id,
            &bcp_hash("aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb", "cccccccccccccccc"),
        )
        .unwrap();
    let updated = store.file(a.id).unwrap().unwrap();
    assert_eq!(updated.phash.as_deref(), Some("aaaaaaaaaaaaaaaa"));
    std::fs::remove_dir_all(dir).ok();
}

fn bcp_hash(p: &str, d: &str, a: &str) -> bbcp_core::store::HashUpdate {
    bbcp_core::store::HashUpdate {
        phash: Some(p.into()),
        dhash: Some(d.into()),
        ahash: Some(a.into()),
        width: Some(10),
        height: Some(10),
        status: Some("ok".into()),
        error: None,
    }
}
