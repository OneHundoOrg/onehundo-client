use serde::{Deserialize, Serialize};
use std::env;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static FALLBACK_UUID_COUNTER: AtomicU64 = AtomicU64::new(1);

fn default_plane_a_max_frame_gaps_before_downgrade() -> u64 {
    10
}

fn default_watch_poll_interval_ms() -> u64 {
    5_000
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchedObservationDir {
    pub path: PathBuf,
    #[serde(default)]
    pub canonical_game_id: Option<String>,
}

/// Generate a RFC 4122 UUID v4 using OS randomness (`/dev/urandom`) with monotonic fallback.
pub fn generate_uuid_v4() -> String {
    let mut bytes = [0u8; 16];
    let filled = fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .is_ok();
    if !filled {
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        let c = FALLBACK_UUID_COUNTER.fetch_add(1, Ordering::Relaxed);
        let a = t ^ 0x9e37_79b9_7f4a_7c15;
        let b = c.wrapping_mul(0xbf58_476d_1ce4_e5b9) ^ (t >> 17);
        bytes[..8].copy_from_slice(&a.to_le_bytes());
        bytes[8..].copy_from_slice(&b.to_le_bytes());
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

pub fn default_socket_path() -> PathBuf {
    if let Ok(p) = env::var("ONEHUNDO_SOCKET") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Ok(dir) = env::var("XDG_RUNTIME_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("onehundo.sock");
        }
    }
    PathBuf::from("/tmp/onehundo.sock")
}

pub fn default_config_dir() -> PathBuf {
    if let Ok(dir) = env::var("ONEHUNDO_CONFIG_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Ok(xdg) = env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("onehundo");
        }
    }
    if let Ok(home) = env::var("HOME") {
        return PathBuf::from(home).join(".config").join("onehundo");
    }
    PathBuf::from("/tmp/onehundo-config")
}

pub fn default_data_dir() -> PathBuf {
    if let Ok(dir) = env::var("ONEHUNDO_DATA_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Ok(xdg) = env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("onehundo");
        }
    }
    if let Ok(home) = env::var("HOME") {
        return PathBuf::from(home).join(".local").join("share").join("onehundo");
    }
    PathBuf::from("/tmp/onehundo-data")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    pub device_uuid: String,
    pub device_name: String,
    pub socket_path: PathBuf,
    pub data_dir: PathBuf,
    pub config_dir: PathBuf,
    pub packs_dirs: Vec<PathBuf>,
    pub platform_url: Option<String>,
    pub health_bind_addr: String,
    pub sync_interval_ms: u64,
    pub sync_batch_size: usize,
    pub notifications_enabled: bool,
    pub shm_poll_interval_ms: u64,
    #[serde(default = "default_plane_a_max_frame_gaps_before_downgrade")]
    pub plane_a_max_frame_gaps_before_downgrade: u64,
    #[serde(default)]
    pub watched_observation_dirs: Vec<WatchedObservationDir>,
    #[serde(default = "default_watch_poll_interval_ms")]
    pub watch_poll_interval_ms: u64,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        let config_dir = default_config_dir();
        let data_dir = default_data_dir();
        let packs_dir = data_dir.join("packs");
        Self {
            device_uuid: String::new(),
            device_name: default_hostname(),
            socket_path: default_socket_path(),
            data_dir,
            config_dir,
            packs_dirs: vec![packs_dir],
            platform_url: env::var("ONEHUNDO_PLATFORM_URL").ok().filter(|s| !s.is_empty()),
            health_bind_addr: "127.0.0.1:19850".to_string(),
            sync_interval_ms: 2_500,
            sync_batch_size: 100,
            notifications_enabled: true,
            shm_poll_interval_ms: 2,
            plane_a_max_frame_gaps_before_downgrade: default_plane_a_max_frame_gaps_before_downgrade(),
            watched_observation_dirs: Vec::new(),
            watch_poll_interval_ms: default_watch_poll_interval_ms(),
        }
    }
}

fn default_hostname() -> String {
    fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "onehundo-desktop".to_string())
}

/// Credentials stored separately with restricted permissions; Debug redacts all secrets.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Credentials {
    pub api_key: Option<String>,
    pub device_id: Option<String>,
    pub device_token: Option<String>,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("device_id", &self.device_id)
            .field("device_token", &self.device_token.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

impl Credentials {
    pub fn active_bearer(&self) -> Option<&str> {
        self.device_token
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| self.api_key.as_deref().filter(|s| !s.is_empty()))
    }
}

/// Redact bearer tokens or API keys from error messages before logging/diagnostics.
pub fn redact_secrets(input: &str, creds: Option<&Credentials>) -> String {
    let mut out = input.to_string();
    if let Some(c) = creds {
        if let Some(k) = &c.api_key {
            if !k.is_empty() {
                out = out.replace(k, "[REDACTED_API_KEY]");
            }
        }
        if let Some(t) = &c.device_token {
            if !t.is_empty() {
                out = out.replace(t, "[REDACTED_DEVICE_TOKEN]");
            }
        }
    }
    // Also scrub any accidental `oh_live_...` prefix
    while let Some(idx) = out.find("oh_live_") {
        let end = out[idx..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
            .map(|rel| idx + rel)
            .unwrap_or(out.len());
        out.replace_range(idx..end, "[REDACTED_API_KEY]");
    }
    out
}

pub fn load_or_init_config(config_dir: &Path) -> Result<DaemonConfig, String> {
    fs::create_dir_all(config_dir)
        .map_err(|e| format!("failed to create config dir '{}': {}", config_dir.display(), e))?;
    let cfg_path = config_dir.join("config.json");
    if cfg_path.exists() {
        let raw = fs::read_to_string(&cfg_path)
            .map_err(|e| format!("failed to read config '{}': {}", cfg_path.display(), e))?;
        let mut cfg: DaemonConfig = serde_json::from_str(&raw)
            .map_err(|e| format!("invalid config JSON '{}': {}", cfg_path.display(), e))?;
        if cfg.device_uuid.is_empty() {
            cfg.device_uuid = generate_uuid_v4();
        }
        cfg.config_dir = config_dir.to_path_buf();
        Ok(cfg)
    } else {
        let mut cfg = DaemonConfig::default();
        cfg.device_uuid = generate_uuid_v4();
        cfg.config_dir = config_dir.to_path_buf();
        let json = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
        fs::write(&cfg_path, json)
            .map_err(|e| format!("failed to write config '{}': {}", cfg_path.display(), e))?;
        Ok(cfg)
    }
}

pub fn load_credentials(config_dir: &Path) -> Credentials {
    let path = config_dir.join("credentials.json");
    let mut creds = fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<Credentials>(&s).ok())
        .unwrap_or_default();
    if creds.api_key.is_none() {
        if let Ok(env_key) = env::var("ONEHUNDO_API_KEY") {
            if !env_key.is_empty() {
                creds.api_key = Some(env_key);
            }
        }
    }
    creds
}

pub fn save_credentials(config_dir: &Path, creds: &Credentials) -> Result<(), String> {
    fs::create_dir_all(config_dir)
        .map_err(|e| format!("failed to create config dir '{}': {}", config_dir.display(), e))?;
    let path = config_dir.join("credentials.json");
    let json = serde_json::to_string_pretty(creds).map_err(|e| e.to_string())?;
    fs::write(&path, json)
        .map_err(|e| format!("failed to write credentials '{}': {}", path.display(), e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}
