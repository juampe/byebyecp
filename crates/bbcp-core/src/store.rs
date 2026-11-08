//! SQLite inventory. The database is the single source of truth between
//! stages and is always kept on a local filesystem (never on a 9p mount).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::config::Root;
use crate::model::{Action, FileRow, GroupKind, MediaKind, PlannedAction, Sidecar};

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS roots (
    id         INTEGER PRIMARY KEY,
    label      TEXT NOT NULL UNIQUE,
    path       TEXT NOT NULL,
    kind       TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS files (
    id                 INTEGER PRIMARY KEY,
    root_id            INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
    path               TEXT NOT NULL UNIQUE,
    rel_path           TEXT NOT NULL,
    media              TEXT NOT NULL,
    size               INTEGER NOT NULL,
    mtime              INTEGER NOT NULL,
    width              INTEGER,
    height             INTEGER,
    duration           REAL,
    bitrate            INTEGER,
    codec              TEXT,
    sha256             TEXT,
    phash              TEXT,
    dhash              TEXT,
    ahash              TEXT,
    capture_ts         TEXT,
    capture_source     TEXT,
    fingerprint_status TEXT,
    meta_status        TEXT,
    error              TEXT
);
CREATE INDEX IF NOT EXISTS files_root   ON files(root_id);
CREATE INDEX IF NOT EXISTS files_media  ON files(media);
CREATE INDEX IF NOT EXISTS files_sha256 ON files(sha256);
CREATE INDEX IF NOT EXISTS files_phash  ON files(phash, dhash, ahash);

CREATE TABLE IF NOT EXISTS sidecars (
    id              INTEGER PRIMARY KEY,
    root_id         INTEGER REFERENCES roots(id) ON DELETE CASCADE,
    path            TEXT NOT NULL UNIQUE,
    google_id       TEXT,
    title           TEXT,
    taken_ts        TEXT,
    creation_ts     TEXT,
    description     TEXT,
    matched_file_id INTEGER REFERENCES files(id)
);
CREATE INDEX IF NOT EXISTS sidecars_google  ON sidecars(google_id);
CREATE INDEX IF NOT EXISTS sidecars_matched ON sidecars(matched_file_id);

CREATE TABLE IF NOT EXISTS groups (
    id   INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    key  TEXT NOT NULL,
    UNIQUE(kind, key)
);

CREATE TABLE IF NOT EXISTS group_members (
    group_id INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    file_id  INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    role     TEXT NOT NULL,
    PRIMARY KEY (group_id, file_id)
);

CREATE TABLE IF NOT EXISTS plan_actions (
    file_id      INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    action       TEXT NOT NULL,
    group_key    TEXT,
    reference_id INTEGER,
    target_path  TEXT,
    bytes        INTEGER NOT NULL,
    mad          REAL,
    reason       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS plan_actions_action ON plan_actions(action);

CREATE TABLE IF NOT EXISTS approx_pairs (
    extra_id INTEGER PRIMARY KEY,
    album_id INTEGER NOT NULL,
    distance INTEGER,
    same     INTEGER NOT NULL,
    error    TEXT
);
"#;

/// A lightweight row produced by `scan`, before metadata exists.
#[derive(Debug, Clone)]
pub struct ScanEntry {
    pub path: PathBuf,
    pub rel_path: String,
    pub media: MediaKind,
    pub size: u64,
    pub mtime: i64,
}

/// Metadata produced by `probe` (videos) or `fingerprint` (images).
#[derive(Debug, Clone, Default)]
pub struct MetaUpdate {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration: Option<f64>,
    pub bitrate: Option<u64>,
    pub codec: Option<String>,
    pub status: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct HashUpdate {
    pub phash: Option<String>,
    pub dhash: Option<String>,
    pub ahash: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub status: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GroupRow {
    pub id: i64,
    pub kind: String,
    pub key: String,
    pub members: Vec<(i64, String)>,
}

/// `(extra_id, album_id, frame distance, same)`.
pub type ApproxPair = (i64, i64, Option<u32>, bool);

pub struct Store {
    pub con: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creando {}", parent.display()))?;
        }
        let con = Connection::open(path)
            .with_context(|| format!("abriendo base de datos {}", path.display()))?;
        con.busy_timeout(std::time::Duration::from_secs(60))?;
        con.execute_batch(SCHEMA).context("creando esquema")?;
        Ok(Self { con })
    }

    pub fn set_kv(&self, key: &str, value: &str) -> Result<()> {
        self.con.execute(
            "INSERT INTO meta(key,value) VALUES(?,?)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_kv(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .con
            .query_row("SELECT value FROM meta WHERE key=?", params![key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    /// Register or update every configured root. Paths are canonicalised so
    /// the same folder configured twice cannot create duplicate rows.
    pub fn sync_roots(&mut self, roots: &[Root]) -> Result<usize> {
        let tx = self.con.transaction()?;
        let now = Utc::now().to_rfc3339();
        let mut n = 0;
        for r in roots {
            let path = r
                .path
                .canonicalize()
                .unwrap_or_else(|_| r.path.clone())
                .to_string_lossy()
                .to_string();
            tx.execute(
                "INSERT INTO roots(label,path,kind,created_at) VALUES(?,?,?,?)
                 ON CONFLICT(label) DO UPDATE SET path=excluded.path, kind=excluded.kind",
                params![r.label, path, r.kind, now],
            )?;
            n += 1;
        }
        tx.commit()?;
        Ok(n)
    }

    pub fn root_id(&self, label: &str) -> Result<i64> {
        self.con
            .query_row("SELECT id FROM roots WHERE label=?", params![label], |r| {
                r.get(0)
            })
            .with_context(|| format!("raíz desconocida: {label}; ejecuta `bbcp roots sync`"))
    }

    pub fn root_path(&self, label: &str) -> Result<PathBuf> {
        let p: String = self.con.query_row(
            "SELECT path FROM roots WHERE label=?",
            params![label],
            |r| r.get(0),
        )?;
        Ok(PathBuf::from(p))
    }

    /// Insert new files and refresh size/mtime of known ones. Returns the
    /// number of rows touched.
    pub fn upsert_scan(&mut self, root_id: i64, entries: &[ScanEntry]) -> Result<usize> {
        let tx = self.con.transaction()?;
        let mut n = 0;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO files(root_id,path,rel_path,media,size,mtime)
                 VALUES(?,?,?,?,?,?)
                 ON CONFLICT(path) DO UPDATE SET
                    root_id=excluded.root_id,
                    rel_path=excluded.rel_path,
                    media=excluded.media,
                    size=excluded.size,
                    mtime=excluded.mtime,
                    error=NULL",
            )?;
            for e in entries {
                stmt.execute(params![
                    root_id,
                    e.path.to_string_lossy(),
                    e.rel_path,
                    e.media.as_str(),
                    e.size as i64,
                    e.mtime,
                ])?;
                n += 1;
            }
        }
        tx.commit()?;
        Ok(n)
    }

    /// Remove rows for files that no longer exist under a root.
    pub fn prune_root(
        &mut self,
        label: &str,
        existing: &std::collections::HashSet<PathBuf>,
    ) -> Result<usize> {
        let root_id = self.root_id(label)?;
        let rows: Vec<(i64, String)> = {
            let mut stmt = self
                .con
                .prepare("SELECT id, path FROM files WHERE root_id=?")?;
            let it = stmt.query_map(params![root_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            it.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut gone = Vec::new();
        for (id, path) in rows {
            if !existing.contains(&PathBuf::from(path)) {
                gone.push(id);
            }
        }
        let tx = self.con.transaction()?;
        {
            let mut stmt = tx.prepare("DELETE FROM files WHERE id=?")?;
            for id in &gone {
                stmt.execute(params![id])?;
            }
        }
        tx.commit()?;
        Ok(gone.len())
    }

    pub fn file(&self, id: i64) -> Result<Option<FileRow>> {
        let mut stmt = self.con.prepare(&format!("{} WHERE f.id=?", FILE_SELECT))?;
        let row = stmt.query_row(params![id], row_to_file).optional()?;
        Ok(row)
    }

    pub fn file_by_path(&self, path: &Path) -> Result<Option<FileRow>> {
        let mut stmt = self
            .con
            .prepare(&format!("{} WHERE f.path=?", FILE_SELECT))?;
        let row = stmt
            .query_row(params![path.to_string_lossy()], row_to_file)
            .optional()?;
        Ok(row)
    }

    /// Every row, including documents and other non-media files.
    pub fn all(&self) -> Result<Vec<FileRow>> {
        let mut stmt = self
            .con
            .prepare(&format!("{} ORDER BY f.id", FILE_SELECT))?;
        let it = stmt.query_map([], row_to_file)?;
        Ok(it.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Groups with their members and roles.
    pub fn groups_with_members(&self) -> Result<Vec<GroupRow>> {
        let mut groups = Vec::new();
        let mut stmt = self
            .con
            .prepare("SELECT id, kind, key FROM groups ORDER BY kind, key")?;
        let it = stmt.query_map([], |r| {
            Ok(GroupRow {
                id: r.get(0)?,
                kind: r.get(1)?,
                key: r.get(2)?,
                members: Vec::new(),
            })
        })?;
        groups.extend(it.collect::<rusqlite::Result<Vec<_>>>()?);
        let mut by_id: std::collections::HashMap<i64, usize> =
            groups.iter().enumerate().map(|(i, g)| (g.id, i)).collect();
        let mut stmt = self
            .con
            .prepare("SELECT group_id, file_id, role FROM group_members ORDER BY file_id")?;
        let it = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in it {
            let (gid, fid, role) = row?;
            if let Some(i) = by_id.get(&gid).copied() {
                groups[i].members.push((fid, role));
            }
        }
        by_id.clear();
        Ok(groups)
    }

    /// All media rows, ordered by id for deterministic output.
    pub fn media(&self) -> Result<Vec<FileRow>> {
        let mut stmt = self.con.prepare(&format!(
            "{} WHERE f.media IN ('image','video') ORDER BY f.id",
            FILE_SELECT
        ))?;
        let it = stmt.query_map([], row_to_file)?;
        Ok(it.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn by_root(&self, label: &str) -> Result<Vec<FileRow>> {
        let mut stmt = self
            .con
            .prepare(&format!("{} WHERE r.label=? ORDER BY f.id", FILE_SELECT))?;
        let it = stmt.query_map(params![label], row_to_file)?;
        Ok(it.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn by_root_media(&self, label: &str, media: MediaKind) -> Result<Vec<FileRow>> {
        let mut stmt = self.con.prepare(&format!(
            "{} WHERE r.label=? AND f.media=? ORDER BY f.id",
            FILE_SELECT
        ))?;
        let it = stmt.query_map(params![label, media.as_str()], row_to_file)?;
        Ok(it.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn images(&self) -> Result<Vec<FileRow>> {
        self.by_media(MediaKind::Image)
    }

    pub fn videos(&self) -> Result<Vec<FileRow>> {
        self.by_media(MediaKind::Video)
    }

    fn by_media(&self, media: MediaKind) -> Result<Vec<FileRow>> {
        let mut stmt = self
            .con
            .prepare(&format!("{} WHERE f.media=? ORDER BY f.id", FILE_SELECT))?;
        let it = stmt.query_map(params![media.as_str()], row_to_file)?;
        Ok(it.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Files that still need `probe` (videos only).
    pub fn pending_meta(&self, media: MediaKind, limit: Option<usize>) -> Result<Vec<FileRow>> {
        let sql = format!(
            "{} WHERE f.media=? AND (f.meta_status IS NULL OR f.meta_status='error') ORDER BY f.id",
            FILE_SELECT
        );
        let mut stmt = self.con.prepare(&sql)?;
        let it = stmt.query_map(params![media.as_str()], row_to_file)?;
        let mut rows = it.collect::<rusqlite::Result<Vec<_>>>()?;
        if let Some(n) = limit {
            rows.truncate(n);
        }
        Ok(rows)
    }

    /// Images that still need a perceptual fingerprint.
    pub fn pending_fingerprint(&self, limit: Option<usize>) -> Result<Vec<FileRow>> {
        let sql = format!(
            "{} WHERE f.media='image' AND (f.fingerprint_status IS NULL OR f.fingerprint_status='error') ORDER BY f.id",
            FILE_SELECT
        );
        let mut stmt = self.con.prepare(&sql)?;
        let it = stmt.query_map([], row_to_file)?;
        let mut rows = it.collect::<rusqlite::Result<Vec<_>>>()?;
        if let Some(n) = limit {
            rows.truncate(n);
        }
        Ok(rows)
    }

    /// Videos with a duplicate size somewhere else: the only ones that can
    /// possibly be byte-identical, so the only ones worth hashing in full.
    pub fn hash_candidates(&self) -> Result<Vec<FileRow>> {
        let sql = format!(
            "{} WHERE f.media='video' AND f.sha256 IS NULL AND f.size IN (
                SELECT size FROM files WHERE media='video' GROUP BY size HAVING COUNT(*) > 1
             ) ORDER BY f.id",
            FILE_SELECT
        );
        let mut stmt = self.con.prepare(&sql)?;
        let it = stmt.query_map([], row_to_file)?;
        Ok(it.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn set_meta(&mut self, id: i64, m: &MetaUpdate) -> Result<()> {
        self.con.execute(
            "UPDATE files SET
                width=COALESCE(?, width),
                height=COALESCE(?, height),
                duration=COALESCE(?, duration),
                bitrate=COALESCE(?, bitrate),
                codec=COALESCE(?, codec),
                meta_status=COALESCE(?, meta_status),
                error=COALESCE(?, error)
             WHERE id=?",
            params![
                m.width,
                m.height,
                m.duration,
                m.bitrate.map(|b| b as i64),
                m.codec,
                m.status,
                m.error,
                id
            ],
        )?;
        Ok(())
    }

    pub fn set_fingerprint(&mut self, id: i64, h: &HashUpdate) -> Result<()> {
        self.con.execute(
            "UPDATE files SET
                phash=COALESCE(?, phash),
                dhash=COALESCE(?, dhash),
                ahash=COALESCE(?, ahash),
                width=COALESCE(width, ?),
                height=COALESCE(height, ?),
                fingerprint_status=COALESCE(?, fingerprint_status),
                error=COALESCE(?, error)
             WHERE id=?",
            params![h.phash, h.dhash, h.ahash, h.width, h.height, h.status, h.error, id],
        )?;
        Ok(())
    }

    /// Point a row at its new location after an import. If a previous rescan
    /// already inventoried the destination, the stale source row is removed so
    /// the album row stays the single source of truth.
    pub fn relocate(
        &mut self,
        file_id: i64,
        root_id: i64,
        path: &Path,
        rel_path: &str,
    ) -> Result<()> {
        let target = path.to_string_lossy().to_string();
        let existing: Option<i64> = self
            .con
            .query_row("SELECT id FROM files WHERE path=?", params![target], |r| {
                r.get(0)
            })
            .optional()?;
        if let Some(other) = existing {
            if other != file_id {
                self.con
                    .execute("DELETE FROM files WHERE id=?", params![file_id])?;
                return Ok(());
            }
        }
        self.con.execute(
            "UPDATE files SET root_id=?, path=?, rel_path=? WHERE id=?",
            params![root_id, target, rel_path, file_id],
        )?;
        Ok(())
    }

    /// Drop a row whose file was moved to quarantine or unlinked. Related
    /// plan/group rows cascade.
    pub fn forget_file(&mut self, id: i64) -> Result<()> {
        // A sidecar may point at this row; detach it instead of failing the
        // foreign key check. The metadata itself is kept.
        self.con.execute(
            "UPDATE sidecars SET matched_file_id=NULL WHERE matched_file_id=?",
            params![id],
        )?;
        self.con
            .execute("DELETE FROM files WHERE id=?", params![id])?;
        Ok(())
    }

    pub fn set_sha256(&mut self, id: i64, sha: &str) -> Result<()> {
        self.con
            .execute("UPDATE files SET sha256=? WHERE id=?", params![sha, id])?;
        Ok(())
    }

    pub fn set_date(&mut self, id: i64, ts: &str, source: &str) -> Result<()> {
        self.con.execute(
            "UPDATE files SET capture_ts=?, capture_source=? WHERE id=?",
            params![ts, source, id],
        )?;
        Ok(())
    }

    pub fn set_error(&mut self, id: i64, error: &str) -> Result<()> {
        self.con
            .execute("UPDATE files SET error=? WHERE id=?", params![error, id])?;
        Ok(())
    }

    pub fn clear_groups(&mut self) -> Result<()> {
        self.con.execute("DELETE FROM group_members", [])?;
        self.con.execute("DELETE FROM groups", [])?;
        Ok(())
    }

    pub fn insert_group(
        &mut self,
        kind: GroupKind,
        key: &str,
        members: &[(i64, &str)],
    ) -> Result<i64> {
        let tx = self.con.transaction()?;
        tx.execute(
            "INSERT INTO groups(kind,key) VALUES(?,?)
             ON CONFLICT(kind,key) DO UPDATE SET kind=excluded.kind",
            params![kind.as_str(), key],
        )?;
        let gid: i64 = tx.query_row(
            "SELECT id FROM groups WHERE kind=? AND key=?",
            params![kind.as_str(), key],
            |r| r.get(0),
        )?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO group_members(group_id,file_id,role) VALUES(?,?,?)",
            )?;
            for (fid, role) in members {
                stmt.execute(params![gid, fid, role])?;
            }
        }
        tx.commit()?;
        Ok(gid)
    }

    pub fn group_count(&self) -> Result<i64> {
        Ok(self
            .con
            .query_row("SELECT COUNT(*) FROM groups", [], |r| r.get(0))?)
    }

    pub fn clear_plan(&mut self) -> Result<()> {
        self.con.execute("DELETE FROM plan_actions", [])?;
        Ok(())
    }

    pub fn insert_plan(&mut self, actions: &[PlannedAction]) -> Result<()> {
        let tx = self.con.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO plan_actions
                 (file_id,action,group_key,reference_id,target_path,bytes,mad,reason)
                 VALUES(?,?,?,?,?,?,NULL,?)",
            )?;
            for a in actions {
                stmt.execute(params![
                    a.file_id,
                    a.action.as_str(),
                    a.group_key,
                    a.reference_id,
                    a.target_path
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned()),
                    a.bytes as i64,
                    a.reason,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_mad(&mut self, file_id: i64, mad: f64) -> Result<()> {
        self.con.execute(
            "UPDATE plan_actions SET mad=? WHERE file_id=?",
            params![mad, file_id],
        )?;
        Ok(())
    }

    pub fn set_action(&mut self, file_id: i64, action: Action, reason: &str) -> Result<()> {
        self.con.execute(
            "UPDATE plan_actions SET action=?, reason=? WHERE file_id=?",
            params![action.as_str(), reason, file_id],
        )?;
        Ok(())
    }

    pub fn plan(&self) -> Result<Vec<(PlannedAction, FileRow)>> {
        let sql = "SELECT f.id,f.path,f.rel_path,f.media,f.size,f.mtime,
                f.width,f.height,f.duration,f.bitrate,f.codec,f.sha256,f.phash,f.dhash,f.ahash,
                f.capture_ts,f.capture_source,f.fingerprint_status,f.meta_status,f.error,
                r.label AS root_label,
                p.action AS plan_action, p.group_key AS plan_group, p.reference_id,
                p.target_path, p.bytes AS plan_bytes, p.reason
            FROM files f
            JOIN roots r ON r.id=f.root_id
            JOIN plan_actions p ON p.file_id=f.id
            ORDER BY p.action, f.id";
        let mut stmt = self.con.prepare(sql)?;
        let it = stmt.query_map([], |r| {
            let f = row_to_file(r)?;
            let action = Action::parse(&r.get::<_, String>("plan_action")?).unwrap_or(Action::Keep);
            let reference_id: Option<i64> = r.get("reference_id")?;
            let target: Option<String> = r.get("target_path")?;
            let reason: String = r.get("reason")?;
            let group_key: Option<String> = r.get("plan_group")?;
            let bytes: i64 = r.get("plan_bytes")?;
            Ok((
                PlannedAction {
                    file_id: f.id,
                    action,
                    group_key,
                    reference_id,
                    target_path: target.map(PathBuf::from),
                    bytes: bytes.max(0) as u64,
                    reason,
                },
                f,
            ))
        })?;
        Ok(it.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn plan_of(&self, file_id: i64) -> Result<Option<PlannedAction>> {
        let mut stmt = self.con.prepare(
            "SELECT action,reference_id,target_path,bytes,mad,reason
             FROM plan_actions WHERE file_id=?",
        )?;
        let row = stmt
            .query_row(params![file_id], |r| {
                Ok(PlannedAction {
                    file_id,
                    action: Action::parse(&r.get::<_, String>(0)?).unwrap_or(Action::Keep),
                    group_key: None,
                    reference_id: r.get(1)?,
                    target_path: r.get::<_, Option<String>>(2)?.map(PathBuf::from),
                    bytes: r.get::<_, i64>(3)?.max(0) as u64,
                    reason: r.get(5)?,
                })
            })
            .optional()?;
        Ok(row)
    }

    pub fn set_sidecar(&mut self, s: &Sidecar, root_id: i64) -> Result<i64> {
        self.con.execute(
            "INSERT INTO sidecars(root_id,path,google_id,title,taken_ts,creation_ts,description,matched_file_id)
             VALUES(?,?,?,?,?,?,?,?)
             ON CONFLICT(path) DO UPDATE SET
               root_id=excluded.root_id,
               google_id=excluded.google_id,
               title=excluded.title,
               taken_ts=excluded.taken_ts,
               creation_ts=excluded.creation_ts,
               description=excluded.description,
               matched_file_id=excluded.matched_file_id",
            params![
                root_id,
                s.path.to_string_lossy(),
                s.google_id,
                s.title,
                s.taken_ts,
                s.creation_ts,
                s.description,
                s.matched_file_id
            ],
        )?;
        Ok(self.con.last_insert_rowid())
    }

    pub fn sidecar_for_file(&self, file_id: i64) -> Result<Option<Sidecar>> {
        let mut stmt = self.con.prepare(
            "SELECT id,path,google_id,title,taken_ts,creation_ts,description,matched_file_id
             FROM sidecars WHERE matched_file_id=? ORDER BY id LIMIT 1",
        )?;
        let s = stmt
            .query_row(params![file_id], |r| {
                Ok(Sidecar {
                    id: r.get(0)?,
                    root: String::new(),
                    path: PathBuf::from(r.get::<_, String>(1)?),
                    google_id: r.get(2)?,
                    title: r.get(3)?,
                    taken_ts: r.get(4)?,
                    creation_ts: r.get(5)?,
                    description: r.get(6)?,
                    matched_file_id: r.get(7)?,
                })
            })
            .optional()?;
        Ok(s)
    }

    pub fn sidecars(&self) -> Result<Vec<Sidecar>> {
        let mut stmt = self.con.prepare(
            "SELECT s.id,s.path,s.google_id,s.title,s.taken_ts,s.creation_ts,
                    s.description,s.matched_file_id,r.label
             FROM sidecars s LEFT JOIN roots r ON r.id=s.root_id ORDER BY s.id",
        )?;
        let it = stmt.query_map([], |r| {
            Ok(Sidecar {
                id: r.get(0)?,
                root: r.get::<_, Option<String>>(8)?.unwrap_or_default(),
                path: PathBuf::from(r.get::<_, String>(1)?),
                google_id: r.get(2)?,
                title: r.get(3)?,
                taken_ts: r.get(4)?,
                creation_ts: r.get(5)?,
                description: r.get(6)?,
                matched_file_id: r.get(7)?,
            })
        })?;
        Ok(it.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn set_sidecar_match(&mut self, sidecar_id: i64, file_id: i64) -> Result<()> {
        self.con.execute(
            "UPDATE sidecars SET matched_file_id=? WHERE id=?",
            params![file_id, sidecar_id],
        )?;
        Ok(())
    }

    pub fn set_approx(
        &mut self,
        extra_id: i64,
        album_id: i64,
        distance: Option<u32>,
        same: bool,
        error: Option<&str>,
    ) -> Result<()> {
        self.con.execute(
            "INSERT OR REPLACE INTO approx_pairs(extra_id,album_id,distance,same,error)
             VALUES(?,?,?,?,?)",
            params![
                extra_id,
                album_id,
                distance.map(|d| d as i64),
                if same { 1 } else { 0 },
                error
            ],
        )?;
        Ok(())
    }

    pub fn approx(&self) -> Result<Vec<ApproxPair>> {
        let mut stmt = self.con.prepare(
            "SELECT extra_id,album_id,distance,same FROM approx_pairs ORDER BY extra_id",
        )?;
        let it = stmt.query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get::<_, Option<i64>>(2)?.map(|d| d as u32),
                r.get::<_, i64>(3)? == 1,
            ))
        })?;
        Ok(it.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

const FILE_SELECT: &str = "SELECT f.id,f.path,f.rel_path,f.media,f.size,f.mtime,
        f.width,f.height,f.duration,f.bitrate,f.codec,f.sha256,f.phash,f.dhash,f.ahash,
        f.capture_ts,f.capture_source,f.fingerprint_status,f.meta_status,f.error,
        r.label AS root_label
    FROM files f JOIN roots r ON r.id=f.root_id";

fn row_to_file(r: &Row<'_>) -> rusqlite::Result<FileRow> {
    Ok(FileRow {
        id: r.get("id")?,
        root: r.get("root_label")?,
        path: PathBuf::from(r.get::<_, String>("path")?),
        rel_path: r.get("rel_path")?,
        media: MediaKind::parse(&r.get::<_, String>("media")?),
        size: r.get::<_, i64>("size")?.max(0) as u64,
        mtime: r.get("mtime")?,
        width: r.get("width")?,
        height: r.get("height")?,
        duration: r.get("duration")?,
        bitrate: r.get::<_, Option<i64>>("bitrate")?.map(|b| b.max(0) as u64),
        codec: r.get("codec")?,
        sha256: r.get("sha256")?,
        phash: r.get("phash")?,
        dhash: r.get("dhash")?,
        ahash: r.get("ahash")?,
        capture_ts: r.get("capture_ts")?,
        capture_source: r.get("capture_source")?,
        fingerprint_status: r.get("fingerprint_status")?,
        meta_status: r.get("meta_status")?,
        error: r.get("error")?,
    })
}
