use crate::core::types::{
    now_rfc3339, EvidenceClass, LifecycleSignal, ProgressionMode, TrustTier, VerificationState,
};
use crate::packs::manifest::{LoadedPack, MemoryConditionSpec, MemoryRule};
use crate::storage::journal::{insert_journal_event_tx, record_frame_gap_db, JournalEventInput};
use crate::storage::state::{
    load_plane_a_progress_state, record_trophy_unlock_tx, save_plane_a_progress_state,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuleEvalProgress {
    /// Hit counters per condition index (`core:idx` or `altN:idx`)
    pub hit_counts: HashMap<String, u32>,
    /// Previous frame sampled values per condition key (for delta comparisons)
    pub prev_values: HashMap<String, i64>,
    pub triggered: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryArenaEvaluatorState {
    pub canonical_game_id: String,
    pub session_id: String,
    pub last_sequence_number: u64,
    pub frames_evaluated: u64,
    pub frame_gaps_recorded: u64,
    pub evaluation_incomplete: bool,
    pub rules: HashMap<String, RuleEvalProgress>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameStepOutcome {
    pub sequence_number: u64,
    pub frame_gap_detected: bool,
    pub gap_count: u64,
    pub evaluation_incomplete: bool,
    pub rewound_on_save_load: bool,
    pub unlocked_achievements: Vec<String>,
}

fn parse_addr(s: &str) -> Option<usize> {
    let trimmed = s.trim();
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        usize::from_str_radix(hex, 16).ok()
    } else {
        trimmed.parse::<usize>().ok()
    }
}

fn read_memory_value(mem: &[u8], offset: usize, size: &str) -> Option<i64> {
    match size {
        "u8" => mem.get(offset).map(|&b| b as i64),
        "u16_le" => mem
            .get(offset..offset + 2)
            .map(|s| u16::from_le_bytes([s[0], s[1]]) as i64),
        "u16_be" => mem
            .get(offset..offset + 2)
            .map(|s| u16::from_be_bytes([s[0], s[1]]) as i64),
        "u32_le" => mem
            .get(offset..offset + 4)
            .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as i64),
        "u32_be" => mem
            .get(offset..offset + 4)
            .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]) as i64),
        "bit0" | "bit1" | "bit2" | "bit3" | "bit4" | "bit5" | "bit6" | "bit7" => {
            let bit: u8 = size[3..].parse().ok()?;
            mem.get(offset).map(|&b| ((b >> bit) & 1) as i64)
        }
        _ => mem.get(offset).map(|&b| b as i64),
    }
}

fn compare_val(lhs: i64, op: &str, rhs: i64) -> bool {
    match op {
        "==" => lhs == rhs,
        "!=" => lhs != rhs,
        "<" => lhs < rhs,
        "<=" => lhs <= rhs,
        ">" => lhs > rhs,
        ">=" => lhs >= rhs,
        _ => false,
    }
}

impl MemoryArenaEvaluatorState {
    pub fn new(canonical_game_id: &str, session_id: &str) -> Self {
        Self {
            canonical_game_id: canonical_game_id.to_string(),
            session_id: session_id.to_string(),
            last_sequence_number: 0,
            frames_evaluated: 0,
            frame_gaps_recorded: 0,
            evaluation_incomplete: false,
            rules: HashMap::new(),
        }
    }

