use onehundo_client::core::config::{
    default_config_dir, default_data_dir, default_socket_path, load_credentials, save_credentials,
    DaemonConfig,
};
use onehundo_client::core::engine::DaemonEngine;
use onehundo_client::core::lockfile::{default_lock_path, InstanceLock};
use onehundo_client::observation::external_importers::import_observation_file;
use onehundo_client::storage::journal::open_and_init_db;
use onehundo_client::sync::http::http_request;
use std::env;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

fn print_usage() {
    println!(
        "onehundo-desktop v0.1.0 - Desktop Service & Game Health CLI\n\n\
         SUBCOMMANDS:\n\
         \tstart [onehundod args...]     Launch onehundod background daemon\n\
         \tstop [url]                    Gracefully stop running onehundod instance\n\
         \tstatus [url]                  Show concise daemon status (exit 0 running, 3 stopped)\n\
         \thealth [url]                  Fetch Game Health JSON (default: http://127.0.0.1:19850/health)\n\
         \tpacks [url]                   List loaded packs from Game Health API\n\
         \timport-obs <file.oh-obs.json> Import Plane C external observation file directly to SQLite\n\
         \tset-api-key <key>             Store platform API key securely (0600) in ~/.config/onehundo\n\
         \tsync-now [url]                Trigger immediate cloud sync flush via Game Health API"
    );
}

fn default_health_base(args: &[String]) -> &str {
    args.get(2)
        .map(|s| s.as_str())
        .unwrap_or("http://127.0.0.1:19850")
        .trim_end_matches('/')
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
                .unwrap_or_else(|| std::path::PathBuf::from("onehundod"));
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
                    let packs = v["loaded_packs_count"].as_u64().unwrap_or(0);
                    let queued = v["sync_queue_depth"].as_u64().unwrap_or(0);
                    let synced = v["synced_events_count"].as_u64().unwrap_or(0);
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
                        "onehundod: running ({}, socket={}, packs={}, sessions={}, queued={}, synced={}, platform_configured={}, device_registered={}, last_sync_error={})",
                        pid_str,
                        sock.display(),
                        packs,
                        sessions,
                        queued,
                        synced,
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
            eprintln!("Timed out waiting for onehundod to stop (lockfile: {}).", lock_path.display());
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
        "packs" => {
            let base = default_health_base(&args);
            let url = format!("{}/packs", base);
            match http_request("GET", &url, None, None, None) {
                Ok(resp) => println!("{}", resp.body),
                Err(e) => {
                    eprintln!("Error querying packs: {}", e);
                    std::process::exit(1);
                }
            }
        }
        "import-obs" => {
            let path = match args.get(2) {
                Some(p) => Path::new(p),
                None => {
                    eprintln!("Usage: onehundo-desktop import-obs <file.oh-obs.json>");
                    std::process::exit(2);
                }
            };
            let data_dir = default_data_dir();
            let mut conn = open_and_init_db(&data_dir.join("onehundo_state.db"))
                .expect("failed to open SQLite DB");
            match import_observation_file(&mut conn, path) {
                Ok(summary) => println!(
                    "Imported {} observation(s) ({} deduplicated).",
                    summary.imported, summary.deduplicated
                ),
                Err(e) => {
                    eprintln!("Observation import failed: {}", e);
                    std::process::exit(1);
                }
            }
        }
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
            let url = format!("{}/sync/trigger", base);
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
