use crate::core::config::{generate_uuid_v4, load_credentials, load_or_init_config, Credentials, DaemonConfig};
use crate::core::session::{SessionInfo, SessionManager};
use crate::core::types::{
    now_rfc3339, FairPlayViolation, IngestionPlane, LifecycleSignal, ProgressionMode, TrustTier,
};
use crate::diagnostics::{GameHealthSnapshot, SyncTelemetry};
use crate::evaluation::memory_arena::{FrameStepOutcome, MemoryArenaEvaluatorState};
use crate::evaluation::semantic_events::{
    evaluate_and_commit_plane_b, SemanticCommitOutcome, SemanticEventPayload,
};
use crate::observation::external_importers::{
    import_observation_records, parse_observation_json_str, ObservationImportSummary,
};
use crate::packs::matcher::{MatchQuery, PackMatchResult, PackRegistry};
use crate::storage::journal::{open_and_init_db, query_journal_stats, record_frame_gap_db};
use crate::storage::state::{list_encore_runs, list_stats, list_trophies};
use crate::sync::client::{register_device, sync_pending_batch, SyncBatchSummary};
use crate::ui::notifications::NotificationManager;
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub struct DaemonEngine {
    pub config: DaemonConfig,
    pub credentials: Mutex<Credentials>,
    pub db: Mutex<Connection>,
    pub packs: Mutex<PackRegistry>,
    pub sessions: Mutex<SessionManager>,
    pub plane_a_evaluators: Mutex<HashMap<String, MemoryArenaEvaluatorState>>,
    pub plane_a_regions: Mutex<HashMap<String, HashMap<u32, usize>>>,
    pub notifications: NotificationManager,
    pub sync_telemetry: Mutex<SyncTelemetry>,
    pub started_at: Instant,
}

impl DaemonEngine {
    pub fn initialize(mut config: DaemonConfig) -> Result<Arc<Self>, String> {
        let loaded_cfg = load_or_init_config(&config.config_dir).unwrap_or_else(|_| config.clone());
        // Preserve CLI overrides on config while inheriting persisted device_uuid and platform_url
        if config.device_uuid.is_empty() {
            config.device_uuid = if loaded_cfg.device_uuid.is_empty() {
                generate_uuid_v4()
            } else {
                loaded_cfg.device_uuid
            };
        }
        if config.platform_url.is_none() {
            config.platform_url = loaded_cfg.platform_url;
        }

        let creds = load_credentials(&config.config_dir);
        let db_path = config.data_dir.join("onehundo_state.db");
        let conn = open_and_init_db(&db_path)?;

        let mut packs = PackRegistry::new();
        for dir in &config.packs_dirs {
            let _ = std::fs::create_dir_all(dir);
            let _ = packs.discover_in_dir(dir);
        }

        let sync_tel = SyncTelemetry {
            platform_configured: config.platform_url.is_some(),
            device_registered: creds.device_token.is_some(),
            ..Default::default()
        };

        Ok(Arc::new(Self {
            notifications: NotificationManager::new(config.notifications_enabled),
            config,
            credentials: Mutex::new(creds),
            db: Mutex::new(conn),
            packs: Mutex::new(packs),
            sessions: Mutex::new(SessionManager::new()),
            plane_a_evaluators: Mutex::new(HashMap::new()),
            plane_a_regions: Mutex::new(HashMap::new()),
            sync_telemetry: Mutex::new(sync_tel),
            started_at: Instant::now(),
        }))
    }

    pub fn load_pack_path(&self, path: &Path) -> Result<String, String> {
        let mut packs = self.packs.lock().map_err(|_| "packs lock poisoned".to_string())?;
        packs.load_pack_file(path)
    }

