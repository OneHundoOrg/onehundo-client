//! Plane C External Importers.
//! INVARIANT: Strictly isolated from `daemon/evaluation/`.
//! External observations NEVER route through Plane A or Plane B gameplay evaluators,
//! and MUST carry provenance (`PLATFORM_CONFIRMED`, `LOCALLY_OBSERVED`, `INFERRED`).

use crate::core::types::{
    is_valid_canonical_id, is_valid_uuid, now_rfc3339, now_unix_secs, EventKind, EvidenceClass,
    ProgressionMode, TrustTier, VerificationState,
};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservationImportSummary {
    pub imported: usize,
    pub deduplicated: usize,
    pub observation_ids: Vec<String>,
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
/// and outbox `event_journal` without ever passing through `evaluation/`.
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

    for (idx, rec) in records.iter().enumerate() {
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

        // Queue OBSERVATION_IMPORT in event_journal outbox with strict provenance
        let seq = (idx as i64) + 1;
        let idemp = format!("obs:{}:{}", rec.observation_id, seq);
        let j_ev = JournalEventInput {
            producer_id: rec.observer_id.clone(),
            session_id: rec.observation_id.clone(),
            sequence_number: seq,
            idempotency_key: idemp,
            canonical_game_id: rec.canonical_game_id.clone(),
            event_kind: EventKind::ObservationImport,
            key: rec.key.clone(),
            int_value: rec.int_value,
            float_value: rec.float_value,
            string_value: rec.string_value.clone(),
            occurred_at: rec.observed_at.clone(),
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
                    &rec.observed_at,
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
        observation_ids: ids,
    })
}

pub fn import_observation_file(
    conn: &mut Connection,
    path: &Path,
) -> Result<ObservationImportSummary, String> {
    let raw = fs::read_to_string(path)
        .map_err(|e| format!("failed reading observation file '{}': {}", path.display(), e))?;
    let records = parse_observation_json_str(&raw)?;
    import_observation_records(conn, &records)
}

/// Pure-Rust SHA-256 helper for save-file artifact hashing in Plane C importers.
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let bit_len = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = String::with_capacity(64);
    for word in h {
        out.push_str(&format!("{:08x}", word));
    }
    out
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
    Ok(ObservationRecord {
        schema_version: "0.1.0".to_string(),
        observation_id: crate::core::config::generate_uuid_v4(),
        canonical_game_id: canonical_game_id.to_string(),
        observer_id: observer_id.to_string(),
        source_type: "save_file".to_string(),
        observed_at: now_rfc3339(),
        source_timestamp: None,
        artifact_hash_sha256: Some(sha256_hex(&bytes)),
        record_kind: "ACHIEVEMENT_EVIDENCE".to_string(),
        key: achievement_key.to_string(),
        unlocked: Some(unlocked),
        int_value: None,
        float_value: None,
        string_value: None,
        progression_mode: ProgressionMode::Standard,
        trust_tier: TrustTier::Local,
        evidence_class: EvidenceClass::LocallyObserved,
        verification_state: VerificationState::Unverified,
        provenance_notes: Some(format!("Observed from local save file '{}'", save_path.display())),
    })
}
