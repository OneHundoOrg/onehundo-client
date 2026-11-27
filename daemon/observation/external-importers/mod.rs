//! Plane C External Importers.
//! INVARIANT: Strictly isolated from `daemon/evaluation/`.
//! External observations NEVER route through Plane A or Plane B gameplay evaluators,
//! and MUST carry provenance (`PLATFORM_CONFIRMED`, `LOCALLY_OBSERVED`, `INFERRED`).

pub mod declarative_save;
pub mod generic_export;
pub mod goldberg;
pub mod longshot;
pub mod trophy_dir;
pub mod watcher;

pub use crate::core::types::sha256_hex;

use crate::core::types::{
    deterministic_uuid_from_seed, format_rfc3339_secs, is_valid_canonical_id, is_valid_uuid,
    now_rfc3339, now_unix_secs, EventKind, EvidenceClass, ProgressionMode, TrustTier,
    VerificationState,
};
use crate::packs::matcher::PackRegistry;
use crate::storage::journal::{insert_journal_event_tx, JournalEventInput};
use crate::storage::state::{apply_stat_update_tx, record_trophy_unlock_tx};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

const VALID_SOURCE_TYPES: &[&str] = &[
    "steam_api",
    "psn_trophy",
    "save_file",
    "emulator_sram",
    "log_parser",
];

const VALID_RECORD_KINDS: &[&str] = &[
    "ACHIEVEMENT_EVIDENCE",
    "STAT_SNAPSHOT",
    "PLAYTIME_EVIDENCE",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationRecord {
    pub schema_version: String,
    pub observation_id: String,
    pub canonical_game_id: String,
    pub observer_id: String,
    pub source_type: String,
    pub observed_at: String,
    #[serde(default)]
    pub source_timestamp: Option<String>,
    #[serde(default)]
    pub artifact_hash_sha256: Option<String>,
    pub record_kind: String,
    pub key: String,
    #[serde(default)]
    pub unlocked: Option<bool>,
    #[serde(default)]
    pub int_value: Option<i64>,
    #[serde(default)]
    pub float_value: Option<f64>,
    #[serde(default)]
    pub string_value: Option<String>,
    pub progression_mode: ProgressionMode,
    pub trust_tier: TrustTier,
    pub evidence_class: EvidenceClass,
    pub verification_state: VerificationState,
    #[serde(default)]
    pub provenance_notes: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    pub canonical_game_id: Option<String>,
    pub override_progression_mode: Option<ProgressionMode>,
    pub dry_run: bool,
}

#[derive(Debug, Clone)]
pub struct ParsedObservationSet {
    pub records: Vec<ObservationRecord>,
    pub skipped_locked: usize,
    pub source_kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservationImportSummary {
    pub imported: usize,
    pub deduplicated: usize,
    #[serde(default)]
    pub skipped_locked: usize,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub source_kind: String,
    pub observation_ids: Vec<String>,
    #[serde(default)]
    pub records: Vec<ObservationRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservationHistoryRow {
    pub observation_id: String,
    pub canonical_game_id: String,
    pub observer_id: String,
    pub source_type: String,
    pub record_kind: String,
    pub key: String,
    pub unlocked: Option<bool>,
    #[serde(default)]
    pub int_value: Option<i64>,
    pub source_timestamp: Option<String>,
    pub observed_at: String,
    pub progression_mode: String,
    pub trust_tier: String,
    pub evidence_class: String,
    pub verification_state: String,
    pub artifact_hash_sha256: Option<String>,
}

pub(crate) fn resolve_mode(opts: &ImportOptions) -> ProgressionMode {
    opts.override_progression_mode
        .unwrap_or(ProgressionMode::Standard)
}

/// Normalize external achievement/trophy identifiers to valid `[a-z0-9][a-z0-9._-]{1,127}` IDs.
pub(crate) fn normalize_canonical_id(raw: &str, prefix_if_short: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 8);
    let mut prev_sep = false;
    for ch in raw.trim().chars() {
        let lower = ch.to_ascii_lowercase();
        if matches!(lower, 'a'..='z' | '0'..='9') {
            out.push(lower);
            prev_sep = false;
        } else if matches!(lower, '.' | '_' | '-') {
            if !out.is_empty() && !prev_sep {
                out.push(lower);
                prev_sep = true;
            }
        } else if !out.is_empty() && !prev_sep {
            out.push('_');
            prev_sep = true;
        }
    }
    while out.ends_with(['.', '_', '-']) {
        out.pop();
    }
    if out.is_empty() {
        out = format!("{}_unknown", prefix_if_short);
    } else if out.len() < 2 {
        out = format!("{}_{}", prefix_if_short, out);
    }
    if out.len() > 128 {
        out.truncate(128);
        while out.ends_with(['.', '_', '-']) && out.len() > 2 {
            out.pop();
        }
    }
    out
}

/// Convert JSON timestamp (epoch secs/ms or RFC3339 string) to `Option<String>`.
/// Missing, `0`, empty, `"null"`, or invalid timestamps return `None` — timestamps are NEVER invented.
pub(crate) fn parse_optional_timestamp(val: &serde_json::Value) -> Option<String> {
    match val {
        serde_json::Value::Null => None,
        serde_json::Value::Number(n) => {
            let v = n.as_i64()?;
            if v <= 0 {
                return None;
            }
            let secs = if v > 10_000_000_000 { v / 1000 } else { v };
            Some(format_rfc3339_secs(secs))
        }
        serde_json::Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null") || trimmed == "0" {
                return None;
            }
            if let Ok(v) = trimmed.parse::<i64>() {
                if v <= 0 {
                    return None;
                }
                let secs = if v > 10_000_000_000 { v / 1000 } else { v };
                return Some(format_rfc3339_secs(secs));
            }
            // Accept RFC 3339 timestamps (basic structural check: YYYY-MM-DDTHH:MM:SS...)
            if trimmed.len() >= 20
                && trimmed.as_bytes()[4] == b'-'
                && trimmed.as_bytes()[7] == b'-'
                && matches!(trimmed.as_bytes()[10], b'T' | b't' | b' ')
            {
                return Some(trimmed.to_string());
            }
            None
        }
        _ => None,
    }
}

