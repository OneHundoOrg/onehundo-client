use onehundo_client::core::config::{
    default_config_dir, default_data_dir, default_socket_path, load_credentials,
    load_or_init_config, save_config, save_credentials, DaemonConfig, WatchedObservationDir,
};
use onehundo_client::core::engine::DaemonEngine;
use onehundo_client::core::lockfile::{default_lock_path, InstanceLock};
use onehundo_client::core::types::{ProgressionMode, TrustTier, VerificationState};
use onehundo_client::observation::external_importers::{
    import_observation_path_with_options, parse_observation_path_with_options, ImportOptions,
    ObservationImportSummary,
};
use onehundo_client::packs::matcher::PackRegistry;
use onehundo_client::storage::journal::open_and_init_db;
use onehundo_client::sync::http::http_request;
use onehundo_client::ui::notifications::NotificationManager;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_HEALTH_URL: &str = "http://127.0.0.1:19850";

fn print_version() {
    println!("onehundo-desktop v{}", env!("CARGO_PKG_VERSION"));
}

fn print_usage() {
    println!(
        "onehundo-desktop v{} - Desktop Service & Game Health CLI\n\n\
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
         \twatches [--url <url>] [--config-dir <dir>] [--json]\n\
         \t                              List active Manual Watch Overrides (save/trophy directories)\n\
         \twatch-add <game_id> <path> [--url <url>] [--config-dir <dir>] [--json]\n\
         \t                              Bind a custom save folder path to a canonical game ID\n\
         \twatch-remove <path> [--url <url>] [--config-dir <dir>] [--json]\n\
         \t                              Remove a manual watch override\n\
         \tlauncher-status [--game-id <id>] [--url <url>] [--json]\n\
         \t                              Query active launcher bridge games & completion status\n\
         \tnotify-test [--game-id <id>] [--game <title>] [--title <title>] [--body <text>] [--url <url>] [--json]\n\
         \t                              Dispatch a non-destructive test desktop unlock toast\n\
         \tset-api-key <key>             Store platform API key securely (0600) in ~/.config/onehundo\n\
         \tsync-now [url]                Trigger immediate cloud sync flush via Game Health API\n\
         \t-V, --version                 Print version\n\
         \t-h, --help                    Print help",
        env!("CARGO_PKG_VERSION")
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

fn run_notify_test(args: &[String]) {
    let mut canonical_game_id: Option<String> = None;
    let mut game_title: Option<String> = None;
    let mut title: Option<String> = None;
    let mut body: Option<String> = None;
    let mut url_override: Option<String> = None;
    let mut json_out = false;

    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--game-id" if i + 1 < args.len() => {
                i += 1;
                canonical_game_id = Some(args[i].clone());
            }
            "--game" if i + 1 < args.len() => {
                i += 1;
                game_title = Some(args[i].clone());
            }
            "--title" if i + 1 < args.len() => {
                i += 1;
                title = Some(args[i].clone());
            }
            "--body" if i + 1 < args.len() => {
                i += 1;
                body = Some(args[i].clone());
            }
            "--url" if i + 1 < args.len() => {
                i += 1;
                url_override = Some(args[i].trim_end_matches('/').to_string());
            }
            "--json" => json_out = true,
            other => {
                eprintln!("Unknown option for notify-test: {}", other);
                std::process::exit(2);
            }
        }
        i += 1;
    }

    let base = url_override.as_deref().unwrap_or(DEFAULT_HEALTH_URL);
    let endpoint = format!("{}/notify-test", base);
    let payload = serde_json::json!({
        "canonical_game_id": canonical_game_id,
        "game_title": game_title,
        "title": title,
        "body": body,
    })
    .to_string();

    match http_request("POST", &endpoint, None, Some(&payload), None) {
        Ok(resp) if resp.status < 300 => {
            if json_out {
                println!("{}", resp.body);
            } else {
                let v: serde_json::Value =
                    serde_json::from_str(&resp.body).unwrap_or(serde_json::Value::Null);
                let app = v["event"]["app_name"].as_str().unwrap_or("OneHundo");
                let ev_title = v["event"]["title"].as_str().unwrap_or("");
                let ev_body = v["event"]["body"].as_str().unwrap_or("");
                let dispatched = v["dispatched"].as_bool().unwrap_or(false);
                println!(
                    "Test notification dispatched={} via onehundod: [{}] {} — {}",
                    dispatched, app, ev_title, ev_body
                );
            }
            return;
        }
        Ok(resp) => {
            eprintln!("Daemon notify-test failed ({}): {}", resp.status, resp.body);
            std::process::exit(1);
        }
        Err(e) => {
            if url_override.is_some() {
                eprintln!("Failed reaching daemon at {}: {}", endpoint, e);
                std::process::exit(1);
            }
        }
    }

    // Offline fallback: dispatch directly via local NotificationManager using installed pack metadata.
    let packs_dir = default_data_dir().join("packs");
    let mut packs = PackRegistry::new();
    let _ = packs.discover_in_dir(&packs_dir);

    let requested_game_id = canonical_game_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let matched = if let Some(gid) = requested_game_id {
        packs.find_by_game_id(gid)
    } else {
        packs
            .get("nintendo.oot.longshot.pc")
            .or_else(|| packs.list().into_iter().next())
    };

    let resolved_game_id = matched
        .map(|p| p.manifest.canonical_game_id.clone())
        .or_else(|| requested_game_id.map(str::to_string))
        .unwrap_or_else(|| "nintendo.oot.longshot.pc".to_string());
    let resolved_game_title = game_title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| matched.map(|p| p.manifest.title.clone()))
        .unwrap_or_else(|| "The Legend of Zelda: Ocarina of Time".to_string());
    let resolved_title = title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            matched
                .and_then(|p| p.manifest.achievements.first())
                .map(|a| a.title.clone())
        })
        .unwrap_or_else(|| "It works, it works!".to_string());
    let resolved_body = body
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            matched
                .and_then(|p| p.manifest.achievements.first())
                .map(|a| a.description.clone())
        })
        .unwrap_or_else(|| "Desktop unlock notification test (no progress modified).".to_string());

    let mgr = NotificationManager::new(true);
    let report = mgr.notify_test_sync(
        &resolved_game_id,
        Some(&resolved_game_title),
        &resolved_title,
        &resolved_body,
        ProgressionMode::Standard,
        TrustTier::Local,
        VerificationState::Unverified,
    );

    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "mode": "offline_fallback",
                "dispatched": report.dispatched,
                "os_toasts_enabled": report.os_toasts_enabled,
                "error": report.error,
                "event": report.event,
            }))
            .unwrap_or_default()
        );
    } else {
        println!(
            "Test notification dispatched={} (offline fallback): [{}] {} — {}",
            report.dispatched,
            report.event.app_name.as_deref().unwrap_or("OneHundo"),
            report.event.title,
            report.event.body
        );
    }
    if let Some(err) = report.error {
        eprintln!("Notification error: {}", err);
        std::process::exit(1);
    }
}


