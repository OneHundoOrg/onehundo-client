use crate::core::config::WatchedObservationDir;
use crate::packs::manifest::{LoadedPack, MatcherType};
use crate::packs::matcher::{glob_matches, MatchQuery, PackMatchResult, PackRegistry};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub comm: String,
    pub exe_name: Option<String>,
    pub exe_path: Option<PathBuf>,
    pub cmdline: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub fd_paths: Vec<PathBuf>,
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

fn is_wrapper_binary(name: &str) -> bool {
    let base = Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
        .to_ascii_lowercase();
    matches!(
        base.as_str(),
        "apprun"
            | "wine"
            | "wine64"
            | "wine-preloader"
            | "wine64-preloader"
            | "proton"
            | "umu-run"
            | "pressure-vessel-wrap"
            | "reaper"
            | "gamemoderun"
            | "mangohud"
            | "gamescope"
            | "steam-runtime-launch-client"
            | "env"
            | "python"
            | "python3"
    )
}

fn is_ignored_system_binary(name: &str) -> bool {
    let base = Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
        .to_ascii_lowercase();
    if base.starts_with("onehundo") && base.ends_with(".appimage") {
        return true;
    }
    matches!(
        base.as_str(),
        "onehundod"
            | "onehundo-desktop"
            | "onehundo-deskto"
            | "onehundo"
            | "onehundo-x86_64.appimage"
            | "wineserver"
            | "cargo"
            | "rustc"
            | "systemd"
            | "bash"
            | "sh"
            | "zsh"
            | "fish"
            | "grep"
            | "rg"
            | "find"
            | "code"
            | "vim"
            | "nvim"
            | "firefox"
            | "chrome"
            | "chromium"
    )
}

fn basename_from_arg(arg: &str) -> Option<String> {
    let trimmed = arg.trim().trim_matches('"').trim_matches('\'');
    if trimmed.is_empty() || trimmed.starts_with('-') {
        return None;
    }
    let last = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed).trim();
    if last.is_empty() || is_wrapper_binary(last) || is_ignored_system_binary(last) {
        None
    } else {
        Some(last.to_string())
    }
}

/// Resolves the underlying game executable basename from `/proc/<pid>/cmdline` when `exe`
/// points to a generic wrapper (`AppRun`, Wine/Proton preloader, Steam reaper, `gamemoderun`, etc.).
fn cmdline_executable_hint_from_parts(parts: &[String], comm: &str) -> Option<String> {
    if parts.is_empty() {
        return None;
    }
    if let Some(first_base) = parts.first().and_then(|a| basename_from_arg(a)) {
        let lower = first_base.to_ascii_lowercase();
        if lower.ends_with(".appimage")
            || lower.ends_with(".exe")
            || !is_ignored_system_binary(comm)
        {
            return Some(first_base);
        }
    }
    for arg in parts.iter().skip(1) {
        if let Some(base) = basename_from_arg(arg) {
            let lower = base.to_ascii_lowercase();
            if lower.ends_with(".exe") || lower.ends_with(".appimage") {
                return Some(base);
            }
        }
    }
    for arg in parts.iter().skip(1) {
        let trimmed = arg.trim().trim_matches('"').trim_matches('\'');
        if (trimmed.contains('/') || trimmed.contains('\\')) && !trimmed.contains("=/") {
            if let Some(base) = basename_from_arg(trimmed) {
                return Some(base);
            }
        }
    }
    None
}

fn normalize_host_path_from_arg(arg: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    let trimmed = arg.trim().trim_matches('"').trim_matches('\'');
    if trimmed.is_empty() || trimmed.starts_with('-') {
        return None;
    }
    // Map Wine Z:\foo\bar paths on Linux/Unix to /foo/bar
    if let Some(rest) = trimmed
        .strip_prefix("Z:\\")
        .or_else(|| trimmed.strip_prefix("z:\\"))
        .or_else(|| trimmed.strip_prefix("Z:/"))
        .or_else(|| trimmed.strip_prefix("z:/"))
    {
        return Some(PathBuf::from(format!("/{}", rest.replace('\\', "/"))));
    }
    let pb = PathBuf::from(trimmed);
    if pb.is_absolute() {
        Some(pb)
    } else {
        cwd.map(|c| c.join(pb))
    }
}

fn is_skip_fd_target(p: &Path) -> bool {
    let s = p.to_string_lossy();
    s.starts_with("/dev/")
        || s.starts_with("/proc/")
        || s.starts_with("/sys/")
        || s.starts_with("/usr/")
        || s.starts_with("/lib")
        || s.starts_with("/etc/")
        || s.starts_with("socket:")
        || s.starts_with("pipe:")
        || s.starts_with("anon_inode:")
        || s.starts_with("memfd:")
        || s.contains("/fontconfig/")
        || s.ends_with(".so")
        || s.contains(".so.")
}

