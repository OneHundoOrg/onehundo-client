use crate::packs::manifest::LoadedPack;
use crate::packs::matcher::{MatchQuery, PackRegistry};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub comm: String,
    pub exe_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedProcessMatch {
    pub pid: u32,
    pub executable: String,
    pub canonical_game_id: String,
    pub pack_id: String,
    pub game_title: String,
    pub edition_or_pack: String,
}

pub fn format_edition_or_pack(pack: &LoadedPack) -> String {
    pack.mapped_oap_edition
        .clone()
        .unwrap_or_else(|| pack.manifest.pack_id.clone())
}

fn current_os_label() -> &'static str {
    match std::env::consts::OS {
        "linux" => "linux",
        "windows" => "windows",
        "macos" => "macos",
        other => other,
    }
}

/// Scans `/proc/<pid>` entries on Linux to discover running game executables without spawning external tools.
#[cfg(target_os = "linux")]
pub fn scan_running_processes() -> Vec<ProcessInfo> {
    let self_pid = std::process::id();
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return out;
    };
    for entry in entries.flatten() {
        let fname = entry.file_name();
        let Some(pid_str) = fname.to_str() else {
            continue;
        };
        let Ok(pid) = pid_str.parse::<u32>() else {
            continue;
        };
        if pid == self_pid || pid <= 2 {
            continue;
        }
        let proc_dir = entry.path();
        let comm = fs::read_to_string(proc_dir.join("comm"))
            .ok()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let exe_name = fs::read_link(proc_dir.join("exe")).ok().and_then(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.trim_end_matches(" (deleted)").to_string())
        });
        if comm.is_empty() && exe_name.is_none() {
            continue;
        }
        out.push(ProcessInfo {
            pid,
            comm,
            exe_name,
        });
    }
    out
}

#[cfg(not(target_os = "linux"))]
pub fn scan_running_processes() -> Vec<ProcessInfo> {
    Vec::new()
}

fn is_ignored_system_binary(name: &str) -> bool {
    let base = Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
        .to_ascii_lowercase();
    matches!(
        base.as_str(),
        "onehundod"
            | "onehundo-desktop"
            | "onehundo"
            | "cargo"
            | "rustc"
            | "systemd"
            | "bash"
            | "sh"
            | "zsh"
            | "fish"
    )
}

pub fn match_running_processes(
    procs: &[ProcessInfo],
    packs: &PackRegistry,
) -> Vec<DetectedProcessMatch> {
    let os = current_os_label().to_string();
    let mut matches = Vec::new();
    for proc_info in procs {
        let mut candidates: Vec<&str> = Vec::new();
        if let Some(ref exe) = proc_info.exe_name {
            if !exe.is_empty() && !is_ignored_system_binary(exe) {
                candidates.push(exe.as_str());
            }
        }
        if !proc_info.comm.is_empty()
            && !is_ignored_system_binary(&proc_info.comm)
            && !candidates.iter().any(|c| *c == proc_info.comm.as_str())
        {
            candidates.push(proc_info.comm.as_str());
        }

        for candidate in candidates {
            let query = MatchQuery {
                executable: Some(candidate.to_string()),
                os: Some(os.clone()),
                ..Default::default()
            };
            if let Some((pack, matched)) = packs.match_pack(&query) {
                if matched.matched_by == "Executable" {
                    matches.push(DetectedProcessMatch {
                        pid: proc_info.pid,
                        executable: candidate.to_string(),
                        canonical_game_id: pack.manifest.canonical_game_id.clone(),
                        pack_id: pack.manifest.pack_id.clone(),
                        game_title: pack.manifest.title.clone(),
                        edition_or_pack: format_edition_or_pack(pack),
                    });
                    break;
                }
            }
        }
    }
    matches.sort_by(|a, b| a.canonical_game_id.cmp(&b.canonical_game_id));
    matches
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn matches_known_recomp_and_emulator_executables_and_ignores_daemon() {
        let mut reg = PackRegistry::new();
        let packs_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("packs");
        assert!(reg.discover_in_dir(&packs_dir) > 0);

        let procs = vec![
            ProcessInfo {
                pid: 1001,
                comm: "onehundo-deskto".to_string(),
                exe_name: Some("onehundo-desktop".to_string()),
            },
            ProcessInfo {
                pid: 1002,
                comm: "bbport".to_string(),
                exe_name: Some("bbport".to_string()),
            },
            ProcessInfo {
                pid: 1003,
                comm: "Zelda64Recompil".to_string(),
                exe_name: Some("Zelda64Recompiled".to_string()),
            },
            ProcessInfo {
                pid: 1004,
                comm: "firefox".to_string(),
                exe_name: Some("firefox".to_string()),
            },
        ];

        let detected = match_running_processes(&procs, &reg);
        let ids: Vec<&str> = detected.iter().map(|m| m.canonical_game_id.as_str()).collect();
        assert!(ids.contains(&"sony.bloodborne"), "detected={:?}", detected);
        assert!(ids.contains(&"nintendo.majoras_mask"), "detected={:?}", detected);
        assert_eq!(detected.len(), 2);
    }
}
