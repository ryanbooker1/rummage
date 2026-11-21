use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;
use tokio::sync::mpsc::Sender;

use crate::app::IndexMsg;
use crate::config::Location;
use crate::db;
use crate::ssh_client::SshClient;

pub async fn rebuild_index(
    locations: Vec<Location>,
    credentials: HashMap<String, String>,
    db_path: std::path::PathBuf,
    tx: Sender<IndexMsg>,
) {
    tokio::task::spawn_blocking(move || {
        if let Err(e) = rebuild_index_blocking(&locations, &credentials, &db_path, &tx) {
            eprintln!("Index rebuild error: {}", e);
        }
        let _ = tx.blocking_send(IndexMsg::Done);
    });
}

fn log_err(msg: &str) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true).append(true)
        .mode(0o600).open("/tmp/rummage-index.log")
    {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "[{}] {}", ts, msg);
    }
}

fn rebuild_index_blocking(
    locations: &[Location],
    credentials: &HashMap<String, String>,
    db_path: &Path,
    tx: &Sender<IndexMsg>,
) -> Result<()> {
    let conn = db::open_db(db_path)?;
    db::create_tables(&conn)?;

    let mut current = 0usize;

    for location in locations.iter().filter(|l| l.enabled) {
        if location.kind == "ssh" {
            let password = match credentials.get(&location.id) {
                Some(p) => p.clone(),
                None => {
                    let msg = format!("No credentials for SSH location '{}' — unlock first (Tab → Settings)", location.name);
                    log_err(&msg);
                    let _ = tx.blocking_send(IndexMsg::Error(msg));
                    continue;
                }
            };

            let ssh_addr = format!(
                "{}@{}:{}",
                location.ssh_user.as_deref().unwrap_or("user"),
                location.ssh_host.as_deref().unwrap_or("?"),
                location.ssh_port,
            );

            // Report "connecting" immediately
            let _ = tx.blocking_send(IndexMsg::Progress {
                location_name: location.name.clone(),
                current,
                total: 0,
                path: format!("Connecting to {}…", ssh_addr),
            });

            log_err(&format!("Attempting SSH connect to {} for location '{}'", ssh_addr, location.name));
            let client = match SshClient::connect(location, &password) {
                Ok(c) => {
                    log_err(&format!("SSH connected OK to {}", ssh_addr));
                    c
                }
                Err(e) => {
                    let msg = format!("SSH failed for '{}' ({}): {}", location.name, ssh_addr, e);
                    log_err(&msg);
                    let _ = tx.blocking_send(IndexMsg::Error(msg));
                    continue;
                }
            };

            db::delete_location_files(&conn, &location.id)?;

            // Warmup: run a trivial command before the first heavy find to avoid
            // an SSH channel setup race that causes find to return EOF immediately.
            let _ = client.exec_streamed("true", |_| {});

            // Report "listing files" before the slow find command
            let _ = tx.blocking_send(IndexMsg::Progress {
                location_name: location.name.clone(),
                current,
                total: 0,
                path: format!("Listing files on {}…", ssh_addr),
            });

            // Stream files into a buffer; batch-write to DB every BATCH_SIZE rows.
            // Writing one row per SQLite transaction is catastrophically slow at scale
            // (1.9M files × 1 fsync each = many minutes). Batching gives ~100× speedup.
            const BATCH: usize = 5_000;
            let loc_id = location.id.clone();
            let loc_name = location.name.clone();
            let mut file_buf: Vec<String> = Vec::with_capacity(BATCH);
            let mut last_path = String::new();

            let stream_result = client.list_files_streamed(&location.path, |file_path| {
                file_buf.push(file_path.to_string());
                last_path = file_path.to_string();
                current += 1;
                if file_buf.len() >= BATCH {
                    db::batch_upsert_files(&conn, &loc_id, &file_buf, false).ok();
                    file_buf.clear();
                    // Use try_send so a full channel never blocks the SSH read thread.
                    let _ = tx.try_send(IndexMsg::Progress {
                        location_name: loc_name.clone(),
                        current,
                        total: 0,
                        path: last_path.clone(),
                    });
                }
            });
            // Flush remainder
            if !file_buf.is_empty() {
                db::batch_upsert_files(&conn, &loc_id, &file_buf, false).ok();
            }
            if let Err(e) = stream_result {
                let msg = format!("SSH list files failed for '{}': {}", location.name, e);
                log_err(&msg);
                let _ = tx.blocking_send(IndexMsg::Error(msg));
                continue;
            }
            log_err(&format!("SSH files streamed for '{}': {} files", location.name, current));

            let loc_id = location.id.clone();
            let loc_name = location.name.clone();
            let _ = tx.blocking_send(IndexMsg::Progress {
                location_name: location.name.clone(),
                current,
                total: 0,
                path: format!("Listing dirs on {}…", ssh_addr),
            });
            let mut dir_buf: Vec<String> = Vec::with_capacity(BATCH);
            let mut last_dir = String::new();
            client.list_dirs_streamed(&location.path, |dir_path| {
                dir_buf.push(dir_path.to_string());
                last_dir = dir_path.to_string();
                current += 1;
                if dir_buf.len() >= BATCH {
                    db::batch_upsert_files(&conn, &loc_id, &dir_buf, true).ok();
                    dir_buf.clear();
                    let _ = tx.try_send(IndexMsg::Progress {
                        location_name: loc_name.clone(),
                        current,
                        total: 0,
                        path: last_dir.clone(),
                    });
                }
            }).ok();
            if !dir_buf.is_empty() {
                db::batch_upsert_files(&conn, &loc_id, &dir_buf, true).ok();
            }
            log_err(&format!("SSH index complete for '{}': {} total entries", location.name, current));
        } else {
            // Local indexing — walk everything (files + dirs)
            db::delete_location_files(&conn, &location.id)?;

            let root = std::path::Path::new(&location.path);
            if !root.exists() {
                eprintln!("Local path does not exist: {}", location.path);
                continue;
            }

            // Report start of this location immediately
            let _ = tx.blocking_send(IndexMsg::Progress {
                location_name: location.name.clone(),
                current,
                total: 0,
                path: location.path.clone(),
            });

            let walker = ignore::WalkBuilder::new(root)
                .hidden(true)
                .git_ignore(true)
                .filter_entry(|e| {
                    let name = e.file_name().to_string_lossy();
                    !matches!(name.as_ref(),
                        ".cache" | "node_modules" | ".git" | "Trash" | "__pycache__"
                    )
                })
                .build();

            const BATCH: usize = 5_000;
            let mut file_buf: Vec<(String, i64, bool)> = Vec::with_capacity(BATCH);

            for entry in walker.flatten() {
                let path = entry.path();
                let is_dir = path.is_dir();
                if path == root {
                    continue;
                }

                let mtime = path
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);

                let path_str = path.to_string_lossy().to_string();
                file_buf.push((path_str.clone(), mtime, is_dir));
                current += 1;

                if file_buf.len() >= BATCH {
                    let (dirs, files): (Vec<_>, Vec<_>) = file_buf.drain(..).partition(|(_, _, d)| *d);
                    let dir_paths: Vec<String> = dirs.into_iter().map(|(p, _, _)| p).collect();
                    let file_paths: Vec<String> = files.into_iter().map(|(p, _, _)| p).collect();
                    if !dir_paths.is_empty() {
                        db::batch_upsert_files(&conn, &location.id, &dir_paths, true)?;
                    }
                    if !file_paths.is_empty() {
                        db::batch_upsert_files(&conn, &location.id, &file_paths, false)?;
                    }
                    let _ = tx.blocking_send(IndexMsg::Progress {
                        location_name: location.name.clone(),
                        current,
                        total: 0,
                        path: path_str,
                    });
                }
            }

            // Flush remainder
            if !file_buf.is_empty() {
                let (dirs, files): (Vec<_>, Vec<_>) = file_buf.drain(..).partition(|(_, _, d)| *d);
                let dir_paths: Vec<String> = dirs.into_iter().map(|(p, _, _)| p).collect();
                let file_paths: Vec<String> = files.into_iter().map(|(p, _, _)| p).collect();
                if !dir_paths.is_empty() {
                    db::batch_upsert_files(&conn, &location.id, &dir_paths, true)?;
                }
                if !file_paths.is_empty() {
                    db::batch_upsert_files(&conn, &location.id, &file_paths, false)?;
                }
            }
        }
    }

    let _ = tx.blocking_send(IndexMsg::Progress {
        location_name: String::new(),
        current,
        total: 0,
        path: String::new(),
    });

    Ok(())
}