#[cfg(target_os = "linux")]
fn read_proc_fd_paths(proc_dir: &Path, limit: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(proc_dir.join("fd")) else {
        return out;
    };
    let mut seen = HashSet::new();
    for entry in rd.flatten().take(limit) {
        let Ok(target) = fs::read_link(entry.path()) else {
            continue;
        };
        if !target.is_absolute() || is_skip_fd_target(&target) {
            continue;
        }
        if seen.insert(target.clone()) {
            out.push(target);
        }
    }
    out
}

fn base_name_lower(s: &str) -> String {
    Path::new(s)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(s)
        .to_ascii_lowercase()
}

fn is_opengoal_candidate(proc_info: &ProcessInfo) -> bool {
    let mut names = Vec::new();
    if let Some(ref exe) = proc_info.exe_name {
        names.push(base_name_lower(exe));
    }
    if !proc_info.comm.is_empty() {
        names.push(base_name_lower(&proc_info.comm));
    }
    if let Some(ref ep) = proc_info.exe_path {
        if let Some(name) = ep.file_name().and_then(|s| s.to_str()) {
            names.push(name.to_ascii_lowercase());
        }
    }
    names
        .iter()
        .any(|n| n == "gk" || n == "gk.exe" || n.starts_with("opengoal"))
}

/// Identifies known emulator launchers from process executable/comm without false-matching
/// desktop utilities (e.g. KDE's file manager binary `/usr/bin/dolphin`).
pub fn detect_emulator_launcher(proc_info: &ProcessInfo) -> Option<&'static str> {
    let mut names = Vec::new();
    if let Some(ref exe) = proc_info.exe_name {
        names.push(base_name_lower(exe));
    }
    if !proc_info.comm.is_empty() {
        names.push(base_name_lower(&proc_info.comm));
    }
    if let Some(ref ep) = proc_info.exe_path {
        names.push(ep.to_string_lossy().to_ascii_lowercase());
    }

    for n in &names {
        if n.contains("shadps4") {
            return Some("shadps4");
        }
        if n.contains("rpcs3") {
            return Some("rpcs3");
        }
        if n == "cemu"
            || n == "cemu.exe"
            || n.starts_with("cemu-")
            || n.starts_with("cemu_")
            || n.contains("info.cemu.cemu")
        {
            return Some("cemu");
        }
        if n.contains("pcsx2") {
            return Some("pcsx2");
        }
        if n.contains("vita3k") {
            return Some("vita3k");
        }
        if n.contains("retroarch") {
            return Some("retroarch");
        }
        if n.contains("mupen64plus")
            || n.contains("simple64")
            || n.contains("project64")
            || n == "rmg"
            || n == "rmg.exe"
            || n.contains("rosalie")
        {
            return Some("mupen64plus");
        }
        if n.contains("ryujinx")
            || n.contains("yuzu")
            || n.contains("suyu")
            || n.contains("citron")
            || n == "eden"
            || n.starts_with("eden-")
            || n.starts_with("eden_")
            || n.contains("eden.appimage")
        {
            return Some("switch");
        }
        if n.contains("dolphin-emu")
            || n.contains("dolphin_emu")
            || n == "dolphin.exe"
            || n.contains("org.dolphinemu")
        {
            return Some("dolphin");
        }
        if n.contains("dolphin") && (n.contains("emu") || n.ends_with(".appimage")) {
            return Some("dolphin");
        }
    }

    // Avoid mistaking KDE's `dolphin` file manager for Dolphin Emulator unless args/FDs show GC/Wii media.
    if names.iter().any(|n| n == "dolphin") {
        let has_emu_evidence = proc_info.cmdline.iter().any(|a| {
            let l = a.to_ascii_lowercase();
            l.contains("dolphin-emu")
                || l.ends_with(".rvz")
                || l.ends_with(".gcm")
                || l.ends_with(".wbfs")
                || l.ends_with(".gcz")
        }) || proc_info.fd_paths.iter().any(|p| {
            let l = p.to_string_lossy().to_ascii_lowercase();
            l.contains("dolphin-emu")
                || l.ends_with(".rvz")
                || l.ends_with(".gcm")
                || l.ends_with(".wbfs")
                || l.ends_with(".gci")
        });
        if has_emu_evidence {
            return Some("dolphin");
        }
    }

    None
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
        if !comm.is_empty() && is_ignored_system_binary(&comm) && !is_wrapper_binary(&comm) {
            continue;
        }
        let exe_path = fs::read_link(proc_dir.join("exe")).ok();
        let raw_exe_name = exe_path.as_ref().and_then(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.trim_end_matches(" (deleted)").to_string())
        });
        let cmdline: Vec<String> = fs::read(proc_dir.join("cmdline"))
            .ok()
            .map(|bytes| {
                bytes
                    .split(|b| *b == 0)
                    .filter(|s| !s.is_empty())
                    .take(32)
                    .map(|s| String::from_utf8_lossy(s).into_owned())
                    .collect()
            })
            .unwrap_or_default();

        let exe_name = match raw_exe_name {
            Some(ref name) if is_wrapper_binary(name) => {
                cmdline_executable_hint_from_parts(&cmdline, &comm).or_else(|| Some(name.clone()))
            }
            Some(name) => Some(name),
            None => cmdline_executable_hint_from_parts(&cmdline, &comm),
        };

        if let Some(ref name) = exe_name {
            if is_ignored_system_binary(name) {
                continue;
            }
        }
        if comm.is_empty() && exe_name.is_none() {
            continue;
        }

        let cwd = fs::read_link(proc_dir.join("cwd")).ok();
        let mut info = ProcessInfo {
            pid,
            comm,
            exe_name,
            exe_path,
            cmdline,
            cwd,
            fd_paths: Vec::new(),
        };
        if detect_emulator_launcher(&info).is_some() || is_opengoal_candidate(&info) {
            info.fd_paths = read_proc_fd_paths(&proc_dir, 256);
        }
        out.push(info);
    }
    out
}