pub fn validate_observation_record(rec: &ObservationRecord) -> Result<(), String> {
    if rec.schema_version != "0.1.0" {
        return Err(format!(
            "unsupported observation schema_version '{}'",
            rec.schema_version
        ));
    }
    if !is_valid_uuid(&rec.observation_id) {
        return Err(format!("invalid observation_id UUID '{}'", rec.observation_id));
    }
    if !is_valid_canonical_id(&rec.canonical_game_id) {
        return Err(format!(
            "invalid canonical_game_id '{}'",
            rec.canonical_game_id
        ));
    }
    if rec.observer_id.trim().is_empty() || rec.observer_id.len() > 128 {
        return Err("observer_id must be 1..=128 chars".into());
    }
    if !VALID_SOURCE_TYPES.contains(&rec.source_type.as_str()) {
        return Err(format!("invalid source_type '{}'", rec.source_type));
    }
    if !VALID_RECORD_KINDS.contains(&rec.record_kind.as_str()) {
        return Err(format!("invalid record_kind '{}'", rec.record_kind));
    }
    if rec.key.trim().is_empty() || rec.key.len() > 128 {
        return Err("observation key must be 1..=128 chars".into());
    }
    // Critical Plane C provenance invariant:
    if !rec.evidence_class.is_valid_plane_c() {
        return Err(format!(
            "Plane C evidence_class '{}' forbidden (must be PLATFORM_CONFIRMED, LOCALLY_OBSERVED, or INFERRED)",
            rec.evidence_class
        ));
    }
    if let Some(hash) = &rec.artifact_hash_sha256 {
        if hash.len() != 64 || !hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err("artifact_hash_sha256 must be 64 lowercase hex chars".into());
        }
    }
    Ok(())
}

