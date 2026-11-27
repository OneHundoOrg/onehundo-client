use onehundo_client::core::config::{save_credentials, DaemonConfig, WatchedObservationDir};
use onehundo_client::core::engine::DaemonEngine;
use onehundo_client::core::lockfile::{default_lock_path, InstanceLock, LockAcquireError};
use onehundo_client::core::types::ProgressionMode;
use onehundo_client::diagnostics::server::start_health_http_server;
use onehundo_client::ipc::unix_socket::start_unix_ipc_server;
use std::env;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

static SIGNAL_STOP: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn handle_shutdown_signal(_sig: i32) {
    SIGNAL_STOP.store(true, Ordering::SeqCst);
}

#[cfg(unix)]
fn install_signal_handlers() {
    const SIGINT: i32 = 2;
    const SIGTERM: i32 = 15;
    extern "C" {
        fn signal(sig: i32, handler: extern "C" fn(i32)) -> usize;
    }
    unsafe {
        signal(SIGINT, handle_shutdown_signal);
        signal(SIGTERM, handle_shutdown_signal);
    }
}

#[cfg(not(unix))]
fn install_signal_handlers() {}

fn print_version() {
    println!("onehundod v{}", env!("CARGO_PKG_VERSION"));
}

fn print_help() {
    println!(
        "onehundod v{} - OneHundo Local Daemon\n\n\
         OPTIONS:\n\
         \t--socket <path>           Unix socket path (default: $XDG_RUNTIME_DIR/onehundo.sock or /tmp/onehundo.sock)\n\
         \t--lock-file <path>        Single-instance lockfile path (default: $XDG_RUNTIME_DIR/onehundod.lock)\n\
         \t--config-dir <path>       Config directory (default: ~/.config/onehundo)\n\
         \t--data-dir <path>         Data/SQLite directory (default: ~/.local/share/onehundo)\n\
         \t--pack <file.onehundo>    Load a specific .onehundo pack file\n\
         \t--packs-dir <path>        Additional directory to scan for .onehundo packs\n\
         \t--watch-dir <path>        Directory to watch for Plane C external observation exports\n\
         \t--watch-interval-ms <n>   Observation directory poll interval in ms (default: 5000)\n\
         \t--watch-debounce-ms <n>   Save-file change settle debounce in ms (default: 500)\n\
         \t--retroarch-addr <addr>   RetroArch UDP network cmd addr (default: 127.0.0.1:55355, 'none' to disable)\n\
         \t--mode <mode>             Initial progression mode: STANDARD, ENCORE, or SPECTATOR\n\
         \t--health-addr <addr>      Game Health HTTP bind addr (default: 127.0.0.1:19850, 'none' to disable)\n\
         \t--platform-url <url>      onehundo-platform base URL (e.g. https://api.onehundo.org)\n\
         \t--api-key <key>           Save API key into credentials.json (never logged)\n\
         \t--sync-interval-ms <n>    Background sync interval in ms (default: 2500)\n\
         \t--no-notify               Disable OS notify-send desktop toasts\n\
         \t-V, --version             Print version\n\
         \t-h, --help                Print help",
        env!("CARGO_PKG_VERSION")
    );
}