#[cfg(not(target_os = "linux"))]
pub fn scan_running_processes() -> Vec<ProcessInfo> {
    Vec::new()
}

fn dir_contains_watchable_file(dir: &Path, watch_file_globs: &[String]) -> bool {
    let builtin_names = [
        "trophies.jsonl",
        "trophies.json",
        "trophy.json",
        "tropusr.dat",
        "tropusr.xml",
        "trop.xml",
        "trop.sfm",
        "tropconf.sfm",
        "trophies.xml",
        "tropusr.json",
        "achievements.json",
        "achievements.ini",
        "progress.sav",
    ];
    let Ok(rd) = fs::read_dir(dir) else {
        return false;
    };
    for entry in rd.flatten().take(128) {
        let p = entry.path();
        if p.is_file() {
            if let Some(fname) = p.file_name().and_then(|s| s.to_str()) {
                let lower = fname.to_ascii_lowercase();
                if builtin_names.contains(&lower.as_str()) || lower.ends_with(".oh-obs.json") {
                    return true;
                }
                let p_norm = p.to_string_lossy().replace('\\', "/");
                if watch_file_globs
                    .iter()
                    .any(|g| glob_matches(g, fname) || glob_matches(g, &p_norm))
                {
                    return true;
                }
            }
        }
    }
    dir.join("savedata").is_dir()
}

fn is_unsafe_broad_root(dir: &Path) -> bool {
    if dir == Path::new("/") || dir == Path::new("/usr") || dir == Path::new("/tmp") {
        return true;
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() && dir == Path::new(&home) {
            return true;
        }
    }
    false
}

pub fn discover_observation_dirs_from_hints(
    canonical_game_id: &str,
    cwd: Option<&Path>,
    exe_path: Option<&Path>,
    cmdline: &[String],
    fd_paths: &[PathBuf],
    watch_file_globs: &[String],
) -> Vec<WatchedObservationDir> {
    let mut dirs = Vec::new();
    let mut candidates: Vec<PathBuf> = Vec::new();

    for win in cmdline.windows(2) {
        if win[0] == "--user" || win[0] == "--save-dir" || win[0] == "-proj-path" {
            if let Some(p) = normalize_host_path_from_arg(&win[1], cwd) {
                candidates.push(p);
            }
        }
    }
    // Check wrapper target binaries inside cmdline (AppImage / Wine / Proton `.exe`)
    for arg in cmdline {
        let lower = arg.to_ascii_lowercase();
        if lower.ends_with(".exe") || lower.ends_with(".appimage") {
            if let Some(host) = normalize_host_path_from_arg(arg, cwd) {
                if let Some(parent) = host.parent() {
                    candidates.push(parent.to_path_buf());
                    candidates.push(parent.join("user"));
                    candidates.push(parent.join("Save"));
                    candidates.push(parent.join("saves"));
                    candidates.push(parent.join("savedata"));
                }
            }
        }
    }
    // Check open FDs belonging to the running game or emulator
    for fd in fd_paths {
        if let Some(parent) = fd.parent() {
            candidates.push(parent.to_path_buf());
        }
    }
    if let Some(c) = cwd {
        candidates.push(c.join("user"));
        candidates.push(c.join("Save"));
        candidates.push(c.join("saves"));
        candidates.push(c.join("savedata"));
        candidates.push(c.to_path_buf());
    }
    if let Some(ep) = exe_path.and_then(|p| p.parent()) {
        candidates.push(ep.join("user"));
        candidates.push(ep.join("Save"));
        candidates.push(ep.join("saves"));
        candidates.push(ep.to_path_buf());
        if let Some(gp) = ep.parent() {
            candidates.push(gp.join("user"));
        }
    }

    for cand in candidates {
        if !cand.is_dir() || is_unsafe_broad_root(&cand) {
            continue;
        }
        if !dir_contains_watchable_file(&cand, watch_file_globs) {
            continue;
        }
        let canon = fs::canonicalize(&cand).unwrap_or(cand);
        if !dirs
            .iter()
            .any(|d: &WatchedObservationDir| d.path == canon)
        {
            dirs.push(WatchedObservationDir {
                path: canon,
                canonical_game_id: Some(canonical_game_id.to_string()),
            });
        }
    }
    dirs
}