pub fn parse_observation_json_str(json_str: &str) -> Result<Vec<ObservationRecord>, String> {
    let val: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| format!("invalid observation JSON: {}", e))?;
    let records: Vec<ObservationRecord> = match val {
        serde_json::Value::Array(_) => serde_json::from_value(val)
            .map_err(|e| format!("invalid observation array: {}", e))?,
        serde_json::Value::Object(_) => {
            let single: ObservationRecord = serde_json::from_value(val)
                .map_err(|e| format!("invalid observation object: {}", e))?;
            vec![single]
        }
        _ => return Err("observation payload must be JSON object or array".into()),
    };
    if records.is_empty() {
        return Err("observation payload contains zero records".into());
    }
    for r in &records {
        validate_observation_record(r)?;
    }
    Ok(records)
}

/// Import validated Plane C observation records directly into `observation_journal`
/// and outbox `event_journal` without ever passing through runtime evaluators.
pub fn import_observation_records(
    conn: &mut Connection,
    records: &[ObservationRecord],
) -> Result<ObservationImportSummary, String> {
    for r in records {
        validate_observation_record(r)?;
    }

    let now = now_unix_secs();
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| format!("failed to open observation tx: {}", e))?;

    let mut imported = 0usize;
    let mut deduplicated = 0usize;
    let mut ids = Vec::new();

    for rec in records {
        let raw_json = serde_json::to_string(rec).map_err(|e| e.to_string())?;
        let rows = tx
            .execute(
                "INSERT OR IGNORE INTO observation_journal (
                    observation_id, canonical_game_id, observer_id, source_type,
                    observed_at, source_timestamp, artifact_hash_sha256,
                    record_kind, key, unlocked, int_value, float_value, string_value,
                    progression_mode, trust_tier, evidence_class, verification_state,
                    provenance_notes, raw_json, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
                params![
                    rec.observation_id,
                    rec.canonical_game_id,
                    rec.observer_id,
                    rec.source_type,
                    rec.observed_at,
                    rec.source_timestamp,
                    rec.artifact_hash_sha256,
                    rec.record_kind,
                    rec.key,
                    rec.unlocked.map(|b| if b { 1i64 } else { 0i64 }),
                    rec.int_value,
                    rec.float_value,
                    rec.string_value,
                    rec.progression_mode.as_str(),
                    rec.trust_tier.as_str(),
                    rec.evidence_class.as_str(),
                    rec.verification_state.as_str(),
                    rec.provenance_notes,
                    raw_json,
                    now
                ],
            )
            .map_err(|e| format!("failed inserting observation_journal: {}", e))?;

        if rows == 0 {
            deduplicated += 1;
            continue;
        }
        imported += 1;
        ids.push(rec.observation_id.clone());

        // Queue OBSERVATION_IMPORT in event_journal outbox with strict provenance.
        // For ACHIEVEMENT_EVIDENCE, platform treats int_value != 0 as unlock when matching a pack key.
        let seq = 1i64;
        let idemp = format!("obs:{}:{}", rec.observation_id, seq);
        let occurred_at = rec
            .source_timestamp
            .as_deref()
            .unwrap_or(&rec.observed_at)
            .to_string();
        let outbox_int = rec.int_value.or_else(|| {
            if rec.record_kind == "ACHIEVEMENT_EVIDENCE" {
                rec.unlocked.map(|b| if b { 1 } else { 0 })
            } else {
                None
            }
        });
        let j_ev = JournalEventInput {
            producer_id: rec.observer_id.clone(),
            session_id: rec.observation_id.clone(),
            sequence_number: seq,
            idempotency_key: idemp,
            canonical_game_id: rec.canonical_game_id.clone(),
            event_kind: EventKind::ObservationImport,
            key: rec.key.clone(),
            int_value: outbox_int,
            float_value: rec.float_value,
            string_value: rec.string_value.clone(),
            occurred_at: occurred_at.clone(),
            progression_mode: rec.progression_mode,
            trust_tier: rec.trust_tier,
            evidence_class: rec.evidence_class,
            verification_state: rec.verification_state,
            raw_payload: raw_json,
        };
        let _ = insert_journal_event_tx(&tx, &j_ev)?;

        // Materialize locally ONLY if not SPECTATOR
        if rec.progression_mode != ProgressionMode::Spectator {
            if rec.record_kind == "ACHIEVEMENT_EVIDENCE" && rec.unlocked == Some(true) {
                let _ = record_trophy_unlock_tx(
                    &tx,
                    &rec.canonical_game_id,
                    &rec.key,
                    &rec.observation_id,
                    &occurred_at,
                    rec.progression_mode,
                    rec.trust_tier,
                    rec.evidence_class,
                    rec.verification_state,
                )?;
            } else if rec.record_kind == "STAT_SNAPSHOT" || rec.record_kind == "PLAYTIME_EVIDENCE" {
                let _ = apply_stat_update_tx(
                    &tx,
                    &rec.canonical_game_id,
                    rec.progression_mode,
                    &rec.key,
                    None,
                    rec.int_value,
                    rec.float_value,
                    rec.string_value.as_deref(),
                )?;
            }
        }
    }

    tx.commit()
        .map_err(|e| format!("failed committing observation tx: {}", e))?;

    Ok(ObservationImportSummary {
        imported,
        deduplicated,
        skipped_locked: 0,
        dry_run: false,
        source_kind: "observation_records".to_string(),
        observation_ids: ids,
        records: Vec::new(),
    })
}

