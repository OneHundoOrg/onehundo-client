use crate::core::autostart::{
    default_autostart_file_path, is_autostart_enabled, preferred_launch_executable,
    set_autostart_enabled,
};
use crate::core::config::{desktop_default_config, save_credentials, Credentials};
use crate::core::engine::DaemonEngine;
use crate::core::lockfile::{default_lock_path, InstanceLock, LockAcquireError};
use crate::core::runner::{start_daemon_runtime, DaemonRuntimeHandles};
use crate::core::types::ProgressionMode;
use crate::diagnostics::GameHealthSnapshot;
use crate::sync::http::http_request;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct BackendActionResult {
    pub message: String,
    pub is_error: bool,
}

enum BackendKind {
    Embedded {
        engine: Arc<DaemonEngine>,
        runtime: Mutex<Option<DaemonRuntimeHandles>>,
        _lock: InstanceLock,
    },
    Attached {
        base_url: String,
        config_dir: PathBuf,
        device_name: String,
    },
}

pub struct DesktopBackend {
    kind: BackendKind,
    autostart_path: PathBuf,
}

impl DesktopBackend {
    pub fn connect_or_start_embedded() -> Result<Self, String> {
        let cfg = desktop_default_config();
        let lock_path = default_lock_path(&cfg.socket_path);
        let autostart_path = default_autostart_file_path();

        match InstanceLock::acquire(&lock_path) {
            Ok(lock) => {
                let engine = DaemonEngine::initialize(cfg)?;
                let shutdown = Arc::new(AtomicBool::new(false));
                let runtime = start_daemon_runtime(Arc::clone(&engine), true, shutdown, None)?;
                Ok(Self {
                    kind: BackendKind::Embedded {
                        engine,
                        runtime: Mutex::new(Some(runtime)),
                        _lock: lock,
                    },
                    autostart_path,
                })
            }
            Err(LockAcquireError::AlreadyRunning { .. }) => {
                let base_url = format!("http://{}", cfg.health_bind_addr);
                Ok(Self {
                    kind: BackendKind::Attached {
                        base_url,
                        config_dir: cfg.config_dir,
                        device_name: cfg.device_name,
                    },
                    autostart_path,
                })
            }
            Err(LockAcquireError::Io(e)) => Err(e),
        }
    }

    /// Creates a backend wired directly to an in-memory/temp engine for unit & integration tests.
    pub fn from_test_engine(engine: Arc<DaemonEngine>, autostart_path: PathBuf) -> Result<Self, String> {
        let lock_path = default_lock_path(&engine.config.socket_path);
        let lock = InstanceLock::acquire(&lock_path)
            .map_err(|e| format!("test lock error: {:?}", e))?;
        Ok(Self {
            kind: BackendKind::Embedded {
                engine,
                runtime: Mutex::new(None),
                _lock: lock,
            },
            autostart_path,
        })
    }

    pub fn is_embedded(&self) -> bool {
        matches!(self.kind, BackendKind::Embedded { .. })
    }

    pub fn device_name(&self) -> &str {
        match &self.kind {
            BackendKind::Embedded { engine, .. } => &engine.config.device_name,
            BackendKind::Attached { device_name, .. } => device_name,
        }
    }

