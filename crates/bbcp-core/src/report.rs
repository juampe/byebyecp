//! Markdown report of the inventory, the plan and the safety checks.

use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use chrono::Local;

use crate::config::Config;
use crate::model::{Action, MediaKind};
use crate::store::Store;
use crate::walk::format_bytes;

pub fn write_report(store: &Store, cfg: &Config) -> Result<PathBuf> {
    let day = Local::now().format("%Y-%m-%d").to_string();
    let dir = cfg.paths.manifests.join(&day);
    fs::create_dir_all(&dir)?;
    let path = dir.join("report.md");

    let mut out = String::new();
    writeln!(out, "# Bye Bye Cloud Photos — report {day}\n")?;
    writeln!(out, "Album: `{}`\n", cfg.album.display())?;

    writeln!(out, "## Inventory\n")?;
    writeln!(out, "| root | kind | media | files | size |")?;
    writeln!(out, "|---|---|---|---:|---:|")?;
    let mut stmt = store.con.prepare(
        "SELECT r.label, r.kind, f.media, COUNT(*), COALESCE(SUM(f.size),0)
         FROM files f JOIN roots r ON r.id=f.root_id
         GROUP BY r.label, r.kind, f.media ORDER BY r.label, f.media",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, i64>(4)?,
        ))
    })?;
    for row in rows {
        let (root, kind, media, n, bytes) = row?;
        writeln!(
            out,
            "| {root} | {kind} | {media} | {n} | {} |",
            format_bytes(bytes.max(0) as u64)
        )?;
    }

    writeln!(out, "\n## Plan\n")?;
    writeln!(out, "| action | files | size |")?;
    writeln!(out, "|---|---:|---:|")?;
    let mut stmt = store.con.prepare(
        "SELECT action, COUNT(*), COALESCE(SUM(bytes),0)
         FROM plan_actions GROUP BY action ORDER BY action",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    for row in rows {
        let (action, n, bytes) = row?;
        writeln!(
            out,
            "| {action} | {n} | {} |",
            format_bytes(bytes.max(0) as u64)
        )?;
    }

    let groups: i64 = store
        .con
        .query_row("SELECT COUNT(*) FROM groups", [], |r| r.get(0))?;
    let members: i64 = store
        .con
        .query_row("SELECT COUNT(*) FROM group_members", [], |r| r.get(0))?;
    writeln!(out, "\nExact groups: **{groups}** ({members} members).\n")?;

    let (with_date, without_date): (i64, i64) = store.con.query_row(
        "SELECT SUM(CASE WHEN capture_ts IS NOT NULL THEN 1 ELSE 0 END),
                SUM(CASE WHEN capture_ts IS NULL THEN 1 ELSE 0 END)
         FROM files WHERE media IN ('image','video')",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    writeln!(
        out,
        "Capture date: **{with_date}** known, **{without_date}** unknown.\n"
    )?;

    let verified: i64 = store.con.query_row(
        "SELECT COUNT(*) FROM plan_actions WHERE mad IS NOT NULL",
        [],
        |r| r.get(0),
    )?;
    writeln!(out, "Images verified by MAD: **{verified}**.\n")?;

    let mut stmt = store.con.prepare(
        "SELECT action, media, COUNT(*), COALESCE(SUM(bytes),0)
         FROM plan_actions p JOIN files f ON f.id=p.file_id
         GROUP BY action, media ORDER BY action, media",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })?;
    writeln!(out, "## Plan by media\n")?;
    writeln!(out, "| action | media | files | size |")?;
    writeln!(out, "|---|---|---:|---:|")?;
    for row in rows {
        let (action, media, n, bytes) = row?;
        writeln!(
            out,
            "| {action} | {media} | {n} | {} |",
            format_bytes(bytes.max(0) as u64)
        )?;
    }

    writeln!(out, "\n## Safety\n")?;
    writeln!(
        out,
        "- Duplicates are moved to `{}` (reversible).",
        cfg.paths.quarantine.display()
    )?;
    writeln!(
        out,
        "- Source deletions require `--only` with the external actions."
    )?;
    writeln!(out, "- Every `upgrade`/`import` is SHA-256 verified.")?;
    writeln!(out, "- `keep_distinct` means a human must review the file.")?;

    fs::write(&path, out)?;
    let _ = (Action::Keep, MediaKind::Image);
    Ok(path)
}