pub fn parse_observation_path_with_options(
    path: &Path,
    packs: Option<&PackRegistry>,
    opts: &ImportOptions,
) -> Result<ParsedObservationSet, String> {
    if !path.exists() {
        return Err(format!("observation path '{}' does not exist", path.display()));
    }

    if path.is_dir() {
        let has_trophy = [
            "TROPUSR.DAT",
            "TROP.XML",
            "TROPUSR.XML",
            "TROP.SFM",
            "TROPCONF.SFM",
            "trophies.xml",
            "TROPUSR.json",
            "trophies.json",
            "trophy.json",
        ]
        .iter()
        .any(|f| path.join(f).is_file());
        if has_trophy {
            return trophy_dir::parse_trophy_directory(path, packs, opts);
        }

        if path.join("achievements.json").is_file() || path.join("achievements.ini").is_file() {
            return goldberg::parse_goldberg_path(path, opts);
        }

        if longshot::directory_has_longshot_saves(path) {
            return longshot::parse_longshot_path(path, opts);
        }

        if let Some(reg) = packs {
            if let Some(parsed) = declarative_save::try_parse_with_packs(path, reg, opts)? {
                return Ok(parsed);
            }
        }

        // Fall back to first `*.oh-obs.json` inside directory if present
        if let Ok(rd) = fs::read_dir(path) {
            let mut obs_files: Vec<_> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && p.file_name()
                            .and_then(|s| s.to_str())
                            .map(|n| n.ends_with(".oh-obs.json"))
                            .unwrap_or(false)
                })
                .collect();
            obs_files.sort();
            if !obs_files.is_empty() {
                let mut combined = Vec::new();
                for f in obs_files {
                    let parsed = generic_export::parse_generic_or_protocol_file(&f, opts)?;
                    combined.extend(parsed.records);
                }
                return Ok(ParsedObservationSet {
                    records: combined,
                    skipped_locked: 0,
                    source_kind: "observation_directory".to_string(),
                });
            }
        }

        return Err(format!(
            "no recognized Goldberg/GSE, trophy directory, Longshot progress.sav, declarative save files, or .oh-obs.json files found in '{}'",
            path.display()
        ));
    }

    let fname = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match fname.as_str() {
        "achievements.json" | "achievements.ini" => goldberg::parse_goldberg_path(path, opts),
        "progress.sav" => longshot::parse_longshot_path(path, opts),
        "tropusr.dat" | "trop.xml" | "tropusr.xml" | "trop.sfm" | "tropconf.sfm"
        | "trophies.xml" | "tropusr.json" | "trophies.json" | "trophy.json" => {
            trophy_dir::parse_trophy_file(path, packs, opts)
        }
        _ => {
            if let Some(reg) = packs {
                if let Some(parsed) = declarative_save::try_parse_with_packs(path, reg, opts)? {
                    return Ok(parsed);
                }
            }
            generic_export::parse_generic_or_protocol_file(path, opts)
        }
    }
}

