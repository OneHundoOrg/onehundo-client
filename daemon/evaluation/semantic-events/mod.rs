use crate::core::types::{
    EventKind, EvidenceClass, ProgressionMode, TrustTier, VerificationState,
};
use crate::packs::manifest::{LoadedPack, SemanticRule};
use crate::storage::journal::{insert_journal_event_tx, JournalEventInput};
use crate::storage::state::{apply_stat_update_tx, record_trophy_unlock_tx};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticEventPayload {
    pub event_type: String,
    pub canonical_game_id: String,
    pub producer_id: String,
    pub session_id: String,
    pub sequence_number: i64,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub timestamp_nanos: Option<u64>,
    pub key: String,
    #[serde(default)]
    pub int_value: Option<i64>,
    #[serde(default)]
    pub float_value: Option<f64>,
    #[serde(default)]
    pub string_value: Option<String>,
    #[serde(default)]
    pub progression_mode: Option<String>,
    #[serde(default)]
    pub trust_tier: Option<String>,
    #[serde(default)]
    pub evidence_class: Option<String>,
    #[serde(default)]
    pub verification_state: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnlockedAchievementInfo {
    pub canonical_achievement_id: String,
    pub title: String,
    pub description: String,
    pub points: u32,
    pub progression_mode: ProgressionMode,
    pub trust_tier: TrustTier,
    pub verification_state: VerificationState,
    pub first_time: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticCommitOutcome {
    pub sequence_number: i64,
    pub deduplicated: bool,
    pub newly_unlocked: Vec<UnlockedAchievementInfo>,
    pub updated_stat_int: Option<i64>,
}

fn eval_cmp(val: i64, op: &str, threshold: i64) -> bool {
    match op {
        ">=" => val >= threshold,
        ">" => val > threshold,
        "==" => val == threshold,
        "<=" => val <= threshold,
        "<" => val < threshold,
        "!=" => val != threshold,
        _ => val >= threshold,
    }
}

pub fn evaluate_and_commit_plane_b(
    conn: &mut Connection,
    pack: Option<&LoadedPack>,
    ev: &SemanticEventPayload,
    occurred_at: &str,
    effective_mode: ProgressionMode,
    effective_trust: TrustTier,
    raw_payload: &str,
) -> Result<SemanticCommitOutcome, String> {
    let event_kind = EventKind::from_str_strict(&ev.event_type)
        .ok_or_else(|| format!("unsupported event_type '{}'", ev.event_type))?;
    if ev.sequence_number < 1 {
        return Err("sequence_number must be >= 1".to_string());
    }
    if ev.key.trim().is_empty() {
        return Err("event key must not be empty".to_string());
    }

    let idempotency_key = ev
        .idempotency_key
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}:{}:{}", ev.producer_id, ev.session_id, ev.sequence_number));

    let evidence_class = ev
        .evidence_class
        .as_deref()
        .and_then(EvidenceClass::from_str_strict)
        .unwrap_or(EvidenceClass::DirectIntegrationEvent);

    // Determine effective verification state based on trust lane + pack producer authorization
    let producer_authorized = pack
        .and_then(|p| p.manifest.profiles.semantic.as_ref())
        .map(|s| s.enabled && s.producer_ids.iter().any(|pid| pid == &ev.producer_id))
        .unwrap_or(false);

    let effective_verification = match effective_trust {
        TrustTier::Local => VerificationState::Unverified,
        TrustTier::VerifiedIntegration => {
            if producer_authorized {
                VerificationState::Verified
            } else {
                VerificationState::Pending
            }
        }
        TrustTier::Competitive => VerificationState::Pending,
    };

    let journal_input = JournalEventInput {
        producer_id: ev.producer_id.clone(),
        session_id: ev.session_id.clone(),
        sequence_number: ev.sequence_number,
        idempotency_key,
        canonical_game_id: ev.canonical_game_id.clone(),
        event_kind,
        key: ev.key.clone(),
        int_value: ev.int_value,
        float_value: ev.float_value,
        string_value: ev.string_value.clone(),
        occurred_at: occurred_at.to_string(),
        progression_mode: effective_mode,
        trust_tier: effective_trust,
        evidence_class,
        verification_state: effective_verification,
        raw_payload: raw_payload.to_string(),
    };

    // Begin IMMEDIATE transaction for strict Commit-Before-Ack durability
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| format!("failed to begin SQLite transaction: {}", e))?;

    let inserted = insert_journal_event_tx(&tx, &journal_input)?;
    if !inserted {
        tx.commit()
            .map_err(|e| format!("failed to commit deduplicated tx: {}", e))?;
        return Ok(SemanticCommitOutcome {
            sequence_number: ev.sequence_number,
            deduplicated: true,
            newly_unlocked: Vec::new(),
            updated_stat_int: None,
        });
    }

    let mut updated_stat_int: Option<i64> = None;
    let mut candidate_achievements: Vec<String> = Vec::new();

    match event_kind {
        EventKind::StatSet => {
            updated_stat_int = apply_stat_update_tx(
                &tx,
                &ev.canonical_game_id,
                effective_mode,
                &ev.key,
                None,
                ev.int_value,
                ev.float_value,
                ev.string_value.as_deref(),
            )?;
            if let Some(p) = pack {
                for rule in &p.semantic_rules {
                    if (rule.event == "STAT_SET" || rule.event == "STAT_INC" || rule.event == "STAT")
                        && rule.stat.as_deref() == Some(ev.key.as_str())
                    {
                        if let (Some(cur), Some(thresh)) = (updated_stat_int, rule.threshold) {
                            let op = rule.op.as_deref().unwrap_or(">=");
                            if eval_cmp(cur, op, thresh) {
                                candidate_achievements.push(rule.achievement_id.clone());
                            }
                        }
                    }
                }
            }
        }
        EventKind::StatInc => {
            let delta = ev.int_value.unwrap_or(1);
            updated_stat_int = apply_stat_update_tx(
                &tx,
                &ev.canonical_game_id,
                effective_mode,
                &ev.key,
                Some(delta),
                None,
                None,
                None,
            )?;
            if let Some(p) = pack {
                for rule in &p.semantic_rules {
                    if (rule.event == "STAT_INC" || rule.event == "STAT_SET" || rule.event == "STAT")
                        && rule.stat.as_deref() == Some(ev.key.as_str())
                    {
                        if let (Some(cur), Some(thresh)) = (updated_stat_int, rule.threshold) {
                            let op = rule.op.as_deref().unwrap_or(">=");
                            if eval_cmp(cur, op, thresh) {
                                candidate_achievements.push(rule.achievement_id.clone());
                            }
                        }
                    }
                }
            }
        }
        EventKind::StateObserve => {
            let _ = apply_stat_update_tx(
                &tx,
                &ev.canonical_game_id,
                effective_mode,
                &ev.key,
                None,
                None,
                None,
                ev.string_value.as_deref(),
            )?;
            if let Some(p) = pack {
                for rule in &p.semantic_rules {
                    if rule.event == "STATE_OBSERVE"
                        && rule.key.as_deref() == Some(ev.key.as_str())
                        && (rule.value.is_none()
                            || rule.value.as_deref() == ev.string_value.as_deref())
                    {
                        candidate_achievements.push(rule.achievement_id.clone());
                    }
                }
            }
        }
        EventKind::UnlockRequest => {
            if let Some(p) = pack {
                if let Some(rule_match) = find_unlock_rule(&p.semantic_rules, &ev.key) {
                    candidate_achievements.push(rule_match.achievement_id.clone());
                } else if p
                    .manifest
                    .achievements
                    .iter()
                    .any(|a| a.canonical_achievement_id == ev.key)
                {
                    candidate_achievements.push(ev.key.clone());
                }
            } else {
                // Frictionless LOCAL quick integration even without a pre-installed pack
                if effective_trust == TrustTier::Local {
                    candidate_achievements.push(ev.key.clone());
                }
            }
        }
        EventKind::ObservationImport => {
            return Err("Plane C ObservationImport must never enter Plane B evaluator".into());
        }
    }

    let mut newly_unlocked = Vec::new();
    for ach_id in candidate_achievements {
        let def = pack.and_then(|p| {
            p.manifest
                .achievements
                .iter()
                .find(|a| a.canonical_achievement_id == ach_id)
        });

        // Respect supported_modes if defined in pack
        if let Some(d) = def {
            if !d.supported_modes.contains(&effective_mode) {
                continue;
            }
        }

        // Check if SPECTATOR: never mutate trophy_state
        if effective_mode == ProgressionMode::Spectator {
            continue;
        }

        // Dual-lane trust: if achievement requires higher trust tier than current active session trust,
        // keep as LOCAL / UNVERIFIED rather than deleting or crashing.
        let ach_ver = if let Some(d) = def {
            if effective_trust >= d.minimum_trust_tier && effective_verification == VerificationState::Verified {
                VerificationState::Verified
            } else if effective_trust == TrustTier::Local {
                VerificationState::Unverified
            } else {
                effective_verification
            }
        } else {
            effective_verification
        };

        let first_time = record_trophy_unlock_tx(
            &tx,
            &ev.canonical_game_id,
            &ach_id,
            &ev.session_id,
            occurred_at,
            effective_mode,
            effective_trust,
            evidence_class,
            ach_ver,
        )?;

        // Also allow notification on ENCORE replay or first STANDARD unlock
        if first_time || effective_mode == ProgressionMode::Encore {
            // Check existing state for title/points
            let (title, description, points) = if let Some(d) = def {
                (d.title.clone(), d.description.clone(), d.points)
            } else {
                (ach_id.clone(), "Local integration unlock".to_string(), 10)
            };
            newly_unlocked.push(UnlockedAchievementInfo {
                canonical_achievement_id: ach_id,
                title,
                description,
                points,
                progression_mode: effective_mode,
                trust_tier: effective_trust,
                verification_state: ach_ver,
                first_time,
            });
        }
    }

    // COMMIT BEFORE ACK
    tx.commit()
        .map_err(|e| format!("failed to commit Plane B transaction: {}", e))?;

    Ok(SemanticCommitOutcome {
        sequence_number: ev.sequence_number,
        deduplicated: false,
        newly_unlocked,
        updated_stat_int,
    })
}

fn find_unlock_rule<'a>(rules: &'a [SemanticRule], ach_id: &str) -> Option<&'a SemanticRule> {
    rules
        .iter()
        .find(|r| r.event == "UNLOCK_REQUEST" && r.achievement_id == ach_id)
}