    pub fn serialize_progress(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|e| e.to_string())
    }

    pub fn restore_progress(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| e.to_string())
    }

    pub fn reset_all_hit_counts(&mut self) {
        for progress in self.rules.values_mut() {
            progress.hit_counts.clear();
            progress.prev_values.clear();
        }
    }

    /// Equivalent to `rc_client_do_frame`: steps Plane A evaluation for one guest memory frame.
    /// Explicitly detects sequence gaps and `FRAME_GAP` signals — never silently drops frames.
    pub fn step_frame(
        &mut self,
        conn: &mut Connection,
        pack: Option<&LoadedPack>,
        producer_id: &str,
        sequence_number: u64,
        lifecycle_signal: LifecycleSignal,
        frame_gap_count: u32,
        guest_memory: &[u8],
        region_offsets: &HashMap<u32, usize>,
        progression_mode: ProgressionMode,
        trust_tier: TrustTier,
    ) -> Result<FrameStepOutcome, String> {
        let mut gap_detected = false;
        let mut recorded_gap = 0u64;
        let mut rewound = false;

        // 1. Sequence-gap detection
        if self.last_sequence_number > 0 && sequence_number > self.last_sequence_number + 1 {
            let skipped = sequence_number - self.last_sequence_number - 1;
            gap_detected = true;
            recorded_gap += skipped;
        }
        if lifecycle_signal == LifecycleSignal::FrameGap {
            gap_detected = true;
            recorded_gap += (frame_gap_count.max(1)) as u64;
        }

        if gap_detected {
            self.frame_gaps_recorded += recorded_gap;
            self.evaluation_incomplete = true;
            record_frame_gap_db(
                conn,
                &self.session_id,
                producer_id,
                &self.canonical_game_id,
                recorded_gap,
                sequence_number,
                if lifecycle_signal == LifecycleSignal::FrameGap {
                    "explicit_frame_gap_signal"
                } else {
                    "sequence_number_discontinuity"
                },
            )?;
        }

        self.last_sequence_number = sequence_number.max(self.last_sequence_number);

        // 2. Save-state load / reset handling
        match lifecycle_signal {
            LifecycleSignal::SaveStateLoad => {
                // Restore serialized progress if present, otherwise clear hit counts
                if let Some(saved_json) =
                    load_plane_a_progress_state(conn, &self.canonical_game_id, &self.session_id)?
                {
                    if let Ok(restored) = Self::restore_progress(&saved_json) {
                        self.rules = restored.rules;
                    } else {
                        self.reset_all_hit_counts();
                    }
                } else {
                    self.reset_all_hit_counts();
                }
                rewound = true;
            }
            LifecycleSignal::SoftReset | LifecycleSignal::HardReset => {
                self.reset_all_hit_counts();
                rewound = true;
            }
            LifecycleSignal::NormalFrame | LifecycleSignal::FrameGap => {}
        }

        self.frames_evaluated += 1;

        let mut unlocked_achievements = Vec::new();
        if let Some(p) = pack {
            if p.manifest
                .profiles
                .memory
                .as_ref()
                .map(|m| m.enabled)
                .unwrap_or(false)
            {
                for rule in &p.memory_rules {
                    let ach_def = p
                        .manifest
                        .achievements
                        .iter()
                        .find(|a| a.canonical_achievement_id == rule.achievement_id);
                    if let Some(def) = ach_def {
                        if !def.supported_modes.contains(&progression_mode) {
                            continue;
                        }
                    }
                    if self.eval_memory_rule(rule, guest_memory, region_offsets) {
                        unlocked_achievements.push(rule.achievement_id.clone());
                    }
                }
            }
        }

        // Persist unlocked achievements in SQLite (never mutate in SPECTATOR mode!)
        if !unlocked_achievements.is_empty() && progression_mode != ProgressionMode::Spectator {
            let now = now_rfc3339();
            let ver_state = match trust_tier {
                TrustTier::Local => VerificationState::Unverified,
                TrustTier::VerifiedIntegration => VerificationState::Verified,
                TrustTier::Competitive => VerificationState::Pending,
            };

            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(|e| e.to_string())?;
            for (idx, ach_id) in unlocked_achievements.iter().enumerate() {
                let seq_i64 = ((sequence_number as i64) * 1000) + (idx as i64) + 1;
                let idemp = format!("{}:{}:planeA:{}", producer_id, self.session_id, seq_i64);
                let j_ev = JournalEventInput {
                    producer_id: producer_id.to_string(),
                    session_id: self.session_id.clone(),
                    sequence_number: seq_i64,
                    idempotency_key: idemp,
                    canonical_game_id: self.canonical_game_id.clone(),
                    event_kind: crate::core::types::EventKind::UnlockRequest,
                    key: ach_id.clone(),
                    int_value: Some(1),
                    float_value: None,
                    string_value: None,
                    occurred_at: now.clone(),
                    progression_mode,
                    trust_tier,
                    evidence_class: EvidenceClass::RuntimeObserved,
                    verification_state: ver_state,
                    raw_payload: format!(
                        "{{\"plane\":\"A\",\"frame_seq\":{},\"achievement\":\"{}\"}}",
                        sequence_number, ach_id
                    ),
                };
                let _ = insert_journal_event_tx(&tx, &j_ev)?;
                let _ = record_trophy_unlock_tx(
                    &tx,
                    &self.canonical_game_id,
                    ach_id,
                    &self.session_id,
                    &now,
                    progression_mode,
                    trust_tier,
                    EvidenceClass::RuntimeObserved,
                    ver_state,
                )?;
            }
            tx.commit().map_err(|e| e.to_string())?;
        }

        // Periodically checkpoint Plane A evaluation progress every 60 frames
        if self.frames_evaluated % 60 == 0 {
            if let Ok(json) = self.serialize_progress() {
                let _ = save_plane_a_progress_state(
                    conn,
                    &self.canonical_game_id,
                    &self.session_id,
                    &json,
                );
            }
        }

        Ok(FrameStepOutcome {
            sequence_number,
            frame_gap_detected: gap_detected,
            gap_count: recorded_gap,
            evaluation_incomplete: self.evaluation_incomplete,
            rewound_on_save_load: rewound,
            unlocked_achievements,
        })
    }

    fn eval_memory_rule(
        &mut self,
        rule: &MemoryRule,
        mem: &[u8],
        region_offsets: &HashMap<u32, usize>,
    ) -> bool {
        let progress = self
            .rules
            .entry(rule.achievement_id.clone())
            .or_default();
        if progress.triggered {
            return false;
        }

        let core_ok = eval_condition_group("core", &rule.conditions, mem, region_offsets, progress);
        if core_ok {
            if rule.alt_groups.is_empty() {
                progress.triggered = true;
                return true;
            }
            for (gi, alt) in rule.alt_groups.iter().enumerate() {
                let prefix = format!("alt{}", gi);
                if eval_condition_group(&prefix, alt, mem, region_offsets, progress) {
                    progress.triggered = true;
                    return true;
                }
            }
        }
        false
    }
}