    pub fn start_session(
        &self,
        session_id: String,
        producer_id: String,
        canonical_game_id: String,
        query: MatchQuery,
        plane: IngestionPlane,
        mode: ProgressionMode,
        trust: TrustTier,
    ) -> (SessionInfo, Option<PackMatchResult>) {
        let matched = self
            .packs
            .lock()
            .ok()
            .and_then(|reg| reg.match_pack(&query).map(|(_, r)| r));
        let pack_id = matched.as_ref().map(|m| m.pack_id.clone());
        let resolved_game_id = matched
            .as_ref()
            .map(|m| m.canonical_game_id.clone())
            .unwrap_or(canonical_game_id);

        let info = self
            .sessions
            .lock()
            .unwrap()
            .start_session(session_id, producer_id, resolved_game_id, pack_id, plane, mode, trust);

        if plane == IngestionPlane::MemoryArena {
            let mut evals = self.plane_a_evaluators.lock().unwrap();
            evals.insert(
                info.session_id.clone(),
                MemoryArenaEvaluatorState::new(&info.canonical_game_id, &info.session_id),
            );
        }

        (info, matched)
    }

    pub fn end_session(&self, session_id: &str) {
        if let Ok(mut s) = self.sessions.lock() {
            s.end_session(session_id);
        }
    }

    pub fn handle_plane_b_event(
        &self,
        session_id_override: Option<&str>,
        ev: &SemanticEventPayload,
        raw_payload: &str,
    ) -> Result<SemanticCommitOutcome, String> {
        let sid = session_id_override.unwrap_or(&ev.session_id);

        let (pack_id, mode, trust) = {
            let mut sessions = self.sessions.lock().unwrap();
            if let Some(s) = sessions.get_mut(sid) {
                s.last_sequence_number = s.last_sequence_number.max(ev.sequence_number as u64);
                let mode = ev
                    .progression_mode
                    .as_deref()
                    .and_then(ProgressionMode::from_str_strict)
                    .unwrap_or(s.progression_mode);
                // Effective trust tier respects automatic downgrade (never escalates above active_trust_tier)
                let req_trust = ev
                    .trust_tier
                    .as_deref()
                    .and_then(TrustTier::from_str_strict)
                    .unwrap_or(s.active_trust_tier);
                let eff_trust = req_trust.min(s.active_trust_tier);
                (s.matched_pack_id.clone(), mode, eff_trust)
            } else {
                let mode = ev
                    .progression_mode
                    .as_deref()
                    .and_then(ProgressionMode::from_str_strict)
                    .unwrap_or(ProgressionMode::Standard);
                let trust = ev
                    .trust_tier
                    .as_deref()
                    .and_then(TrustTier::from_str_strict)
                    .unwrap_or(TrustTier::Local);
                (None, mode, trust)
            }
        };

        let pack_clone = {
            let packs = self.packs.lock().unwrap();
            pack_id
                .as_deref()
                .and_then(|pid| packs.get(pid).cloned())
                .or_else(|| {
                    packs
                        .match_pack(&MatchQuery {
                            canonical_game_id: Some(ev.canonical_game_id.clone()),
                            sidecar_uuid: None,
                            rom_hash: None,
                            recomp_game_id: None,
                            build_id: None,
                            asset_archive_hash: None,
                            platform_id: None,
                            region: None,
                        })
                        .map(|(p, _)| p.clone())
                })
        };

        let occurred_at = now_rfc3339();
        let outcome = {
            let mut db = self.db.lock().unwrap();
            evaluate_and_commit_plane_b(
                &mut db,
                pack_clone.as_ref(),
                ev,
                &occurred_at,
                mode,
                trust,
                raw_payload,
            )?
        };

        for unlocked in &outcome.newly_unlocked {
            self.notifications.notify_unlock(
                &ev.canonical_game_id,
                &unlocked.title,
                &unlocked.description,
                unlocked.points,
                unlocked.progression_mode,
                unlocked.trust_tier,
                unlocked.verification_state,
            );
        }

        Ok(outcome)
    }

