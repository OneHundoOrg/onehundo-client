use onehundo_client::core::config::{
    default_config_dir, default_data_dir, default_socket_path, load_credentials, save_credentials,
    DaemonConfig,
};
use onehundo_client::core::engine::DaemonEngine;
use onehundo_client::core::lockfile::{default_lock_path, InstanceLock};
use onehundo_client::core::types::ProgressionMode;
use onehundo_client::observation::external_importers::{
    import_observation_path_with_options, parse_observation_path_with_options, ImportOptions,
    ObservationImportSummary,
};
use onehundo_client::packs::matcher::PackRegistry;
use onehundo_client::storage::journal::open_and_init_db;
use onehundo_client::sync::http::http_request;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_HEALTH_URL: &str = "http://127.0.0.1:19850";

fn print_usage() {
    println!(
        "onehundo-desktop v0.1.0 - Desktop Service & Game Health CLI\n\n\
         SUBCOMMANDS:\n\
         \tstart [onehundod args...]     Launch onehundod background daemon\n\
         \tstop [url]                    Gracefully stop running onehundod instance\n\
         \tstatus [url]                  Show concise daemon status (exit 0 running, 3 stopped)\n\
         \thealth [url]                  Fetch Game Health JSON (default: http://127.0.0.1:19850/health)\n\
         \tpacks [--url <url>] [--json]  List installed .onehundo packs (ID, version, OAP edition, SHA-256)\n\
         \tmode <STANDARD|ENCORE|SPECTATOR> [--session <id>] [--url <url>]\n\
         \t                              Switch daemon/session progression mode\n\
         \timport-obs <path> [--game-id <id>] [--mode <mode>] [--dry-run] [--data-dir <dir>] [--url <url>] [--json]\n\
         \t                              Import Plane C external observation (Goldberg, RPCS3/shadPS4, .oh-obs.json)\n\
         \tset-api-key <key>             Store platform API key securely (0600) in ~/.config/onehundo\n\
         \tsync-now [url]                Trigger immediate cloud sync flush via Game Health API"
    );
}

fn default_health_base(args: &[String]) -> &str {
    args.get(2)
        .map(|s| s.as_str())
        .unwrap_or(DEFAULT_HEALTH_URL)
        .trim_end_matches('/')
}

fn run_packs(args: &[String]) {
    let mut url: Option<String> = None;
    let mut json_out = false;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--url" if i + 1 < args.len() => {
                i += 1;
                url = Some(args[i].trim_end_matches('/').to_string());
            }
            "--json" => json_out = true,
            other if !other.starts_with('-') && url.is_none() => {
                url = Some(other.trim_end_matches('/').to_string());
            }
            other => {
                eprintln!("Unknown option for packs: {}", other);
                std::process::exit(2);
            }
        }
        i += 1;
    }

    let base = url.as_deref().unwrap_or(DEFAULT_HEALTH_URL);
    let endpoint = format!("{}/packs", base);
    if let Ok(resp) = http_request("GET", &endpoint, None, None, None) {
        if resp.status == 200 {
            if json_out {
                println!("{}", resp.body);
                return;
            }
            let v: serde_json::Value =
                serde_json::from_str(&resp.body).unwrap_or(serde_json::Value::Null);
            if let Some(summaries) = v["pack_summaries"].as_array() {
                print_pack_summaries_json(summaries);
                return;
            }
            println!("{}", resp.body);
            return;
        }
    }

    // Offline fallback: inspect local packs directory directly
    let packs_dir = default_data_dir().join("packs");
    let mut reg = PackRegistry::new();
    let _ = reg.discover_in_dir(&packs_dir);
    let summaries = reg.summaries();
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "pack_summaries": summaries,
                "errors": reg.load_errors(),
            }))
            .unwrap_or_default()
        );
    } else if summaries.is_empty() {
        println!("No .onehundo packs found in {} (daemon offline).", packs_dir.display());
    } else {
        for s in summaries {
            println!(
                "{}  v{}  game={}  edition={}  achievements={}  sha256={}",
                s.pack_id,
                s.pack_version.as_deref().unwrap_or("-"),
                s.canonical_game_id,
                s.mapped_oap_edition.as_deref().unwrap_or("unmapped"),
                s.achievement_count,
                s.pack_sha256
            );
        }
    }
}

fn print_pack_summaries_json(summaries: &[serde_json::Value]) {
    if summaries.is_empty() {
        println!("No installed .onehundo packs reported by daemon.");
        return;
    }
    for s in summaries {
        println!(
            "{}  v{}  game={}  edition={}  achievements={}  sha256={}",
            s["pack_id"].as_str().unwrap_or("-"),
            s["pack_version"].as_str().unwrap_or("-"),
            s["canonical_game_id"].as_str().unwrap_or("-"),
            s["mapped_oap_edition"].as_str().unwrap_or("unmapped"),
            s["achievement_count"].as_u64().unwrap_or(0),
            s["pack_sha256"].as_str().unwrap_or("-")
        );
    }
}