#[cfg(target_os = "linux")]
pub fn discover_process_observation_dirs_with_globs(
    matches: &[DetectedProcessMatch],
    watch_file_globs: &[String],
) -> Vec<WatchedObservationDir> {
    let mut dirs = Vec::new();
    for m in matches {
        let proc_dir = PathBuf::from(format!("/proc/{}", m.pid));
        let cwd = fs::read_link(proc_dir.join("cwd")).ok();
        let exe_path = fs::read_link(proc_dir.join("exe")).ok();
        let cmdline: Vec<String> = fs::read(proc_dir.join("cmdline"))
            .ok()
            .map(|bytes| {
                bytes
                    .split(|b| *b == 0)
                    .filter(|s| !s.is_empty())
                    .take(32)
                    .map(|s| String::from_utf8_lossy(s).into_owned())
                    .collect()
            })
            .unwrap_or_default();
        let fd_paths = read_proc_fd_paths(&proc_dir, 256);
        for d in discover_observation_dirs_from_hints(
            &m.canonical_game_id,
            cwd.as_deref(),
            exe_path.as_deref(),
            &cmdline,
            &fd_paths,
            watch_file_globs,
        ) {
            if !dirs.iter().any(|existing: &WatchedObservationDir| existing.path == d.path) {
                dirs.push(d);
            }
        }
    }
    dirs
}

#[cfg(not(target_os = "linux"))]
pub fn discover_process_observation_dirs_with_globs(
    _matches: &[DetectedProcessMatch],
    _watch_file_globs: &[String],
) -> Vec<WatchedObservationDir> {
    Vec::new()
}

pub fn discover_process_observation_dirs(
    matches: &[DetectedProcessMatch],
) -> Vec<WatchedObservationDir> {
    discover_process_observation_dirs_with_globs(matches, &[])
}

fn contains_token_ci(haystack: &str, token: &str) -> bool {
    let h_lower = haystack.to_ascii_lowercase();
    let t_lower = token.to_ascii_lowercase();
    let mut search_from = 0usize;
    while let Some(rel_idx) = h_lower[search_from..].find(&t_lower) {
        let start = search_from + rel_idx;
        let end = start + t_lower.len();
        let prev_ok = h_lower[..start]
            .chars()
            .next_back()
            .map(|c| !c.is_ascii_alphanumeric())
            .unwrap_or(true);
        let next_ok = h_lower[end..]
            .chars()
            .next()
            .map(|c| !c.is_ascii_alphanumeric())
            .unwrap_or(true);
        if prev_ok && next_ok {
            return true;
        }
        search_from = end;
    }
    false
}