fn run_watches(args: &[String]) {
    let mut url: Option<String> = None;
    let mut cfg_dir_override: Option<PathBuf> = None;
    let mut json_out = false;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--url" if i + 1 < args.len() => {
                i += 1;
                url = Some(args[i].trim_end_matches('/').to_string());
            }
            "--config-dir" if i + 1 < args.len() => {
                i += 1;
                cfg_dir_override = Some(PathBuf::from(&args[i]));
            }
            "--json" => json_out = true,
            other => {
                eprintln!("Unknown option for watches: {}", other);
                std::process::exit(2);
            }
        }
        i += 1;
    }

    if cfg_dir_override.is_none() || url.is_some() {
        let base = url.as_deref().unwrap_or(DEFAULT_HEALTH_URL);
        let endpoint = format!("{}/watches", base);
        if let Ok(resp) = http_request("GET", &endpoint, None, None, None) {
            if resp.status < 300 {
                if json_out {
                    println!("{}", resp.body);
                    return;
                }
                let v: serde_json::Value =
                    serde_json::from_str(&resp.body).unwrap_or(serde_json::Value::Null);
                if let Some(arr) = v["watched_dirs"].as_array() {
                    if arr.is_empty() {
                        println!("No manual watch overrides configured.");
                    } else {
                        for item in arr {
                            println!(
                                "{} -> {}",
                                item["canonical_game_id"].as_str().unwrap_or("auto"),
                                item["path"].as_str().unwrap_or("-")
                            );
                        }
                    }
                    return;
                }
            }
        }
    }

    let cfg_dir = cfg_dir_override.unwrap_or_else(default_config_dir);
    let cfg = load_or_init_config(&cfg_dir).unwrap_or_default();
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "watched_dirs": cfg.watched_observation_dirs
            }))
            .unwrap_or_default()
        );
    } else if cfg.watched_observation_dirs.is_empty() {
        println!("No manual watch overrides configured in {}.", cfg_dir.display());
    } else {
        for w in cfg.watched_observation_dirs {
            println!(
                "{} -> {}",
                w.canonical_game_id.as_deref().unwrap_or("auto"),
                w.path.display()
            );
        }
    }
}