fn run_mode(args: &[String]) {
    let Some(mode_raw) = args.get(2) else {
        eprintln!("Usage: onehundo-desktop mode <STANDARD|ENCORE|SPECTATOR> [--session <id>] [--url <url>]");
        std::process::exit(2);
    };
    let Some(mode) = ProgressionMode::from_str_strict(mode_raw.trim()) else {
        eprintln!(
            "Invalid progression mode '{}' (expected STANDARD, ENCORE, or SPECTATOR)",
            mode_raw
        );
        std::process::exit(2);
    };

    let mut session_id: Option<String> = None;
    let mut url: String = DEFAULT_HEALTH_URL.to_string();
    let mut i = 3;
    while i < args.len() {
        match args[i].as_str() {
            "--session" if i + 1 < args.len() => {
                i += 1;
                session_id = Some(args[i].clone());
            }
            "--url" if i + 1 < args.len() => {
                i += 1;
                url = args[i].trim_end_matches('/').to_string();
            }
            other => {
                eprintln!("Unknown option for mode: {}", other);
                std::process::exit(2);
            }
        }
        i += 1;
    }

    let endpoint = format!("{}/session/mode", url);
    let body = serde_json::json!({
        "mode": mode.as_str(),
        "session_id": session_id,
    })
    .to_string();

    match http_request("POST", &endpoint, None, Some(&body), None) {
        Ok(resp) if resp.status < 300 => println!("{}", resp.body),
        Ok(resp) => {
            eprintln!("Mode update rejected ({}): {}", resp.status, resp.body);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Failed reaching Game Health server at {}: {}", endpoint, e);
            std::process::exit(1);
        }
    }
}

