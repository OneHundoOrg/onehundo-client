use crate::core::types::{
    now_rfc3339, EventKind, EvidenceClass, LifecycleSignal, ProgressionMode, TrustTier,
    VerificationState,
};
use crate::packs::manifest::{LoadedPack, MemoryConditionSpec, MemoryRule};
use crate::storage::journal::{insert_journal_event_tx, record_frame_gap_db, JournalEventInput};
use crate::storage::state::{
    apply_stat_update_tx, load_plane_a_progress_state, record_trophy_unlock_tx,
    save_plane_a_progress_state,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuleEvalProgress {
    /// Hit counters per condition index (`core:idx` or `altN:idx`)
    pub hit_counts: HashMap<String, u32>,
    /// Previous frame sampled values per operand key (RetroAchievements Delta)
    pub prev_values: HashMap<String, i64>,
    /// Last different sampled values per operand key (RetroAchievements Prior)
    #[serde(default)]
    pub prior_values: HashMap<String, i64>,
    /// Last measured progress numerator/denominator when a Measured condition is present
    #[serde(default)]
    pub measured_current: Option<u32>,
    #[serde(default)]
    pub measured_target: Option<u32>,
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
    #[serde(default)]
    pub stat_last_values: HashMap<String, i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmittedPlaneAStat {
    pub stat_key: String,
    pub event_kind: EventKind,
    pub int_value: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameStepOutcome {
    pub sequence_number: u64,
    pub frame_gap_detected: bool,
    pub gap_count: u64,
    pub evaluation_incomplete: bool,
    pub rewound_on_save_load: bool,
    pub unlocked_achievements: Vec<String>,
    pub emitted_stats: Vec<EmittedPlaneAStat>,
}

fn parse_addr(s: &str) -> Option<usize> {
    let trimmed = s.trim();
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        usize::from_str_radix(hex, 16).ok()
    } else if let Some(hex) = trimmed.strip_prefix('h').or_else(|| trimmed.strip_prefix('H')) {
        usize::from_str_radix(hex, 16).ok()
    } else {
        trimmed.parse::<usize>().ok()
    }
}

fn parse_int_literal(s: &str) -> Option<i64> {
    let t = s.trim();
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16).ok()
    } else if let Some(hex) = t.strip_prefix('h').or_else(|| t.strip_prefix('H')) {
        i64::from_str_radix(hex, 16).ok()
    } else {
        t.parse::<i64>().ok()
    }
}

pub fn read_memory_value(mem: &[u8], offset: usize, size: &str) -> Option<i64> {
    match size {
        "u8" => mem.get(offset).map(|&b| b as i64),
        "i8" => mem.get(offset).map(|&b| (b as i8) as i64),
        "u16_le" => mem
            .get(offset..offset + 2)
            .map(|s| u16::from_le_bytes([s[0], s[1]]) as i64),
        "u16_be" => mem
            .get(offset..offset + 2)
            .map(|s| u16::from_be_bytes([s[0], s[1]]) as i64),
        "i16_le" => mem
            .get(offset..offset + 2)
            .map(|s| i16::from_le_bytes([s[0], s[1]]) as i64),
        "i16_be" => mem
            .get(offset..offset + 2)
            .map(|s| i16::from_be_bytes([s[0], s[1]]) as i64),
        "u24_le" => mem
            .get(offset..offset + 3)
            .map(|s| ((s[0] as u32) | ((s[1] as u32) << 8) | ((s[2] as u32) << 16)) as i64),
        "u24_be" => mem
            .get(offset..offset + 3)
            .map(|s| (((s[0] as u32) << 16) | ((s[1] as u32) << 8) | (s[2] as u32)) as i64),
        "u32_le" => mem
            .get(offset..offset + 4)
            .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as i64),
        "u32_be" => mem
            .get(offset..offset + 4)
            .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]) as i64),
        "i32_le" => mem
            .get(offset..offset + 4)
            .map(|s| i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as i64),
        "i32_be" => mem
            .get(offset..offset + 4)
            .map(|s| i32::from_be_bytes([s[0], s[1], s[2], s[3]]) as i64),
        "lower4" => mem.get(offset).map(|&b| (b & 0x0F) as i64),
        "upper4" => mem.get(offset).map(|&b| ((b >> 4) & 0x0F) as i64),
        "bitcount" => mem.get(offset).map(|&b| b.count_ones() as i64),
        "bit0" | "bit1" | "bit2" | "bit3" | "bit4" | "bit5" | "bit6" | "bit7" => {
            let bit: u8 = size[3..].parse().ok()?;
            mem.get(offset).map(|&b| ((b >> bit) & 1) as i64)
        }
        _ => mem.get(offset).map(|&b| b as i64),
    }
}

