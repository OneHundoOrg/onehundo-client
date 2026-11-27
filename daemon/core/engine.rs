use crate::core::config::{
    generate_uuid_v4, load_credentials, load_or_init_config, save_config, Credentials,
    DaemonConfig, WatchedObservationDir,
};
use crate::core::session::{SessionInfo, SessionManager};
use crate::core::types::{
    now_rfc3339, FairPlayViolation, IngestionPlane, LifecycleSignal, ProgressionMode, TrustTier,
    VerificationState,
};
use crate::diagnostics::{
    GameHealthSnapshot, LauncherGameTelemetry, PlaneARingTelemetry, PlaneATelemetry,
    RetroArchTelemetry, SyncTelemetry,
};
use crate::ipc::retroarch::RetroArchClient;
use crate::evaluation::memory_arena::{FrameStepOutcome, MemoryArenaEvaluatorState};
use crate::evaluation::semantic_events::{
    evaluate_and_commit_plane_b, SemanticCommitOutcome, SemanticEventPayload,
};
use crate::ipc::shm_arena::ShmRingStats;
use crate::observation::external_importers::watcher::{DirectoryWatcherState, WatchPollSummary};
use crate::observation::external_importers::{
    import_observation_path_with_options, import_observation_records, list_observation_history,
    parse_observation_json_str, ImportOptions, ObservationImportSummary,
};
use crate::packs::matcher::{MatchQuery, PackMatchResult, PackRegistry};
use crate::storage::journal::{open_and_init_db, query_journal_stats, record_frame_gap_db};
use crate::storage::state::{list_encore_runs, list_stats, list_trophies};
use crate::sync::client::{register_device, sync_pending_batch, SyncBatchSummary};
use crate::ui::notifications::{NotificationDispatchReport, NotificationManager};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub struct DaemonEngine {
    pub config: DaemonConfig,
    pub credentials: Mutex<Credentials>,
    pub db: Mutex<Connection>,
    pub packs: Mutex<PackRegistry>,
    pub sessions: Mutex<SessionManager>,
    pub active_progression_mode: Mutex<ProgressionMode>,
    pub plane_a_evaluators: Mutex<HashMap<String, MemoryArenaEvaluatorState>>,
    pub plane_a_regions: Mutex<HashMap<String, HashMap<u32, usize>>>,
    pub plane_a_rings: Mutex<HashMap<String, PlaneARingTelemetry>>,
    pub plane_b_events_committed: AtomicU64,
    pub watched_dirs: Mutex<Vec<WatchedObservationDir>>,
    pub watcher_state: Mutex<DirectoryWatcherState>,
    pub launcher_games: Mutex<HashMap<String, LauncherGameTelemetry>>,
    pub retroarch_telemetry: Mutex<RetroArchTelemetry>,
    pub retroarch_sessions: Mutex<HashMap<String, (String, u64)>>,
    pub notifications: NotificationManager,
    pub sync_telemetry: Mutex<SyncTelemetry>,
    pub started_at: Instant,
}

