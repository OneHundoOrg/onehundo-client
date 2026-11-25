use crate::core::types::{now_rfc3339, ProgressionMode, TrustTier, VerificationState};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::process::Command;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationEvent {
    pub timestamp: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub canonical_game_id: Option<String>,
    pub progression_mode: Option<String>,
    pub trust_tier: Option<String>,
    pub verification_state: Option<String>,
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

    pub fn notify_unlock(
        &self,
        canonical_game_id: &str,
        title: &str,
        description: &str,
        points: u32,
        mode: ProgressionMode,
        trust: TrustTier,
        verification: VerificationState,
    ) {
        let heading = format!("Achievement Unlocked ({} pts)", points);
        let body = format!(
            "{} — {} [{} / {} / {}]",
            title, description, mode, trust, verification
        );
        self.push_and_dispatch(NotificationEvent {
            timestamp: now_rfc3339(),
            kind: "achievement_unlock".to_string(),
            title: heading,
            body,
            canonical_game_id: Some(canonical_game_id.to_string()),
            progression_mode: Some(mode.as_str().to_string()),
            trust_tier: Some(trust.as_str().to_string()),
            verification_state: Some(verification.as_str().to_string()),
        });
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
            title: heading,
            body,
            canonical_game_id: Some(canonical_game_id.to_string()),
            progression_mode: None,
            trust_tier: None,
            verification_state: None,
        });
    }

    fn push_and_dispatch(&self, ev: NotificationEvent) {
        eprintln!("[onehundo-notify] {}: {}", ev.title, ev.body);
        if self.os_toasts_enabled {
            let _ = Command::new("notify-send")
                .args(["--app-name=OneHundo", &ev.title, &ev.body])
                .spawn();
        }
        if let Ok(mut hist) = self.history.lock() {
            if hist.len() >= self.max_history {
                hist.pop_front();
            }
            hist.push_back(ev);
        }
    }

    pub fn recent(&self) -> Vec<NotificationEvent> {
        self.history
            .lock()
            .map(|h| h.iter().cloned().collect())
            .unwrap_or_default()
    }
}
