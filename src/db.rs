use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::Path;

use crate::app::IndexedFile;
use crate::config::Location;

pub fn open_db(db_path: &Path) -> Result<Connection> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create data dir: {}", parent.display()))?;
    }
    let conn = Connection::open(db_path)
        .with_context(|| format!("Failed to open database at {}", db_path.display()))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
    Ok(conn)
}

/// Write a batch of paths in a single transaction — dramatically faster than
/// calling upsert_file() per row (avoids 1 fsync per row).
pub fn batch_upsert_files(
    conn: &Connection,
    location_id: &str,
    paths: &[String],
    is_dir: bool,
) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    conn.execute_batch("BEGIN")?;
    {
        let mut stmt = conn.prepare_cached(
            "INSERT INTO files (location_id, path, mtime, is_dir)
             VALUES (?1, ?2, 0, ?3)
             ON CONFLICT(location_id, path) DO UPDATE
             SET mtime = excluded.mtime, is_dir = excluded.is_dir",
        )?;
        for path in paths {
            stmt.execute(params![location_id, path, is_dir as i64])?;
        }
    }
    conn.execute_batch("COMMIT")?;
    Ok(())
}

pub fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS files (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            location_id TEXT NOT NULL,
            path        TEXT NOT NULL,
            mtime       INTEGER NOT NULL DEFAULT 0,
            is_dir      INTEGER NOT NULL DEFAULT 0,
            UNIQUE(location_id, path)
        );
        CREATE INDEX IF NOT EXISTS idx_files_location ON files(location_id);
        CREATE INDEX IF NOT EXISTS idx_files_path ON files(path);
        ",
    )
    .context("Failed to create tables")?;

    // Migration: add is_dir column to existing DBs (ignore error if already present)
    let _ = conn.execute(
        "ALTER TABLE files ADD COLUMN is_dir INTEGER NOT NULL DEFAULT 0",
        [],
    );

    Ok(())
}


pub fn delete_location_files(conn: &Connection, location_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM files WHERE location_id = ?1",
        params![location_id],
    )
    .context("Failed to delete location files")?;
    Ok(())
}

pub fn clear_all_files(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM files", [])
        .context("Failed to clear all files")?;
    Ok(())
}

pub fn load_all_files(conn: &Connection, locations: &[Location]) -> Result<Vec<IndexedFile>> {
    let mut stmt =
        conn.prepare("SELECT location_id, path, is_dir FROM files ORDER BY path")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;

    let mut files = Vec::new();
    for row in rows {
        let (location_id, path, is_dir_int) = row?;
        let display_path = if let Some(loc) = locations.iter().find(|l| l.id == location_id) {
            loc.display_path(&path)
        } else {
            path.clone()
        };
        files.push(IndexedFile {
            location_id,
            path,
            display_path,
            is_dir: is_dir_int != 0,
        });
    }
    Ok(files)
}

