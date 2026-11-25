pub mod server;

use crate::core::config::WatchedObservationDir;
use crate::core::session::SessionInfo;
use crate::core::types::ProgressionMode;
use crate::ipc::shm_arena::SlotOccupancy;
use crate::observation::external_importers::ObservationHistoryRow;
use crate::packs::manifest::PackSummary;
use crate::ui::notifications::NotificationEvent;
use serde::{Deserialize, Serialize};

fn default_progression_mode() -> ProgressionMode {
    ProgressionMode::Standard
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncTelemetry {
    pub platform_configured: bool,
    pub device_registered: bool,
    pub last_sync_at: Option<String>,
    pub last_receipt_status: Option<String>,
    pub last_acceptance_status: Option<String>,
    pub last_sync_error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlaneARingTelemetry {
    pub session_id: String,
    pub shm_name: String,
    pub active: bool,
    pub snapshots_consumed: u64,
    pub last_sequence_number: u64,
    pub sequence_gaps_detected: u64,
    pub producer_frame_gaps_reported: u64,
    #[serde(default)]
    pub shm_frame_gaps_reported: u64,
    #[serde(default)]
    pub ipc_frame_gaps_reported: u64,
    pub crc_errors: u64,
    pub last_poll_latency_us: u64,
    pub max_poll_latency_us: u64,
    pub last_eval_latency_us: u64,
    pub max_eval_latency_us: u64,
    pub unlocked_rules_count: usize,
    pub total_rules_count: usize,
    pub slot_occupancy: SlotOccupancy,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlaneATelemetry {
    pub active_rings_count: usize,
    pub total_snapshots_consumed: u64,
    pub total_sequence_gaps: u64,
    pub total_crc_errors: u64,
    pub last_eval_latency_us: u64,
    pub max_eval_latency_us: u64,
    pub rings: Vec<PlaneARingTelemetry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameHealthSnapshot {
    pub daemon_version: String,
    pub uptime_seconds: u64,
    #[serde(default = "default_progression_mode")]
    pub active_progression_mode: ProgressionMode,
    pub loaded_packs_count: usize,
    pub pack_load_errors_count: usize,
    pub total_journal_events: u64,
    #[serde(default)]
    pub plane_b_events_committed: u64,
    pub sync_queue_depth: u64,
    pub synced_events_count: u64,
    pub rejected_events_count: u64,
    pub total_frame_gaps: u64,
    pub observations_imported: u64,
    pub last_receipt_status: Option<String>,
    pub last_acceptance_status: Option<String>,
    pub sync: SyncTelemetry,
    pub plane_a: PlaneATelemetry,
    pub sessions: Vec<SessionInfo>,
    #[serde(default)]
    pub installed_packs: Vec<PackSummary>,
    #[serde(default)]
    pub recent_observations: Vec<ObservationHistoryRow>,
    #[serde(default)]
    pub watched_dirs: Vec<WatchedObservationDir>,
    pub recent_notifications: Vec<NotificationEvent>,
}