    pub fn platform_url(&self, snapshot: Option<&GameHealthSnapshot>) -> String {
        if let Some(s) = snapshot {
            if let Some(ref url) = s.platform_url {
                if !url.trim().is_empty() {
                    return url.clone();
                }
            }
        }
        match &self.kind {
            BackendKind::Embedded { engine, .. } => engine
                .config
                .platform_url
                .clone()
                .unwrap_or_else(|| "https://onehundo.org".to_string()),
            BackendKind::Attached { .. } => std::env::var("ONEHUNDO_PLATFORM_URL")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "https://onehundo.org".to_string()),
        }
    }

    pub fn fetch_snapshot(&self) -> Result<GameHealthSnapshot, String> {
        match &self.kind {
            BackendKind::Embedded { engine, .. } => engine.health_snapshot(),
            BackendKind::Attached { base_url, .. } => {
                let resp = http_request("GET", &format!("{}/health", base_url), None, None, None)?;
                if resp.status >= 300 {
                    return Err(format!("Daemon health error ({}): {}", resp.status, resp.body));
                }
                serde_json::from_str::<GameHealthSnapshot>(&resp.body)
                    .map_err(|e| format!("Invalid daemon health JSON: {}", e))
            }
        }
    }

    pub fn set_tracking_paused(&self, paused: bool) -> Result<bool, String> {
        match &self.kind {
            BackendKind::Embedded { engine, .. } => Ok(engine.set_tracking_paused(paused)),
            BackendKind::Attached { base_url, .. } => {
                let body = serde_json::json!({ "paused": paused }).to_string();
                let resp = http_request(
                    "POST",
                    &format!("{}/session/pause", base_url),
                    None,
                    Some(&body),
                    None,
                )?;
                if resp.status >= 300 {
                    return Err(format!("Pause request failed ({}): {}", resp.status, resp.body));
                }
                let v: serde_json::Value =
                    serde_json::from_str(&resp.body).unwrap_or(serde_json::Value::Null);
                Ok(v["tracking_paused"].as_bool().unwrap_or(paused))
            }
        }
    }

    pub fn set_progression_mode(&self, mode: ProgressionMode) -> Result<(), String> {
        match &self.kind {
            BackendKind::Embedded { engine, .. } => {
                let _ = engine.set_progression_mode(None, mode);
                Ok(())
            }
            BackendKind::Attached { base_url, .. } => {
                let body = serde_json::json!({ "mode": mode.as_str() }).to_string();
                let resp = http_request(
                    "POST",
                    &format!("{}/session/mode", base_url),
                    None,
                    Some(&body),
                    None,
                )?;
                if resp.status >= 300 {
                    return Err(format!("Mode update failed ({}): {}", resp.status, resp.body));
                }
                Ok(())
            }
        }
    }

    pub fn send_test_notification(&self) -> Result<String, String> {
        match &self.kind {
            BackendKind::Embedded { engine, .. } => {
                let report = engine.trigger_test_notification(None, None, None, None);
                if let Some(err) = report.error {
                    Err(err)
                } else if report.dispatched {
                    Ok(format!(
                        "Sent test notification: {} — {}",
                        report.event.title, report.event.body
                    ))
                } else {
                    Ok("Preview notification logged (OS toasts disabled)".to_string())
                }
            }
            BackendKind::Attached { base_url, .. } => {
                let resp = http_request(
                    "POST",
                    &format!("{}/notify-test", base_url),
                    None,
                    Some("{}"),
                    None,
                )?;
                if resp.status >= 300 {
                    return Err(format!("Test notification failed ({}): {}", resp.status, resp.body));
                }
                Ok("Test notification sent".to_string())
            }
        }
    }

    pub fn add_game_folder(
        &self,
        canonical_game_id: Option<String>,
        path: &Path,
    ) -> Result<(), String> {
        match &self.kind {
            BackendKind::Embedded { engine, .. } => {
                engine.add_watch_override(canonical_game_id, path)?;
                let _ = engine.poll_watched_observation_dirs();
                Ok(())
            }
            BackendKind::Attached { base_url, .. } => {
                let body = serde_json::json!({
                    "canonical_game_id": canonical_game_id,
                    "path": path.display().to_string(),
                })
                .to_string();
                let resp = http_request(
                    "POST",
                    &format!("{}/watches", base_url),
                    None,
                    Some(&body),
                    None,
                )?;
                if resp.status >= 300 {
                    return Err(format!("Could not add folder ({}): {}", resp.status, resp.body));
                }
                let _ = http_request(
                    "POST",
                    &format!("{}/watches/poll", base_url),
                    None,
                    Some("{}"),
                    None,
                );
                Ok(())
            }
        }
    }

    pub fn remove_game_folder(&self, path: &Path) -> Result<bool, String> {
        match &self.kind {
            BackendKind::Embedded { engine, .. } => engine.remove_watch_override(path),
            BackendKind::Attached { base_url, .. } => {
                let body = serde_json::json!({ "path": path.display().to_string() }).to_string();
                let resp = http_request(
                    "POST",
                    &format!("{}/watches/remove", base_url),
                    None,
                    Some(&body),
                    None,
                )?;
                if resp.status >= 300 {
                    return Err(format!("Could not remove folder ({}): {}", resp.status, resp.body));
                }
                Ok(true)
            }
        }
    }

    pub fn sync_now(&self) -> Result<String, String> {
        match &self.kind {
            BackendKind::Embedded { engine, .. } => {
                let res = engine.sync_now()?;
                Ok(format!("Synced {} event(s)", res.synced_count))
            }
            BackendKind::Attached { base_url, .. } => {
                let resp = http_request(
                    "POST",
                    &format!("{}/sync-now", base_url),
                    None,
                    Some("{}"),
                    None,
                )?;
                if resp.status >= 300 {
                    return Err(format!("Cloud sync failed ({}): {}", resp.status, resp.body));
                }
                let v: serde_json::Value =
                    serde_json::from_str(&resp.body).unwrap_or(serde_json::Value::Null);
                let count = v["synced_count"].as_u64().unwrap_or(0);
                Ok(format!("Synced {} event(s)", count))
            }
        }
    }

    pub fn save_account(
        &self,
        api_key: String,
        username: Option<String>,
    ) -> Result<(), String> {
        let trimmed = api_key.trim().to_string();
        if trimmed.is_empty() {
            return Err("API key must not be empty".to_string());
        }
        match &self.kind {
            BackendKind::Embedded { engine, .. } => {
                engine.set_account_credentials(Some(trimmed), username, false)?;
                Ok(())
            }
            BackendKind::Attached {
                base_url,
                config_dir,
                ..
            } => {
                let body = serde_json::json!({
                    "api_key": trimmed,
                    "username": username,
                    "sign_out": false,
                })
                .to_string();
                if let Ok(resp) = http_request(
                    "POST",
                    &format!("{}/auth/credentials", base_url),
                    None,
                    Some(&body),
                    None,
                ) {
                    if resp.status < 300 {
                        return Ok(());
                    }
                }
                // Fallback for older daemon instance: save directly 0600
                let creds = Credentials {
                    api_key: Some(trimmed),
                    device_id: None,
                    device_token: None,
                };
                save_credentials(config_dir, &creds)
            }
        }
    }

    pub fn sign_out(&self) -> Result<(), String> {
        match &self.kind {
            BackendKind::Embedded { engine, .. } => {
                engine.set_account_credentials(None, None, true)?;
                Ok(())
            }
            BackendKind::Attached {
                base_url,
                config_dir,
                ..
            } => {
                let body = serde_json::json!({ "sign_out": true }).to_string();
                if let Ok(resp) = http_request(
                    "POST",
                    &format!("{}/auth/credentials", base_url),
                    None,
                    Some(&body),
                    None,
                ) {
                    if resp.status < 300 {
                        return Ok(());
                    }
                }
                save_credentials(config_dir, &Credentials::default())
            }
        }
    }

    pub fn is_launch_at_login(&self) -> bool {
        is_autostart_enabled(&self.autostart_path)
    }

    pub fn set_launch_at_login(&self, enabled: bool) -> Result<bool, String> {
        let exec = preferred_launch_executable();
        set_autostart_enabled(&self.autostart_path, enabled, &exec, true)
    }

    pub fn shutdown(&self) {
        if let BackendKind::Embedded { ref runtime, .. } = self.kind {
            if let Ok(mut guard) = runtime.lock() {
                if let Some(rt) = guard.take() {
                    rt.shutdown_and_join();
                }
            }
        }
    }
}