fn detect_opengoal_recomp_id(proc_info: &ProcessInfo) -> Option<&'static str> {
    let mut hints = Vec::new();
    hints.extend(proc_info.cmdline.iter().cloned());
    if let Some(ref c) = proc_info.cwd {
        hints.push(c.to_string_lossy().into_owned());
    }
    if let Some(ref ep) = proc_info.exe_path {
        hints.push(ep.to_string_lossy().into_owned());
    }
    for fd in &proc_info.fd_paths {
        hints.push(fd.to_string_lossy().into_owned());
    }

    for hint in &hints {
        if contains_token_ci(hint, "opengoal_jak3")
            || contains_token_ci(hint, "jak3")
            || contains_token_ci(hint, "jak-3")
            || contains_token_ci(hint, "jak_3")
        {
            return Some("opengoal_jak3");
        }
    }
    for hint in &hints {
        if contains_token_ci(hint, "opengoal_jak2")
            || contains_token_ci(hint, "jak2")
            || contains_token_ci(hint, "jak-2")
            || contains_token_ci(hint, "jak_2")
        {
            return Some("opengoal_jak2");
        }
    }
    for hint in &hints {
        if contains_token_ci(hint, "opengoal_jak1")
            || contains_token_ci(hint, "jak1")
            || contains_token_ci(hint, "jak-1")
            || contains_token_ci(hint, "jak_1")
            || contains_token_ci(hint, "jak_and_daxter")
            || contains_token_ci(hint, "jak-and-daxter")
            || contains_token_ci(hint, "precursor_legacy")
        {
            return Some("opengoal_jak1");
        }
    }
    None
}

fn extract_steam_app_id(cmdline: &[String]) -> Option<String> {
    for arg in cmdline {
        let lower = arg.to_ascii_lowercase();
        if let Some(val) = lower.strip_prefix("appid=") {
            let clean = val.trim();
            if !clean.is_empty() && clean.chars().all(|c| c.is_ascii_digit()) {
                return Some(clean.to_string());
            }
        }
    }
    for win in cmdline.windows(2) {
        if win[0].eq_ignore_ascii_case("-applaunch") || win[0].eq_ignore_ascii_case("applaunch") {
            let clean = win[1].trim();
            if !clean.is_empty() && clean.chars().all(|c| c.is_ascii_digit()) {
                return Some(clean.to_string());
            }
        }
    }
    None
}

fn collect_proc_hint_strings(proc_info: &ProcessInfo) -> (Vec<String>, Vec<String>) {
    let mut hints = Vec::new();
    let mut compacted = Vec::new();
    let mut push_hint = |raw: String| {
        let norm = raw.replace('\\', "/").to_ascii_lowercase();
        if !norm.is_empty() {
            compacted.push(norm.replace(['/', '-', '_', '.'], ""));
            hints.push(norm);
        }
    };
    for a in &proc_info.cmdline {
        push_hint(a.clone());
    }
    if let Some(ref c) = proc_info.cwd {
        push_hint(c.to_string_lossy().into_owned());
    }
    if let Some(ref ep) = proc_info.exe_path {
        push_hint(ep.to_string_lossy().into_owned());
    }
    for fd in &proc_info.fd_paths {
        push_hint(fd.to_string_lossy().into_owned());
    }
    (hints, compacted)
}

fn hint_matches_token(hints: &[String], compacted: &[String], token: &str) -> bool {
    let t = token.trim().to_ascii_lowercase();
    if t.len() < 4 {
        return false;
    }
    let t_compact = t.replace(['/', '-', '_', '.'], "");
    hints.iter().any(|h| h.contains(&t))
        || (t_compact.len() >= 8 && compacted.iter().any(|h| h.contains(&t_compact)))
}

struct ScoredProcessCandidate<'a> {
    score: (u8, &'a str),
    pack: &'a LoadedPack,
    result: PackMatchResult,
    display_exec: String,
}

fn consider_candidate<'a>(
    best: &mut Option<ScoredProcessCandidate<'a>>,
    pack: &'a LoadedPack,
    res: PackMatchResult,
    display_exec: String,
) {
    let key = (res.precedence_rank, pack.manifest.pack_id.as_str());
    let better = best.as_ref().map(|b| key < b.score).unwrap_or(true);
    if better {
        *best = Some(ScoredProcessCandidate {
            score: key,
            pack,
            result: res,
            display_exec,
        });
    }
}

