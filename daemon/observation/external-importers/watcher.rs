//! Polling directory watcher for Plane C external observation artifacts.
//! Scans configured directories (Goldberg/GSE, RPCS3/shadPS4 trophy dirs, Longshot `progress.sav`, `.oh-obs.json`)
//! and detects added/modified files without duplicating already-imported records.

use crate::core::config::WatchedObservationDir;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const MAX_SCAN_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified_ns: u128,
}

#[derive(Debug, Default)]
pub struct DirectoryWatcherState {
    seen: HashMap<PathBuf, FileStamp>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WatchPollSummary {
    pub files_scanned: usize,
    pub files_changed: usize,
    pub imported: usize,
    pub deduplicated: usize,
    pub errors: Vec<String>,
}

fn is_watchable_observation_file(path: &Path) -> bool {
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

fn collect_watchable_files(
    dir: &Path,
    depth: usize,
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
        if p.is_dir() {
            collect_watchable_files(&p, depth + 1, out);
        } else if p.is_file() && is_watchable_observation_file(&p) {
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

    /// Return `(path, canonical_game_id)` pairs for newly added or modified observation files.
    pub fn collect_changed_targets(
        &mut self,
        watched_dirs: &[WatchedObservationDir],
    ) -> (usize, Vec<(PathBuf, Option<String>)>) {
        let mut total_scanned = 0usize;
        let mut changed = Vec::new();

        for w in watched_dirs {
            if !w.path.exists() {
                continue;
            }
            let mut files = Vec::new();
            if w.path.is_file() {
                files.push(w.path.clone());
            } else {
                collect_watchable_files(&w.path, 0, &mut files);
            }

            for file in files {
                total_scanned += 1;
                let Some(stamp) = read_file_stamp(&file) else {
                    continue;
                };
                let is_changed = self.seen.get(&file) != Some(&stamp);
                if is_changed {
                    self.seen.insert(file.clone(), stamp);
                    changed.push((file, w.canonical_game_id.clone()));
                }
            }
        }

        (total_scanned, changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