fn run_watch_add(args: &[String]) {
    let mut positionals: Vec<String> = Vec::new();
    let mut url: Option<String> = None;
    let mut cfg_dir_override: Option<PathBuf> = None;
    let mut json_out = false;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--url" if i + 1 < args.len() => {
                i += 1;
                url = Some(args[i].trim_end_matches('/').to_string());
            }
            "--config-dir" if i + 1 < args.len() => {
                i += 1;
                cfg_dir_override = Some(PathBuf::from(&args[i]));
            }
            "--json" => json_out = true,
            other if !other.starts_with('-') => positionals.push(other.to_string()),
            other => {
                eprintln!("Unknown option for watch-add: {}", other);
                std::process::exit(2);
            }
        }
        i += 1;
    }

    if positionals.len() < 2 {
        eprintln!(
            "Usage: onehundo-desktop watch-add <canonical_game_id> <folder_path> [--url <url>] [--config-dir <dir>] [--json]"
        );
        std::process::exit(2);
    }
    let game_id = positionals[0].clone();
    let raw_path = PathBuf::from(&positionals[1]);
    let canon = std::fs::canonicalize(&raw_path).unwrap_or_else(|e| {
        eprintln!("Invalid watch path '{}': {}", raw_path.display(), e);
        std::process::exit(1);
    });

    if cfg_dir_override.is_none() || url.is_some() {
        let base = url.as_deref().unwrap_or(DEFAULT_HEALTH_URL);
        let endpoint = format!("{}/watches", base);
        let payload = serde_json::json!({
            "canonical_game_id": game_id,
            "path": canon.display().to_string(),
        })
        .to_string();
        match http_request("POST", &endpoint, None, Some(&payload), None) {
            Ok(resp) if resp.status < 300 => {
                if json_out {
                    println!("{}", resp.body);
                } else {
                    println!("Bound watch override via onehundod: {} -> {}", game_id, canon.display());
                }
                return;
            }
            Ok(resp) => {
                eprintln!("watch-add failed ({}): {}", resp.status, resp.body);
                std::process::exit(1);
            }
            Err(e) if url.is_some() => {
                eprintln!("Failed reaching daemon at {}: {}", endpoint, e);
                std::process::exit(1);
            }
            Err(_) => {}
        }
    }

    let cfg_dir = cfg_dir_override.unwrap_or_else(default_config_dir);
    let mut cfg = load_or_init_config(&cfg_dir).unwrap_or_else(|e| {
        eprintln!("Failed reading config: {}", e);
        std::process::exit(1);
    });
    let entry = WatchedObservationDir {
        path: canon.clone(),
        canonical_game_id: Some(game_id.clone()),
    };
    if let Some(existing) = cfg.watched_observation_dirs.iter_mut().find(|w| w.path == canon) {
        existing.canonical_game_id = entry.canonical_game_id.clone();
    } else {
        cfg.watched_observation_dirs.push(entry.clone());
    }
    if let Err(e) = save_config(&cfg_dir, &cfg) {
        eprintln!("Failed saving config: {}", e);
        std::process::exit(1);
    }
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "watched_dir": entry,
                "watched_dirs": cfg.watched_observation_dirs,
            }))
            .unwrap_or_default()
        );
    } else {
        println!("Saved watch override (offline): {} -> {}", game_id, canon.display());
    }
}

