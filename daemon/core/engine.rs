use crate::core::config::{
    generate_uuid_v4, load_credentials, load_or_init_config, save_config, save_credentials,
    Credentials, DaemonConfig, WatchedObservationDir,
};
use crate::core::process_monitor::{
    discover_process_observation_dirs_with_globs, format_edition_or_pack,
    match_running_processes, scan_running_processes, DetectedProcessMatch, ProcessInfo,
};
use crate::core::session::{SessionInfo, SessionManager};
use crate::core::types::{
    now_rfc3339, FairPlayViolation, IngestionPlane, LifecycleSignal, ProgressionMode, TrustTier,
    VerificationState,
};
use crate::diagnostics::{
    ActiveTrackingTarget, GameHealthSnapshot, LauncherGameTelemetry, LibraryGameProgress,
    PlaneARingTelemetry,
    PlaneATelemetry, RecentTrophyItem, RetroArchTelemetry, SyncTelemetry,
};
use crate::evaluation::memory_arena::{FrameStepOutcome, MemoryArenaEvaluatorState};
use crate::evaluation::semantic_events::{
    evaluate_and_commit_plane_b, SemanticCommitOutcome, SemanticEventPayload,
};
use crate::ipc::retroarch::RetroArchClient;
use crate::ipc::shm_arena::ShmRingStats;
use crate::observation::external_importers::watcher::{DirectoryWatcherState, WatchPollSummary};
use crate::observation::external_importers::{
    import_observation_path_with_options, import_observation_records, list_observation_history,
    parse_observation_json_str, ImportOptions, ObservationImportSummary,
};
use crate::packs::matcher::{glob_matches, MatchQuery, PackMatchResult, PackRegistry};
use crate::storage::journal::{open_and_init_db, query_journal_stats, record_frame_gap_db};
use crate::storage::state::{list_encore_runs, list_stats, list_trophies};
use crate::sync::client::{register_device, sync_pending_batch, SyncBatchSummary};
use crate::ui::notifications::{
    NotificationDispatchReport, NotificationManager, SessionNotificationTracker,
};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub struct DaemonEngine {
    pub config: DaemonConfig,
    pub credentials: Mutex<Credentials>,
    pub account_username: Mutex<Option<String>>,
    pub db: Mutex<Connection>,
    pub packs: Mutex<PackRegistry>,
    pub sessions: Mutex<SessionManager>,
    pub active_progression_mode: Mutex<ProgressionMode>,
    pub tracking_paused: AtomicBool,
    pub active_game_notifications: Mutex<SessionNotificationTracker>,
    pub detected_processes: Mutex<Vec<DetectedProcessMatch>>,
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
    pub last_process_scan_at: Mutex<Option<String>>,
    pub last_watch_scan_at: Mutex<Option<String>>,
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
        if config.account_username.is_none() {
            config.account_username = loaded_cfg.account_username;
        }
        if !config.tracking_paused {
            config.tracking_paused = loaded_cfg.tracking_paused;
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
            if !dir.as_os_str().is_empty() {
                let _ = packs.discover_in_dir(dir);
            }
        }

        let sync_tel = SyncTelemetry {
            platform_configured: config.platform_url.is_some(),
            device_registered: creds.device_token.is_some(),
            ..Default::default()
        };
        let initial_watched = config.watched_observation_dirs.clone();
        let initial_username = config.account_username.clone();
        let initial_paused = config.tracking_paused;
        let ra_tel = RetroArchTelemetry {
            configured_addr: config.retroarch_udp_addr.clone(),
            ..Default::default()
        };

        Ok(Arc::new(Self {
            notifications: NotificationManager::new(config.notifications_enabled),
            config,
            credentials: Mutex::new(creds),
            account_username: Mutex::new(initial_username),
            db: Mutex::new(conn),
            packs: Mutex::new(packs),
            sessions: Mutex::new(SessionManager::new()),
            active_progression_mode: Mutex::new(ProgressionMode::Standard),
            tracking_paused: AtomicBool::new(initial_paused),
            active_game_notifications: Mutex::new(SessionNotificationTracker::new()),
            detected_processes: Mutex::new(Vec::new()),
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
            last_process_scan_at: Mutex::new(None),
            last_watch_scan_at: Mutex::new(None),
            started_at: Instant::now(),
        }))
    }

    fn persist_current_config(&self) -> Result<(), String> {
        let mut persist_cfg = self.config.clone();
        persist_cfg.watched_observation_dirs = self.list_watch_overrides();
        persist_cfg.account_username = self.account_username.lock().unwrap().clone();
        persist_cfg.tracking_paused = self.is_tracking_paused();
        save_config(&self.config.config_dir, &persist_cfg)
    }

    pub fn is_tracking_paused(&self) -> bool {
        self.tracking_paused.load(Ordering::Relaxed)
    }

    pub fn set_tracking_paused(&self, paused: bool) -> bool {
        self.tracking_paused.store(paused, Ordering::SeqCst);
        if paused {
            if let Ok(mut procs) = self.detected_processes.lock() {
                procs.clear();
            }
            if let Ok(mut tracker) = self.active_game_notifications.lock() {
                tracker.sync_active_prefix("game:", &HashSet::new());
            }
        }
        let _ = self.persist_current_config();
        paused
    }

    pub fn account_status(&self) -> (bool, Option<String>, bool, Option<String>) {
        let signed_in = self.credentials.lock().unwrap().active_bearer().is_some();
        let username = self.account_username.lock().unwrap().clone();
        let device_registered = self.sync_telemetry.lock().unwrap().device_registered;
        let platform_url = self.config.platform_url.clone();
        (signed_in, username, device_registered, platform_url)
    }

    pub fn set_account_credentials(
        &self,
        api_key: Option<String>,
        username: Option<String>,
        sign_out: bool,
    ) -> Result<(bool, Option<String>, bool), String> {
        if sign_out {
            let empty = Credentials::default();
            save_credentials(&self.config.config_dir, &empty)?;
            *self.credentials.lock().unwrap() = empty;
            *self.account_username.lock().unwrap() = None;
            {
                let mut tel = self.sync_telemetry.lock().unwrap();
                tel.device_registered = false;
                tel.last_sync_error = None;
            }
            self.persist_current_config()?;
            return Ok((false, None, false));
        }

        let clean_key = api_key
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty());
        let clean_user = username
            .map(|u| u.trim().trim_start_matches('@').to_string())
            .filter(|u| !u.is_empty());

        if let Some(key) = clean_key {
            let mut creds = self.credentials.lock().unwrap();
            if creds.api_key.as_deref() != Some(key.as_str()) {
                creds.device_id = None;
                creds.device_token = None;
            }
            creds.api_key = Some(key);
            save_credentials(&self.config.config_dir, &creds)?;
        }
        if clean_user.is_some() {
            *self.account_username.lock().unwrap() = clean_user;
        }
        self.persist_current_config()?;

        let mut registered = self.sync_telemetry.lock().unwrap().device_registered;
        if self.config.platform_url.is_some()
            && self.credentials.lock().unwrap().active_bearer().is_some()
        {
            if let Ok(ok) = self.ensure_device_registered() {
                registered = ok;
            }
        }
        let signed_in = self.credentials.lock().unwrap().active_bearer().is_some();
        let current_user = self.account_username.lock().unwrap().clone();
        Ok((signed_in, current_user, registered))
    }

    fn maybe_notify_game_active(
        &self,
        canonical_game_id: &str,
        game_title: &str,
        edition_or_pack: &str,
    ) {
        if self.is_tracking_paused() {
            return;
        }
        let session_key = format!("game:{}", canonical_game_id);
        let pending = self
            .active_game_notifications
            .lock()
            .unwrap()
            .on_game_active(&session_key, canonical_game_id, game_title, edition_or_pack);
        if let Some(note) = pending {
            let mode = self.current_progression_mode();
            self.notifications.notify_game_active(
                &note.canonical_game_id,
                &note.game_title,
                &note.edition_or_pack,
                mode,
            );
        }
    }

    fn is_game_currently_tracked(&self, canonical_game_id: &str) -> bool {
        let session_active = self
            .sessions
            .lock()
            .unwrap()
            .all_sessions()
            .iter()
            .any(|s| s.active && s.canonical_game_id == canonical_game_id);
        if session_active {
            return true;
        }
        let launcher_active = self
            .launcher_games
            .lock()
            .unwrap()
            .values()
            .any(|g| g.active && g.canonical_game_id == canonical_game_id);
        if launcher_active {
            return true;
        }
        self.detected_processes
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.canonical_game_id == canonical_game_id)
    }

    fn clear_game_active_if_untracked(&self, canonical_game_id: &str) {
        if !self.is_game_currently_tracked(canonical_game_id) {
            let key = format!("game:{}", canonical_game_id);
            self.active_game_notifications
                .lock()
                .unwrap()
                .on_game_inactive(&key);
        }
    }

    pub fn evaluate_detected_processes(&self, procs: &[ProcessInfo]) -> Vec<DetectedProcessMatch> {
        if self.is_tracking_paused() {
            return Vec::new();
        }
        let detected = {
            let packs = self.packs.lock().unwrap();
            match_running_processes(procs, &packs)
        };
        let prev_games: HashSet<String> = self
            .detected_processes
            .lock()
            .unwrap()
            .iter()
            .map(|p| p.canonical_game_id.clone())
            .collect();
        let next_games: HashSet<String> = detected
            .iter()
            .map(|p| p.canonical_game_id.clone())
            .collect();

        *self.detected_processes.lock().unwrap() = detected.clone();
        *self.last_process_scan_at.lock().unwrap() = Some(now_rfc3339());

        for exited in prev_games.difference(&next_games) {
            self.clear_game_active_if_untracked(exited);
        }
        for m in &detected {
            self.maybe_notify_game_active(&m.canonical_game_id, &m.game_title, &m.edition_or_pack);
        }
        detected
    }

    pub fn poll_running_processes(&self) -> usize {
        if self.is_tracking_paused() {
            return 0;
        }
        let procs = scan_running_processes();
        self.evaluate_detected_processes(&procs).len()
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
        let (matched, game_title, edition_or_pack) = {
            let reg = self.packs.lock().unwrap();
            if let Some((pack, r)) = reg.match_pack(&query) {
                (
                    Some(r),
                    Some(pack.manifest.title.clone()),
                    Some(format_edition_or_pack(pack)),
                )
            } else {
                (None, None, None)
            }
        };
        let pack_id = matched.as_ref().map(|m| m.pack_id.clone());
        let resolved_game_id = matched
            .as_ref()
            .map(|m| m.canonical_game_id.clone())
            .unwrap_or(canonical_game_id);

        let info = self.sessions.lock().unwrap().start_session(
            session_id,
            producer_id,
            resolved_game_id.clone(),
            pack_id.clone(),
            plane,
            mode,
            trust,
        );

        if plane == IngestionPlane::MemoryArena {
            let mut evals = self.plane_a_evaluators.lock().unwrap();
            evals.insert(
                info.session_id.clone(),
                MemoryArenaEvaluatorState::new(&info.canonical_game_id, &info.session_id),
            );
        }

        if let (Some(title), Some(ed_pack)) = (game_title, edition_or_pack) {
            self.maybe_notify_game_active(&resolved_game_id, &title, &ed_pack);
        }

        (info, matched)
    }

    pub fn end_session(&self, session_id: &str) {
        let ended_gid = {
            let mut s = self.sessions.lock().unwrap();
            let gid = s.get(session_id).map(|info| info.canonical_game_id.clone());
            s.end_session(session_id);
            gid
        };
        if let Ok(mut rings) = self.plane_a_rings.lock() {
            if let Some(r) = rings.get_mut(session_id) {
                r.active = false;
            }
        }
        if let Some(gid) = ended_gid {
            self.clear_game_active_if_untracked(&gid);
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
        if self.is_tracking_paused() {
            return Ok(SemanticCommitOutcome {
                sequence_number: ev.sequence_number,
                deduplicated: false,
                newly_unlocked: Vec::new(),
                updated_stat_int: None,
            });
        }

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
        if self.is_tracking_paused() {
            return Ok(FrameStepOutcome {
                sequence_number,
                frame_gap_detected: false,
                gap_count: 0,
                evaluation_incomplete: false,
                rewound_on_save_load: false,
                unlocked_achievements: Vec::new(),
                emitted_stats: Vec::new(),
            });
        }

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
            let unlocked_imports: Vec<_> = summary
                .records
                .iter()
                .filter(|rec| {
                    imported_ids.contains(rec.observation_id.as_str())
                        && rec.record_kind == "ACHIEVEMENT_EVIDENCE"
                        && rec.unlocked == Some(true)
                })
                .collect();
            if unlocked_imports.len() <= 3 {
                for rec in unlocked_imports {
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

        {
            let mut dirs = self.watched_dirs.lock().unwrap();
            if let Some(existing) = dirs.iter_mut().find(|d| d.path == canon) {
                existing.canonical_game_id = entry.canonical_game_id.clone();
            } else {
                dirs.push(entry.clone());
            }
        }

        self.persist_current_config()?;
        Ok(entry)
    }

    pub fn remove_watch_override(&self, path: &Path) -> Result<bool, String> {
        let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let removed = {
            let mut dirs = self.watched_dirs.lock().unwrap();
            let before = dirs.len();
            dirs.retain(|d| d.path != path && d.path != canon);
            dirs.len() != before
        };
        if removed {
            self.persist_current_config()?;
        }
        Ok(removed)
    }

    pub fn poll_watched_observation_dirs(&self) -> WatchPollSummary {
        self.poll_watched_observation_dirs_with_debounce(0)
    }

    pub fn poll_watched_observation_dirs_with_debounce(&self, debounce_ms: u64) -> WatchPollSummary {
        if self.is_tracking_paused() {
            return WatchPollSummary::default();
        }
        let mut dirs = self.list_watch_overrides();
        let active_procs = self.detected_processes.lock().unwrap().clone();
        let (extra_globs, pack_watch_dirs, has_longshot_pack) = {
            let packs = self.packs.lock().unwrap();
            (
                packs.all_watch_file_globs(),
                resolve_pack_watch_dirs_with_home(&packs, self.config.watch_home_override.as_deref()),
                packs.get(LONGSHOT_PACK_GAME_ID).is_some(),
            )
        };
        for runtime_dir in discover_process_observation_dirs_with_globs(&active_procs, &extra_globs) {
            if !dirs.iter().any(|d| d.path == runtime_dir.path) {
                dirs.push(runtime_dir);
            }
        }
        for auto_dir in pack_watch_dirs {
            if !dirs.iter().any(|d| d.path == auto_dir.path) {
                dirs.push(auto_dir);
            }
        }
        let (scanned, changed) = {
            let mut watcher = self.watcher_state.lock().unwrap();
            watcher.collect_changed_targets_with_globs(&dirs, &extra_globs, debounce_ms)
        };
        *self.last_watch_scan_at.lock().unwrap() = Some(now_rfc3339());
        let mut out = WatchPollSummary {
            files_scanned: scanned,
            files_changed: changed.len(),
            ..Default::default()
        };
        // The same save can be reached through several overlapping watch roots (config overrides,
        // process-discovered save dirs, pack watch paths) that each carry a different owning
        // `canonical_game_id`. Import each file once, under a single deterministic owner, so one
        // unlock never lands under two packs (double notifications + mis-mapped achievement ids).
        let mut seen_files: HashSet<PathBuf> = HashSet::new();
        for (file_path, game_id) in changed {
            if !seen_files.insert(file_path.clone()) {
                continue;
            }
            let game_id = owning_game_id_for_save(&file_path, game_id, has_longshot_pack);
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

        let (pack_id, matched_game_id, game_title, edition_or_pack, total_achievements) = {
            let packs = self.packs.lock().unwrap();
            if let Some((pack, matched)) = packs.match_pack(&query) {
                (
                    Some(matched.pack_id),
                    matched.canonical_game_id,
                    Some(pack.manifest.title.clone()),
                    Some(format_edition_or_pack(pack)),
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
                (None, fallback_gid, None, None, 0usize)
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
            pack_id: pack_id.clone(),
            game_title: game_title.clone(),
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

        if active {
            let notify_title = game_title
                .as_deref()
                .unwrap_or(&matched_game_id);
            let notify_pack = edition_or_pack
                .as_deref()
                .or(pack_id.as_deref())
                .unwrap_or(&launcher_clean);
            self.maybe_notify_game_active(&matched_game_id, notify_title, notify_pack);
        } else {
            self.clear_game_active_if_untracked(&matched_game_id);
        }

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
        if self.is_tracking_paused() {
            return Ok(());
        }
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
                let ended: Vec<String> = self
                    .retroarch_sessions
                    .lock()
                    .unwrap()
                    .drain()
                    .map(|(_, (sid, _))| sid)
                    .collect();
                for sid in ended {
                    self.end_session(&sid);
                }
                let mut tel = self.retroarch_telemetry.lock().unwrap();
                tel.connected = false;
                tel.session_id = None;
                tel.active_system = None;
                tel.active_content = None;
                tel.content_crc = None;
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
            let ended: Vec<String> = self
                .retroarch_sessions
                .lock()
                .unwrap()
                .drain()
                .map(|(_, (sid, _))| sid)
                .collect();
            for sid in ended {
                self.end_session(&sid);
            }
            return Ok(());
        }

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
        if self.credentials.lock().unwrap().active_bearer().is_none() {
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
        let (stats, recent_observations, trophies) = {
            let db = self.db.lock().unwrap();
            (
                query_journal_stats(&db)?,
                list_observation_history(&db, 20)?,
                list_trophies(&db, None)?,
            )
        };

        let sessions = self.sessions.lock().unwrap().all_sessions();
        let active_procs = self.detected_processes.lock().unwrap().clone();
        let launcher_games: Vec<LauncherGameTelemetry> = {
            let mut v: Vec<LauncherGameTelemetry> =
                self.launcher_games.lock().unwrap().values().cloned().collect();
            v.sort_by(|a, b| a.canonical_game_id.cmp(&b.canonical_game_id));
            v
        };

        let mut active_game_ids: HashSet<String> =
            sessions.iter().filter(|s| s.active).map(|s| s.canonical_game_id.clone()).collect();
        for p in &active_procs {
            active_game_ids.insert(p.canonical_game_id.clone());
        }
        for l in &launcher_games {
            if l.active {
                active_game_ids.insert(l.canonical_game_id.clone());
            }
        }

        let mut unlocked_by_game: HashMap<String, HashSet<String>> = HashMap::new();
        for t in &trophies {
            if t.progression_mode != "SPECTATOR" {
                unlocked_by_game
                    .entry(t.canonical_game_id.clone())
                    .or_default()
                    .insert(t.canonical_achievement_id.clone());
            }
        }

        let (
            loaded_packs_count,
            pack_load_errors_count,
            installed_packs,
            library_games,
            recent_trophies,
        ) = {
            let packs = self.packs.lock().unwrap();
            let list = packs.list();
            let mut games_seen_for_progress: HashSet<String> = HashSet::new();
            let mut lib_games = Vec::with_capacity(list.len());
            for pack in &list {
                let gid = &pack.manifest.canonical_game_id;
                if !games_seen_for_progress.insert(gid.clone()) {
                    continue;
                }
                let total = pack.manifest.achievements.len();
                let unlocked = unlocked_by_game.get(gid).map(|s| s.len()).unwrap_or(0);
                let pct = if total > 0 {
                    ((unlocked.min(total) as u64) * 100) / (total as u64)
                } else {
                    0
                };
                lib_games.push(LibraryGameProgress {
                    canonical_game_id: gid.clone(),
                    pack_id: pack.manifest.pack_id.clone(),
                    title: pack.manifest.title.clone(),
                    mapped_oap_edition: pack.mapped_oap_edition.clone(),
                    unlocked_count: unlocked,
                    total_achievements: total,
                    completion_percent: pct,
                    active: !self.is_tracking_paused() && active_game_ids.contains(gid),
                    edition_or_pack: format_edition_or_pack(pack),
                });
            }
            lib_games.sort_by(|a, b| b.active.cmp(&a.active).then_with(|| a.title.cmp(&b.title)));

            let mut trophy_rows = trophies;
            trophy_rows.sort_by(|a, b| b.last_unlocked_at.cmp(&a.last_unlocked_at));
            let recent: Vec<RecentTrophyItem> = trophy_rows
                .into_iter()
                .take(10)
                .map(|tr| {
                    let pack_opt = packs.find_by_game_id(&tr.canonical_game_id);
                    let game_title = pack_opt
                        .map(|p| p.manifest.title.clone())
                        .unwrap_or_else(|| tr.canonical_game_id.clone());
                    let achievement_title = pack_opt
                        .and_then(|p| {
                            p.manifest
                                .achievements
                                .iter()
                                .find(|a| a.canonical_achievement_id == tr.canonical_achievement_id)
                                .map(|a| a.title.clone())
                        })
                        .unwrap_or_else(|| tr.canonical_achievement_id.clone());
                    RecentTrophyItem {
                        canonical_game_id: tr.canonical_game_id,
                        game_title,
                        canonical_achievement_id: tr.canonical_achievement_id,
                        achievement_title,
                        unlocked_at: if tr.last_unlocked_at.trim().is_empty() {
                            None
                        } else {
                            Some(tr.last_unlocked_at)
                        },
                        progression_mode: tr.progression_mode,
                    }
                })
                .collect();

            (
                list.len(),
                packs.load_errors().len(),
                packs.summaries(),
                lib_games,
                recent,
            )
        };

        let retroarch = self.retroarch_telemetry.lock().unwrap().clone();
        let paused = self.is_tracking_paused();
        let mut active_trackings: Vec<ActiveTrackingTarget> = Vec::new();
        if !paused {
            let mut seen_active: HashMap<String, usize> = HashMap::new();
            let lib_by_gid: HashMap<&str, &LibraryGameProgress> = library_games
                .iter()
                .map(|g| (g.canonical_game_id.as_str(), g))
                .collect();

            for sess in sessions.iter().filter(|s| s.active) {
                let is_retroarch = retroarch.session_id.as_deref() == Some(sess.session_id.as_str());
                let source = if is_retroarch {
                    "RetroArch UDP".to_string()
                } else {
                    match sess.ingestion_plane {
                        IngestionPlane::MemoryArena => "Live memory bridge".to_string(),
                        IngestionPlane::SemanticEvent => "Live game hook".to_string(),
                        IngestionPlane::ExternalObservation => "Observation import".to_string(),
                    }
                };
                let detail = if is_retroarch {
                    retroarch
                        .active_content
                        .clone()
                        .or_else(|| retroarch.active_system.clone())
                        .unwrap_or_else(|| "RetroArch core active".to_string())
                } else {
                    format!("{} session", sess.producer_id)
                };
                let lib = lib_by_gid.get(sess.canonical_game_id.as_str());
                let idx = active_trackings.len();
                seen_active.insert(sess.canonical_game_id.clone(), idx);
                active_trackings.push(ActiveTrackingTarget {
                    canonical_game_id: sess.canonical_game_id.clone(),
                    pack_id: sess
                        .matched_pack_id
                        .clone()
                        .or_else(|| lib.map(|g| g.pack_id.clone()))
                        .unwrap_or_else(|| sess.producer_id.clone()),
                    game_title: lib
                        .map(|g| g.title.clone())
                        .unwrap_or_else(|| sess.canonical_game_id.clone()),
                    edition_or_pack: lib
                        .map(|g| g.edition_or_pack.clone())
                        .unwrap_or_else(|| sess.producer_id.clone()),
                    source,
                    detail,
                    unlocked_count: lib.map(|g| g.unlocked_count).unwrap_or(0),
                    total_achievements: lib.map(|g| g.total_achievements).unwrap_or(0),
                    completion_percent: lib.map(|g| g.completion_percent).unwrap_or(0),
                });
            }

            for proc_match in &active_procs {
                let detail = format!("{} (PID {})", proc_match.executable, proc_match.pid);
                if let Some(&existing_idx) = seen_active.get(&proc_match.canonical_game_id) {
                    if !active_trackings[existing_idx].detail.contains(&proc_match.executable) {
                        active_trackings[existing_idx].detail = format!(
                            "{} · {}",
                            active_trackings[existing_idx].detail, detail
                        );
                    }
                    continue;
                }
                let lib = lib_by_gid.get(proc_match.canonical_game_id.as_str());
                let idx = active_trackings.len();
                seen_active.insert(proc_match.canonical_game_id.clone(), idx);
                active_trackings.push(ActiveTrackingTarget {
                    canonical_game_id: proc_match.canonical_game_id.clone(),
                    pack_id: proc_match.pack_id.clone(),
                    game_title: proc_match.game_title.clone(),
                    edition_or_pack: proc_match.edition_or_pack.clone(),
                    source: "Running process detected".to_string(),
                    detail,
                    unlocked_count: lib.map(|g| g.unlocked_count).unwrap_or(0),
                    total_achievements: lib.map(|g| g.total_achievements).unwrap_or(0),
                    completion_percent: lib.map(|g| g.completion_percent).unwrap_or(0),
                });
            }

            for lg in launcher_games.iter().filter(|g| g.active) {
                if seen_active.contains_key(&lg.canonical_game_id) {
                    continue;
                }
                let lib = lib_by_gid.get(lg.canonical_game_id.as_str());
                let detail = lg
                    .executable
                    .clone()
                    .or_else(|| lg.app_id.clone())
                    .unwrap_or_else(|| lg.launcher.clone());
                let idx = active_trackings.len();
                seen_active.insert(lg.canonical_game_id.clone(), idx);
                active_trackings.push(ActiveTrackingTarget {
                    canonical_game_id: lg.canonical_game_id.clone(),
                    pack_id: lg
                        .pack_id
                        .clone()
                        .unwrap_or_else(|| lg.launcher.clone()),
                    game_title: lg
                        .game_title
                        .clone()
                        .or_else(|| lib.map(|g| g.title.clone()))
                        .unwrap_or_else(|| lg.canonical_game_id.clone()),
                    edition_or_pack: lib
                        .map(|g| g.edition_or_pack.clone())
                        .or_else(|| lg.pack_id.clone())
                        .unwrap_or_else(|| lg.launcher.clone()),
                    source: format!("Launcher ({})", lg.launcher),
                    detail,
                    unlocked_count: lg.unlocked_count,
                    total_achievements: lg.total_achievements,
                    completion_percent: lg.completion_percent,
                });
            }
        }

        let active_progression_mode = self.current_progression_mode();
        let sync = self.sync_telemetry.lock().unwrap().clone();
        let recent_notifications = self.notifications.recent();
        let (account_signed_in, account_username, _, platform_url) = self.account_status();
        let last_process_scan_at = self.last_process_scan_at.lock().unwrap().clone();
        let last_watch_scan_at = self.last_watch_scan_at.lock().unwrap().clone();

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
            tracking_paused: self.is_tracking_paused(),
            account_signed_in,
            account_username,
            platform_url,
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
            library_games,
            active_trackings,
            last_process_scan_at,
            last_watch_scan_at,
            recent_trophies,
            recent_observations,
            watched_dirs: self.list_watch_overrides(),
            launcher_games,
            retroarch,
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

fn watch_os_matches(spec_os: &str, current_os: &str) -> bool {
    let spec = spec_os.trim().to_ascii_lowercase();
    if spec == "any" || spec == current_os {
        return true;
    }
    current_os == "linux" && spec == "steamos"
}

fn expand_watch_vars(raw: &str, home_override: Option<&Path>) -> Option<String> {
    let mut s = raw.trim().replace('\\', "/");
    if s.is_empty() {
        return None;
    }
    let home = home_override
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .map(|p| p.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_string());

    if s == "~" || s.starts_with("~/") {
        let h = home.as_ref()?;
        s = if s == "~" {
            h.clone()
        } else {
            format!("{}/{}", h, &s[2..])
        };
    }
    if s.contains("$HOME") || s.contains("${HOME}") {
        let h = home.as_ref()?;
        s = s.replace("${HOME}", h).replace("$HOME", h);
    }
    if s.contains("$XDG_CONFIG_HOME") || s.contains("${XDG_CONFIG_HOME}") {
        let cfg = std::env::var_os("XDG_CONFIG_HOME")
            .map(|v| PathBuf::from(v).to_string_lossy().replace('\\', "/"))
            .or_else(|| home.as_ref().map(|h| format!("{}/.config", h)))?;
        s = s
            .replace("${XDG_CONFIG_HOME}", &cfg)
            .replace("$XDG_CONFIG_HOME", &cfg);
    }
    if s.contains("$XDG_DATA_HOME") || s.contains("${XDG_DATA_HOME}") {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(|v| PathBuf::from(v).to_string_lossy().replace('\\', "/"))
            .or_else(|| home.as_ref().map(|h| format!("{}/.local/share", h)))?;
        s = s
            .replace("${XDG_DATA_HOME}", &data)
            .replace("$XDG_DATA_HOME", &data);
    }
    if s.contains("%USERPROFILE%") {
        let h = home.as_ref()?;
        s = s.replace("%USERPROFILE%", h);
    }
    if s.contains("%APPDATA%") {
        let val = std::env::var_os("APPDATA")
            .map(|v| PathBuf::from(v).to_string_lossy().replace('\\', "/"))?;
        s = s.replace("%APPDATA%", &val);
    }
    if s.contains("%LOCALAPPDATA%") {
        let val = std::env::var_os("LOCALAPPDATA")
            .map(|v| PathBuf::from(v).to_string_lossy().replace('\\', "/"))?;
        s = s.replace("%LOCALAPPDATA%", &val);
    }
    if s.contains('%') || s.contains('$') {
        return None;
    }
    Some(s)
}

fn split_glob_path(expanded: &str) -> Option<(PathBuf, Vec<String>)> {
    let normalized = expanded.trim().replace('\\', "/");
    if normalized.is_empty() {
        return None;
    }
    let bytes = normalized.as_bytes();
    let (root, rest) = if normalized.starts_with('/') {
        (PathBuf::from("/"), normalized.trim_start_matches('/'))
    } else if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/' {
        (PathBuf::from(&normalized[..3]), &normalized[3..])
    } else {
        return None;
    };

    let mut segs: Vec<String> = rest
        .split('/')
        .filter(|seg| !seg.is_empty() && *seg != ".")
        .map(str::to_string)
        .collect();

    while let Some(last) = segs.last() {
        if last == "**"
            || last == "*"
            || ((last.contains('*') || last.contains('?')) && last.contains('.'))
        {
            segs.pop();
        } else {
            break;
        }
    }

    if segs.is_empty() {
        return None;
    }
    Some((root, segs))
}

fn resolve_glob_segments(
    curr: &Path,
    segs: &[String],
    depth: usize,
    out: &mut Vec<PathBuf>,
) {
    const MAX_RESULTS: usize = 64;
    const MAX_WALK_DEPTH: usize = 6;
    if out.len() >= MAX_RESULTS || depth > 8 {
        return;
    }
    if segs.is_empty() {
        let target = if curr.is_dir() {
            Some(curr.to_path_buf())
        } else if curr.is_file() {
            curr.parent().map(Path::to_path_buf)
        } else {
            None
        };
        if let Some(dir) = target {
            if !out.contains(&dir) {
                out.push(dir);
            }
        }
        return;
    }

    let head = &segs[0];
    let tail = &segs[1..];
    if head == "**" {
        resolve_glob_segments(curr, tail, depth, out);
        if depth < MAX_WALK_DEPTH && curr.is_dir() {
            if let Ok(rd) = std::fs::read_dir(curr) {
                let mut subdirs: Vec<PathBuf> = rd
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect();
                subdirs.sort();
                for sub in subdirs {
                    if out.len() >= MAX_RESULTS {
                        break;
                    }
                    resolve_glob_segments(&sub, segs, depth + 1, out);
                }
            }
        }
    } else if head.contains('*') || head.contains('?') {
        if !curr.is_dir() {
            return;
        }
        if let Ok(rd) = std::fs::read_dir(curr) {
            let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            entries.sort();
            for entry in entries {
                if out.len() >= MAX_RESULTS {
                    break;
                }
                let Some(name) = entry.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                if !glob_matches(head, name) {
                    continue;
                }
                if tail.is_empty() {
                    let target = if entry.is_dir() {
                        Some(entry)
                    } else if entry.is_file() {
                        Some(curr.to_path_buf())
                    } else {
                        None
                    };
                    if let Some(dir) = target {
                        if !out.contains(&dir) {
                            out.push(dir);
                        }
                    }
                } else if entry.is_dir() {
                    resolve_glob_segments(&entry, tail, depth + 1, out);
                }
            }
        }
    } else {
        let next = curr.join(head);
        if next.exists() {
            resolve_glob_segments(&next, tail, depth + 1, out);
        }
    }
}

pub fn resolve_watch_path_glob_with_home(raw: &str, home_override: Option<&Path>) -> Vec<PathBuf> {
    let Some(expanded) = expand_watch_vars(raw, home_override) else {
        return Vec::new();
    };
    let Some((root, segs)) = split_glob_path(&expanded) else {
        return Vec::new();
    };

    let first_wild = segs
        .iter()
        .position(|s| s.contains('*') || s.contains('?'))
        .unwrap_or(segs.len());
    let mut prefix = root;
    for seg in &segs[..first_wild] {
        prefix.push(seg);
    }
    if !prefix.exists() {
        return Vec::new();
    }

    let mut out = Vec::new();
    let rest = &segs[first_wild..];
    resolve_glob_segments(&prefix, rest, 0, &mut out);
    out
}

fn is_shared_multi_game_dir(resolved_dir: &Path) -> bool {
    let norm = resolved_dir.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
    norm.ends_with("/retroarch/saves")
        || norm.ends_with("/mupen64plus/save")
        || norm.ends_with("/dolphin-emu/gc")
        || norm.ends_with("/dolphin emulator/gc")
        || (norm.ends_with("/trophy") && !norm.contains("/cusa") && !norm.contains("/npwr"))
}

/// Pack that owns the achievement-id space emitted by the native Longshot `progress.sav` importer.
const LONGSHOT_PACK_GAME_ID: &str = "nintendo.oot.longshot.pc";

fn is_longshot_save_name(path_or_glob: &str) -> bool {
    path_or_glob.to_ascii_lowercase().contains("progress.sav")
}

/// Longshot binary saves are always parsed by `longshot.rs`, whose `a01`..`a13` ids belong to the
/// `nintendo.oot.longshot.pc` pack. Other packs that merely match the Longshot executable (e.g. the
/// multi-fork `nintendo.oot.forks` pack) use a different id layout, so they must never own these files.
fn owning_game_id_for_save(
    file: &Path,
    watch_game_id: Option<String>,
    has_longshot_pack: bool,
) -> Option<String> {
    let is_progress_sav = file
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("progress.sav"));
    if has_longshot_pack && is_progress_sav {
        Some(LONGSHOT_PACK_GAME_ID.to_string())
    } else {
        watch_game_id
    }
}

pub fn resolve_pack_watch_dirs_with_home(
    packs: &PackRegistry,
    home_override: Option<&Path>,
) -> Vec<WatchedObservationDir> {
    let current_os = match std::env::consts::OS {
        "linux" => "linux",
        "windows" => "windows",
        "macos" => "macos",
        other => other,
    };
    let has_longshot_pack = packs.get(LONGSHOT_PACK_GAME_ID).is_some();
    let mut resolved_order: Vec<PathBuf> = Vec::new();
    let mut owners: HashMap<PathBuf, Option<String>> = HashMap::new();

    for pack in packs.list() {
        let Some(obs) = &pack.manifest.profiles.observation else {
            continue;
        };
        if !obs.enabled {
            continue;
        }
        for wp in &obs.watch_paths {
            if !watch_os_matches(&wp.os, current_os) {
                continue;
            }
            let effective_gid = if has_longshot_pack && is_longshot_save_name(&wp.path_glob) {
                LONGSHOT_PACK_GAME_ID
            } else {
                pack.manifest.canonical_game_id.as_str()
            };

            for dir in resolve_watch_path_glob_with_home(&wp.path_glob, home_override) {
                let canon = std::fs::canonicalize(&dir).unwrap_or(dir);
                let shared_gid = if is_shared_multi_game_dir(&canon) {
                    None
                } else {
                    Some(effective_gid.to_string())
                };
                if let Some(existing) = owners.get_mut(&canon) {
                    if *existing != shared_gid {
                        *existing = None;
                    }
                } else {
                    resolved_order.push(canon.clone());
                    owners.insert(canon, shared_gid);
                }
            }
        }
    }

    resolved_order
        .into_iter()
        .map(|path| WatchedObservationDir {
            canonical_game_id: owners.remove(&path).flatten(),
            path,
        })
        .collect()
}

pub fn resolve_pack_watch_dirs(packs: &PackRegistry) -> Vec<WatchedObservationDir> {
    resolve_pack_watch_dirs_with_home(packs, None)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packs::matcher::MatchQuery;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_engine_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "onehundo-eng-test-{}-{}-{}",
            label,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn longshot_progress_sav_is_always_owned_by_longshot_pack() {
        let sav = Path::new("/saves/save/uid/slot_01/progress.sav");
        // Process-discovered dir attributed to the multi-fork pack must not steal Longshot saves.
        assert_eq!(
            owning_game_id_for_save(sav, Some("nintendo.oot.switch2".into()), true).as_deref(),
            Some(LONGSHOT_PACK_GAME_ID)
        );
        assert_eq!(
            owning_game_id_for_save(sav, None, true).as_deref(),
            Some(LONGSHOT_PACK_GAME_ID)
        );
        // Without the Longshot pack installed, keep the watch root's owner.
        assert_eq!(
            owning_game_id_for_save(sav, Some("nintendo.oot.switch2".into()), false).as_deref(),
            Some("nintendo.oot.switch2")
        );
        // Non-Longshot saves keep their watch root's owner.
        assert_eq!(
            owning_game_id_for_save(
                Path::new("/soh/Save/file1.sav"),
                Some("nintendo.oot.switch2".into()),
                true
            )
            .as_deref(),
            Some("nintendo.oot.switch2")
        );
    }

    #[test]
    fn process_detection_notifies_once_per_session_and_rearms_after_exit() {
        let dir = test_engine_dir("proc_notify");
        let packs_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("packs");
        let mut cfg = DaemonConfig::default();
        cfg.config_dir = dir.join("cfg");
        cfg.data_dir = dir.join("data");
        cfg.packs_dirs = vec![packs_dir];
        cfg.notifications_enabled = false;

        let engine = DaemonEngine::initialize(cfg).expect("init");

        let running_bb = vec![ProcessInfo {
            pid: 55123,
            comm: "bbport".to_string(),
            exe_name: Some("bbport".to_string()),
            ..Default::default()
        }];

        // Tick 1: game starts -> single notification fired
        let matched1 = engine.evaluate_detected_processes(&running_bb);
        assert_eq!(matched1.len(), 1);
        assert_eq!(engine.notifications.recent().len(), 1);
        assert_eq!(engine.notifications.recent()[0].title, "OneHundo Active");
        assert!(
            engine.notifications.recent()[0].body.contains("Bloodborne"),
            "body={}",
            engine.notifications.recent()[0].body
        );

        // Tick 2: game still running -> deduplicated, no extra notification
        let matched2 = engine.evaluate_detected_processes(&running_bb);
        assert_eq!(matched2.len(), 1);
        assert_eq!(engine.notifications.recent().len(), 1);

        // Tick 3: game exits -> notification state re-arms
        let matched3 = engine.evaluate_detected_processes(&[]);
        assert!(matched3.is_empty());
        assert_eq!(engine.notifications.recent().len(), 1);

        // Tick 4: game relaunches -> fires again once
        let matched4 = engine.evaluate_detected_processes(&running_bb);
        assert_eq!(matched4.len(), 1);
        assert_eq!(engine.notifications.recent().len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pause_tracking_gates_unlocks_and_process_notifications() {
        let dir = test_engine_dir("pause_gate");
        let mut cfg = DaemonConfig::default();
        cfg.config_dir = dir.join("cfg");
        cfg.data_dir = dir.join("data");
        cfg.packs_dirs = vec![];
        cfg.notifications_enabled = false;

        let engine = DaemonEngine::initialize(cfg).expect("init");
        engine.set_tracking_paused(true);
        assert!(engine.is_tracking_paused());

        let ev = SemanticEventPayload {
            event_type: "UNLOCK_REQUEST".to_string(),
            canonical_game_id: "sony.bloodborne".to_string(),
            producer_id: "bbport".to_string(),
            session_id: "sess-pause".to_string(),
            sequence_number: 1,
            idempotency_key: None,
            timestamp_nanos: None,
            key: "ach_pause".to_string(),
            int_value: None,
            float_value: None,
            string_value: None,
            progression_mode: Some("STANDARD".to_string()),
            trust_tier: Some("LOCAL".to_string()),
            evidence_class: None,
            verification_state: None,
        };
        let out = engine
            .handle_plane_b_event(None, &ev, "{}")
            .expect("paused plane b returns ok");
        assert!(out.newly_unlocked.is_empty());
        assert_eq!(engine.health_snapshot().unwrap().total_journal_events, 0);

        engine.set_tracking_paused(false);
        let (_, _) = engine.start_session(
            "sess-pause".to_string(),
            "bbport".to_string(),
            "sony.bloodborne".to_string(),
            MatchQuery::default(),
            IngestionPlane::SemanticEvent,
            ProgressionMode::Standard,
            TrustTier::Local,
        );
        let out2 = engine
            .handle_plane_b_event(None, &ev, "{}")
            .expect("resumed plane b unlocks");
        assert_eq!(out2.newly_unlocked.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
