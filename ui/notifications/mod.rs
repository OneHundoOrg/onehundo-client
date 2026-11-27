use crate::core::types::{now_rfc3339, ProgressionMode, TrustTier, VerificationState};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::process::Command;
use std::sync::Mutex;

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

    fn run_notify_send(app_name: &str, title: &str, body: &str) -> Result<(), String> {
        let out = Command::new("notify-send")
            .args([format!("--app-name={}", app_name), title.to_string(), body.to_string()])
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
            let app_name = ev.app_name.as_deref().unwrap_or("OneHundo").to_string();
            let title = ev.title.clone();
            let body = ev.body.clone();
            std::thread::spawn(move || {
                if let Err(err) = Self::run_notify_send(&app_name, &title, &body) {
                    eprintln!("[onehundo-notify-error] {}", err);
                }
            });
        }
        self.record_history(ev);
    }

    fn push_and_dispatch_sync(&self, ev: NotificationEvent) -> NotificationDispatchReport {
        eprintln!("[onehundo-notify] {}: {}", ev.title, ev.body);
        let (dispatched, error) = if self.os_toasts_enabled {
            let app_name = ev.app_name.as_deref().unwrap_or("OneHundo");
            match Self::run_notify_send(app_name, &ev.title, &ev.body) {
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