fn size_mask(size: &str) -> i64 {
    match size {
        "u8" | "i8" | "bitcount" => 0xFF,
        "u16_le" | "u16_be" | "i16_le" | "i16_be" => 0xFFFF,
        "u24_le" | "u24_be" => 0xFF_FFFF,
        "u32_le" | "u32_be" | "i32_le" | "i32_be" => 0xFFFF_FFFF,
        "lower4" | "upper4" => 0x0F,
        s if s.starts_with("bit") => 0x01,
        _ => 0xFF,
    }
}

fn compare_val(lhs: i64, op: &str, rhs: i64) -> bool {
    match op {
        "==" | "=" => lhs == rhs,
        "!=" => lhs != rhs,
        "<" => lhs < rhs,
        "<=" => lhs <= rhs,
        ">" => lhs > rhs,
        ">=" => lhs >= rhs,
        _ => false,
    }
}

/// Parse a RetroAchievements-style `memaddr` condition string (e.g.
/// `0xH0020=1_d0xH0010=5.2._R:0xH0030=99S0xM0021=1`) into core + alt condition groups.
pub fn parse_rcheevos_memaddr(
    memaddr: &str,
) -> Result<(Vec<MemoryConditionSpec>, Vec<Vec<MemoryConditionSpec>>), String> {
    let trimmed = memaddr.trim();
    if trimmed.is_empty() {
        return Err("empty rcheevos memaddr".to_string());
    }
    let mut groups: Vec<Vec<MemoryConditionSpec>> = Vec::new();
    for raw_group in trimmed.split('S') {
        let mut group = Vec::new();
        for raw_cond in raw_group.split('_') {
            let c = raw_cond.trim();
            if c.is_empty() {
                continue;
            }
            group.push(parse_single_ra_condition(c)?);
        }
        if !group.is_empty() {
            groups.push(group);
        }
    }
    if groups.is_empty() {
        return Err("memaddr contained no condition groups".to_string());
    }
    let core = groups.remove(0);
    Ok((core, groups))
}

