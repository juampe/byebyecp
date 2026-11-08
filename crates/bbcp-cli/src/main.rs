//! `bbcp` — command-line interface for Bye Bye Cloud Photos.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use bbcp_core::apply::{self, ApplyOptions, RemoveMode};
use bbcp_core::config::Config;
use bbcp_core::model::{Action, MediaKind};
use bbcp_core::store::{HashUpdate, MetaUpdate, Store};
use bbcp_core::walk::format_bytes;
use bbcp_core::{
    dates, doctor, google, group, hash, imagehash, plan, report, stage, verify, video, walk,
};
use clap::{Parser, Subcommand};
use rayon::prelude::*;

#[derive(Parser)]
#[command(
    name = "bbcp",
    version,
    about = "Bye Bye Cloud Photos — safe, reversible media consolidation",
    long_about = None
)]
struct Cli {
    /// Configuration file.
    #[arg(long, global = true, default_value = "bbcp.toml")]
    config: PathBuf,
    /// Override the database path from the config.
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a starter configuration and the database.
    Init {
        /// Album directory.
        #[arg(long)]
        album: Option<PathBuf>,
        /// Overwrite an existing configuration.
        #[arg(long)]
        force: bool,
    },
    /// Check tools, paths and permissions.
    Doctor,
    /// Inventory the configured roots.
    Scan {
        /// Only these root labels (repeatable). Default: all.
        #[arg(long)]
        root: Vec<String>,
        /// Delete inventory rows for files that no longer exist.
        #[arg(long)]
        prune: bool,
    },
    /// Parse Google Takeout JSON sidecars and match them to files.
    Sidecars,
    /// Extract video metadata with ffprobe.
    Probe {
        #[arg(long, default_value_t = 0)]
        limit: usize,
        #[arg(long, default_value_t = 16)]
        workers: usize,
    },
    /// Compute perceptual hashes for images.
    Fingerprint {
        #[arg(long, default_value_t = 0)]
        limit: usize,
        #[arg(long, default_value_t = 16)]
        workers: usize,
    },
    /// Compute SHA-256 for candidate videos.
    Hash {
        /// Hash every video, not only size-collision candidates.
        #[arg(long)]
        all: bool,
        #[arg(long, default_value_t = 16)]
        workers: usize,
    },
    /// Build exact duplicate groups.
    Group {
        /// Remove previous groups first.
        #[arg(long)]
        reset: bool,
    },
    /// Fill missing capture dates.
    Dates,
    /// Build the consolidation plan.
    Plan,
    /// Verify image deletions with pixel MAD.
    Verify {
        #[arg(long, default_value_t = 8)]
        workers: usize,
    },
    /// Execute the plan (dry run unless --execute).
    Apply {
        #[arg(long)]
        execute: bool,
        /// Comma-separated actions: upgrade,import,delete,external_covered,external_dup.
        #[arg(long, default_value = "upgrade,import,delete")]
        only: String,
        /// What to do with album duplicates.
        #[arg(long, default_value = "quarantine")]
        album_remove: String,
        /// What to do with source duplicates (only when included in --only).
        #[arg(long, default_value = "unlink")]
        external_remove: String,
        /// Remove the source after a verified upgrade/import.
        #[arg(long)]
        consume_sources: bool,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long, default_value_t = 8)]
        workers: usize,
    },
    /// Move unique media to a staging folder for review.
    Stage {
        /// Only these source roots (repeatable). Default: all.
        #[arg(long)]
        root: Vec<String>,
        #[arg(long)]
        execute: bool,
        /// List the files that would be staged and exit.
        #[arg(long)]
        list: bool,
    },
    /// Generate Google Photos deletion batches.
    Google,
    /// Write a Markdown report.
    Report,
    /// Compare two images by MAD.
    Compare { left: PathBuf, right: PathBuf },
    /// Quick totals.
    Status,
}

fn load(cli: &Cli) -> Result<Config> {
    let mut cfg = Config::load(&cli.config)?;
    if let Some(db) = &cli.db {
        cfg.paths.db = db.clone();
    }
    cfg.validate()?;
    cfg.ensure_dirs()?;
    Ok(cfg)
}

fn store(cfg: &Config) -> Result<Store> {
    let mut s = Store::open(&cfg.paths.db)?;
    s.sync_roots(&cfg.roots)?;
    Ok(s)
}