pub fn import_observation_path_with_options(
    conn: &mut Connection,
    path: &Path,
    packs: Option<&PackRegistry>,
    opts: &ImportOptions,
) -> Result<ObservationImportSummary, String> {
    let parsed = parse_observation_path_with_options(path, packs, opts)?;
    if opts.dry_run {
        let ids = parsed
            .records
            .iter()
            .map(|r| r.observation_id.clone())
            .collect();
        return Ok(ObservationImportSummary {
            imported: 0,
            deduplicated: 0,
            skipped_locked: parsed.skipped_locked,
            dry_run: true,
            source_kind: parsed.source_kind,
            observation_ids: ids,
            records: parsed.records,
        });
    }

    let mut summary = import_observation_records(conn, &parsed.records)?;
    summary.skipped_locked = parsed.skipped_locked;
    summary.source_kind = parsed.source_kind;
    summary.records = parsed.records;
    Ok(summary)
}

pub fn import_observation_file(
    conn: &mut Connection,
    path: &Path,
) -> Result<ObservationImportSummary, String> {
    import_observation_path_with_options(conn, path, None, &ImportOptions::default())
}

pub fn list_observation_history(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<ObservationHistoryRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT observation_id, canonical_game_id, observer_id, source_type,
                    record_kind, key, unlocked, int_value, source_timestamp, observed_at,
                    progression_mode, trust_tier, evidence_class, verification_state,
                    artifact_hash_sha256
             FROM observation_journal
             ORDER BY created_at DESC, rowid DESC
             LIMIT ?1",
        )
        .map_err(|e| format!("failed preparing observation_journal query: {}", e))?;

    let rows = stmt
        .query_map(params![limit.max(1) as i64], |r| {
            let unlocked_int: Option<i64> = r.get(6)?;
            Ok(ObservationHistoryRow {
                observation_id: r.get(0)?,
                canonical_game_id: r.get(1)?,
                observer_id: r.get(2)?,
                source_type: r.get(3)?,
                record_kind: r.get(4)?,
                key: r.get(5)?,
                unlocked: unlocked_int.map(|v| v != 0),
                int_value: r.get(7)?,
                source_timestamp: r.get(8)?,
                observed_at: r.get(9)?,
                progression_mode: r.get(10)?,
                trust_tier: r.get(11)?,
                evidence_class: r.get(12)?,
                verification_state: r.get(13)?,
                artifact_hash_sha256: r.get(14)?,
            })
        })
        .map_err(|e| format!("failed querying observation_journal: {}", e))?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("observation_journal decode error: {}", e))?);
    }
    Ok(out)
}

/// Passive save-file JSON importer helper for Plane C (`LOCALLY_OBSERVED`, `LOCAL`, `UNVERIFIED`).
pub fn build_save_file_observation(
    canonical_game_id: &str,
    observer_id: &str,
    save_path: &Path,
    achievement_key: &str,
    unlocked: bool,
) -> Result<ObservationRecord, String> {
    let bytes = fs::read(save_path)
        .map_err(|e| format!("failed to read save file '{}': {}", save_path.display(), e))?;
    let artifact_hash = sha256_hex(&bytes);
    let key = normalize_canonical_id(achievement_key, "ach");
    let seed = format!(
        "save_file:{}:{}:{}:{}",
        canonical_game_id, observer_id, key, artifact_hash
    );
    Ok(ObservationRecord {
        schema_version: "0.1.0".to_string(),
        observation_id: deterministic_uuid_from_seed(&seed),
        canonical_game_id: canonical_game_id.to_string(),
        observer_id: observer_id.to_string(),
        source_type: "save_file".to_string(),
        observed_at: now_rfc3339(),
        source_timestamp: None,
        artifact_hash_sha256: Some(artifact_hash),
        record_kind: "ACHIEVEMENT_EVIDENCE".to_string(),
        key,
        unlocked: Some(unlocked),
        int_value: Some(if unlocked { 1 } else { 0 }),
        float_value: None,
        string_value: None,
        progression_mode: ProgressionMode::Standard,
        trust_tier: TrustTier::Local,
        evidence_class: EvidenceClass::LocallyObserved,
        verification_state: VerificationState::Unverified,
        provenance_notes: Some(format!("Observed from local save file '{}'", save_path.display())),
    })
}