    pub fn register_plane_a_region(&self, session_id: &str, region_id: u32, offset_in_arena: usize) {
        if let Ok(mut regions) = self.plane_a_regions.lock() {
            regions
                .entry(session_id.to_string())
                .or_default()
                .insert(region_id, offset_in_arena);
        }
    }

    pub fn handle_plane_a_frame(
        &self,
        session_id: &str,
        sequence_number: u64,
        lifecycle_signal: LifecycleSignal,
        frame_gap_count: u32,
        guest_memory: &[u8],
    ) -> Result<FrameStepOutcome, String> {
        if lifecycle_signal == LifecycleSignal::SaveStateLoad {
            let _ = self.apply_fair_play_violation(session_id, FairPlayViolation::SaveStateLoad);
        }

        let (producer_id, canonical_game_id, pack_id, mode, trust) = {
            let sessions = self.sessions.lock().unwrap();
            let s = sessions
                .get(session_id)
                .ok_or_else(|| format!("unknown session '{}'", session_id))?;
            (
                s.producer_id.clone(),
                s.canonical_game_id.clone(),
                s.matched_pack_id.clone(),
                s.progression_mode,
                s.active_trust_tier,
            )
        };

        let pack_clone = {
            let packs = self.packs.lock().unwrap();
            pack_id.as_deref().and_then(|pid| packs.get(pid).cloned())
        };
        let regions = self
            .plane_a_regions
            .lock()
            .unwrap()
            .get(session_id)
            .cloned()
            .unwrap_or_default();

        let outcome = {
            let mut evals = self.plane_a_evaluators.lock().unwrap();
            let eval = evals
                .entry(session_id.to_string())
                .or_insert_with(|| MemoryArenaEvaluatorState::new(&canonical_game_id, session_id));
            let mut db = self.db.lock().unwrap();
            eval.step_frame(
                &mut db,
                pack_clone.as_ref(),
                &producer_id,
                sequence_number,
                lifecycle_signal,
                frame_gap_count,
                guest_memory,
                &regions,
                mode,
                trust,
            )?
        };

        {
            let mut sessions = self.sessions.lock().unwrap();
            if let Some(s) = sessions.get_mut(session_id) {
                s.frame_snapshots_evaluated += 1;
                s.last_sequence_number = s.last_sequence_number.max(sequence_number);
                if outcome.frame_gap_detected {
                    s.frame_gaps_recorded += outcome.gap_count;
                    s.evaluation_incomplete = true;
                }
            }
        }

        if outcome.frame_gap_detected {
            self.notifications
                .notify_frame_gap(&canonical_game_id, outcome.gap_count);
        }

        for ach_id in &outcome.unlocked_achievements {
            let (title, desc, pts) = pack_clone
                .as_ref()
                .and_then(|p| {
                    p.manifest
                        .achievements
                        .iter()
                        .find(|a| &a.canonical_achievement_id == ach_id)
                })
                .map(|a| (a.title.clone(), a.description.clone(), a.points))
                .unwrap_or_else(|| (ach_id.clone(), "Plane A unlock".to_string(), 10));
            let ver = if trust == TrustTier::VerifiedIntegration {
                crate::core::types::VerificationState::Verified
            } else {
                crate::core::types::VerificationState::Unverified
            };
            self.notifications
                .notify_unlock(&canonical_game_id, &title, &desc, pts, mode, trust, ver);
        }

        Ok(outcome)
    }