fn main() {
    install_signal_handlers();

    let mut cfg = DaemonConfig::default();
    let mut extra_packs: Vec<PathBuf> = Vec::new();
    let mut api_key_arg: Option<String> = None;
    let mut lock_file_arg: Option<PathBuf> = None;
    let mut initial_mode: Option<ProgressionMode> = None;
    let mut health_enabled = true;

    let args: Vec<String> = env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_help();
                return;
            }
            "-V" | "--version" => {
                print_version();
                return;
            }
            "--socket" if i + 1 < args.len() => {
                i += 1;
                cfg.socket_path = PathBuf::from(&args[i]);
            }
            "--lock-file" if i + 1 < args.len() => {
                i += 1;
                lock_file_arg = Some(PathBuf::from(&args[i]));
            }
            "--config-dir" if i + 1 < args.len() => {
                i += 1;
                cfg.config_dir = PathBuf::from(&args[i]);
            }
            "--data-dir" if i + 1 < args.len() => {
                i += 1;
                cfg.data_dir = PathBuf::from(&args[i]);
                cfg.packs_dirs.push(cfg.data_dir.join("packs"));
            }
            "--pack" if i + 1 < args.len() => {
                i += 1;
                extra_packs.push(PathBuf::from(&args[i]));
            }
            "--packs-dir" if i + 1 < args.len() => {
                i += 1;
                cfg.packs_dirs.push(PathBuf::from(&args[i]));
            }
            "--watch-dir" if i + 1 < args.len() => {
                i += 1;
                cfg.watched_observation_dirs.push(WatchedObservationDir {
                    path: PathBuf::from(&args[i]),
                    canonical_game_id: None,
                });
            }
            "--watch-interval-ms" if i + 1 < args.len() => {
                i += 1;
                if let Ok(ms) = args[i].parse::<u64>() {
                    cfg.watch_poll_interval_ms = ms;
                }
            }
            "--watch-debounce-ms" if i + 1 < args.len() => {
                i += 1;
                if let Ok(ms) = args[i].parse::<u64>() {
                    cfg.watch_debounce_ms = ms;
                }
            }
            "--retroarch-addr" if i + 1 < args.len() => {
                i += 1;
                if args[i] == "none" {
                    cfg.retroarch_udp_addr = None;
                } else {
                    cfg.retroarch_udp_addr = Some(args[i].clone());
                }
            }
            "--mode" if i + 1 < args.len() => {
                i += 1;
                match ProgressionMode::from_str_strict(args[i].trim()) {
                    Some(m) => initial_mode = Some(m),
                    None => {
                        eprintln!(
                            "Invalid --mode '{}' (expected STANDARD, ENCORE, or SPECTATOR)",
                            args[i]
                        );
                        std::process::exit(2);
                    }
                }
            }
            "--health-addr" if i + 1 < args.len() => {
                i += 1;
                if args[i] == "none" {
                    health_enabled = false;
                } else {
                    cfg.health_bind_addr = args[i].clone();
                }
            }
            "--platform-url" if i + 1 < args.len() => {
                i += 1;
                cfg.platform_url = Some(args[i].clone());
            }
            "--api-key" if i + 1 < args.len() => {
                i += 1;
                api_key_arg = Some(args[i].clone());
            }
            "--sync-interval-ms" if i + 1 < args.len() => {
                i += 1;
                if let Ok(ms) = args[i].parse::<u64>() {
                    cfg.sync_interval_ms = ms;
                }
            }
            "--no-notify" => {
                cfg.notifications_enabled = false;
            }
            other => {
                eprintln!("Unknown argument: {}", other);
                std::process::exit(2);
            }
        }
        i += 1;
    }

    let lock_path = lock_file_arg.unwrap_or_else(|| default_lock_path(&cfg.socket_path));
    let _instance_lock = match InstanceLock::acquire(&lock_path) {
        Ok(lock) => lock,
        Err(LockAcquireError::AlreadyRunning { path, pid }) => {
            let pid_note = pid.map(|p| format!(" (pid {})", p)).unwrap_or_default();
            eprintln!(
                "[onehundod] Another onehundod instance is already running{} (lockfile: {}); exiting cleanly.",
                pid_note,
                path.display()
            );
            std::process::exit(0);
        }
        Err(LockAcquireError::Io(err)) => {
            eprintln!("[onehundod] Failed to acquire instance lockfile: {}", err);
            std::process::exit(1);
        }
    };

    let engine = match DaemonEngine::initialize(cfg.clone()) {
        Ok(e) => e,
        Err(err) => {
            eprintln!("[onehundod] Failed to initialize daemon: {}", err);
            std::process::exit(1);
        }
    };

    if let Some(mode) = initial_mode {
        let _ = engine.set_progression_mode(None, mode);
        println!("[onehundod] Initial progression mode set to {}", mode.as_str());
    }

    if let Some(key) = api_key_arg {
        let mut creds = engine.credentials.lock().unwrap();
        creds.api_key = Some(key);
        if let Err(e) = save_credentials(&engine.config.config_dir, &creds) {
            eprintln!("[onehundod] Failed saving credentials: {}", e);
            std::process::exit(1);
        }
    }

    for p in extra_packs {
        match engine.load_pack_path(&p) {
            Ok(id) => println!("[onehundod] Loaded pack '{}' from {}", id, p.display()),
            Err(e) => eprintln!("[onehundod] Failed loading pack '{}': {}", p.display(), e),
        }
    }

    let shutdown = Arc::new(AtomicBool::new(false));
    let ipc_handle = match start_unix_ipc_server(
        Arc::clone(&engine),
        &engine.config.socket_path,
        Arc::clone(&shutdown),
    ) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("[onehundod] IPC listener error: {}", e);
            std::process::exit(1);
        }
    };
    println!(
        "[onehundod] IPC listening on {}",
        engine.config.socket_path.display()
    );

    let health_handle = if health_enabled {
        match start_health_http_server(
            Arc::clone(&engine),
            &engine.config.health_bind_addr,
            Arc::clone(&shutdown),
        ) {
            Ok((addr, h)) => {
                println!("[onehundod] Game Health UI & API listening on http://{}", addr);
                Some(h)
            }
            Err(e) => {
                eprintln!("[onehundod] Warning: Game Health server disabled ({})", e);
                None
            }
        }
    } else {
        None
    };

    let watch_handle = {
        let eng = Arc::clone(&engine);
        let sd = Arc::clone(&shutdown);
        let interval_ms = engine.config.watch_poll_interval_ms.max(250);
        let debounce_ms = engine.config.watch_debounce_ms;
        Some(thread::spawn(move || {
            let initial = eng.poll_watched_observation_dirs();
            if initial.imported > 0 || !initial.errors.is_empty() {
                println!(
                    "[onehundod-watch] initial scan: {} imported, {} deduplicated, {} errors",
                    initial.imported,
                    initial.deduplicated,
                    initial.errors.len()
                );
            }
            while !sd.load(Ordering::Relaxed) && !SIGNAL_STOP.load(Ordering::Relaxed) {
                let mut slept = 0u64;
                while slept < interval_ms
                    && !sd.load(Ordering::Relaxed)
                    && !SIGNAL_STOP.load(Ordering::Relaxed)
                {
                    let step = (interval_ms - slept).min(100);
                    thread::sleep(Duration::from_millis(step));
                    slept += step;
                }
                if sd.load(Ordering::Relaxed) || SIGNAL_STOP.load(Ordering::Relaxed) {
                    break;
                }
                let res = eng.poll_watched_observation_dirs_with_debounce(debounce_ms);
                if res.imported > 0 || !res.errors.is_empty() {
                    println!(
                        "[onehundod-watch] imported={}, deduplicated={}, errors={:?}",
                        res.imported, res.deduplicated, res.errors
                    );
                }
            }
        }))
    };

    let retroarch_handle = if engine.config.retroarch_udp_addr.is_some() {
        let eng = Arc::clone(&engine);
        let sd = Arc::clone(&shutdown);
        Some(thread::spawn(move || {
            while !sd.load(Ordering::Relaxed) && !SIGNAL_STOP.load(Ordering::Relaxed) {
                let _ = eng.poll_retroarch_once();
                let mut slept = 0u64;
                while slept < 500
                    && !sd.load(Ordering::Relaxed)
                    && !SIGNAL_STOP.load(Ordering::Relaxed)
                {
                    thread::sleep(Duration::from_millis(100));
                    slept += 100;
                }
            }
        }))
    } else {
        None
    };

    let sync_handle = if engine.config.platform_url.is_some() {
        let eng = Arc::clone(&engine);
        let sd = Arc::clone(&shutdown);
        let interval_ms = engine.config.sync_interval_ms.max(250);
        Some(thread::spawn(move || {
            if let Err(e) = eng.sync_if_pending() {
                eprintln!("[onehundod-sync] initial sync check failed: {}", e);
            }
            while !sd.load(Ordering::Relaxed) && !SIGNAL_STOP.load(Ordering::Relaxed) {
                let mut slept = 0u64;
                while slept < interval_ms
                    && !sd.load(Ordering::Relaxed)
                    && !SIGNAL_STOP.load(Ordering::Relaxed)
                {
                    let step = (interval_ms - slept).min(100);
                    thread::sleep(Duration::from_millis(step));
                    slept += step;
                }
                if sd.load(Ordering::Relaxed) || SIGNAL_STOP.load(Ordering::Relaxed) {
                    break;
                }
                if let Err(e) = eng.sync_if_pending() {
                    eprintln!("[onehundod-sync] background sync error: {}", e);
                }
            }
        }))
    } else {
        None
    };

    while !shutdown.load(Ordering::Relaxed) {
        if SIGNAL_STOP.load(Ordering::Relaxed) {
            shutdown.store(true, Ordering::SeqCst);
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    let _ = ipc_handle.join();
    if let Some(h) = health_handle {
        let _ = h.join();
    }
    if let Some(h) = watch_handle {
        let _ = h.join();
    }
    if let Some(h) = retroarch_handle {
        let _ = h.join();
    }
    if let Some(h) = sync_handle {
        let _ = h.join();
    }
    let _ = std::fs::remove_file(&engine.config.socket_path);
    println!("[onehundod] Shutdown complete.");
}
