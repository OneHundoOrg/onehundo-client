//! Generic OneHundo Observation JSON (`observation-record.schema.json`) and
//! generic local save-export JSON importer.
//! Strictly isolated from gameplay evaluators; generic save exports emit
//! `LOCALLY_OBSERVED` / `LOCAL` / `UNVERIFIED` provenance and never invent timestamps.

use super::{
    normalize_canonical_id, parse_observation_json_str, parse_optional_timestamp, resolve_mode,
    validate_observation_record, ImportOptions, ObservationRecord, ParsedObservationSet,
};
use crate::core::types::{
    deterministic_uuid_from_seed, is_valid_canonical_id, now_rfc3339, sha256_hex, EvidenceClass,
    TrustTier, VerificationState,
};
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct GenericAchievementExport {
    #[serde(alias = "canonical_achievement_id", alias = "achievement_id", alias = "key")]
    id: String,
    #[serde(default = "default_true")]
    unlocked: bool,
    #[serde(default, alias = "source_timestamp", alias = "timestamp", alias = "earned_at")]
    unlocked_at: Option<serde_json::Value>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct GenericStatExport {
    key: String,
    #[serde(default)]
    int_value: Option<i64>,
    #[serde(default)]
    float_value: Option<f64>,
    #[serde(default)]
    string_value: Option<String>,
    #[serde(default, alias = "timestamp")]
    source_timestamp: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct GenericSaveExportDoc {
    canonical_game_id: String,
    #[serde(default)]
    observer_id: Option<String>,
    #[serde(default)]
    source_type: Option<String>,
    #[serde(default)]
    achievements: Vec<serde_json::Value>,
    #[serde(default)]
    stats: Vec<GenericStatExport>,
}

pub fn parse_generic_or_protocol_file(
    path: &Path,
    opts: &ImportOptions,
) -> Result<ParsedObservationSet, String> {
    let bytes = fs::read(path)
        .map_err(|e| format!("failed reading observation file '{}': {}", path.display(), e))?;
    let artifact_hash = sha256_hex(&bytes);
    let raw = String::from_utf8(bytes)
        .map_err(|_| format!("observation file '{}' is not valid UTF-8", path.display()))?;

    // 1. Try canonical protocol ObservationRecord JSON (object or array)
    if let Ok(mut records) = parse_observation_json_str(&raw) {
        for r in &mut records {
            if let Some(gid) = opts.canonical_game_id.as_deref().filter(|s| !s.is_empty()) {
                r.canonical_game_id = gid.to_string();
            }
            if let Some(mode) = opts.override_progression_mode {
                r.progression_mode = mode;
            }
            if r.artifact_hash_sha256.is_none() {
                r.artifact_hash_sha256 = Some(artifact_hash.clone());
            }
            validate_observation_record(r)?;
        }
        return Ok(ParsedObservationSet {
            records,
            skipped_locked: 0,
            source_kind: "observation_record".to_string(),
        });
    }

    // 2. Try newline-delimited ObservationRecord JSON (.jsonl / .ndjson)
    if path
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| e.eq_ignore_ascii_case("jsonl") || e.eq_ignore_ascii_case("ndjson"))
        .unwrap_or(false)
    {
        let mut records = Vec::new();
        for (idx, line) in raw.lines().enumerate() {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            let mut single: ObservationRecord = serde_json::from_str(t)
                .map_err(|e| format!("invalid JSONL observation line {}: {}", idx + 1, e))?;
            if let Some(gid) = opts.canonical_game_id.as_deref().filter(|s| !s.is_empty()) {
                single.canonical_game_id = gid.to_string();
            }
            if let Some(mode) = opts.override_progression_mode {
                single.progression_mode = mode;
            }
            if single.artifact_hash_sha256.is_none() {
                single.artifact_hash_sha256 = Some(artifact_hash.clone());
            }
            validate_observation_record(&single)?;
            records.push(single);
        }
        if !records.is_empty() {
            return Ok(ParsedObservationSet {
                records,
                skipped_locked: 0,
                source_kind: "observation_jsonl".to_string(),
            });
        }
    }

    // 3. Try generic local save-export document `{ canonical_game_id, achievements, stats }`
    let doc: GenericSaveExportDoc = serde_json::from_str(&raw).map_err(|e| {
        format!(
            "file '{}' is neither an observation-record JSON nor a generic save-export JSON: {}",
            path.display(),
            e
        )
    })?;

    let game_id = opts
        .canonical_game_id
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or(doc.canonical_game_id);
    if !is_valid_canonical_id(&game_id) {
        return Err(format!("invalid canonical_game_id '{}'", game_id));
    }

    let source_type = doc
        .source_type
        .unwrap_or_else(|| "save_file".to_string());
    let observer_id = doc
        .observer_id
        .unwrap_or_else(|| "plane_c.generic_save_export".to_string());
    let mode = resolve_mode(opts);

    let mut records = Vec::new();
    let mut skipped_locked = 0usize;

    for item in doc.achievements {
        let parsed: GenericAchievementExport = match item {
            serde_json::Value::String(id) => GenericAchievementExport {
                id,
                unlocked: true,
                unlocked_at: None,
            },
            other => serde_json::from_value(other)
                .map_err(|e| format!("invalid achievement entry in save export: {}", e))?,
        };
        if !parsed.unlocked {
            skipped_locked += 1;
            continue;
        }
        let key = normalize_canonical_id(&parsed.id, "ach");
        let ts = parsed.unlocked_at.as_ref().and_then(parse_optional_timestamp);
        let seed = format!(
            "save_export:{}:{}:{}:{}",
            game_id,
            mode.as_str(),
            key,
            parsed.id.trim()
        );
        let rec = ObservationRecord {
            schema_version: "0.1.0".to_string(),
            observation_id: deterministic_uuid_from_seed(&seed),
            canonical_game_id: game_id.clone(),
            observer_id: observer_id.clone(),
            source_type: source_type.clone(),
            observed_at: now_rfc3339(),
            source_timestamp: ts,
            artifact_hash_sha256: Some(artifact_hash.clone()),
            record_kind: "ACHIEVEMENT_EVIDENCE".to_string(),
            key,
            unlocked: Some(true),
            int_value: Some(1),
            float_value: None,
            string_value: None,
            progression_mode: mode,
            trust_tier: TrustTier::Local,
            evidence_class: EvidenceClass::LocallyObserved,
            verification_state: VerificationState::Unverified,
            provenance_notes: Some(format!(
                "Observed from local save export '{}'",
                path.display()
            )),
        };
        validate_observation_record(&rec)?;
        records.push(rec);
    }

    for st in doc.stats {
        let key = normalize_canonical_id(&st.key, "stat");
        let ts = st
            .source_timestamp
            .as_ref()
            .and_then(parse_optional_timestamp);
        let seed = format!(
            "save_export_stat:{}:{}:{}:{:?}:{:?}:{:?}",
            game_id,
            mode.as_str(),
            key,
            st.int_value,
            st.float_value,
            st.string_value
        );
        let rec = ObservationRecord {
            schema_version: "0.1.0".to_string(),
            observation_id: deterministic_uuid_from_seed(&seed),
            canonical_game_id: game_id.clone(),
            observer_id: observer_id.clone(),
            source_type: source_type.clone(),
            observed_at: now_rfc3339(),
            source_timestamp: ts,
            artifact_hash_sha256: Some(artifact_hash.clone()),
            record_kind: "STAT_SNAPSHOT".to_string(),
            key,
            unlocked: None,
            int_value: st.int_value,
            float_value: st.float_value,
            string_value: st.string_value,
            progression_mode: mode,
            trust_tier: TrustTier::Local,
            evidence_class: EvidenceClass::LocallyObserved,
            verification_state: VerificationState::Unverified,
            provenance_notes: Some(format!(
                "Observed stat snapshot from '{}'",
                path.display()
            )),
        };
        validate_observation_record(&rec)?;
        records.push(rec);
    }

    Ok(ParsedObservationSet {
        records,
        skipped_locked,
        source_kind: "generic_save_export".to_string(),
    })
}