pub fn match_running_processes(
    procs: &[ProcessInfo],
    packs: &PackRegistry,
) -> Vec<DetectedProcessMatch> {
    let os = current_os_label().to_string();
    let mut matches = Vec::new();

    for proc_info in procs {
        let mut best: Option<ScoredProcessCandidate<'_>> = None;
        let exec_display_base = proc_info
            .exe_name
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| proc_info.comm.clone());

        // 1. OpenGOAL (`gk` / `gk.exe` / `opengoal*`): disambiguate Jak 1 / Jak II / Jak 3
        if is_opengoal_candidate(proc_info) {
            if let Some(recomp_id) = detect_opengoal_recomp_id(proc_info) {
                let query = MatchQuery {
                    recomp_game_id: Some(recomp_id.to_string()),
                    launcher: Some("opengoal".to_string()),
                    os: Some(os.clone()),
                    ..Default::default()
                };
                if let Some((pack, res)) = packs.match_pack(&query) {
                    let label = format!("{} ({})", exec_display_base, recomp_id);
                    consider_candidate(&mut best, pack, res, label);
                }
            }
        }

        // 2. Steam Proton / Linux `AppId=<id>` launch wrapper hints
        if let Some(app_id) = extract_steam_app_id(&proc_info.cmdline) {
            let query = MatchQuery {
                app_id: Some(app_id.clone()),
                launcher: Some("steam".to_string()),
                os: Some(os.clone()),
                ..Default::default()
            };
            if let Some((pack, res)) = packs.match_pack(&query) {
                if res.matched_by == "AppId" {
                    let label = format!("{} (Steam {})", exec_display_base, app_id);
                    consider_candidate(&mut best, pack, res, label);
                }
            }
        }

        // 3. Emulators (`shadps4`, `rpcs3`, `cemu`, `dolphin`, `pcsx2`, `vita3k`, `retroarch`, `mupen64plus`, Switch)
        if let Some(emu) = detect_emulator_launcher(proc_info) {
            let (hints, compacted) = collect_proc_hint_strings(proc_info);
            for pack in packs.list() {
                for m in &pack.manifest.matchers {
                    match m.matcher_type {
                        MatcherType::PlatformId => {
                            if hint_matches_token(&hints, &compacted, &m.value) {
                                let query = MatchQuery {
                                    platform_id: Some(m.value.clone()),
                                    launcher: Some(emu.to_string()),
                                    os: Some(os.clone()),
                                    ..Default::default()
                                };
                                if let Some((matched_pack, res)) = packs.match_pack(&query) {
                                    consider_candidate(
                                        &mut best,
                                        matched_pack,
                                        res,
                                        format!("{} ({})", emu, m.value),
                                    );
                                }
                            }
                        }
                        MatcherType::RecompGameId => {
                            if hint_matches_token(&hints, &compacted, &m.value) {
                                let query = MatchQuery {
                                    recomp_game_id: Some(m.value.clone()),
                                    launcher: Some(emu.to_string()),
                                    os: Some(os.clone()),
                                    ..Default::default()
                                };
                                if let Some((matched_pack, res)) = packs.match_pack(&query) {
                                    consider_candidate(
                                        &mut best,
                                        matched_pack,
                                        res,
                                        format!("{} ({})", emu, m.value),
                                    );
                                }
                            }
                        }
                        MatcherType::RomHash => {
                            if hint_matches_token(&hints, &compacted, &m.value) {
                                let query = MatchQuery {
                                    rom_hash: Some(m.value.clone()),
                                    launcher: Some(emu.to_string()),
                                    os: Some(os.clone()),
                                    ..Default::default()
                                };
                                if let Some((matched_pack, res)) = packs.match_pack(&query) {
                                    consider_candidate(
                                        &mut best,
                                        matched_pack,
                                        res,
                                        format!("{} ({})", emu, m.value),
                                    );
                                }
                            }
                        }
                        MatcherType::SavePathGlob => {
                            for p in proc_info
                                .fd_paths
                                .iter()
                                .filter_map(|fp| fp.to_str())
                            {
                                let p_norm = p.replace('\\', "/");
                                if glob_matches(&m.value, &p_norm) {
                                    let query = MatchQuery {
                                        save_path: Some(p_norm),
                                        launcher: Some(emu.to_string()),
                                        os: Some(os.clone()),
                                        ..Default::default()
                                    };
                                    if let Some((matched_pack, res)) = packs.match_pack(&query) {
                                        consider_candidate(
                                            &mut best,
                                            matched_pack,
                                            res,
                                            format!("{} ({})", emu, matched_pack.manifest.pack_id),
                                        );
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            if best.is_none() && emu == "retroarch" {
                let query = MatchQuery {
                    executable: Some("retroarch".to_string()),
                    launcher: Some("retroarch".to_string()),
                    os: Some(os.clone()),
                    ..Default::default()
                };
                if let Some((matched_pack, res)) = packs.match_pack(&query) {
                    consider_candidate(&mut best, matched_pack, res, exec_display_base.clone());
                }
            }
        }

        // 4. Native / wrapper game executables (`exe_name`, `comm`)
        let mut exec_candidates: Vec<&str> = Vec::new();
        if let Some(ref exe) = proc_info.exe_name {
            if !exe.is_empty() && !is_ignored_system_binary(exe) {
                exec_candidates.push(exe.as_str());
            }
        }
        if !proc_info.comm.is_empty()
            && !is_ignored_system_binary(&proc_info.comm)
            && !exec_candidates.iter().any(|c| *c == proc_info.comm.as_str())
        {
            exec_candidates.push(proc_info.comm.as_str());
        }

        for cand in exec_candidates {
            let query = MatchQuery {
                executable: Some(cand.to_string()),
                os: Some(os.clone()),
                ..Default::default()
            };
            if let Some((pack, res)) = packs.match_pack(&query) {
                if res.matched_by == "Executable" {
                    // Never allow generic OpenGOAL `gk` fallback to misattribute Jak 1/2/3.
                    if is_opengoal_candidate(proc_info) && pack.manifest.pack_id.starts_with("opengoal.") {
                        continue;
                    }
                    consider_candidate(&mut best, pack, res, cand.to_string());
                }
            }
        }

        if let Some(chosen) = best {
            matches.push(DetectedProcessMatch {
                pid: proc_info.pid,
                executable: chosen.display_exec,
                canonical_game_id: chosen.pack.manifest.canonical_game_id.clone(),
                pack_id: chosen.pack.manifest.pack_id.clone(),
                game_title: chosen.pack.manifest.title.clone(),
                edition_or_pack: format_edition_or_pack(chosen.pack),
            });
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
                ..Default::default()
            },
            ProcessInfo {
                pid: 1002,
                comm: "bb-probe".to_string(),
                exe_name: Some("bb-probe".to_string()),
                ..Default::default()
            },
            ProcessInfo {
                pid: 1006,
                comm: "bbport".to_string(),
                exe_name: Some("bbport".to_string()),
                ..Default::default()
            },
            ProcessInfo {
                pid: 1003,
                comm: "Zelda64Recompil".to_string(),
                exe_name: Some("Zelda64Recompiled".to_string()),
                ..Default::default()
            },
            ProcessInfo {
                pid: 1004,
                comm: "firefox".to_string(),
                exe_name: Some("firefox".to_string()),
                ..Default::default()
            },
            ProcessInfo {
                pid: 1005,
                comm: "MainThread".to_string(),
                exe_name: Some("Longshot-x86_64.AppImage".to_string()),
                ..Default::default()
            },
        ];

        let detected = match_running_processes(&procs, &reg);
        let ids: Vec<&str> = detected.iter().map(|m| m.canonical_game_id.as_str()).collect();
        assert!(ids.contains(&"sony.bloodborne"), "detected={:?}", detected);
        assert!(ids.contains(&"nintendo.majoras_mask"), "detected={:?}", detected);
        // Longshot is owned by its own pack (a01..a13 ids from longshot.rs), never the multi-fork pack.
        assert!(
            detected.iter().any(|m| m.pid == 1005 && m.pack_id == "nintendo.oot.longshot.pc"),
            "detected={:?}",
            detected
        );
        assert_eq!(detected.len(), 4);
        assert!(detected
            .iter()
            .any(|m| m.executable == "bb-probe" && m.pack_id == "sony.bloodborne.bbport.pc"));
    }

    #[test]
    fn resolves_cmdline_wrappers_for_gamemoderun_wine_and_python() {
        assert_eq!(
            cmdline_executable_hint_from_parts(
                &[
                    "gamemoderun".to_string(),
                    "/opt/games/sm64ex/sm64.us.f3dex2e".to_string()
                ],
                "gamemoderun"
            ),
            Some("sm64.us.f3dex2e".to_string())
        );
        assert_eq!(
            cmdline_executable_hint_from_parts(
                &[
                    "wine64-preloader".to_string(),
                    "Z:\\home\\j\\Games\\SA2\\sonic2app.exe".to_string()
                ],
                "wine64-preloade"
            ),
            Some("sonic2app.exe".to_string())
        );
    }

    #[test]
    fn disambiguates_opengoal_and_emulator_processes_and_avoids_kde_dolphin_or_gk_false_positives() {
        let mut reg = PackRegistry::new();
        let packs_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("packs");
        assert!(reg.discover_in_dir(&packs_dir) > 0);

        let procs = vec![
            // KDE file manager (`/usr/bin/dolphin`) and generic `pkgkitd` must NEVER match!
            ProcessInfo {
                pid: 2001,
                comm: "dolphin".to_string(),
                exe_name: Some("dolphin".to_string()),
                exe_path: Some(PathBuf::from("/usr/bin/dolphin")),
                cmdline: vec!["/usr/bin/dolphin".to_string(), "/home/j/Downloads".to_string()],
                ..Default::default()
            },
            ProcessInfo {
                pid: 2002,
                comm: "pkgkitd".to_string(),
                exe_name: Some("pkgkitd".to_string()),
                ..Default::default()
            },
            // OpenGOAL Jak II (`gk` with jak2 arg) -> opengoal.jak_2
            ProcessInfo {
                pid: 2003,
                comm: "gk".to_string(),
                exe_name: Some("gk".to_string()),
                cmdline: vec![
                    "gk".to_string(),
                    "-v".to_string(),
                    "--".to_string(),
                    "-proj-path".to_string(),
                    "/home/j/.config/OpenGOAL/jak2/data".to_string(),
                ],
                ..Default::default()
            },
            // OpenGOAL Jak 3 -> opengoal.jak_3
            ProcessInfo {
                pid: 2004,
                comm: "gk".to_string(),
                exe_name: Some("gk".to_string()),
                cwd: Some(PathBuf::from("/home/j/Games/OpenGOAL/jak3")),
                ..Default::default()
            },
            // RPCS3 running Demon's Souls (`BLUS30443`)
            ProcessInfo {
                pid: 2005,
                comm: "rpcs3".to_string(),
                exe_name: Some("rpcs3-v0.0.34-x86_64.AppImage".to_string()),
                fd_paths: vec![PathBuf::from(
                    "/home/j/.config/rpcs3/dev_hdd0/game/BLUS30443/USRDIR/EBOOT.BIN",
                )],
                ..Default::default()
            },
            // shadPS4 running Bloodborne (`CUSA03173`) -> sony.bloodborne.forks
            ProcessInfo {
                pid: 2006,
                comm: "shadps4".to_string(),
                exe_name: Some("shadPS4-qt.AppImage".to_string()),
                cmdline: vec!["shadPS4".to_string(), "/games/CUSA03173/eboot.bin".to_string()],
                ..Default::default()
            },
            // Cemu running Breath of the Wild (`00050000/101c9400` with slash in mlc01 path)
            ProcessInfo {
                pid: 2007,
                comm: "Cemu".to_string(),
                exe_name: Some("Cemu_2.4.AppImage".to_string()),
                fd_paths: vec![PathBuf::from(
                    "/home/j/.local/share/Cemu/mlc01/usr/save/00050000/101c9400/user/80000001/0/game_data.sav",
                )],
                ..Default::default()
            },
            // Dolphin Emulator running Pokémon Colosseum (`GC6E01` shader cache / `.gci` save)
            ProcessInfo {
                pid: 2008,
                comm: "dolphin-emu".to_string(),
                exe_name: Some("dolphin-emu".to_string()),
                fd_paths: vec![PathBuf::from(
                    "/home/j/.local/share/dolphin-emu/GC/USA/Card A/01-GC6E-pokemon.gci",
                )],
                ..Default::default()
            },
            // Steam Proton `reaper` launching Sonic Adventure 2 (`AppId=213610`)
            ProcessInfo {
                pid: 2009,
                comm: "reaper".to_string(),
                exe_name: Some("sonic2app.exe".to_string()),
                cmdline: vec![
                    "reaper".to_string(),
                    "SteamLaunch".to_string(),
                    "AppId=213610".to_string(),
                    "--".to_string(),
                    "sonic2app.exe".to_string(),
                ],
                ..Default::default()
            },
        ];

        let detected = match_running_processes(&procs, &reg);
        assert!(
            !detected.iter().any(|m| m.pid == 2001 || m.pid == 2002),
            "KDE dolphin or pkgkitd must not false match: {:?}",
            detected
        );
        assert_eq!(
            detected.iter().find(|m| m.pid == 2003).map(|m| m.canonical_game_id.as_str()),
            Some("opengoal.jak_2")
        );
        assert_eq!(
            detected.iter().find(|m| m.pid == 2004).map(|m| m.canonical_game_id.as_str()),
            Some("opengoal.jak_3")
        );
        assert_eq!(
            detected.iter().find(|m| m.pid == 2005).map(|m| m.canonical_game_id.as_str()),
            Some("sony.demons_souls")
        );
        let bb_emu = detected.iter().find(|m| m.pid == 2006).expect("shadPS4 Bloodborne");
        assert_eq!(bb_emu.pack_id, "sony.bloodborne.forks");
        assert_eq!(
            detected.iter().find(|m| m.pid == 2007).map(|m| m.canonical_game_id.as_str()),
            Some("nintendo.breath_of_the_wild")
        );
        assert_eq!(
            detected.iter().find(|m| m.pid == 2008).map(|m| m.canonical_game_id.as_str()),
            Some("nintendo.pokemon_colosseum")
        );
        assert_eq!(
            detected.iter().find(|m| m.pid == 2009).map(|m| m.canonical_game_id.as_str()),
            Some("sega.sonic_adventure_2")
        );
    }
}