fn pool(workers: usize) -> Result<rayon::ThreadPool> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(workers.max(1))
        .build()
        .context("creating worker pool")
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Command::Init { album, force } => return init(&cli, album.clone(), *force),
        Command::Compare { left, right } => {
            let value = verify::compare(left, right)?;
            println!("MAD = {value:.4}");
            return Ok(());
        }
        Command::Doctor => return doctor_cmd(&cli),
        _ => {}
    }

    let cfg = load(&cli)?;
    match &cli.command {
        Command::Scan { root, prune } => scan(&cfg, root, *prune),
        Command::Sidecars => sidecars(&cfg),
        Command::Probe { limit, workers } => probe(&cfg, *limit, *workers),
        Command::Fingerprint { limit, workers } => fingerprint(&cfg, *limit, *workers),
        Command::Hash { all, workers } => hash_cmd(&cfg, *all, *workers),
        Command::Group { reset } => group_cmd(&cfg, *reset),
        Command::Dates => dates_cmd(&cfg),
        Command::Plan => plan_cmd(&cfg),
        Command::Verify { workers } => verify_cmd(&cfg, *workers),
        Command::Apply {
            execute,
            only,
            album_remove,
            external_remove,
            consume_sources,
            limit,
            workers,
        } => apply_cmd(
            &cfg,
            *execute,
            only,
            album_remove,
            external_remove,
            *consume_sources,
            *limit,
            *workers,
        ),
        Command::Stage {
            root,
            execute,
            list,
        } => stage_cmd(&cfg, root, *execute, *list),
        Command::Google => google_cmd(&cfg),
        Command::Report => report_cmd(&cfg),
        Command::Status => status(&cfg),
        Command::Init { .. } | Command::Doctor | Command::Compare { .. } => unreachable!(),
    }
}

fn init(cli: &Cli, album: Option<PathBuf>, force: bool) -> Result<()> {
    let path = &cli.config;
    if path.exists() && !force {
        anyhow::bail!(
            "{} already exists (use --force to overwrite)",
            path.display()
        );
    }
    let album = album.unwrap_or_else(|| PathBuf::from("Album"));
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    std::fs::write(path, bbcp_core::config::example_config(&album))
        .with_context(|| format!("writing {}", path.display()))?;
    let cfg = Config::load(path)?;
    cfg.ensure_dirs()?;
    let mut store = Store::open(&cfg.paths.db)?;
    store.sync_roots(&cfg.roots)?;
    println!("created {}", path.display());
    println!("album: {}", cfg.album.display());
    println!("database: {}", cfg.paths.db.display());
    println!(
        "\nNext: edit {} and add your Takeout roots.",
        path.display()
    );
    Ok(())
}

fn doctor_cmd(cli: &Cli) -> Result<()> {
    let cfg = Config::load(&cli.config)?;
    cfg.validate()?;
    let report = doctor(&cfg)?;
    println!("ffmpeg:  {}", if report.ffmpeg { "ok" } else { "MISSING" });
    println!("ffprobe: {}", if report.ffprobe { "ok" } else { "MISSING" });
    println!("sqlite:  {}", report.sqlite);
    println!(
        "album:   {} ({}, writable={})",
        cfg.album.display(),
        if report.album_exists {
            "exists"
        } else {
            "MISSING"
        },
        report.album_writable
    );
    for (label, exists, files) in &report.roots_ok {
        println!(
            "root {label:16} {} ({files} files)",
            if *exists { "ok" } else { "MISSING" }
        );
    }
    if let Some(free) = report.free_bytes {
        println!("free:    {}", format_bytes(free));
    }
    if !report.ffmpeg || !report.ffprobe {
        anyhow::bail!("install ffmpeg and ffprobe before procressing videos");
    }
    Ok(())
}

fn scan(cfg: &Config, roots: &[String], prune: bool) -> Result<()> {
    let mut store = store(cfg)?;
    for root in &cfg.roots {
        if !roots.is_empty() && !roots.contains(&root.label) {
            continue;
        }
        println!("scanning {} ({})", root.label, root.path.display());
        let result = walk::scan_root(root)?;
        let (images, videos, other, bytes) = walk::summarise(&result.entries);
        let root_id = store.root_id(&root.label)?;
        let touched = store.upsert_scan(root_id, &result.entries)?;
        println!(
            "  {touched} rows: {images} images, {videos} videos, {other} other, {}",
            format_bytes(bytes)
        );
        if !result.errors.is_empty() {
            eprintln!("  {} walk errors", result.errors.len());
        }
        if prune {
            let existing = walk::existing_paths(&result.entries);
            let gone = store.prune_root(&root.label, &existing)?;
            if gone > 0 {
                println!("  pruned {gone} missing rows");
            }
        }
    }
    Ok(())
}

fn sidecars(cfg: &Config) -> Result<()> {
    let mut store = store(cfg)?;
    let n = dates::import_sidecars(&mut store, cfg)?;
    println!("matched sidecars: {n}");
    Ok(())
}

