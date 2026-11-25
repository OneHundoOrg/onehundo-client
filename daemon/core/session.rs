use super::types::{
    is_valid_uuid, now_rfc3339, FairPlayViolation, IngestionPlane, ProgressionMode, TrustTier,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub producer_id: String,
    pub canonical_game_id: String,
    pub matched_pack_id: Option<String>,
    pub ingestion_plane: IngestionPlane,
    pub progression_mode: ProgressionMode,
    pub initial_trust_tier: TrustTier,
    pub active_trust_tier: TrustTier,
    pub downgrade_reason: Option<String>,
    pub last_sequence_number: u64,
    pub frame_snapshots_evaluated: u64,
    pub frame_gaps_recorded: u64,
    pub evaluation_incomplete: bool,
    pub started_at: String,
    pub active: bool,
}

impl SessionInfo {
    pub fn is_downgraded(&self) -> bool {
        self.active_trust_tier < self.initial_trust_tier
    }
}

#[derive(Debug, Default)]
pub struct SessionManager {
    sessions: HashMap<String, SessionInfo>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    pub fn start_session(
        &mut self,
        session_id: String,
        producer_id: String,
        canonical_game_id: String,
        matched_pack_id: Option<String>,
        ingestion_plane: IngestionPlane,
        progression_mode: ProgressionMode,
        trust_tier: TrustTier,
    ) -> SessionInfo {
        let sid = if is_valid_uuid(&session_id) {
            session_id
        } else {
            super::config::generate_uuid_v4()
        };
        let info = SessionInfo {
            session_id: sid.clone(),
            producer_id,
            canonical_game_id,
            matched_pack_id,
            ingestion_plane,
            progression_mode,
            initial_trust_tier: trust_tier,
            active_trust_tier: trust_tier,
            downgrade_reason: None,
            last_sequence_number: 0,
            frame_snapshots_evaluated: 0,
            frame_gaps_recorded: 0,
            evaluation_incomplete: false,
            started_at: now_rfc3339(),
            active: true,
        };
        self.sessions.insert(sid, info.clone());
        info
    }

    pub fn get(&self, session_id: &str) -> Option<&SessionInfo> {
        self.sessions.get(session_id)
    }

    pub fn get_mut(&mut self, session_id: &str) -> Option<&mut SessionInfo> {
        self.sessions.get_mut(session_id)
    }

    /// Dual-lane trust invariant:
    /// If a VERIFIED_INTEGRATION or COMPETITIVE session triggers disallowed fair-play actions
    /// (e.g. save-state load, cheat/memory-poke toggle, debug console override),
    /// automatically downgrade the session to LOCAL rather than deleting progress or crashing.
    pub fn trigger_fair_play_violation(
        &mut self,
        session_id: &str,
        violation: FairPlayViolation,
    ) -> Option<(TrustTier, TrustTier, String)> {
        let s = self.sessions.get_mut(session_id)?;
        let prev = s.active_trust_tier;
        if prev > TrustTier::Local {
            s.active_trust_tier = TrustTier::Local;
            let reason = violation.as_str().to_string();
            s.downgrade_reason = Some(reason.clone());
            return Some((prev, TrustTier::Local, reason));
        }
        None
    }

    pub fn record_frame_gap(&mut self, session_id: &str, gap_count: u64) {
        if let Some(s) = self.sessions.get_mut(session_id) {
            s.frame_gaps_recorded += gap_count.max(1);
            s.evaluation_incomplete = true;
        }
    }

    pub fn end_session(&mut self, session_id: &str) {
        if let Some(s) = self.sessions.get_mut(session_id) {
            s.active = false;
        }
    }

    pub fn all_sessions(&self) -> Vec<SessionInfo> {
        self.sessions.values().cloned().collect()
    }
}
