use crate::core::types::{
    now_unix_secs, EvidenceClass, ProgressionMode, TrustTier, VerificationState,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrophyRecord {
    pub canonical_game_id: String,
    pub canonical_achievement_id: String,
    pub progression_mode: String,
    pub first_unlocked_at: String,
    pub last_unlocked_at: String,
    pub replay_count: i64,
    pub trust_tier: String,
    pub evidence_class: String,
    pub verification_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatRecord {
    pub canonical_game_id: String,
    pub progression_mode: String,
    pub stat_key: String,
    pub int_value: Option<i64>,
    pub float_value: Option<f64>,
    pub string_value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncoreRunRecord {
    pub id: i64,
    pub canonical_game_id: String,
    pub canonical_achievement_id: String,
    pub session_id: String,
    pub unlocked_at: String,
    pub trust_tier: String,
    pub evidence_class: String,
    pub verification_state: String,
}

/// Update materialized stat state inside an active SQLite transaction.
/// SPECTATOR mode NEVER mutates persistent player progression.
pub fn apply_stat_update_tx(
    tx: &Transaction<'_>,
    canonical_game_id: &str,
    progression_mode: ProgressionMode,
    stat_key: &str,
    delta: Option<i64>,
    abs_int: Option<i64>,
    abs_float: Option<f64>,
    abs_string: Option<&str>,
) -> Result<Option<i64>, String> {
    if progression_mode == ProgressionMode::Spectator {
        return Ok(None);
    }
    let mode_str = progression_mode.as_str();
    let now = now_unix_secs();

    if let Some(d) = delta {
        let current: i64 = tx
            .query_row(
                "SELECT COALESCE(int_value, 0) FROM stat_state
                 WHERE canonical_game_id = ?1 AND progression_mode = ?2 AND stat_key = ?3",
                params![canonical_game_id, mode_str, stat_key],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .unwrap_or(0);
        let next = current.saturating_add(d);
        tx.execute(
            "INSERT INTO stat_state (canonical_game_id, progression_mode, stat_key, int_value, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(canonical_game_id, progression_mode, stat_key)
             DO UPDATE SET int_value = excluded.int_value, updated_at = excluded.updated_at",
            params![canonical_game_id, mode_str, stat_key, next, now],
        )
        .map_err(|e| e.to_string())?;
        return Ok(Some(next));
    }

    if let Some(v) = abs_int {
        tx.execute(
            "INSERT INTO stat_state (canonical_game_id, progression_mode, stat_key, int_value, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(canonical_game_id, progression_mode, stat_key)
             DO UPDATE SET int_value = excluded.int_value, updated_at = excluded.updated_at",
            params![canonical_game_id, mode_str, stat_key, v, now],
        )
        .map_err(|e| e.to_string())?;
        return Ok(Some(v));
    }

    if let Some(fv) = abs_float {
        tx.execute(
            "INSERT INTO stat_state (canonical_game_id, progression_mode, stat_key, float_value, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(canonical_game_id, progression_mode, stat_key)
             DO UPDATE SET float_value = excluded.float_value, updated_at = excluded.updated_at",
            params![canonical_game_id, mode_str, stat_key, fv, now],
        )
        .map_err(|e| e.to_string())?;
    } else if let Some(sv) = abs_string {
        tx.execute(
            "INSERT INTO stat_state (canonical_game_id, progression_mode, stat_key, string_value, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(canonical_game_id, progression_mode, stat_key)
             DO UPDATE SET string_value = excluded.string_value, updated_at = excluded.updated_at",
            params![canonical_game_id, mode_str, stat_key, sv, now],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(None)
}

/// Record an achievement unlock in materialized `trophy_state`.
/// - SPECTATOR: NEVER mutates `trophy_state` or `encore_runs`. Returns `Ok(false)`.
/// - STANDARD: Preserves `first_unlocked_at` forever (`INSERT OR IGNORE`). Returns `Ok(true)` only on first unlock.
/// - ENCORE: Preserves historical `first_unlocked_at` (never overwrites `STANDARD` nor previous `ENCORE` first unlock),
///           increments `replay_count`, and logs each replay run separately in `encore_runs`.
pub fn record_trophy_unlock_tx(
    tx: &Transaction<'_>,
    canonical_game_id: &str,
    canonical_achievement_id: &str,
    session_id: &str,
    unlocked_at: &str,
    progression_mode: ProgressionMode,
    trust_tier: TrustTier,
    evidence_class: EvidenceClass,
    verification_state: VerificationState,
) -> Result<bool, String> {
    if progression_mode == ProgressionMode::Spectator {
        return Ok(false);
    }

    let mode_str = progression_mode.as_str();
    let existing: Option<String> = tx
        .query_row(
            "SELECT first_unlocked_at FROM trophy_state
             WHERE canonical_game_id = ?1 AND canonical_achievement_id = ?2 AND progression_mode = ?3",
            params![canonical_game_id, canonical_achievement_id, mode_str],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    let is_first_unlock = existing.is_none();

    if is_first_unlock {
        tx.execute(
            "INSERT INTO trophy_state (
                canonical_game_id, canonical_achievement_id, progression_mode,
                first_unlocked_at, last_unlocked_at, replay_count,
                trust_tier, evidence_class, verification_state
            ) VALUES (?1, ?2, ?3, ?4, ?4, 1, ?5, ?6, ?7)",
            params![
                canonical_game_id,
                canonical_achievement_id,
                mode_str,
                unlocked_at,
                trust_tier.as_str(),
                evidence_class.as_str(),
                verification_state.as_str()
            ],
        )
        .map_err(|e| e.to_string())?;
    } else if progression_mode == ProgressionMode::Encore {
        // Preserve historical first_unlocked_at while tracking replay count & last_unlocked_at
        tx.execute(
            "UPDATE trophy_state
             SET replay_count = replay_count + 1,
                 last_unlocked_at = ?4
             WHERE canonical_game_id = ?1 AND canonical_achievement_id = ?2 AND progression_mode = ?3",
            params![canonical_game_id, canonical_achievement_id, mode_str, unlocked_at],
        )
        .map_err(|e| e.to_string())?;
    }

    if progression_mode == ProgressionMode::Encore {
        tx.execute(
            "INSERT INTO encore_runs (
                canonical_game_id, canonical_achievement_id, session_id, unlocked_at,
                trust_tier, evidence_class, verification_state
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                canonical_game_id,
                canonical_achievement_id,
                session_id,
                unlocked_at,
                trust_tier.as_str(),
                evidence_class.as_str(),
                verification_state.as_str()
            ],
        )
        .map_err(|e| e.to_string())?;
    }

    Ok(is_first_unlock)
}

pub fn save_plane_a_progress_state(
    conn: &Connection,
    canonical_game_id: &str,
    session_id: &str,
    state_json: &str,
) -> Result<(), String> {
    let now = now_unix_secs();
    conn.execute(
        "INSERT INTO plane_a_save_state (canonical_game_id, session_id, state_json, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(canonical_game_id, session_id)
         DO UPDATE SET state_json = excluded.state_json, updated_at = excluded.updated_at",
        params![canonical_game_id, session_id, state_json, now],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn load_plane_a_progress_state(
    conn: &Connection,
    canonical_game_id: &str,
    session_id: &str,
) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT state_json FROM plane_a_save_state WHERE canonical_game_id = ?1 AND session_id = ?2",
        params![canonical_game_id, session_id],
        |r| r.get(0),
    )
    .optional()
    .map_err(|e| e.to_string())
}

pub fn list_trophies(
    conn: &Connection,
    canonical_game_id: Option<&str>,
) -> Result<Vec<TrophyRecord>, String> {
    let sql = if canonical_game_id.is_some() {
        "SELECT canonical_game_id, canonical_achievement_id, progression_mode,
                first_unlocked_at, last_unlocked_at, replay_count,
                trust_tier, evidence_class, verification_state
         FROM trophy_state WHERE canonical_game_id = ?1
         ORDER BY first_unlocked_at ASC"
    } else {
        "SELECT canonical_game_id, canonical_achievement_id, progression_mode,
                first_unlocked_at, last_unlocked_at, replay_count,
                trust_tier, evidence_class, verification_state
         FROM trophy_state ORDER BY first_unlocked_at ASC"
    };
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mapper = |r: &rusqlite::Row<'_>| -> rusqlite::Result<TrophyRecord> {
        Ok(TrophyRecord {
            canonical_game_id: r.get(0)?,
            canonical_achievement_id: r.get(1)?,
            progression_mode: r.get(2)?,
            first_unlocked_at: r.get(3)?,
            last_unlocked_at: r.get(4)?,
            replay_count: r.get(5)?,
            trust_tier: r.get(6)?,
            evidence_class: r.get(7)?,
            verification_state: r.get(8)?,
        })
    };
    if let Some(gid) = canonical_game_id {
        let rows = stmt.query_map(params![gid], mapper).map_err(|e| e.to_string())?;
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
    } else {
        let rows = stmt.query_map([], mapper).map_err(|e| e.to_string())?;
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
    }
    Ok(out)
}

pub fn list_stats(
    conn: &Connection,
    canonical_game_id: Option<&str>,
) -> Result<Vec<StatRecord>, String> {
    let sql = if canonical_game_id.is_some() {
        "SELECT canonical_game_id, progression_mode, stat_key, int_value, float_value, string_value
         FROM stat_state WHERE canonical_game_id = ?1 ORDER BY stat_key ASC"
    } else {
        "SELECT canonical_game_id, progression_mode, stat_key, int_value, float_value, string_value
         FROM stat_state ORDER BY canonical_game_id ASC, stat_key ASC"
    };
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mapper = |r: &rusqlite::Row<'_>| -> rusqlite::Result<StatRecord> {
        Ok(StatRecord {
            canonical_game_id: r.get(0)?,
            progression_mode: r.get(1)?,
            stat_key: r.get(2)?,
            int_value: r.get(3)?,
            float_value: r.get(4)?,
            string_value: r.get(5)?,
        })
    };
    if let Some(gid) = canonical_game_id {
        let rows = stmt.query_map(params![gid], mapper).map_err(|e| e.to_string())?;
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
    } else {
        let rows = stmt.query_map([], mapper).map_err(|e| e.to_string())?;
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
    }
    Ok(out)
}

pub fn list_encore_runs(conn: &Connection) -> Result<Vec<EncoreRunRecord>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, canonical_game_id, canonical_achievement_id, session_id,
                    unlocked_at, trust_tier, evidence_class, verification_state
             FROM encore_runs ORDER BY id ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(EncoreRunRecord {
                id: r.get(0)?,
                canonical_game_id: r.get(1)?,
                canonical_achievement_id: r.get(2)?,
                session_id: r.get(3)?,
                unlocked_at: r.get(4)?,
                trust_tier: r.get(5)?,
                evidence_class: r.get(6)?,
                verification_state: r.get(7)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}
