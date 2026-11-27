use crate::core::engine::DaemonEngine;
use crate::diagnostics::server::start_health_http_server;
use crate::ipc::unix_socket::start_unix_ipc_server;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

pub struct DaemonRuntimeHandles {
    pub health_addr: Option<std::net::SocketAddr>,
    engine: Arc<DaemonEngine>,
    shutdown: Arc<AtomicBool>,
    handles: Vec<thread::JoinHandle<()>>,
}

impl DaemonRuntimeHandles {
    pub fn shutdown_and_join(self) {
        self.shutdown.store(true, Ordering::SeqCst);
        for h in self.handles {
            let _ = h.join();
        }
        let _ = std::fs::remove_file(&self.engine.config.socket_path);
    }
}

fn sleep_interruptible(total_ms: u64, shutdown: &AtomicBool, external_stop: Option<&AtomicBool>) {
    let mut slept = 0u64;
    while slept < total_ms && !shutdown.load(Ordering::Relaxed) {
        if let Some(ext) = external_stop {
            if ext.load(Ordering::Relaxed) {
                break;
            }
        }
        let step = (total_ms - slept).min(100);
        thread::sleep(Duration::from_millis(step));
        slept += step;
    }
}

/// Spawns all background daemon runtime loops (Unix IPC, Game Health HTTP API,
/// save/observation watcher, RetroArch UDP bridge, process monitor, and cloud sync).
pub fn start_daemon_runtime(
    engine: Arc<DaemonEngine>,
    health_enabled: bool,
    shutdown: Arc<AtomicBool>,
    external_stop: Option<Arc<AtomicBool>>,
) -> Result<DaemonRuntimeHandles, String> {
    let mut handles = Vec::new();

    let ipc_handle =
        start_unix_ipc_server(Arc::clone(&engine), &engine.config.socket_path, Arc::clone(&shutdown))?;
    handles.push(ipc_handle);

    let health_addr = if health_enabled {
        match start_health_http_server(
            Arc::clone(&engine),
            &engine.config.health_bind_addr,
            Arc::clone(&shutdown),
        ) {
            Ok((addr, h)) => {
                handles.push(h);
                Some(addr)
            }
            Err(e) => {
                eprintln!("[onehundod] Warning: Game Health server disabled ({})", e);
                None
            }
        }
    } else {
        None
    };

    // Save/observation directory watcher thread
    {
        let eng = Arc::clone(&engine);
        let sd = Arc::clone(&shutdown);
        let ext = external_stop.clone();
        let interval_ms = engine.config.watch_poll_interval_ms.max(250);
        let debounce_ms = engine.config.watch_debounce_ms;
        handles.push(thread::spawn(move || {
            let initial = eng.poll_watched_observation_dirs();
            if initial.imported > 0 || !initial.errors.is_empty() {
                eprintln!(
                    "[onehundod-watch] initial scan: {} imported, {} deduplicated, {} errors",
                    initial.imported,
                    initial.deduplicated,
                    initial.errors.len()
                );
            }
            while !sd.load(Ordering::Relaxed)
                && !ext.as_ref().map(|e| e.load(Ordering::Relaxed)).unwrap_or(false)
            {
                sleep_interruptible(interval_ms, &sd, ext.as_deref());
                if sd.load(Ordering::Relaxed)
                    || ext.as_ref().map(|e| e.load(Ordering::Relaxed)).unwrap_or(false)
                {
                    break;
                }
                let res = eng.poll_watched_observation_dirs_with_debounce(debounce_ms);
                if res.imported > 0 || !res.errors.is_empty() {
                    eprintln!(
                        "[onehundod-watch] imported={}, deduplicated={}, errors={:?}",
                        res.imported, res.deduplicated, res.errors
                    );
                }
            }
        }));
    }

    // Running game process monitor (fires "OneHundo Active" toast once per launch)
    {
        let eng = Arc::clone(&engine);
        let sd = Arc::clone(&shutdown);
        let ext = external_stop.clone();
        handles.push(thread::spawn(move || {
            while !sd.load(Ordering::Relaxed)
                && !ext.as_ref().map(|e| e.load(Ordering::Relaxed)).unwrap_or(false)
            {
                let _ = eng.poll_running_processes();
                sleep_interruptible(2_000, &sd, ext.as_deref());
            }
        }));
    }

    // RetroArch UDP network memory bridge
    if engine.config.retroarch_udp_addr.is_some() {
        let eng = Arc::clone(&engine);
        let sd = Arc::clone(&shutdown);
        let ext = external_stop.clone();
        handles.push(thread::spawn(move || {
            while !sd.load(Ordering::Relaxed)
                && !ext.as_ref().map(|e| e.load(Ordering::Relaxed)).unwrap_or(false)
            {
                let _ = eng.poll_retroarch_once();
                sleep_interruptible(500, &sd, ext.as_deref());
            }
        }));
    }

    // Platform cloud sync worker
    if engine.config.platform_url.is_some() {
        let eng = Arc::clone(&engine);
        let sd = Arc::clone(&shutdown);
        let ext = external_stop.clone();
        let interval_ms = engine.config.sync_interval_ms.max(250);
        handles.push(thread::spawn(move || {
            if let Err(e) = eng.sync_if_pending() {
                eprintln!("[onehundod-sync] initial sync check failed: {}", e);
            }
            while !sd.load(Ordering::Relaxed)
                && !ext.as_ref().map(|e| e.load(Ordering::Relaxed)).unwrap_or(false)
            {
                sleep_interruptible(interval_ms, &sd, ext.as_deref());
                if sd.load(Ordering::Relaxed)
                    || ext.as_ref().map(|e| e.load(Ordering::Relaxed)).unwrap_or(false)
                {
                    break;
                }
                if let Err(e) = eng.sync_if_pending() {
                    eprintln!("[onehundod-sync] background sync error: {}", e);
                }
            }
        }));
    }

    Ok(DaemonRuntimeHandles {
        health_addr,
        engine,
        shutdown,
        handles,
    })
}