    pub fn handle_lifecycle_signal(
        &self,
        session_id: &str,
        signal: LifecycleSignal,
        frame_gap_count: u32,
    ) -> Result<(), String> {
        match signal {
            LifecycleSignal::FrameGap => {
                let gap = (frame_gap_count.max(1)) as u64;
                let (producer_id, game_id, seq) = {
                    let mut sessions = self.sessions.lock().unwrap();
                    sessions.record_frame_gap(session_id, gap);
                    if let Some(s) = sessions.get(session_id) {
                        (
                            s.producer_id.clone(),
                            s.canonical_game_id.clone(),
                            s.last_sequence_number,
                        )
                    } else {
                        ("unknown".to_string(), "unknown".to_string(), 0)
                    }
                };
                if let Ok(mut evals) = self.plane_a_evaluators.lock() {
                    if let Some(eval) = evals.get_mut(session_id) {
                        eval.frame_gaps_recorded += gap;
                        eval.evaluation_incomplete = true;
                    }
                }
                let db = self.db.lock().unwrap();
                record_frame_gap_db(
                    &db,
                    session_id,
                    &producer_id,
                    &game_id,
                    gap,
                    seq,
                    "ipc_lifecycle_frame_gap",
                )?;
                self.notifications.notify_frame_gap(&game_id, gap);
            }
            LifecycleSignal::SaveStateLoad => {
                let _ = self.apply_fair_play_violation(session_id, FairPlayViolation::SaveStateLoad);
                if let Ok(mut evals) = self.plane_a_evaluators.lock() {
                    if let Some(eval) = evals.get_mut(session_id) {
                        eval.reset_all_hit_counts();
                    }
                }
            }
            LifecycleSignal::SoftReset | LifecycleSignal::HardReset => {
                if let Ok(mut evals) = self.plane_a_evaluators.lock() {
                    if let Some(eval) = evals.get_mut(session_id) {
                        eval.reset_all_hit_counts();
                    }
                }
            }
            LifecycleSignal::NormalFrame => {}
        }
        Ok(())
    }

    pub fn apply_fair_play_violation(
        &self,
        session_id: &str,
        violation: FairPlayViolation,
    ) -> Option<(TrustTier, TrustTier, String)> {
        let downgrade = self
            .sessions
            .lock()
            .unwrap()
            .trigger_fair_play_violation(session_id, violation);
        if let Some((from, to, ref reason)) = downgrade {
            let game_id = self
                .sessions
                .lock()
                .unwrap()
                .get(session_id)
                .map(|s| s.canonical_game_id.clone())
                .unwrap_or_else(|| "unknown".to_string());
            self.notifications
                .notify_trust_downgrade(&game_id, from, to, reason);
        }
        downgrade
    }

    pub fn import_plane_c_json(&self, json_str: &str) -> Result<ObservationImportSummary, String> {
        let records = parse_observation_json_str(json_str)?;
        let mut db = self.db.lock().unwrap();
        import_observation_records(&mut db, &records)
    }

