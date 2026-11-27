use crate::core::types::{now_rfc3339, ProgressionMode, TrustTier, VerificationState};
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

const TROPHY_UNLOCK_PNG: &[u8] = include_bytes!("../desktop/assets/trophy_unlock.png");
const TROPHY_ACTIVE_PNG: &[u8] = include_bytes!("../desktop/assets/trophy_active.png");
const TROPHY_WARNING_PNG: &[u8] = include_bytes!("../desktop/assets/trophy_warning.png");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationEvent {
    pub timestamp: String,
    pub kind: String,
    #[serde(default)]
    pub app_name: Option<String>,
    pub title: String,
    pub body: String,
    pub canonical_game_id: Option<String>,
    pub progression_mode: Option<String>,
    pub trust_tier: Option<String>,
    pub verification_state: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationDispatchReport {
    pub dispatched: bool,
    pub os_toasts_enabled: bool,
    #[serde(default)]
    pub error: Option<String>,
    pub event: NotificationEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveGameNotification {
    pub session_key: String,
    pub canonical_game_id: String,
    pub game_title: String,
    pub edition_or_pack: String,
    pub title: String,
    pub body: String,
}

/// Tracks active game sessions so "OneHundo Active" fires once per game launch
/// and re-arms cleanly when the game exits.
#[derive(Debug, Default)]
pub struct SessionNotificationTracker {
    active_keys: HashSet<String>,
}

impl SessionNotificationTracker {
    pub fn new() -> Self {
        Self {
            active_keys: HashSet::new(),
        }
    }

    pub fn on_game_active(
        &mut self,
        session_key: &str,
        canonical_game_id: &str,
        game_title: &str,
        edition_or_pack: &str,
    ) -> Option<ActiveGameNotification> {
        let key = session_key.trim();
        if key.is_empty() || !self.active_keys.insert(key.to_string()) {
            return None;
        }
        let clean_title = {
            let t = game_title.trim();
            if t.is_empty() {
                canonical_game_id
            } else {
                t
            }
        };
        let clean_pack = {
            let p = edition_or_pack.trim();
            if p.is_empty() {
                canonical_game_id
            } else {
                p
            }
        };
        Some(ActiveGameNotification {
            session_key: key.to_string(),
            canonical_game_id: canonical_game_id.to_string(),
            game_title: clean_title.to_string(),
            edition_or_pack: clean_pack.to_string(),
            title: "OneHundo Active".to_string(),
            body: format!("Tracking {} ({})", clean_title, clean_pack),
        })
    }

    pub fn on_game_inactive(&mut self, session_key: &str) -> bool {
        self.active_keys.remove(session_key.trim())
    }

    /// Re-arm any previously active sessions with prefix `prefix` that are not present in `still_active`.
    pub fn sync_active_prefix(&mut self, prefix: &str, still_active: &HashSet<String>) -> usize {
        let before = self.active_keys.len();
        self.active_keys
            .retain(|k| !k.starts_with(prefix) || still_active.contains(k));
        before.saturating_sub(self.active_keys.len())
    }

    pub fn is_active(&self, session_key: &str) -> bool {
        self.active_keys.contains(session_key.trim())
    }

    pub fn active_count(&self) -> usize {
        self.active_keys.len()
    }
}

pub struct NotificationManager {
    os_toasts_enabled: bool,
    history: Mutex<VecDeque<NotificationEvent>>,
    max_history: usize,
}

impl NotificationManager {
    pub fn new(os_toasts_enabled: bool) -> Self {
        Self {
            os_toasts_enabled,
            history: Mutex::new(VecDeque::with_capacity(64)),
            max_history: 64,
        }
    }

    pub fn os_toasts_enabled(&self) -> bool {
        self.os_toasts_enabled
    }

    fn icon_asset_for_kind(kind: &str) -> (&'static str, &'static [u8]) {
        match kind {
            "game_active" => ("trophy-active-v1.png", TROPHY_ACTIVE_PNG),
            "trust_downgrade" | "frame_gap" => ("trophy-warning-v1.png", TROPHY_WARNING_PNG),
            _ => ("trophy-unlock-v1.png", TROPHY_UNLOCK_PNG),
        }
    }

    fn notification_icon_cache_dir() -> PathBuf {
        if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
            let trimmed = xdg.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed).join("onehundo").join("icons");
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            let trimmed = home.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed)
                    .join(".cache")
                    .join("onehundo")
                    .join("icons");
            }
        }
        std::env::temp_dir().join("onehundo-icons")
    }

    pub fn ensure_cached_icon_in_dir(dir: &Path, kind: &str) -> Result<PathBuf, String> {
        let (filename, bytes) = Self::icon_asset_for_kind(kind);
        fs::create_dir_all(dir)
            .map_err(|e| format!("failed creating notification icon dir {:?}: {}", dir, e))?;
        let target = dir.join(filename);
        if let Ok(meta) = fs::metadata(&target) {
            if meta.len() == bytes.len() as u64 {
                return Ok(target);
            }
        }
        let tmp = dir.join(format!(".{}.{}.tmp", filename, std::process::id()));
        fs::write(&tmp, bytes)
            .map_err(|e| format!("failed writing notification icon {:?}: {}", tmp, e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644));
        }
        fs::rename(&tmp, &target)
            .map_err(|e| format!("failed finalizing notification icon {:?}: {}", target, e))?;
        Ok(target)
    }

    fn ensure_cached_icon(kind: &str) -> Option<PathBuf> {
        let dir = Self::notification_icon_cache_dir();
        Self::ensure_cached_icon_in_dir(&dir, kind).ok()
    }

    fn build_unlock_event(
        kind: &str,
        canonical_game_id: &str,
        game_title: Option<&str>,
        title: &str,
        description: &str,
        mode: ProgressionMode,
        trust: TrustTier,
        verification: VerificationState,
    ) -> NotificationEvent {
        let app_name = game_title
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string);
        let heading = format!("🏆 {}", title);
        let body = description.to_string();
        NotificationEvent {
            timestamp: now_rfc3339(),
            kind: kind.to_string(),
            app_name,
            title: heading,
            body,
            canonical_game_id: Some(canonical_game_id.to_string()),
            progression_mode: Some(mode.as_str().to_string()),
            trust_tier: Some(trust.as_str().to_string()),
            verification_state: Some(verification.as_str().to_string()),
        }
    }

    pub fn notify_game_active(
        &self,
        canonical_game_id: &str,
        game_title: &str,
        edition_or_pack: &str,
        mode: ProgressionMode,
    ) -> NotificationDispatchReport {
        let clean_title = {
            let t = game_title.trim();
            if t.is_empty() {
                canonical_game_id
            } else {
                t
            }
        };
        let clean_pack = {
            let p = edition_or_pack.trim();
            if p.is_empty() {
                canonical_game_id
            } else {
                p
            }
        };
        let ev = NotificationEvent {
            timestamp: now_rfc3339(),
            kind: "game_active".to_string(),
            app_name: Some("OneHundo".to_string()),
            title: "OneHundo Active".to_string(),
            body: format!("Tracking {} ({})", clean_title, clean_pack),
            canonical_game_id: Some(canonical_game_id.to_string()),
            progression_mode: Some(mode.as_str().to_string()),
            trust_tier: None,
            verification_state: None,
        };
        self.push_and_dispatch_async_report(ev)
    }

    pub fn notify_unlock(
        &self,
        canonical_game_id: &str,
        game_title: Option<&str>,
        title: &str,
        description: &str,
        _points: u32,
        mode: ProgressionMode,
        trust: TrustTier,
        verification: VerificationState,
    ) {
        let ev = Self::build_unlock_event(
            "achievement_unlock",
            canonical_game_id,
            game_title,
            title,
            description,
            mode,
            trust,
            verification,
        );
        self.push_and_dispatch(ev);
    }

    pub fn notify_test_sync(
        &self,
        canonical_game_id: &str,
        game_title: Option<&str>,
        title: &str,
        description: &str,
        mode: ProgressionMode,
        trust: TrustTier,
        verification: VerificationState,
    ) -> NotificationDispatchReport {
        let ev = Self::build_unlock_event(
            "test_notification",
            canonical_game_id,
            game_title,
            title,
            description,
            mode,
            trust,
            verification,
        );
        self.push_and_dispatch_sync(ev)
    }

    pub fn notify_trust_downgrade(
        &self,
        canonical_game_id: &str,
        from: TrustTier,
        to: TrustTier,
        reason: &str,
    ) {
        let heading = "Fair-Play Notice: Session Downgraded".to_string();
        let body = format!(
            "{}: {} -> {} ({}). Local progress preserved.",
            canonical_game_id, from, to, reason
        );
        self.push_and_dispatch(NotificationEvent {
            timestamp: now_rfc3339(),
            kind: "trust_downgrade".to_string(),
            app_name: None,
            title: heading,
            body,
            canonical_game_id: Some(canonical_game_id.to_string()),
            progression_mode: None,
            trust_tier: Some(to.as_str().to_string()),
            verification_state: Some("UNVERIFIED".to_string()),
        });
    }

    pub fn notify_frame_gap(&self, canonical_game_id: &str, gap_count: u64) {
        let heading = "Game Health Warning: Frame Gap Detected".to_string();
        let body = format!(
            "{}: {} frame(s) skipped; deterministic evaluation marked incomplete.",
            canonical_game_id, gap_count
        );
        self.push_and_dispatch(NotificationEvent {
            timestamp: now_rfc3339(),
            kind: "frame_gap".to_string(),
            app_name: None,
            title: heading,
            body,
            canonical_game_id: Some(canonical_game_id.to_string()),
            progression_mode: None,
            trust_tier: None,
            verification_state: None,
        });
    }

    fn dispatch_os_notification(
        kind: &str,
        app_name: &str,
        title: &str,
        body: &str,
    ) -> Result<(), String> {
        let icon_path = Self::ensure_cached_icon(kind);
        let mut notif = notify_rust::Notification::new();
        notif.appname(app_name).summary(title).body(body);
        if let Some(icon_str) = icon_path.as_deref().and_then(Path::to_str) {
            notif.icon(icon_str).image_path(icon_str);
        } else {
            notif.icon("onehundo");
        }
        match notif.show() {
            Ok(_) => Ok(()),
            Err(dbus_err) => {
                // Fallback to notify-send CLI when running in environments without zbus support.
                match Self::run_notify_send(icon_path.as_deref(), app_name, title, body) {
                    Ok(()) => Ok(()),
                    Err(cli_err) => Err(format!("notify-rust: {}; {}", dbus_err, cli_err)),
                }
            }
        }
    }

    fn run_notify_send(
        icon_path: Option<&Path>,
        app_name: &str,
        title: &str,
        body: &str,
    ) -> Result<(), String> {
        let mut args = vec![format!("--app-name={}", app_name)];
        if let Some(icon_str) = icon_path.and_then(Path::to_str) {
            args.push(format!("--icon={}", icon_str));
        }
        args.push(title.to_string());
        args.push(body.to_string());
        let out = Command::new("notify-send")
            .args(&args)
            .output()
            .map_err(|e| format!("failed launching notify-send: {}", e))?;
        if out.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let detail = if stderr.is_empty() {
                format!("exit status {}", out.status)
            } else {
                format!("exit status {}: {}", out.status, stderr)
            };
            Err(format!("notify-send failed ({})", detail))
        }
    }

    fn record_history(&self, ev: NotificationEvent) {
        if let Ok(mut hist) = self.history.lock() {
            if hist.len() >= self.max_history {
                hist.pop_front();
            }
            hist.push_back(ev);
        }
    }

    fn push_and_dispatch(&self, ev: NotificationEvent) {
        eprintln!("[onehundo-notify] {}: {}", ev.title, ev.body);
        if self.os_toasts_enabled {
            let kind = ev.kind.clone();
            let app_name = ev.app_name.as_deref().unwrap_or("OneHundo").to_string();
            let title = ev.title.clone();
            let body = ev.body.clone();
            std::thread::spawn(move || {
                if let Err(err) = Self::dispatch_os_notification(&kind, &app_name, &title, &body) {
                    eprintln!("[onehundo-notify-error] {}", err);
                }
            });
        }
        self.record_history(ev);
    }

    fn push_and_dispatch_async_report(&self, ev: NotificationEvent) -> NotificationDispatchReport {
        eprintln!("[onehundo-notify] {}: {}", ev.title, ev.body);
        if self.os_toasts_enabled {
            let kind = ev.kind.clone();
            let app_name = ev.app_name.as_deref().unwrap_or("OneHundo").to_string();
            let title = ev.title.clone();
            let body = ev.body.clone();
            std::thread::spawn(move || {
                if let Err(err) = Self::dispatch_os_notification(&kind, &app_name, &title, &body) {
                    eprintln!("[onehundo-notify-error] {}", err);
                }
            });
        }
        self.record_history(ev.clone());
        NotificationDispatchReport {
            dispatched: self.os_toasts_enabled,
            os_toasts_enabled: self.os_toasts_enabled,
            error: None,
            event: ev,
        }
    }

    fn push_and_dispatch_sync(&self, ev: NotificationEvent) -> NotificationDispatchReport {
        eprintln!("[onehundo-notify] {}: {}", ev.title, ev.body);
        let (dispatched, error) = if self.os_toasts_enabled {
            let app_name = ev.app_name.as_deref().unwrap_or("OneHundo");
            match Self::dispatch_os_notification(&ev.kind, app_name, &ev.title, &ev.body) {
                Ok(()) => (true, None),
                Err(err) => {
                    eprintln!("[onehundo-notify-error] {}", err);
                    (false, Some(err))
                }
            }
        } else {
            (false, None)
        };
        self.record_history(ev.clone());
        NotificationDispatchReport {
            dispatched,
            os_toasts_enabled: self.os_toasts_enabled,
            error,
            event: ev,
        }
    }

    pub fn recent(&self) -> Vec<NotificationEvent> {
        self.history
            .lock()
            .map(|h| h.iter().cloned().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_notification_tracker_deduplicates_and_rearms_on_exit() {
        let mut tracker = SessionNotificationTracker::new();
        let first = tracker.on_game_active(
            "game:nintendo.oot",
            "nintendo.oot",
            "The Legend of Zelda: Ocarina of Time",
            "nintendo.oot.longshot.pc",
        );
        let first = first.expect("first activation must fire");
        assert_eq!(first.title, "OneHundo Active");
        assert_eq!(
            first.body,
            "Tracking The Legend of Zelda: Ocarina of Time (nintendo.oot.longshot.pc)"
        );

        // Repeated poll ticks while game remains active must not fire again.
        assert!(tracker
            .on_game_active(
                "game:nintendo.oot",
                "nintendo.oot",
                "The Legend of Zelda: Ocarina of Time",
                "nintendo.oot.longshot.pc",
            )
            .is_none());
        assert_eq!(tracker.active_count(), 1);

        // Exiting the game re-arms the notification.
        assert!(tracker.on_game_inactive("game:nintendo.oot"));
        assert!(!tracker.is_active("game:nintendo.oot"));

        let relaunched = tracker.on_game_active(
            "game:nintendo.oot",
            "nintendo.oot",
            "The Legend of Zelda: Ocarina of Time",
            "nintendo.oot.longshot.pc",
        );
        assert!(relaunched.is_some());
    }

    #[test]
    fn sync_active_prefix_only_clears_missing_sessions_with_matching_prefix() {
        let mut tracker = SessionNotificationTracker::new();
        assert!(tracker
            .on_game_active("proc:sony.bloodborne", "sony.bloodborne", "Bloodborne", "bbport")
            .is_some());
        assert!(tracker
            .on_game_active("session:s1", "nintendo.sm64", "Super Mario 64", "sm64")
            .is_some());

        let still_running: HashSet<String> = HashSet::new();
        let removed = tracker.sync_active_prefix("proc:", &still_running);
        assert_eq!(removed, 1);
        assert!(!tracker.is_active("proc:sony.bloodborne"));
        assert!(tracker.is_active("session:s1"));
    }

    #[test]
    fn notify_game_active_records_event_without_os_dispatch_when_disabled() {
        let mgr = NotificationManager::new(false);
        let report = mgr.notify_game_active(
            "sony.bloodborne",
            "Bloodborne",
            "PS4 / shadPS4",
            ProgressionMode::Standard,
        );
        assert!(!report.dispatched);
        assert!(!report.os_toasts_enabled);
        assert_eq!(report.event.kind, "game_active");
        assert_eq!(report.event.app_name.as_deref(), Some("OneHundo"));
        assert_eq!(report.event.title, "OneHundo Active");
        assert_eq!(report.event.body, "Tracking Bloodborne (PS4 / shadPS4)");
        assert_eq!(mgr.recent().len(), 1);
    }

    #[test]
    fn notification_icons_materialize_valid_pngs_idempotently() {
        let temp_dir = std::env::temp_dir().join(format!(
            "onehundo-notify-icon-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_dir);

        let png_magic = b"\x89PNG\r\n\x1a\n";
        let kinds = [
            ("achievement_unlock", "trophy-unlock-v1.png"),
            ("test_notification", "trophy-unlock-v1.png"),
            ("game_active", "trophy-active-v1.png"),
            ("trust_downgrade", "trophy-warning-v1.png"),
            ("frame_gap", "trophy-warning-v1.png"),
        ];

        for (kind, expected_name) in kinds {
            let path = NotificationManager::ensure_cached_icon_in_dir(&temp_dir, kind)
                .expect("materializing embedded PNG should succeed");
            assert_eq!(path.file_name().and_then(|s| s.to_str()), Some(expected_name));
            let bytes = fs::read(&path).expect("icon PNG should be readable");
            assert!(bytes.starts_with(png_magic), "invalid PNG header for {}", kind);
            assert!(bytes.len() > 512, "unexpectedly small PNG for {}", kind);

            // Second call is idempotent and preserves the exact path and bytes.
            let second = NotificationManager::ensure_cached_icon_in_dir(&temp_dir, kind)
                .expect("idempotent cache hit should succeed");
            assert_eq!(second, path);
        }

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