fn run_watch_remove(args: &[String]) {
    let mut raw_path: Option<PathBuf> = None;
    let mut url: Option<String> = None;
    let mut cfg_dir_override: Option<PathBuf> = None;
    let mut json_out = false;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--url" if i + 1 < args.len() => {
                i += 1;
                url = Some(args[i].trim_end_matches('/').to_string());
            }
            "--config-dir" if i + 1 < args.len() => {
                i += 1;
                cfg_dir_override = Some(PathBuf::from(&args[i]));
            }
            "--json" => json_out = true,
            other if !other.starts_with('-') && raw_path.is_none() => {
                raw_path = Some(PathBuf::from(other));
            }
            other => {
                eprintln!("Unknown option for watch-remove: {}", other);
                std::process::exit(2);
            }
        }
        i += 1;
    }

    let Some(target) = raw_path else {
        eprintln!("Usage: onehundo-desktop watch-remove <folder_path> [--url <url>] [--config-dir <dir>] [--json]");
        std::process::exit(2);
    };
    let canon = std::fs::canonicalize(&target).unwrap_or_else(|_| target.clone());

    if cfg_dir_override.is_none() || url.is_some() {
        let base = url.as_deref().unwrap_or(DEFAULT_HEALTH_URL);
        let endpoint = format!("{}/watches/remove", base);
        let payload = serde_json::json!({
            "path": canon.display().to_string(),
        })
        .to_string();
        if let Ok(resp) = http_request("POST", &endpoint, None, Some(&payload), None) {
            if resp.status < 300 {
                if json_out {
                    println!("{}", resp.body);
                } else {
                    println!("Removed watch override via onehundod: {}", canon.display());
                }
                return;
            }
        }
    }

    let cfg_dir = cfg_dir_override.unwrap_or_else(default_config_dir);
    let mut cfg = load_or_init_config(&cfg_dir).unwrap_or_default();
    let before = cfg.watched_observation_dirs.len();
    cfg.watched_observation_dirs
        .retain(|w| w.path != target && w.path != canon);
    let removed = cfg.watched_observation_dirs.len() != before;
    if removed {
        let _ = save_config(&cfg_dir, &cfg);
    }
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "removed": removed,
                "watched_dirs": cfg.watched_observation_dirs,
            }))
            .unwrap_or_default()
        );
    } else {
        println!("Removed={} watch override for {}", removed, canon.display());
    }
}

fn run_launcher_status(args: &[String]) {
    let mut game_id: Option<String> = None;
    let mut url: Option<String> = None;
    let mut json_out = false;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--game-id" if i + 1 < args.len() => {
                i += 1;
                game_id = Some(args[i].clone());
            }
            "--url" if i + 1 < args.len() => {
                i += 1;
                url = Some(args[i].trim_end_matches('/').to_string());
            }
            "--json" => json_out = true,
            other => {
                eprintln!("Unknown option for launcher-status: {}", other);
                std::process::exit(2);
            }
        }
        i += 1;
    }

    let base = url.as_deref().unwrap_or(DEFAULT_HEALTH_URL);
    let endpoint = if let Some(ref gid) = game_id {
        format!("{}/v1/launchers/status?canonical_game_id={}", base, gid)
    } else {
        format!("{}/v1/launchers/status", base)
    };
    match http_request("GET", &endpoint, None, None, None) {
        Ok(resp) if resp.status < 300 => {
            if json_out {
                println!("{}", resp.body);
            } else {
                let v: serde_json::Value =
                    serde_json::from_str(&resp.body).unwrap_or(serde_json::Value::Null);
                let games = v["games"].as_array().cloned().unwrap_or_default();
                if games.is_empty() {
                    println!("No launcher games reported.");
                } else {
                    for g in games {
                        println!(
                            "{} [{}] pack={} unlocked={}/{} ({}%) active={}",
                            g["canonical_game_id"].as_str().unwrap_or("-"),
                            g["launcher"].as_str().unwrap_or("-"),
                            g["pack_id"].as_str().unwrap_or("none"),
                            g["unlocked_count"].as_u64().unwrap_or(0),
                            g["total_achievements"].as_u64().unwrap_or(0),
                            g["completion_percent"].as_u64().unwrap_or(0),
                            g["active"].as_bool().unwrap_or(false)
                        );
                    }
                }
            }
        }
        Ok(resp) => {
            eprintln!("launcher-status failed ({}): {}", resp.status, resp.body);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Failed reaching daemon at {}: {}", endpoint, e);
            std::process::exit(1);
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        print_usage();
        return;
    }

    match args[1].as_str() {
        "-h" | "--help" | "help" => print_usage(),
        "-V" | "--version" | "version" => print_version(),
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
        "watches" => run_watches(&args),
        "watch-add" => run_watch_add(&args),
        "watch-remove" => run_watch_remove(&args),
        "launcher-status" => run_launcher_status(&args),
        "notify-test" => run_notify_test(&args),
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