    pub fn pending_sync_count(&self) -> Result<u64, String> {
        let db = self.db.lock().unwrap();
        let count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM event_journal WHERE sync_state = 'PENDING'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| format!("failed counting pending sync events: {}", e))?;
        Ok(count.max(0) as u64)
    }

    pub fn ensure_device_registered(&self) -> Result<bool, String> {
        let base_url = self
            .config
            .platform_url
            .clone()
            .ok_or_else(|| "platform_url not configured".to_string())?;
        let mut creds = self.credentials.lock().unwrap().clone();
        if creds.device_token.is_some() {
            return Ok(true);
        }
        let res = register_device(
            &base_url,
            &self.config.device_uuid,
            &self.config.device_name,
            &self.config.config_dir,
            &mut creds,
        );
        let mut tel = self.sync_telemetry.lock().unwrap();
        match res {
            Ok(_) => {
                *self.credentials.lock().unwrap() = creds;
                tel.device_registered = true;
                tel.last_sync_error = None;
                Ok(true)
            }
            Err(err) => {
                tel.device_registered = false;
                tel.last_sync_error = Some(err.clone());
                Err(err)
            }
        }
    }

    pub fn sync_if_pending(&self) -> Result<Option<SyncBatchSummary>, String> {
        if self.config.platform_url.is_none() {
            return Ok(None);
        }
        self.ensure_device_registered()?;
        if self.pending_sync_count()? == 0 {
            return Ok(None);
        }

        let mut total_synced = 0usize;
        let mut last_receipt: Option<String> = None;
        let mut last_acceptance: Option<String> = None;

        while self.pending_sync_count()? > 0 {
            let batch = self.sync_now()?;
            if batch.synced_count == 0 {
                break;
            }
            total_synced += batch.synced_count;
            if batch.batch_receipt_status.is_some() {
                last_receipt = batch.batch_receipt_status;
            }
            if batch.last_acceptance_status.is_some() {
                last_acceptance = batch.last_acceptance_status;
            }
        }

        Ok(Some(SyncBatchSummary {
            synced_count: total_synced,
            batch_receipt_status: last_receipt,
            last_acceptance_status: last_acceptance,
        }))
    }

    pub fn sync_now(&self) -> Result<SyncBatchSummary, String> {
        let base_url = self
            .config
            .platform_url
            .clone()
            .ok_or_else(|| "platform_url not configured".to_string())?;

        let mut creds = self.credentials.lock().unwrap().clone();
        if creds.device_token.is_none() {
            let _ = register_device(
                &base_url,
                &self.config.device_uuid,
                &self.config.device_name,
                &self.config.config_dir,
                &mut creds,
            )?;
            *self.credentials.lock().unwrap() = creds.clone();
        }

        let res = {
            let mut db = self.db.lock().unwrap();
            sync_pending_batch(&mut db, &base_url, &creds, self.config.sync_batch_size)
        };

        let mut tel = self.sync_telemetry.lock().unwrap();
        tel.device_registered = creds.device_token.is_some();
        match &res {
            Ok(summary) => {
                tel.last_sync_at = Some(now_rfc3339());
                if summary.batch_receipt_status.is_some() {
                    tel.last_receipt_status = summary.batch_receipt_status.clone();
                }
                if summary.last_acceptance_status.is_some() {
                    tel.last_acceptance_status = summary.last_acceptance_status.clone();
                }
                tel.last_sync_error = None;
            }
            Err(err) => {
                tel.last_sync_error = Some(err.clone());
            }
        }
        res
    }

    pub fn health_snapshot(&self) -> Result<GameHealthSnapshot, String> {
        let stats = {
            let db = self.db.lock().unwrap();
            query_journal_stats(&db)?
        };
        let (loaded_packs_count, pack_load_errors_count) = {
            let packs = self.packs.lock().unwrap();
            (packs.list().len(), packs.load_errors().len())
        };
        let sessions = self.sessions.lock().unwrap().all_sessions();
        let sync = self.sync_telemetry.lock().unwrap().clone();
        let recent_notifications = self.notifications.recent();

        Ok(GameHealthSnapshot {
            daemon_version: crate::core::types::PROTOCOL_VERSION.to_string(),
            uptime_seconds: self.started_at.elapsed().as_secs(),
            loaded_packs_count,
            pack_load_errors_count,
            total_journal_events: stats.total_events,
            sync_queue_depth: stats.pending_sync_count,
            synced_events_count: stats.synced_count,
            rejected_events_count: stats.rejected_count,
            total_frame_gaps: stats.total_frame_gaps,
            observations_imported: stats.observation_count,
            last_receipt_status: stats.last_receipt_status,
            last_acceptance_status: stats.last_acceptance_status,
            sync,
            sessions,
            recent_notifications,
        })
    }

    pub fn progression_json(&self, canonical_game_id: Option<&str>) -> Result<serde_json::Value, String> {
        let db = self.db.lock().unwrap();
        let trophies = list_trophies(&db, canonical_game_id)?;
        let stats = list_stats(&db, canonical_game_id)?;
        let encore_runs = list_encore_runs(&db)?;
        Ok(serde_json::json!({
            "trophies": trophies,
            "stats": stats,
            "encore_runs": encore_runs,
        }))
    }
}