fn eval_condition_group(
    prefix: &str,
    conds: &[MemoryConditionSpec],
    mem: &[u8],
    region_offsets: &HashMap<u32, usize>,
    progress: &mut RuleEvalProgress,
) -> bool {
    if conds.is_empty() {
        return false;
    }

    let mut paused = false;
    let mut reset = false;
    let mut normal_results = Vec::new();

    for (idx, cond) in conds.iter().enumerate() {
        let key = format!("{}:{}", prefix, idx);
        let addr = match parse_addr(&cond.address) {
            Some(a) => a,
            None => return false,
        };
        let base = region_offsets.get(&cond.region_id).copied().unwrap_or(0);
        let offset = base.saturating_add(addr);
        let cur_val = match read_memory_value(mem, offset, &cond.size) {
            Some(v) => v,
            None => return false,
        };
        let prev_val = progress.prev_values.get(&key).copied().unwrap_or(cur_val);
        progress.prev_values.insert(key.clone(), cur_val);

        let rhs = if cond.compare_delta {
            prev_val
        } else {
            cond.value.unwrap_or(0)
        };
        let matched = compare_val(cur_val, &cond.op, rhs);

        match cond.kind.as_str() {
            "pause_if" => {
                if matched {
                    paused = true;
                }
            }
            "reset_if" => {
                if matched {
                    reset = true;
                }
            }
            _ => {
                normal_results.push((key, cond.required_hits, matched));
            }
        }
    }

    if paused {
        return false;
    }
    if reset {
        for (key, _, _) in &normal_results {
            progress.hit_counts.insert(key.clone(), 0);
        }
        return false;
    }

    let mut all_ok = !normal_results.is_empty();
    for (key, req_hits, matched) in normal_results {
        if req_hits > 0 {
            let hits = progress.hit_counts.entry(key).or_insert(0);
            if matched && *hits < req_hits {
                *hits += 1;
            }
            if *hits < req_hits {
                all_ok = false;
            }
        } else if !matched {
            all_ok = false;
        }
    }
    all_ok
}