impl DaemonEngine {
    pub fn initialize(mut config: DaemonConfig) -> Result<Arc<Self>, String> {
        let loaded_cfg = load_or_init_config(&config.config_dir).unwrap_or_else(|_| config.clone());
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
        for w in loaded_cfg.watched_observation_dirs {
            if !config.watched_observation_dirs.iter().any(|x| x.path == w.path) {
                config.watched_observation_dirs.push(w);
            }
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
        let initial_watched = config.watched_observation_dirs.clone();
        let ra_tel = RetroArchTelemetry {
            configured_addr: config.retroarch_udp_addr.clone(),
            ..Default::default()
        };

        Ok(Arc::new(Self {
            notifications: NotificationManager::new(config.notifications_enabled),
            config,
            credentials: Mutex::new(creds),
            db: Mutex::new(conn),
            packs: Mutex::new(packs),
            sessions: Mutex::new(SessionManager::new()),
            active_progression_mode: Mutex::new(ProgressionMode::Standard),
            plane_a_evaluators: Mutex::new(HashMap::new()),
            plane_a_regions: Mutex::new(HashMap::new()),
            plane_a_rings: Mutex::new(HashMap::new()),
            plane_b_events_committed: AtomicU64::new(0),
            watched_dirs: Mutex::new(initial_watched),
            watcher_state: Mutex::new(DirectoryWatcherState::new()),
            launcher_games: Mutex::new(HashMap::new()),
            retroarch_telemetry: Mutex::new(ra_tel),
            retroarch_sessions: Mutex::new(HashMap::new()),
            sync_telemetry: Mutex::new(sync_tel),
            started_at: Instant::now(),
        }))
    }

    pub fn load_pack_path(&self, path: &Path) -> Result<String, String> {
        let mut packs = self.packs.lock().map_err(|_| "packs lock poisoned".to_string())?;
        packs.load_pack_file(path)
    }

    pub fn current_progression_mode(&self) -> ProgressionMode {
        *self.active_progression_mode.lock().unwrap()
    }

    pub fn set_progression_mode(
        &self,
        session_id: Option<&str>,
        mode: ProgressionMode,
    ) -> Vec<SessionInfo> {
        if session_id.is_none() {
            *self.active_progression_mode.lock().unwrap() = mode;
        }
        self.sessions.lock().unwrap().set_mode(session_id, mode)
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
        if let Ok(mut rings) = self.plane_a_rings.lock() {
            if let Some(r) = rings.get_mut(session_id) {
                r.active = false;
            }
        }
    }

    pub fn update_plane_a_ring_telemetry(&self, session_id: &str, ring: ShmRingStats) {
        if let Ok(mut rings) = self.plane_a_rings.lock() {
            let entry = rings
                .entry(session_id.to_string())
                .or_insert_with(|| PlaneARingTelemetry {
                    session_id: session_id.to_string(),
                    ..Default::default()
                });
            entry.shm_name = ring.shm_name;
            entry.active = ring.active;
            entry.snapshots_consumed = ring.snapshots_consumed;
            entry.last_sequence_number = ring.last_sequence_number;
            entry.sequence_gaps_detected = ring.sequence_gaps_detected;
            entry.shm_frame_gaps_reported = ring.producer_frame_gaps_reported;
            entry.producer_frame_gaps_reported =
                entry.shm_frame_gaps_reported.saturating_add(entry.ipc_frame_gaps_reported);
            entry.crc_errors = ring.crc_errors;
            entry.last_poll_latency_us = ring.last_poll_latency_us;
            entry.max_poll_latency_us = entry.max_poll_latency_us.max(ring.max_poll_latency_us);
            entry.slot_occupancy = ring.slot_occupancy;
        }
    }

    pub fn handle_plane_b_event(
        &self,
        session_id_override: Option<&str>,
        ev: &SemanticEventPayload,
        raw_payload: &str,
    ) -> Result<SemanticCommitOutcome, String> {
        let sid = session_id_override.unwrap_or(&ev.session_id);
        let fallback_mode = self.current_progression_mode();

        let (pack_id, mode, trust) = {
            let mut sessions = self.sessions.lock().unwrap();
            if let Some(s) = sessions.get_mut(sid) {
                s.last_sequence_number = s.last_sequence_number.max(ev.sequence_number as u64);
                let mode = ev
                    .progression_mode
                    .as_deref()
                    .and_then(ProgressionMode::from_str_strict)
                    .unwrap_or(s.progression_mode);
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
                    .unwrap_or(fallback_mode);
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
                            ..Default::default()
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
        if !outcome.deduplicated {
            self.plane_b_events_committed.fetch_add(1, Ordering::Relaxed);
        }

        let game_title = pack_clone.as_ref().map(|p| p.manifest.title.as_str());
        for unlocked in &outcome.newly_unlocked {
            self.notifications.notify_unlock(
                &ev.canonical_game_id,
                game_title,
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
        let eval_start = Instant::now();

        if lifecycle_signal == LifecycleSignal::SaveStateLoad {
            let _ = self.apply_fair_play_violation(session_id, FairPlayViolation::SaveStateLoad);
        }

        let max_gaps = self.config.plane_a_max_frame_gaps_before_downgrade;
        if max_gaps > 0 {
            let projected_gaps = {
                let evals = self.plane_a_evaluators.lock().unwrap();
                evals.get(session_id).map(|eval| {
                    let skipped = if eval.last_sequence_number > 0
                        && sequence_number > eval.last_sequence_number + 1
                    {
                        sequence_number - eval.last_sequence_number - 1
                    } else {
                        0
                    };
                    let explicit = if lifecycle_signal == LifecycleSignal::FrameGap {
                        (frame_gap_count.max(1)) as u64
                    } else {
                        0
                    };
                    let add = if skipped > 0 && explicit > 0 {
                        skipped.max(explicit)
                    } else {
                        skipped + explicit
                    };
                    eval.frame_gaps_recorded.saturating_add(add)
                })
            };
            if let Some(total) = projected_gaps {
                if total >= max_gaps {
                    let _ = self
                        .apply_fair_play_violation(session_id, FairPlayViolation::ExcessiveFrameGap);
                }
            }
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
            pack_id
                .as_deref()
                .and_then(|pid| packs.get(pid).cloned())
                .or_else(|| packs.find_by_game_id(&canonical_game_id).cloned())
        };
        let regions = self
            .plane_a_regions
            .lock()
            .unwrap()
            .get(session_id)
            .cloned()
            .unwrap_or_default();

        let (outcome, unlocked_rules_count) = {
            let mut evals = self.plane_a_evaluators.lock().unwrap();
            let eval = evals
                .entry(session_id.to_string())
                .or_insert_with(|| MemoryArenaEvaluatorState::new(&canonical_game_id, session_id));
            let mut db = self.db.lock().unwrap();
            let out = eval.step_frame(
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
            )?;
            let unlocked_cnt = eval.rules.values().filter(|r| r.triggered).count();
            (out, unlocked_cnt)
        };

        let eval_us = eval_start.elapsed().as_micros() as u64;
        let total_rules_count = pack_clone
            .as_ref()
            .map(|p| p.memory_rules.len())
            .unwrap_or(0);

        {
            let mut rings = self.plane_a_rings.lock().unwrap();
            let entry = rings
                .entry(session_id.to_string())
                .or_insert_with(|| PlaneARingTelemetry {
                    session_id: session_id.to_string(),
                    active: true,
                    ..Default::default()
                });
            entry.last_eval_latency_us = eval_us;
            entry.max_eval_latency_us = entry.max_eval_latency_us.max(eval_us);
            entry.unlocked_rules_count = unlocked_rules_count;
            entry.total_rules_count = total_rules_count;
        }

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
                VerificationState::Verified
            } else {
                VerificationState::Unverified
            };
            let game_title = pack_clone.as_ref().map(|p| p.manifest.title.as_str());
            self.notifications.notify_unlock(
                &canonical_game_id,
                game_title,
                &title,
                &desc,
                pts,
                mode,
                trust,
                ver,
            );
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
                let total_eval_gaps = {
                    let mut evals = self.plane_a_evaluators.lock().unwrap();
                    if let Some(eval) = evals.get_mut(session_id) {
                        eval.frame_gaps_recorded += gap;
                        eval.evaluation_incomplete = true;
                        eval.frame_gaps_recorded
                    } else {
                        gap
                    }
                };
                if let Ok(mut rings) = self.plane_a_rings.lock() {
                    let entry = rings
                        .entry(session_id.to_string())
                        .or_insert_with(|| PlaneARingTelemetry {
                            session_id: session_id.to_string(),
                            ..Default::default()
                        });
                    entry.ipc_frame_gaps_reported += gap;
                    entry.producer_frame_gaps_reported =
                        entry.shm_frame_gaps_reported.saturating_add(entry.ipc_frame_gaps_reported);
                }
                if self.config.plane_a_max_frame_gaps_before_downgrade > 0
                    && total_eval_gaps >= self.config.plane_a_max_frame_gaps_before_downgrade
                {
                    let _ = self
                        .apply_fair_play_violation(session_id, FairPlayViolation::ExcessiveFrameGap);
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
        let mut records = parse_observation_json_str(json_str)?;
        let active_mode = self.current_progression_mode();
        if active_mode != ProgressionMode::Standard {
            for r in &mut records {
                r.progression_mode = active_mode;
            }
        }
        let mut db = self.db.lock().unwrap();
        import_observation_records(&mut db, &records)
    }

    pub fn import_plane_c_path(
        &self,
        path: &Path,
        canonical_game_id: Option<String>,
        mode_override: Option<ProgressionMode>,
        dry_run: bool,
    ) -> Result<ObservationImportSummary, String> {
        let active_mode = self.current_progression_mode();
        let effective_mode = mode_override.or(if active_mode != ProgressionMode::Standard {
            Some(active_mode)
        } else {
            None
        });
        let opts = ImportOptions {
            canonical_game_id,
            override_progression_mode: effective_mode,
            dry_run,
        };
        let packs_guard = self.packs.lock().unwrap();
        let summary = {
            let mut db = self.db.lock().unwrap();
            import_observation_path_with_options(&mut db, path, Some(&packs_guard), &opts)?
        };
        let pack_meta: HashMap<String, (String, HashMap<String, (String, String)>)> = summary
            .records
            .iter()
            .filter_map(|rec| {
                packs_guard.find_by_game_id(&rec.canonical_game_id).map(|p| {
                    let ach_map: HashMap<String, (String, String)> = p
                        .manifest
                        .achievements
                        .iter()
                        .map(|a| {
                            (
                                a.canonical_achievement_id.clone(),
                                (a.title.clone(), a.description.clone()),
                            )
                        })
                        .collect();
                    (
                        rec.canonical_game_id.clone(),
                        (p.manifest.title.clone(), ach_map),
                    )
                })
            })
            .collect();
        drop(packs_guard);

        if !summary.dry_run && summary.imported > 0 {
            let imported_ids: std::collections::HashSet<&str> =
                summary.observation_ids.iter().map(String::as_str).collect();
            for rec in &summary.records {
                if imported_ids.contains(rec.observation_id.as_str())
                    && rec.record_kind == "ACHIEVEMENT_EVIDENCE"
                    && rec.unlocked == Some(true)
                {
                    let (game_title, ach_title, ach_desc) = pack_meta
                        .get(&rec.canonical_game_id)
                        .map(|(g_title, ach_map)| {
                            if let Some((t, d)) = ach_map.get(&rec.key) {
                                (Some(g_title.as_str()), t.as_str(), d.as_str())
                            } else {
                                (
                                    Some(g_title.as_str()),
                                    rec.key.as_str(),
                                    "Achievement unlocked",
                                )
                            }
                        })
                        .unwrap_or((None, rec.key.as_str(), "Achievement unlocked"));

                    self.notifications.notify_unlock(
                        &rec.canonical_game_id,
                        game_title,
                        ach_title,
                        ach_desc,
                        0,
                        rec.progression_mode,
                        rec.trust_tier,
                        rec.verification_state,
                    );
                }
            }
        }
        Ok(summary)
    }

    pub fn list_watch_overrides(&self) -> Vec<WatchedObservationDir> {
        self.watched_dirs.lock().unwrap().clone()
    }

    pub fn add_watch_override(
        &self,
        canonical_game_id: Option<String>,
        path: &Path,
    ) -> Result<WatchedObservationDir, String> {
        let raw_str = path.to_string_lossy().trim().to_string();
        if raw_str.is_empty() {
            return Err("watch path must not be empty".to_string());
        }
        let meta = std::fs::metadata(path)
            .map_err(|e| format!("watch path '{}' is not accessible: {}", path.display(), e))?;
        if !meta.is_dir() && !meta.is_file() {
            return Err(format!("watch path '{}' is neither directory nor file", path.display()));
        }
        let canon = std::fs::canonicalize(path)
            .map_err(|e| format!("failed canonicalizing watch path '{}': {}", path.display(), e))?;

        let clean_gid = canonical_game_id
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if let Some(gid) = clean_gid.as_deref() {
            if !crate::core::types::is_valid_canonical_id(gid) {
                return Err(format!("invalid canonical_game_id '{}'", gid));
            }
        }

        let entry = WatchedObservationDir {
            path: canon.clone(),
            canonical_game_id: clean_gid,
        };

        let snapshot = {
            let mut dirs = self.watched_dirs.lock().unwrap();
            if let Some(existing) = dirs.iter_mut().find(|d| d.path == canon) {
                existing.canonical_game_id = entry.canonical_game_id.clone();
            } else {
                dirs.push(entry.clone());
            }
            dirs.clone()
        };

        let mut persist_cfg = self.config.clone();
        persist_cfg.watched_observation_dirs = snapshot;
        save_config(&self.config.config_dir, &persist_cfg)?;
        Ok(entry)
    }

    pub fn remove_watch_override(&self, path: &Path) -> Result<bool, String> {
        let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let (removed, snapshot) = {
            let mut dirs = self.watched_dirs.lock().unwrap();
            let before = dirs.len();
            dirs.retain(|d| d.path != path && d.path != canon);
            (dirs.len() != before, dirs.clone())
        };
        if removed {
            let mut persist_cfg = self.config.clone();
            persist_cfg.watched_observation_dirs = snapshot;
            save_config(&self.config.config_dir, &persist_cfg)?;
        }
        Ok(removed)
    }

    pub fn poll_watched_observation_dirs(&self) -> WatchPollSummary {
        self.poll_watched_observation_dirs_with_debounce(0)
    }

    pub fn poll_watched_observation_dirs_with_debounce(&self, debounce_ms: u64) -> WatchPollSummary {
        let dirs = self.list_watch_overrides();
        let extra_globs = {
            let packs = self.packs.lock().unwrap();
            packs.all_watch_file_globs()
        };
        let (scanned, changed) = {
            let mut watcher = self.watcher_state.lock().unwrap();
            watcher.collect_changed_targets_with_globs(&dirs, &extra_globs, debounce_ms)
        };
        let mut out = WatchPollSummary {
            files_scanned: scanned,
            files_changed: changed.len(),
            ..Default::default()
        };
        for (file_path, game_id) in changed {
            match self.import_plane_c_path(&file_path, game_id, None, false) {
                Ok(summary) => {
                    out.imported += summary.imported;
                    out.deduplicated += summary.deduplicated;
                }
                Err(err) => {
                    out.errors.push(format!("{}: {}", file_path.display(), err));
                }
            }
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    pub fn report_launcher_game(
        &self,
        launcher: &str,
        canonical_game_id: Option<String>,
        app_id: Option<String>,
        executable: Option<String>,
        install_dir: Option<String>,
        save_dir: Option<String>,
        os: Option<String>,
        active: bool,
    ) -> Result<LauncherGameTelemetry, String> {
        let launcher_clean = launcher.trim().to_string();
        if launcher_clean.is_empty() {
            return Err("launcher must not be empty".to_string());
        }
        let query = MatchQuery {
            canonical_game_id: canonical_game_id.clone(),
            app_id: app_id.clone(),
            executable: executable.clone(),
            save_path: save_dir.clone(),
            os,
            launcher: Some(launcher_clean.clone()),
            ..Default::default()
        };

        let (pack_id, matched_game_id, game_title, total_achievements) = {
            let packs = self.packs.lock().unwrap();
            if let Some((pack, matched)) = packs.match_pack(&query) {
                (
                    Some(matched.pack_id),
                    matched.canonical_game_id,
                    Some(pack.manifest.title.clone()),
                    pack.manifest.achievements.len(),
                )
            } else {
                let fallback_gid = canonical_game_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .or_else(|| {
                        app_id
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(|id| {
                                format!(
                                    "{}.{}",
                                    launcher_clean.to_ascii_lowercase(),
                                    id.to_ascii_lowercase()
                                )
                            })
                    })
                    .ok_or_else(|| {
                        "could not resolve canonical_game_id (provide canonical_game_id or matching app_id/executable/save_dir)".to_string()
                    })?;
                (None, fallback_gid, None, 0usize)
            }
        };

        if let Some(ref sdir) = save_dir {
            let p = Path::new(sdir);
            if p.exists() && p.is_dir() {
                let _ = self.add_watch_override(Some(matched_game_id.clone()), p);
            }
        }

        let unlocked_count = {
            let db = self.db.lock().unwrap();
            let rows = list_trophies(&db, Some(&matched_game_id)).unwrap_or_default();
            let mut seen = std::collections::HashSet::new();
            for r in rows {
                if r.progression_mode != "SPECTATOR" {
                    seen.insert(r.canonical_achievement_id);
                }
            }
            seen.len()
        };
        let completion_percent = if total_achievements > 0 {
            ((unlocked_count.min(total_achievements) as u64) * 100) / (total_achievements as u64)
        } else {
            0
        };

        let telemetry = LauncherGameTelemetry {
            launcher: launcher_clean.clone(),
            canonical_game_id: matched_game_id.clone(),
            pack_id,
            game_title,
            app_id,
            executable,
            install_dir,
            save_dir,
            active,
            last_seen_at: now_rfc3339(),
            unlocked_count,
            total_achievements,
            completion_percent,
        };

        let key = format!("{}::{}", launcher_clean.to_ascii_lowercase(), matched_game_id);
        self.launcher_games
            .lock()
            .unwrap()
            .insert(key, telemetry.clone());
        Ok(telemetry)
    }

    pub fn query_launcher_status(
        &self,
        canonical_game_id: Option<&str>,
        app_id: Option<&str>,
        executable: Option<&str>,
    ) -> Vec<LauncherGameTelemetry> {
        let mut items: Vec<LauncherGameTelemetry> = self
            .launcher_games
            .lock()
            .unwrap()
            .values()
            .filter(|g| {
                if let Some(gid) = canonical_game_id {
                    if !g.canonical_game_id.eq_ignore_ascii_case(gid) {
                        return false;
                    }
                }
                if let Some(aid) = app_id {
                    if g.app_id.as_deref() != Some(aid) {
                        return false;
                    }
                }
                if let Some(exe) = executable {
                    if g.executable.as_deref() != Some(exe) {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();

        // Refresh live completion metrics from SQLite
        let db = self.db.lock().unwrap();
        for item in &mut items {
            let rows = list_trophies(&db, Some(&item.canonical_game_id)).unwrap_or_default();
            let mut seen = std::collections::HashSet::new();
            for r in rows {
                if r.progression_mode != "SPECTATOR" {
                    seen.insert(r.canonical_achievement_id);
                }
            }
            item.unlocked_count = seen.len();
            if item.total_achievements > 0 {
                item.completion_percent = ((item.unlocked_count.min(item.total_achievements) as u64)
                    * 100)
                    / (item.total_achievements as u64);
            }
        }
        items.sort_by(|a, b| a.canonical_game_id.cmp(&b.canonical_game_id));
        items
    }

    pub fn poll_retroarch_once(&self) -> Result<(), String> {
        let Some(addr) = self.config.retroarch_udp_addr.clone() else {
            return Ok(());
        };
        let client = RetroArchClient::new(&addr);
        {
            let mut tel = self.retroarch_telemetry.lock().unwrap();
            tel.configured_addr = Some(addr.clone());
            tel.polls += 1;
        }

        let status = match client.get_status() {
            Ok(s) => s,
            Err(e) => {
                let mut tel = self.retroarch_telemetry.lock().unwrap();
                tel.connected = false;
                tel.last_error = Some(e);
                return Ok(());
            }
        };

        let existing_version = self.retroarch_telemetry.lock().unwrap().version.clone();
        let ver = existing_version.or_else(|| client.version().ok());

        {
            let mut tel = self.retroarch_telemetry.lock().unwrap();
            tel.connected = true;
            tel.version = ver;
            tel.active_system = status.system.clone();
            tel.active_content = status.content.clone();
            tel.content_crc = status.content_crc.clone();
            tel.last_error = None;
        }

        if status.state != "PLAYING" && status.state != "PAUSED" {
            return Ok(());
        }

        // Resolve matching Plane A pack by content CRC or RetroArch metadata
        let matched_pack = {
            let packs = self.packs.lock().unwrap();
            let by_query = status.content_crc.as_ref().and_then(|crc| {
                packs
                    .match_pack(&MatchQuery {
                        rom_hash: Some(crc.clone()),
                        launcher: Some("retroarch".to_string()),
                        ..Default::default()
                    })
                    .or_else(|| {
                        packs.match_pack(&MatchQuery {
                            rom_hash: Some(crc.clone()),
                            ..Default::default()
                        })
                    })
                    .map(|(p, _)| p.clone())
            });
            by_query.or_else(|| {
                packs
                    .list()
                    .into_iter()
                    .find(|p| {
                        p.manifest.profiles.memory.as_ref().map(|m| {
                            if !m.enabled {
                                return false;
                            }
                            let Some(ra) = &m.retroarch else {
                                return false;
                            };
                            let crc_ok = match (&ra.content_crc, &status.content_crc) {
                                (Some(rc), Some(sc)) => rc.eq_ignore_ascii_case(sc),
                                (None, _) => true,
                                _ => false,
                            };
                            let core_ok = match (&ra.core_name, &status.system) {
                                (Some(rn), Some(ss)) => ss
                                    .to_ascii_lowercase()
                                    .contains(&rn.to_ascii_lowercase()),
                                (None, _) => true,
                                _ => false,
                            };
                            crc_ok && core_ok
                        }).unwrap_or(false)
                    })
                    .cloned()
            })
        };

        let Some(pack) = matched_pack else {
            return Ok(());
        };
        let Some(mem_profile) = pack.manifest.profiles.memory.as_ref() else {
            return Ok(());
        };
        if !mem_profile.enabled || mem_profile.regions.is_empty() {
            return Ok(());
        }

        let session_key = format!(
            "{}::{}",
            pack.manifest.pack_id,
            status.content_crc.as_deref().unwrap_or("active")
        );
        let (sid, seq) = {
            let mut map = self.retroarch_sessions.lock().unwrap();
            if let Some((sid, seq)) = map.get_mut(&session_key) {
                *seq += 1;
                (sid.clone(), *seq)
            } else {
                let sid = generate_uuid_v4();
                drop(map);
                let mode = self.current_progression_mode();
                let (info, _) = self.start_session(
                    sid.clone(),
                    status
                        .system
                        .clone()
                        .unwrap_or_else(|| "retroarch_udp".to_string()),
                    pack.manifest.canonical_game_id.clone(),
                    MatchQuery {
                        canonical_game_id: Some(pack.manifest.canonical_game_id.clone()),
                        rom_hash: status.content_crc.clone(),
                        launcher: Some("retroarch".to_string()),
                        ..Default::default()
                    },
                    IngestionPlane::MemoryArena,
                    mode,
                    TrustTier::Local,
                );
                let mut map2 = self.retroarch_sessions.lock().unwrap();
                map2.insert(session_key, (info.session_id.clone(), 1));
                (info.session_id, 1)
            }
        };

        let mut arena_buf = Vec::new();
        for (idx, region) in mem_profile.regions.iter().enumerate() {
            let base = parse_hex_or_dec_u64(&region.guest_base_address)?;
            let sz = (region.size_bytes as usize).min(1024 * 1024);
            let chunk = match client.read_core_memory(base, sz) {
                Ok(bytes) => bytes,
                Err(e) => {
                    self.retroarch_telemetry.lock().unwrap().last_error = Some(e);
                    return Ok(());
                }
            };
            let start_off = arena_buf.len();
            arena_buf.extend_from_slice(&chunk);
            self.register_plane_a_region(&sid, idx as u32, start_off);
        }

        self.retroarch_telemetry.lock().unwrap().session_id = Some(sid.clone());
        let _ = self.handle_plane_a_frame(&sid, seq, LifecycleSignal::NormalFrame, 0, &arena_buf)?;
        Ok(())
    }

    /// Dispatch a preview desktop unlock notification without mutating SQLite or queueing sync events.
    pub fn trigger_test_notification(
        &self,
        canonical_game_id: Option<&str>,
        game_title: Option<&str>,
        achievement_title: Option<&str>,
        achievement_body: Option<&str>,
    ) -> NotificationDispatchReport {
        let requested_game_id = canonical_game_id
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let (resolved_game_id, pack_title, first_ach) = {
            let packs = self.packs.lock().unwrap();
            let matched = if let Some(gid) = requested_game_id {
                packs.find_by_game_id(gid)
            } else {
                packs
                    .get("nintendo.oot.longshot.pc")
                    .or_else(|| packs.list().into_iter().next())
            };
            if let Some(p) = matched {
                let first = p
                    .manifest
                    .achievements
                    .first()
                    .map(|a| (a.title.clone(), a.description.clone()));
                (
                    p.manifest.canonical_game_id.clone(),
                    Some(p.manifest.title.clone()),
                    first,
                )
            } else {
                (
                    requested_game_id
                        .unwrap_or("nintendo.oot.longshot.pc")
                        .to_string(),
                    None,
                    None,
                )
            }
        };

        let resolved_game_title = game_title
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or(pack_title)
            .unwrap_or_else(|| "The Legend of Zelda: Ocarina of Time".to_string());

        let (default_ach_title, default_ach_body) = first_ach.unwrap_or_else(|| {
            (
                "It works, it works!".to_string(),
                "Desktop unlock notification test (no progress modified).".to_string(),
            )
        });

        let resolved_title = achievement_title
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or(default_ach_title);

        let resolved_body = achievement_body
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or(default_ach_body);

        self.notifications.notify_test_sync(
            &resolved_game_id,
            Some(&resolved_game_title),
            &resolved_title,
            &resolved_body,
            self.current_progression_mode(),
            TrustTier::Local,
            VerificationState::Unverified,
        )
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
        let (stats, recent_observations) = {
            let db = self.db.lock().unwrap();
            (
                query_journal_stats(&db)?,
                list_observation_history(&db, 20)?,
            )
        };
        let (loaded_packs_count, pack_load_errors_count, installed_packs) = {
            let packs = self.packs.lock().unwrap();
            (packs.list().len(), packs.load_errors().len(), packs.summaries())
        };
        let sessions = self.sessions.lock().unwrap().all_sessions();
        let active_progression_mode = self.current_progression_mode();
        let sync = self.sync_telemetry.lock().unwrap().clone();
        let recent_notifications = self.notifications.recent();

        let mut rings: Vec<PlaneARingTelemetry> =
            self.plane_a_rings.lock().unwrap().values().cloned().collect();
        rings.sort_by(|a, b| a.session_id.cmp(&b.session_id));

        let active_rings_count = rings.iter().filter(|r| r.active).count();
        let total_snapshots_consumed = rings.iter().map(|r| r.snapshots_consumed).sum();
        let total_sequence_gaps = rings.iter().map(|r| r.sequence_gaps_detected).sum();
        let total_crc_errors = rings.iter().map(|r| r.crc_errors).sum();
        let last_eval_latency_us = rings.iter().map(|r| r.last_eval_latency_us).max().unwrap_or(0);
        let max_eval_latency_us = rings.iter().map(|r| r.max_eval_latency_us).max().unwrap_or(0);

        let plane_a = PlaneATelemetry {
            active_rings_count,
            total_snapshots_consumed,
            total_sequence_gaps,
            total_crc_errors,
            last_eval_latency_us,
            max_eval_latency_us,
            rings,
        };

        Ok(GameHealthSnapshot {
            daemon_version: crate::core::types::PROTOCOL_VERSION.to_string(),
            uptime_seconds: self.started_at.elapsed().as_secs(),
            active_progression_mode,
            loaded_packs_count,
            pack_load_errors_count,
            total_journal_events: stats.total_events,
            plane_b_events_committed: self.plane_b_events_committed.load(Ordering::Relaxed),
            sync_queue_depth: stats.pending_sync_count,
            synced_events_count: stats.synced_count,
            rejected_events_count: stats.rejected_count,
            total_frame_gaps: stats.total_frame_gaps,
            observations_imported: stats.observation_count,
            last_receipt_status: stats.last_receipt_status,
            last_acceptance_status: stats.last_acceptance_status,
            sync,
            plane_a,
            sessions,
            installed_packs,
            recent_observations,
            watched_dirs: self.list_watch_overrides(),
            launcher_games: {
                let mut v: Vec<LauncherGameTelemetry> =
                    self.launcher_games.lock().unwrap().values().cloned().collect();
                v.sort_by(|a, b| a.canonical_game_id.cmp(&b.canonical_game_id));
                v
            },
            retroarch: self.retroarch_telemetry.lock().unwrap().clone(),
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

fn parse_hex_or_dec_u64(raw: &str) -> Result<u64, String> {
    let t = raw.trim();
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16)
            .map_err(|e| format!("invalid hex address '{}': {}", raw, e))
    } else {
        t.parse::<u64>()
            .map_err(|e| format!("invalid decimal address '{}': {}", raw, e))
    }
}
