pub mod server;

use crate::core::session::SessionInfo;
use crate::ui::notifications::NotificationEvent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncTelemetry {
    pub platform_configured: bool,
    pub device_registered: bool,
    pub last_sync_at: Option<String>,
    pub last_receipt_status: Option<String>,
    pub last_acceptance_status: Option<String>,
    pub last_sync_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameHealthSnapshot {
    pub daemon_version: String,
    pub uptime_seconds: u64,
    pub loaded_packs_count: usize,
    pub pack_load_errors_count: usize,
    pub total_journal_events: u64,
    pub sync_queue_depth: u64,
    pub synced_events_count: u64,
    pub rejected_events_count: u64,
    pub total_frame_gaps: u64,
    pub observations_imported: u64,
    pub last_receipt_status: Option<String>,
    pub last_acceptance_status: Option<String>,
    pub sync: SyncTelemetry,
    pub sessions: Vec<SessionInfo>,
    pub recent_notifications: Vec<NotificationEvent>,
}