fn probe(cfg: &Config, limit: usize, workers: usize) -> Result<()> {
    let mut store = store(cfg)?;
    let rows = store.pending_meta(MediaKind::Video, if limit > 0 { Some(limit) } else { None })?;
    println!("probing {} videos", rows.len());
    let results: Vec<_> = pool(workers)?.install(|| {
        rows.par_iter()
            .map(|f| (f.id, video::probe(&f.path)))
            .collect()
    });
    let mut ok = 0usize;
    let mut err = 0usize;
    for (id, result) in results {
        match result {
            Ok(meta) => {
                store.set_meta(
                    id,
                    &MetaUpdate {
                        width: meta.width,
                        height: meta.height,
                        duration: meta.duration,
                        bitrate: meta.bitrate,
                        codec: meta.codec,
                        status: Some("ok".into()),
                        error: None,
                    },
                )?;
                ok += 1;
            }
            Err(error) => {
                store.set_meta(
                    id,
                    &MetaUpdate {
                        status: Some("error".into()),
                        error: Some(error.to_string()),
                        ..Default::default()
                    },
                )?;
                err += 1;
            }
        }
    }
    println!("probed: {ok} ok, {err} errors");
    Ok(())
}

fn fingerprint(cfg: &Config, limit: usize, workers: usize) -> Result<()> {
    let mut store = store(cfg)?;
    let rows = store.pending_fingerprint(if limit > 0 { Some(limit) } else { None })?;
    println!("fingerprinting {} images", rows.len());
    let results: Vec<_> = pool(workers)?.install(|| {
        rows.par_iter()
            .map(|f| (f.id, imagehash::fingerprint(&f.path)))
            .collect()
    });
    let mut ok = 0usize;
    let mut err = 0usize;
    for (id, result) in results {
        match result {
            Ok(fp) => {
                store.set_fingerprint(
                    id,
                    &HashUpdate {
                        phash: Some(fp.phash),
                        dhash: Some(fp.dhash),
                        ahash: Some(fp.ahash),
                        width: Some(fp.width),
                        height: Some(fp.height),
                        status: Some("ok".into()),
                        error: None,
                    },
                )?;
                ok += 1;
            }
            Err(error) => {
                store.set_fingerprint(
                    id,
                    &HashUpdate {
                        status: Some("error".into()),
                        error: Some(error.to_string()),
                        ..Default::default()
                    },
                )?;
                err += 1;
            }
        }
        if (ok + err).is_multiple_of(500) {
            println!("  {}/{}", ok + err, rows.len());
        }
    }
    println!("fingerprinted: {ok} ok, {err} errors");
    Ok(())
}

fn hash_cmd(cfg: &Config, all: bool, workers: usize) -> Result<()> {
    let mut store = store(cfg)?;
    let rows = if all {
        store
            .videos()?
            .into_iter()
            .filter(|f| f.sha256.is_none())
            .collect::<Vec<_>>()
    } else {
        store.hash_candidates()?
    };
    println!("hashing {} videos", rows.len());
    let results: Vec<_> = pool(workers)?.install(|| {
        rows.par_iter()
            .map(|f| (f.id, hash::sha256_file(&f.path)))
            .collect()
    });
    let mut ok = 0usize;
    let mut err = 0usize;
    for (id, result) in results {
        match result {
            Ok(sha) => {
                store.set_sha256(id, &sha)?;
                ok += 1;
            }
            Err(error) => {
                store.set_error(id, &error.to_string())?;
                err += 1;
            }
        }
    }
    println!("hashed: {ok} ok, {err} errors");
    Ok(())
}

fn group_cmd(cfg: &Config, reset: bool) -> Result<()> {
    let mut store = store(cfg)?;
    if reset {
        store.clear_groups()?;
    }
    let images = group::group_images(&mut store, &cfg.rules)?;
    let videos = group::group_videos(&mut store)?;
    println!(
        "image groups: {} ({} members, {} flat skipped, {} aspect skipped)",
        images.groups, images.members, images.skipped_flat, images.skipped_aspect
    );
    println!(
        "video groups: {} ({} members)",
        videos.groups, videos.members
    );
    Ok(())
}

fn dates_cmd(cfg: &Config) -> Result<()> {
    let mut store = store(cfg)?;
    let stats = dates::fill_dates(&mut store, cfg)?;
    println!(
        "dates: sidecar={} bridge={} exif={} filename={} folder={} none={} rejected={}",
        stats.sidecar,
        stats.bridge,
        stats.exif,
        stats.filename,
        stats.folder,
        stats.none,
        stats.rejected
    );
    Ok(())
}