fn run_import_obs(args: &[String]) {
    let mut target_path: Option<PathBuf> = None;
    let mut canonical_game_id: Option<String> = None;
    let mut mode_override: Option<ProgressionMode> = None;
    let mut dry_run = false;
    let mut data_dir_override: Option<PathBuf> = None;
    let mut url_override: Option<String> = None;
    let mut json_out = false;

    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--game-id" if i + 1 < args.len() => {
                i += 1;
                canonical_game_id = Some(args[i].clone());
            }
            "--mode" if i + 1 < args.len() => {
                i += 1;
                let Some(parsed) = ProgressionMode::from_str_strict(args[i].trim()) else {
                    eprintln!("Invalid --mode '{}' (expected STANDARD, ENCORE, or SPECTATOR)", args[i]);
                    std::process::exit(2);
                };
                mode_override = Some(parsed);
            }
            "--dry-run" => {
                dry_run = true;
            }
            "--data-dir" if i + 1 < args.len() => {
                i += 1;
                data_dir_override = Some(PathBuf::from(&args[i]));
            }
            "--url" if i + 1 < args.len() => {
                i += 1;
                url_override = Some(args[i].trim_end_matches('/').to_string());
            }
            "--json" => {
                json_out = true;
            }
            other if !other.starts_with('-') && target_path.is_none() => {
                target_path = Some(PathBuf::from(other));
            }
            other => {
                eprintln!("Unknown option for import-obs: {}", other);
                std::process::exit(2);
            }
        }
        i += 1;
    }

    let Some(raw_path) = target_path else {
        eprintln!(
            "Usage: onehundo-desktop import-obs <path> [--game-id <id>] [--mode <mode>] [--dry-run] [--data-dir <dir>] [--url <url>]"
        );
        std::process::exit(2);
    };

    let resolved_path = std::fs::canonicalize(&raw_path).unwrap_or_else(|_| raw_path.clone());

    if let Some(base) = url_override.as_deref() {
        let endpoint = format!("{}/v1/observation/import", base);
        let payload = serde_json::json!({
            "path": resolved_path.display().to_string(),
            "canonical_game_id": canonical_game_id,
            "progression_mode": mode_override.map(|m| m.as_str().to_string()),
            "dry_run": dry_run,
        })
        .to_string();
        match http_request("POST", &endpoint, None, Some(&payload), None) {
            Ok(resp) if resp.status < 300 => {
                let summary: ObservationImportSummary =
                    serde_json::from_str(&resp.body).unwrap_or(ObservationImportSummary {
                        imported: 0,
                        deduplicated: 0,
                        skipped_locked: 0,
                        dry_run,
                        source_kind: String::new(),
                        observation_ids: Vec::new(),
                        records: Vec::new(),
                    });
                print_import_summary(&summary, json_out);
                return;
            }
            Ok(resp) => {
                eprintln!("Daemon observation import failed ({}): {}", resp.status, resp.body);
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("Failed reaching daemon at {}: {}", endpoint, e);
                std::process::exit(1);
            }
        }
    }

    // If daemon URL was not explicitly set and --data-dir was not explicitly set, try running daemon first.
    if data_dir_override.is_none() {
        let endpoint = format!("{}/v1/observation/import", DEFAULT_HEALTH_URL);
        let payload = serde_json::json!({
            "path": resolved_path.display().to_string(),
            "canonical_game_id": canonical_game_id,
            "progression_mode": mode_override.map(|m| m.as_str().to_string()),
            "dry_run": dry_run,
        })
        .to_string();
        if let Ok(resp) = http_request("POST", &endpoint, None, Some(&payload), None) {
            if resp.status < 300 {
                if let Ok(summary) = serde_json::from_str::<ObservationImportSummary>(&resp.body) {
                    print_import_summary(&summary, json_out);
                    return;
                }
            } else {
                eprintln!("Daemon observation import failed ({}): {}", resp.status, resp.body);
                std::process::exit(1);
            }
        }
    }

    let data_dir = data_dir_override.unwrap_or_else(default_data_dir);
    let mut packs = PackRegistry::new();
    let _ = packs.discover_in_dir(&data_dir.join("packs"));

    let opts = ImportOptions {
        canonical_game_id,
        override_progression_mode: mode_override,
        dry_run,
    };

    if dry_run {
        match parse_observation_path_with_options(Path::new(&resolved_path), Some(&packs), &opts) {
            Ok(parsed) => {
                let summary = ObservationImportSummary {
                    imported: 0,
                    deduplicated: 0,
                    skipped_locked: parsed.skipped_locked,
                    dry_run: true,
                    source_kind: parsed.source_kind,
                    observation_ids: parsed
                        .records
                        .iter()
                        .map(|r| r.observation_id.clone())
                        .collect(),
                    records: parsed.records,
                };
                print_import_summary(&summary, json_out);
            }
            Err(e) => {
                eprintln!("Observation dry-run failed: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }

    let mut conn = open_and_init_db(&data_dir.join("onehundo_state.db"))
        .expect("failed to open SQLite DB");
    match import_observation_path_with_options(
        &mut conn,
        Path::new(&resolved_path),
        Some(&packs),
        &opts,
    ) {
        Ok(summary) => print_import_summary(&summary, json_out),
        Err(e) => {
            eprintln!("Observation import failed: {}", e);
            std::process::exit(1);
        }
    }
}

fn print_import_summary(summary: &ObservationImportSummary, json_out: bool) {
    if json_out {
        println!("{}", serde_json::to_string_pretty(summary).unwrap_or_default());
        return;
    }
    if summary.dry_run {
        println!(
            "Dry-run ({}): {} unlocked record(s) would be imported ({} locked skipped).",
            summary.source_kind,
            summary.records.len(),
            summary.skipped_locked
        );
        for rec in &summary.records {
            println!(
                "  - {} key={} mode={} evidence={} ts={}",
                rec.canonical_game_id,
                rec.key,
                rec.progression_mode,
                rec.evidence_class,
                rec.source_timestamp.as_deref().unwrap_or("null")
            );
        }
    } else {
        println!(
            "Imported {} observation(s) ({} deduplicated, {} locked skipped) [{}].",
            summary.imported,
            summary.deduplicated,
            summary.skipped_locked,
            summary.source_kind
        );
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        print_usage();
        return;
    }

    match args[1].as_str() {
        "start" => {
            let exe = env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.join("onehundod")))
                .filter(|p| p.exists())
                .unwrap_or_else(|| PathBuf::from("onehundod"));
            let status = Command::new(exe)
                .args(&args[2..])
                .status()
                .expect("failed to launch onehundod");
            std::process::exit(status.code().unwrap_or(1));
        }
        "status" => {
            let base = default_health_base(&args);
            let url = format!("{}/health", base);
            let sock = default_socket_path();
            let lock_path = default_lock_path(&sock);
            let lock_pid = InstanceLock::probe_running(&lock_path);

            match http_request("GET", &url, None, None, None) {
                Ok(resp) => {
                    let v: serde_json::Value =
                        serde_json::from_str(&resp.body).unwrap_or(serde_json::Value::Null);
                    let pid_str = lock_pid
                        .flatten()
                        .map(|p| format!("pid={}", p))
                        .unwrap_or_else(|| "pid=unknown".to_string());
                    let sessions = v["sessions"]
                        .as_array()
                        .or_else(|| v["active_sessions"].as_array())
                        .map(|a| a.len())
                        .unwrap_or(0);
                    let mode = v["active_progression_mode"].as_str().unwrap_or("STANDARD");
                    let packs = v["loaded_packs_count"].as_u64().unwrap_or(0);
                    let queued = v["sync_queue_depth"].as_u64().unwrap_or(0);
                    let synced = v["synced_events_count"].as_u64().unwrap_or(0);
                    let obs = v["observations_imported"].as_u64().unwrap_or(0);
                    let platform_ok = v["sync"]["platform_configured"]
                        .as_bool()
                        .or_else(|| v["platform_configured"].as_bool())
                        .unwrap_or(false);
                    let device_ok = v["sync"]["device_registered"]
                        .as_bool()
                        .or_else(|| v["device_registered"].as_bool())
                        .unwrap_or(false);
                    let last_err = v["sync"]["last_sync_error"]
                        .as_str()
                        .or_else(|| v["last_sync_error"].as_str())
                        .unwrap_or("none");
                    println!(
                        "onehundod: running ({}, mode={}, socket={}, packs={}, sessions={}, queued={}, synced={}, observations={}, platform_configured={}, device_registered={}, last_sync_error={})",
                        pid_str,
                        mode,
                        sock.display(),
                        packs,
                        sessions,
                        queued,
                        synced,
                        obs,
                        platform_ok,
                        device_ok,
                        last_err
                    );
                }
                Err(_) => {
                    if let Some(maybe_pid) = lock_pid {
                        let pid_str = maybe_pid
                            .map(|p| format!("pid={}", p))
                            .unwrap_or_else(|| "pid=unknown".to_string());
                        println!(
                            "onehundod: running ({}, lockfile={}, health_api=unreachable)",
                            pid_str,
                            lock_path.display()
                        );
                    } else {
                        println!(
                            "onehundod: stopped (socket={}, lockfile={})",
                            sock.display(),
                            lock_path.display()
                        );
                        std::process::exit(3);
                    }
                }
            }
        }
        "stop" => {
            let base = default_health_base(&args);
            let url = format!("{}/shutdown", base);
            let sock = default_socket_path();
            let lock_path = default_lock_path(&sock);

            let http_stopped = http_request("POST", &url, None, Some("{}"), None).is_ok();
            let lock_pid = InstanceLock::probe_running(&lock_path);

            if !http_stopped && lock_pid.is_none() {
                println!("onehundod is not running.");
                return;
            }

            if !http_stopped {
                #[cfg(unix)]
                if let Some(Some(pid)) = lock_pid {
                    const SIGTERM: i32 = 15;
                    extern "C" {
                        fn kill(pid: i32, sig: i32) -> i32;
                    }
                    let rc = unsafe { kill(pid as i32, SIGTERM) };
                    if rc != 0 {
                        eprintln!(
                            "Failed signaling onehundod pid {}: {}",
                            pid,
                            std::io::Error::last_os_error()
                        );
                        std::process::exit(1);
                    }
                }
            }

            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if InstanceLock::probe_running(&lock_path).is_none()
                    && http_request("GET", &format!("{}/health", base), None, None, None).is_err()
                {
                    println!("onehundod stopped cleanly.");
                    return;
                }
                thread::sleep(Duration::from_millis(50));
            }
            eprintln!(
                "Timed out waiting for onehundod to stop (lockfile: {}).",
                lock_path.display()
            );
            std::process::exit(1);
        }
        "health" => {
            let base = default_health_base(&args);
            let url = format!("{}/health", base);
            match http_request("GET", &url, None, None, None) {
                Ok(resp) => println!("{}", resp.body),
                Err(e) => {
                    eprintln!("Error querying Game Health: {}", e);
                    std::process::exit(1);
                }
            }
        }
        "packs" => run_packs(&args),
        "mode" => run_mode(&args),
        "import-obs" => run_import_obs(&args),
        "set-api-key" => {
            let key = match args.get(2) {
                Some(k) => k.clone(),
                None => {
                    eprintln!("Usage: onehundo-desktop set-api-key <oh_live_...>");
                    std::process::exit(2);
                }
            };
            let cfg_dir = default_config_dir();
            let mut creds = load_credentials(&cfg_dir);
            creds.api_key = Some(key);
            if let Err(e) = save_credentials(&cfg_dir, &creds) {
                eprintln!("Failed saving API key: {}", e);
                std::process::exit(1);
            }
            println!("API key stored securely in {}/credentials.json", cfg_dir.display());
        }
        "sync-now" => {
            let base = default_health_base(&args);
            let url = format!("{}/sync-now", base);
            match http_request("POST", &url, None, Some("{}"), None) {
                Ok(resp) => println!("{}", resp.body),
                Err(e) => {
                    eprintln!("Sync trigger failed: {}", e);
                    std::process::exit(1);
                }
            }
        }
        "local-status" => {
            let eng = DaemonEngine::initialize(DaemonConfig::default())
                .expect("failed to init local state");
            let snap = eng.health_snapshot().expect("failed to query health");
            println!("{}", serde_json::to_string_pretty(&snap).unwrap());
        }
        _ => print_usage(),
    }
}