fn parse_single_ra_condition(raw: &str) -> Result<MemoryConditionSpec, String> {
    let mut rest = raw.trim();
    let mut kind = "normal".to_string();

    if let Some((prefix, tail)) = rest.split_once(':') {
        kind = match prefix {
            "R" => "reset_if",
            "P" => "pause_if",
            "A" => "add_source",
            "B" => "sub_source",
            "C" => "add_hits",
            "D" => "sub_hits",
            "N" => "and_next",
            "O" => "or_next",
            "Z" => "reset_next_if",
            "M" | "G" => "measured",
            "Q" => "measured_if",
            "T" => "trigger",
            _ => "normal",
        }
        .to_string();
        rest = tail;
    }

    // Parse optional hit count `.N.` or `(N)` at the end
    let mut required_hits: u32 = 0;
    if let Some(stripped) = rest.strip_suffix('.') {
        if let Some(dot_idx) = stripped.rfind('.') {
            if let Ok(hits) = stripped[dot_idx + 1..].parse::<u32>() {
                required_hits = hits;
                rest = &stripped[..dot_idx];
            }
        }
    } else if let Some(stripped) = rest.strip_suffix(')') {
        if let Some(open_idx) = stripped.rfind('(') {
            if let Ok(hits) = stripped[open_idx + 1..].parse::<u32>() {
                required_hits = hits;
                rest = &stripped[..open_idx];
            }
        }
    }

    // Split LHS [op RHS]
    let ops = ["==", "!=", "<=", ">=", "=", "<", ">"];
    let mut found_op: Option<(&str, &str, &str)> = None;
    for op in ops {
        if let Some(idx) = rest.find(op) {
            let lhs_str = &rest[..idx];
            let rhs_str = &rest[idx + op.len()..];
            let norm_op = if op == "=" { "==" } else { op };
            found_op = Some((lhs_str, norm_op, rhs_str));
            break;
        }
    }

    let (lhs_str, op, rhs_opt) = match found_op {
        Some((l, o, r)) => (l, o, Some(r)),
        None => (rest, "==", None),
    };

    let (lhs_type, lhs_size, lhs_addr, lhs_const) = parse_ra_operand(lhs_str)?;
    let (rhs_type, rhs_size, rhs_addr, rhs_const) = match rhs_opt {
        Some(r) => {
            let (rt, rs, ra, rc) = parse_ra_operand(r)?;
            (Some(rt), Some(rs), ra, rc)
        }
        None => (Some("const".to_string()), None, None, Some(0)),
    };

    Ok(MemoryConditionSpec {
        region_id: 0,
        address: lhs_addr.unwrap_or_else(|| "0x0".to_string()),
        size: lhs_size,
        op: op.to_string(),
        value: rhs_const.or(lhs_const),
        compare_delta: rhs_type.as_deref() == Some("delta"),
        kind,
        required_hits,
        lhs_type: Some(lhs_type),
        rhs_type,
        rhs_region_id: None,
        rhs_address: rhs_addr,
        rhs_size,
    })
}