fn plan_cmd(cfg: &Config) -> Result<()> {
    let mut store = store(cfg)?;
    let stats = plan::build_plan(&mut store, cfg)?;
    println!("keep           {:8}  {}", stats.keep, format_bytes(0));
    println!("keep_distinct  {:8}", stats.keep_distinct);
    println!(
        "delete         {:8}  {}",
        stats.delete,
        format_bytes(stats.bytes_delete)
    );
    println!(
        "upgrade        {:8}  {}",
        stats.upgrade,
        format_bytes(stats.bytes_upgrade)
    );
    println!(
        "import         {:8}  {}",
        stats.import,
        format_bytes(stats.bytes_import)
    );
    println!(
        "external       {:8}  {}",
        stats.external_covered + stats.external_dup,
        format_bytes(stats.bytes_external)
    );
    Ok(())
}

fn verify_cmd(cfg: &Config, _workers: usize) -> Result<()> {
    let mut store = store(cfg)?;
    let stats = verify::verify_images(&mut store, cfg)?;
    println!(
        "verified {} image actions: {} confirmed, {} kept distinct, {} errors",
        stats.checked, stats.confirmed, stats.kept_distinct, stats.errors
    );
    Ok(())
}

fn parse_actions(value: &str) -> Result<Vec<Action>> {
    let mut out = Vec::new();
    for part in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let action =
            Action::parse(part).ok_or_else(|| anyhow::anyhow!("unknown action: {part}"))?;
        out.push(action);
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn apply_cmd(
    cfg: &Config,
    execute: bool,
    only: &str,
    album_remove: &str,
    external_remove: &str,
    consume_sources: bool,
    limit: Option<usize>,
    workers: usize,
) -> Result<()> {
    let mut store = store(cfg)?;
    let opts = ApplyOptions {
        only: parse_actions(only)?,
        execute,
        workers,
        album_remove: RemoveMode::parse(album_remove)
            .ok_or_else(|| anyhow::anyhow!("unknown remove mode: {album_remove}"))?,
        external_remove: RemoveMode::parse(external_remove)
            .ok_or_else(|| anyhow::anyhow!("unknown remove mode: {external_remove}"))?,
        consume_sources,
        limit,
    };
    if !execute {
        println!("dry run (add --execute to apply)");
        for (action, files, bytes) in apply::preview(&store, &opts)? {
            println!(
                "  {:18} {:8}  {}",
                action.as_str(),
                files,
                format_bytes(bytes)
            );
        }
        return Ok(());
    }
    let stats = apply::apply(&mut store, cfg, &opts)?;
    println!(
        "upgraded={} imported={} deleted={} external={} missing={} skipped={} errors={}",
        stats.upgraded,
        stats.imported,
        stats.deleted,
        stats.external,
        stats.missing,
        stats.skipped,
        stats.errors
    );
    println!(
        "written={} removed={}",
        format_bytes(stats.bytes_written),
        format_bytes(stats.bytes_removed)
    );
    if stats.errors > 0 {
        anyhow::bail!("{} operations failed; see the journal", stats.errors);
    }
    Ok(())
}

fn stage_cmd(cfg: &Config, roots: &[String], execute: bool, list: bool) -> Result<()> {
    let mut store = store(cfg)?;
    if list {
        let day = chrono::Local::now().format("%Y-%m-%d").to_string();
        let dir = cfg.paths.staging.join(day);
        for path in stage::listing(&dir)? {
            println!("{}", path.display());
        }
        return Ok(());
    }
    let stats = stage::stage(&mut store, cfg, roots, execute)?;
    if execute {
        println!(
            "moved={} bytes={} missing={} collisions={} errors={}",
            stats.moved,
            format_bytes(stats.bytes),
            stats.missing,
            stats.collisions,
            stats.errors
        );
    }
    Ok(())
}

fn google_cmd(cfg: &Config) -> Result<()> {
    let store = store(cfg)?;
    let stats = google::plan(&store, cfg)?;
    println!(
        "google: {} candidates ({}), {} kept, {} batches in {}",
        stats.candidates,
        format_bytes(stats.bytes),
        stats.kept,
        stats.batches,
        cfg.paths.manifests.display()
    );
    Ok(())
}

fn report_cmd(cfg: &Config) -> Result<()> {
    let store = store(cfg)?;
    let path = report::write_report(&store, cfg)?;
    println!("report: {}", path.display());
    Ok(())
}

fn status(cfg: &Config) -> Result<()> {
    let store = store(cfg)?;
    let mut stmt = store.con.prepare(
        "SELECT r.label, f.media, COUNT(*), COALESCE(SUM(f.size),0)
         FROM files f JOIN roots r ON r.id=f.root_id
         GROUP BY r.label, f.media ORDER BY r.label, f.media",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })?;
    for row in rows {
        let (root, media, n, bytes) = row?;
        println!(
            "{root:18} {media:6} {n:8}  {}",
            format_bytes(bytes.max(0) as u64)
        );
    }
    let actions: i64 = store
        .con
        .query_row("SELECT COUNT(*) FROM plan_actions", [], |r| r.get(0))?;
    println!("plan actions: {actions}");
    Ok(())
}
