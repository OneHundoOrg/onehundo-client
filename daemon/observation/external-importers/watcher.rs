//! Polling directory watcher for Plane C external observation artifacts.
//! Scans configured directories (Goldberg/GSE, RPCS3/shadPS4 trophy dirs, Longshot `progress.sav`,
//! `.oh-obs.json`, and pack-declared save `file_glob`s) and supports debounced change detection.

use super::declarative_save::safe_contained_path;
use crate::core::config::WatchedObservationDir;
use crate::packs::matcher::glob_matches;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

const MAX_SCAN_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified_ns: u128,
}

#[derive(Debug, Default)]
pub struct DirectoryWatcherState {
    seen: HashMap<PathBuf, FileStamp>,
    pending: HashMap<PathBuf, (FileStamp, Instant)>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WatchPollSummary {
    pub files_scanned: usize,
    pub files_changed: usize,
    pub imported: usize,
    pub deduplicated: usize,
    pub errors: Vec<String>,
}

fn is_builtin_watchable_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "achievements.json"
            | "achievements.ini"
            | "progress.sav"
            | "tropusr.dat"
            | "tropusr.xml"
            | "trop.xml"
            | "trophies.xml"
            | "tropusr.json"
            | "trophies.json"
            | "trophy.json"
    ) || lower.ends_with(".oh-obs.json")
}

fn is_watchable_for_globs(root: &Path, file: &Path, extra_globs: &[String]) -> bool {
    if is_builtin_watchable_file(file) {
        return true;
    }
    if extra_globs.is_empty() {
        return false;
    }
    let rel = file
        .strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/");
    let fname = file.file_name().and_then(|s| s.to_str()).unwrap_or("");
    extra_globs
        .iter()
        .any(|g| glob_matches(g, &rel) || glob_matches(g, fname))
}

fn collect_watchable_files(
    root: &Path,
    dir: &Path,
    depth: usize,
    extra_globs: &[String],
    out: &mut Vec<PathBuf>,
) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    children.sort();
    for p in children {
        let Ok(meta) = fs::symlink_metadata(&p) else {
            continue;
        };
        if meta.file_type().is_symlink() && safe_contained_path(root, &p).is_err() {
            continue;
        }
        if p.is_dir() {
            collect_watchable_files(root, &p, depth + 1, extra_globs, out);
        } else if p.is_file() && is_watchable_for_globs(root, &p, extra_globs) {
            out.push(p);
        }
    }
}

fn read_file_stamp(path: &Path) -> Option<FileStamp> {
    let meta = fs::metadata(path).ok()?;
    let modified_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    Some(FileStamp {
        len: meta.len(),
        modified_ns,
    })
}

impl DirectoryWatcherState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return `(path, canonical_game_id)` pairs for newly added or modified observation files immediately.
    pub fn collect_changed_targets(
        &mut self,
        watched_dirs: &[WatchedObservationDir],
    ) -> (usize, Vec<(PathBuf, Option<String>)>) {
        self.collect_changed_targets_with_globs(watched_dirs, &[], 0)
    }

    /// Scan configured directories using built-in patterns plus `extra_globs` from loaded packs.
    /// When `debounce_ms > 0`, changed files wait until their size + mtime remain stable for `debounce_ms`.
    pub fn collect_changed_targets_with_globs(
        &mut self,
        watched_dirs: &[WatchedObservationDir],
        extra_globs: &[String],
        debounce_ms: u64,
    ) -> (usize, Vec<(PathBuf, Option<String>)>) {
        let mut total_scanned = 0usize;
        let mut changed = Vec::new();
        let settle = Duration::from_millis(debounce_ms);

        for w in watched_dirs {
            if !w.path.exists() {
                continue;
            }
            let mut files = Vec::new();
            if w.path.is_file() {
                files.push(w.path.clone());
            } else {
                collect_watchable_files(&w.path, &w.path, 0, extra_globs, &mut files);
            }

            for file in files {
                total_scanned += 1;
                let Some(stamp) = read_file_stamp(&file) else {
                    continue;
                };
                if self.seen.get(&file) == Some(&stamp) {
                    self.pending.remove(&file);
                    continue;
                }

                if debounce_ms == 0 {
                    self.pending.remove(&file);
                    self.seen.insert(file.clone(), stamp);
                    changed.push((file, w.canonical_game_id.clone()));
                } else {
                    match self.pending.get(&file) {
                        Some((prev_stamp, since))
                            if prev_stamp == &stamp && since.elapsed() >= settle =>
                        {
                            self.pending.remove(&file);
                            self.seen.insert(file.clone(), stamp);
                            changed.push((file, w.canonical_game_id.clone()));
                        }
                        Some((prev_stamp, _)) if prev_stamp == &stamp => {}
                        _ => {
                            self.pending.insert(file, (stamp, Instant::now()));
                        }
                    }
                }
            }
        }

        (total_scanned, changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn discovers_launcher_v2_nested_longshot_saves() {
        let base = std::env::temp_dir().join(format!(
            "onehundo_watcher_l2_{}",
            crate::core::config::generate_uuid_v4()
        ));
        let save_slot = base
            .join("default")
            .join("saves")
            .join("save")
            .join("5a3c9e1f0b7d24680000000000000001")
            .join("slot_00");
        let work_slot = base
            .join("default")
            .join("saves")
            .join("save-work")
            .join("5a3c9e1f0b7d24680000000000000001")
            .join("slot_01");
        fs::create_dir_all(&save_slot).unwrap();
        fs::create_dir_all(&work_slot).unwrap();
        let p1 = save_slot.join("progress.sav");
        let p2 = work_slot.join("progress.sav");
        fs::write(&p1, b"stub").unwrap();
        fs::write(&p2, b"stub").unwrap();

        let mut watcher = DirectoryWatcherState::new();
        let watched = vec![WatchedObservationDir {
            path: base.clone(),
            canonical_game_id: Some("nintendo.oot.longshot.pc".to_string()),
        }];
        let (scanned, changed) = watcher.collect_changed_targets(&watched);
        assert_eq!(scanned, 2);
        assert_eq!(changed.len(), 2);
        assert!(changed.iter().any(|(p, _)| p == &p1));
        assert!(changed.iter().any(|(p, _)| p == &p2));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn debounces_chunked_writes_until_stable() {
        let base = std::env::temp_dir().join(format!(
            "onehundo_watcher_debounce_{}",
            crate::core::config::generate_uuid_v4()
        ));
        fs::create_dir_all(&base).unwrap();
        let custom_save = base.join("slot1.ohsave");
        fs::write(&custom_save, b"partial").unwrap();

        let globs = vec!["**/*.ohsave".to_string()];
        let watched = vec![WatchedObservationDir {
            path: base.clone(),
            canonical_game_id: Some("custom.game".to_string()),
        }];
        let mut watcher = DirectoryWatcherState::new();

        // First poll with 40ms debounce holds file pending until settle window passes
        let (s1, c1) = watcher.collect_changed_targets_with_globs(&watched, &globs, 40);
        assert_eq!(s1, 1);
        assert!(c1.is_empty());

        thread::sleep(Duration::from_millis(55));
        let (s2, c2) = watcher.collect_changed_targets_with_globs(&watched, &globs, 40);
        assert_eq!(s2, 1);
        assert_eq!(c2.len(), 1);
        assert_eq!(c2[0].0, custom_save);

        let _ = fs::remove_dir_all(&base);
    }
}