fn parse_ra_operand(raw: &str) -> Result<(String, String, Option<String>, Option<i64>), String> {
    let mut s = raw.trim();
    let mut op_type = "mem";
    if let Some(t) = s.strip_prefix('d') {
        op_type = "delta";
        s = t;
    } else if let Some(t) = s.strip_prefix('p') {
        op_type = "prior";
        s = t;
    } else if let Some(t) = s.strip_prefix('~') {
        op_type = "invert";
        s = t;
    }

    if let Some(after) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        let (size, addr_hex) = match after.chars().next() {
            Some('H') | Some('h') if after.len() > 1 && after.as_bytes()[1].is_ascii_hexdigit() => {
                ("u8", &after[1..])
            }
            Some(' ') => ("u16_le", after[1..].trim()),
            Some('I') | Some('i') => ("u16_be", &after[1..]),
            Some('W') | Some('w') => ("u24_le", &after[1..]),
            Some('X') | Some('x') => ("u32_le", &after[1..]),
            Some('G') | Some('g') => ("u32_be", &after[1..]),
            Some('L') | Some('l') => ("lower4", &after[1..]),
            Some('U') | Some('u') => ("upper4", &after[1..]),
            Some('K') | Some('k') => ("bitcount", &after[1..]),
            Some('M') | Some('m') => ("bit0", &after[1..]),
            Some('N') | Some('n') => ("bit1", &after[1..]),
            Some('O') | Some('o') => ("bit2", &after[1..]),
            Some('P') | Some('p') => ("bit3", &after[1..]),
            Some('Q') | Some('q') => ("bit4", &after[1..]),
            Some('R') | Some('r') => ("bit5", &after[1..]),
            Some('S') | Some('s') => ("bit6", &after[1..]),
            Some('T') | Some('t') => ("bit7", &after[1..]),
            _ => ("u16_le", after),
        };
        if addr_hex.is_empty() || usize::from_str_radix(addr_hex, 16).is_err() {
            return Err(format!("invalid RA memory operand '{}'", raw));
        }
        return Ok((
            op_type.to_string(),
            size.to_string(),
            Some(format!("0x{}", addr_hex)),
            None,
        ));
    }

    let val = parse_int_literal(s)
        .ok_or_else(|| format!("invalid RA constant operand '{}'", raw))?;
    Ok(("const".to_string(), "u32_le".to_string(), None, Some(val)))
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
            stat_last_values: HashMap::new(),
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
            progress.prior_values.clear();
            progress.measured_current = None;
            progress.measured_target = None;
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
        // 1. Sequence-gap detection (deduplicate simultaneous skipped + explicit FrameGap)
        let skipped = if self.last_sequence_number > 0
            && sequence_number > self.last_sequence_number + 1
        {
            sequence_number - self.last_sequence_number - 1
        } else {
            0
        };
        let explicit_gap = if lifecycle_signal == LifecycleSignal::FrameGap {
            (frame_gap_count.max(1)) as u64
        } else {
            0
        };
        let recorded_gap = if skipped > 0 && explicit_gap > 0 {
            skipped.max(explicit_gap)
        } else {
            skipped + explicit_gap
        };
        let gap_detected = recorded_gap > 0;

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
        let mut rewound = false;
        match lifecycle_signal {
            LifecycleSignal::SaveStateLoad => {
                if let Some(saved_json) =
                    load_plane_a_progress_state(conn, &self.canonical_game_id, &self.session_id)?
                {
                    if let Ok(restored) = Self::restore_progress(&saved_json) {
                        self.rules = restored.rules;
                        self.stat_last_values = restored.stat_last_values;
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
        let mut emitted_stats = Vec::new();

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

                for stat_rule in &p.memory_stats {
                    let guard_ok = if stat_rule.conditions.is_empty() {
                        true
                    } else {
                        let key = format!("__stat:{}", stat_rule.stat_key);
                        let progress = self.rules.entry(key.clone()).or_default();
                        eval_condition_group(
                            &key,
                            &stat_rule.conditions,
                            guest_memory,
                            region_offsets,
                            progress,
                        )
                        .satisfied
                    };
                    if !guard_ok {
                        continue;
                    }
                    if let Some(addr) = parse_addr(&stat_rule.address) {
                        let base = region_offsets
                            .get(&stat_rule.region_id)
                            .copied()
                            .unwrap_or(0);
                        if let Some(cur) =
                            read_memory_value(guest_memory, base.saturating_add(addr), &stat_rule.size)
                        {
                            let kind = EventKind::from_str_strict(&stat_rule.event_kind)
                                .unwrap_or(EventKind::StatSet);
                            match kind {
                                EventKind::StatInc => {
                                    let prev = self
                                        .stat_last_values
                                        .get(&stat_rule.stat_key)
                                        .copied()
                                        .unwrap_or(0);
                                    if cur > prev {
                                        emitted_stats.push(EmittedPlaneAStat {
                                            stat_key: stat_rule.stat_key.clone(),
                                            event_kind: EventKind::StatInc,
                                            int_value: cur - prev,
                                        });
                                    }
                                    self.stat_last_values.insert(stat_rule.stat_key.clone(), cur);
                                }
                                _ => {
                                    let prev =
                                        self.stat_last_values.get(&stat_rule.stat_key).copied();
                                    let skip_initial_zero =
                                        prev.is_none() && cur == 0 && stat_rule.only_when_nonzero;
                                    if prev != Some(cur) && !skip_initial_zero {
                                        emitted_stats.push(EmittedPlaneAStat {
                                            stat_key: stat_rule.stat_key.clone(),
                                            event_kind: EventKind::StatSet,
                                            int_value: cur,
                                        });
                                    }
                                    self.stat_last_values.insert(stat_rule.stat_key.clone(), cur);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Persist unlocked achievements & stats in SQLite (never mutate in SPECTATOR mode!)
        if (!unlocked_achievements.is_empty() || !emitted_stats.is_empty())
            && progression_mode != ProgressionMode::Spectator
        {
            let now = now_rfc3339();
            let ver_state = match trust_tier {
                TrustTier::Local => VerificationState::Unverified,
                TrustTier::VerifiedIntegration => VerificationState::Verified,
                TrustTier::Competitive => VerificationState::Pending,
            };

            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(|e| e.to_string())?;
            let mut emit_idx: i64 = 0;

            for stat in &emitted_stats {
                emit_idx += 1;
                let seq_i64 = ((sequence_number as i64) * 1000) + emit_idx;
                let idemp = format!("{}:{}:planeA:{}", producer_id, self.session_id, seq_i64);
                let j_ev = JournalEventInput {
                    producer_id: producer_id.to_string(),
                    session_id: self.session_id.clone(),
                    sequence_number: seq_i64,
                    idempotency_key: idemp,
                    canonical_game_id: self.canonical_game_id.clone(),
                    event_kind: stat.event_kind,
                    key: stat.stat_key.clone(),
                    int_value: Some(stat.int_value),
                    float_value: None,
                    string_value: None,
                    occurred_at: now.clone(),
                    progression_mode,
                    trust_tier,
                    evidence_class: EvidenceClass::RuntimeObserved,
                    verification_state: ver_state,
                    raw_payload: format!(
                        "{{\"plane\":\"A\",\"frame_seq\":{},\"stat_key\":\"{}\",\"event_kind\":\"{}\",\"int_value\":{}}}",
                        sequence_number,
                        stat.stat_key,
                        stat.event_kind.as_str(),
                        stat.int_value
                    ),
                };
                let _ = insert_journal_event_tx(&tx, &j_ev)?;
                match stat.event_kind {
                    EventKind::StatInc => {
                        let _ = apply_stat_update_tx(
                            &tx,
                            &self.canonical_game_id,
                            progression_mode,
                            &stat.stat_key,
                            Some(stat.int_value),
                            None,
                            None,
                            None,
                        )?;
                    }
                    _ => {
                        let _ = apply_stat_update_tx(
                            &tx,
                            &self.canonical_game_id,
                            progression_mode,
                            &stat.stat_key,
                            None,
                            Some(stat.int_value),
                            None,
                            None,
                        )?;
                    }
                }
            }

            for ach_id in &unlocked_achievements {
                emit_idx += 1;
                let seq_i64 = ((sequence_number as i64) * 1000) + emit_idx;
                let idemp = format!("{}:{}:planeA:{}", producer_id, self.session_id, seq_i64);
                let j_ev = JournalEventInput {
                    producer_id: producer_id.to_string(),
                    session_id: self.session_id.clone(),
                    sequence_number: seq_i64,
                    idempotency_key: idemp,
                    canonical_game_id: self.canonical_game_id.clone(),
                    event_kind: EventKind::UnlockRequest,
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
            emitted_stats,
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

        let (parsed_core, parsed_alts);
        let (core_conds, alt_groups): (&[MemoryConditionSpec], &[Vec<MemoryConditionSpec>]) =
            if rule.conditions.is_empty() {
                if let Some(memaddr) = &rule.memaddr {
                    match parse_rcheevos_memaddr(memaddr) {
                        Ok((c, a)) => {
                            parsed_core = c;
                            parsed_alts = a;
                            (&parsed_core, &parsed_alts)
                        }
                        Err(_) => return false,
                    }
                } else {
                    (&rule.conditions, &rule.alt_groups)
                }
            } else {
                (&rule.conditions, &rule.alt_groups)
            };

        let core_res = eval_condition_group("core", core_conds, mem, region_offsets, progress);
        let mut alt_ok = alt_groups.is_empty();
        let mut any_reset = core_res.reset_fired;

        if !alt_groups.is_empty() {
            for (gi, alt) in alt_groups.iter().enumerate() {
                let prefix = format!("alt{}", gi);
                let res = eval_condition_group(&prefix, alt, mem, region_offsets, progress);
                if res.reset_fired {
                    any_reset = true;
                }
                if res.satisfied {
                    alt_ok = true;
                }
            }
        }

        if any_reset {
            progress.hit_counts.clear();
            return false;
        }

        if core_res.satisfied && alt_ok {
            progress.triggered = true;
            return true;
        }
        false
    }
}

struct GroupEvalResult {
    satisfied: bool,
    reset_fired: bool,
}

fn read_operand(
    op_key: &str,
    op_type: &str,
    region_id: u32,
    address: &str,
    size: &str,
    const_val: Option<i64>,
    mem: &[u8],
    region_offsets: &HashMap<u32, usize>,
    progress: &mut RuleEvalProgress,
) -> Option<i64> {
    if op_type == "const" {
        return Some(const_val.unwrap_or(0));
    }
    let addr = parse_addr(address)?;
    let base = region_offsets.get(&region_id).copied().unwrap_or(0);
    let offset = base.saturating_add(addr);
    let cur_val = read_memory_value(mem, offset, size)?;

    let prev_val = progress.prev_values.get(op_key).copied().unwrap_or(cur_val);
    let prior_val = progress.prior_values.get(op_key).copied().unwrap_or(cur_val);
    if cur_val != prev_val {
        progress.prior_values.insert(op_key.to_string(), prev_val);
    } else {
        progress
            .prior_values
            .entry(op_key.to_string())
            .or_insert(prior_val);
    }
    progress.prev_values.insert(op_key.to_string(), cur_val);

    let effective_prior = progress.prior_values.get(op_key).copied().unwrap_or(prior_val);

    match op_type {
        "delta" => Some(prev_val),
        "prior" => Some(effective_prior),
        "invert" => Some((!cur_val) & size_mask(size)),
        _ => Some(cur_val),
    }
}

fn eval_condition_group(
    prefix: &str,
    conds: &[MemoryConditionSpec],
    mem: &[u8],
    region_offsets: &HashMap<u32, usize>,
    progress: &mut RuleEvalProgress,
) -> GroupEvalResult {
    if conds.is_empty() {
        return GroupEvalResult {
            satisfied: false,
            reset_fired: false,
        };
    }

    struct EvaluatedCond {
        key: String,
        kind: String,
        required_hits: u32,
        eff_matched: bool,
        lhs_val: i64,
        rhs_val: i64,
        reset_next: bool,
    }

    let mut paused = false;
    let mut reset_fired = false;
    let mut evaluated = Vec::with_capacity(conds.len());

    let mut add_source_acc: i64 = 0;
    let mut chain_mode: Option<&str> = None;
    let mut chain_val: bool = false;
    let mut pending_reset_next = false;

    for (idx, cond) in conds.iter().enumerate() {
        let key = format!("{}:{}", prefix, idx);
        let lhs_type = cond.lhs_type.as_deref().unwrap_or("mem");
        let lhs_val = match read_operand(
            &format!("{}:lhs", key),
            lhs_type,
            cond.region_id,
            &cond.address,
            &cond.size,
            cond.value,
            mem,
            region_offsets,
            progress,
        ) {
            Some(v) => v,
            None => {
                return GroupEvalResult {
                    satisfied: false,
                    reset_fired: false,
                }
            }
        };

        let rhs_type = cond.rhs_type.as_deref().unwrap_or_else(|| {
            if cond.compare_delta {
                "delta"
            } else {
                "const"
            }
        });
        let rhs_region = cond.rhs_region_id.unwrap_or(cond.region_id);
        let rhs_addr = cond.rhs_address.as_deref().unwrap_or(&cond.address);
        let rhs_size = cond.rhs_size.as_deref().unwrap_or(&cond.size);
        let rhs_val = match read_operand(
            &format!("{}:rhs", key),
            rhs_type,
            rhs_region,
            rhs_addr,
            rhs_size,
            cond.value,
            mem,
            region_offsets,
            progress,
        ) {
            Some(v) => v,
            None => {
                return GroupEvalResult {
                    satisfied: false,
                    reset_fired: false,
                }
            }
        };

        let kind = cond.kind.as_str();
        if kind == "add_source" {
            add_source_acc = add_source_acc.saturating_add(lhs_val);
            continue;
        }
        if kind == "sub_source" {
            add_source_acc = add_source_acc.saturating_sub(lhs_val);
            continue;
        }

        let eff_lhs = lhs_val.saturating_add(add_source_acc);
        add_source_acc = 0;
        let raw_matched = compare_val(eff_lhs, &cond.op, rhs_val);

        let eff_matched = match chain_mode {
            Some("and_next") => chain_val && raw_matched,
            Some("or_next") => chain_val || raw_matched,
            _ => raw_matched,
        };

        if kind == "and_next" || kind == "or_next" {
            chain_mode = Some(kind);
            chain_val = eff_matched;
            pending_reset_next = false;
            continue;
        } else {
            chain_mode = None;
        }

        let this_reset_next = pending_reset_next;
        pending_reset_next = kind == "reset_next_if";
        if kind == "reset_next_if" {
            pending_reset_next = eff_matched;
            continue;
        }

        let req_hits = if kind == "once" {
            cond.required_hits.max(1)
        } else {
            cond.required_hits
        };

        evaluated.push(EvaluatedCond {
            key,
            kind: kind.to_string(),
            required_hits: req_hits,
            eff_matched,
            lhs_val: eff_lhs,
            rhs_val,
            reset_next: this_reset_next,
        });
    }

    // Evaluate PauseIf first (group pause freezes hit accumulation & blocks satisfaction)
    for c in &evaluated {
        if c.kind == "pause_if" {
            if c.reset_next {
                progress.hit_counts.insert(c.key.clone(), 0);
            }
            if c.required_hits > 0 {
                let hits = progress.hit_counts.entry(c.key.clone()).or_insert(0);
                if c.eff_matched && *hits < c.required_hits {
                    *hits += 1;
                }
                if *hits >= c.required_hits {
                    paused = true;
                }
            } else if c.eff_matched {
                paused = true;
            }
        }
    }

    if paused {
        return GroupEvalResult {
            satisfied: false,
            reset_fired: false,
        };
    }

    // Evaluate ResetIf
    for c in &evaluated {
        if c.kind == "reset_if" {
            if c.reset_next {
                progress.hit_counts.insert(c.key.clone(), 0);
            }
            if c.required_hits > 0 {
                let hits = progress.hit_counts.entry(c.key.clone()).or_insert(0);
                if c.eff_matched && *hits < c.required_hits {
                    *hits += 1;
                }
                if *hits >= c.required_hits {
                    reset_fired = true;
                }
            } else if c.eff_matched {
                reset_fired = true;
            }
        }
    }

    if reset_fired {
        for c in &evaluated {
            if c.kind != "pause_if" {
                progress.hit_counts.insert(c.key.clone(), 0);
            }
        }
        return GroupEvalResult {
            satisfied: false,
            reset_fired: true,
        };
    }

    let mut pending_hits: i64 = 0;
    let mut any_normal = false;
    let mut all_ok = true;

    for c in &evaluated {
        if c.kind == "pause_if" || c.kind == "reset_if" {
            continue;
        }
        if c.reset_next {
            progress.hit_counts.insert(c.key.clone(), 0);
        }

        if c.kind == "add_hits" || c.kind == "sub_hits" {
            let hits = progress.hit_counts.entry(c.key.clone()).or_insert(0);
            if c.eff_matched && (c.required_hits == 0 || *hits < c.required_hits) {
                *hits += 1;
            }
            if c.kind == "add_hits" {
                pending_hits += *hits as i64;
            } else {
                pending_hits -= *hits as i64;
            }
            continue;
        }

        any_normal = true;

        if c.required_hits > 0 {
            let hits = progress.hit_counts.entry(c.key.clone()).or_insert(0);
            if c.eff_matched {
                let total_with_pending = (*hits as i64) + pending_hits;
                if total_with_pending < c.required_hits as i64 {
                    *hits += 1;
                }
            }
            let effective_hits = ((*hits as i64) + pending_hits).max(0) as u32;
            pending_hits = 0;

            if c.kind == "measured" {
                progress.measured_current = Some(effective_hits.min(c.required_hits));
                progress.measured_target = Some(c.required_hits);
            }
            if effective_hits < c.required_hits {
                all_ok = false;
            }
        } else {
            pending_hits = 0;
            if c.kind == "measured" {
                let cur = c.lhs_val.max(0) as u32;
                let tgt = c.rhs_val.max(1) as u32;
                progress.measured_current = Some(cur.min(tgt));
                progress.measured_target = Some(tgt);
            }
            if !c.eff_matched {
                all_ok = false;
            }
        }
    }

    GroupEvalResult {
        satisfied: any_normal && all_ok,
        reset_fired: false,
    }
}
