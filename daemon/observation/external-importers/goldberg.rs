//! Goldberg / GSE (`achievements.json` / `achievements.ini`) Plane C importer.
//! Strictly isolated from gameplay evaluators; all emitted records carry
//! `LOCALLY_OBSERVED` / `LOCAL` / `UNVERIFIED` provenance and never invent timestamps.

use super::{
    normalize_canonical_id, parse_optional_timestamp, resolve_mode, ImportOptions,
    ObservationRecord, ParsedObservationSet,
};
use crate::core::types::{
    deterministic_uuid_from_seed, now_rfc3339, sha256_hex, EvidenceClass, TrustTier,
    VerificationState,
};
use std::fs;
use std::path::Path;

fn infer_game_id_near(path: &Path, override_id: Option<&str>) -> String {
    if let Some(id) = override_id.map(str::trim).filter(|s| !s.is_empty()) {
        return id.to_string();
    }
    let dir = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };

    for candidate in [
        dir.join("steam_appid.txt"),
        dir.join("steam_settings").join("steam_appid.txt"),
        dir.parent()
            .map(|p| p.join("steam_appid.txt"))
            .unwrap_or_else(|| dir.join("steam_appid.txt")),
    ] {
        if let Ok(txt) = fs::read_to_string(&candidate) {
            let trimmed = txt.trim();
            if !trimmed.is_empty() && trimmed.bytes().all(|b| b.is_ascii_digit()) {
                return format!("steam.{}", trimmed);
            }
        }
    }

    // Check ancestor folder names for a Steam numeric AppID (e.g. `GSE Saves/<appid>/achievements.json`)
    for ancestor in dir.ancestors().take(3) {
        if let Some(name) = ancestor.file_name().and_then(|s| s.to_str()) {
            if name.len() >= 3 && name.len() <= 10 && name.bytes().all(|b| b.is_ascii_digit()) {
                return format!("steam.{}", name);
            }
        }
    }

    "steam.goldberg.local".to_string()
}

fn build_goldberg_record(
    canonical_game_id: &str,
    raw_ach_id: &str,
    source_timestamp: Option<String>,
    artifact_hash: &str,
    source_label: &str,
    opts: &ImportOptions,
) -> ObservationRecord {
    let key = normalize_canonical_id(raw_ach_id, "ach");
    let mode = resolve_mode(opts);
    let seed = format!(
        "goldberg:{}:{}:{}:{}",
        canonical_game_id,
        mode.as_str(),
        key,
        raw_ach_id.trim()
    );
    ObservationRecord {
        schema_version: "0.1.0".to_string(),
        observation_id: deterministic_uuid_from_seed(&seed),
        canonical_game_id: canonical_game_id.to_string(),
        observer_id: "plane_c.goldberg_gse".to_string(),
        source_type: "steam_api".to_string(),
        observed_at: now_rfc3339(),
        source_timestamp,
        artifact_hash_sha256: Some(artifact_hash.to_string()),
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
            "Observed from Goldberg/GSE '{}' (raw_id='{}')",
            source_label, raw_ach_id
        )),
    }
}

fn value_is_truthy(val: Option<&serde_json::Value>) -> bool {
    match val {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
        Some(serde_json::Value::String(s)) => {
            matches!(s.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes")
        }
        _ => false,
    }
}

fn extract_timestamp_from_obj(obj: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    for field in [
        "earned_time",
        "unlock_time",
        "UnlockTime",
        "unlocked_at",
        "earned_at",
        "timestamp",
        "time_unlocked",
    ] {
        if let Some(v) = obj.get(field) {
            if let Some(ts) = parse_optional_timestamp(v) {
                return Some(ts);
            }
        }
    }
    None
}

pub fn parse_goldberg_json_str(
    raw: &str,
    source_path: &Path,
    artifact_hash: &str,
    opts: &ImportOptions,
) -> Result<ParsedObservationSet, String> {
    let root: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| format!("invalid Goldberg JSON '{}': {}", source_path.display(), e))?;

    let mut game_id_override = opts.canonical_game_id.clone();
    let mut records = Vec::new();
    let mut skipped_locked = 0usize;

    let payload = match &root {
        serde_json::Value::Object(map) => {
            if game_id_override.is_none() {
                if let Some(gid) = map.get("canonical_game_id").and_then(|v| v.as_str()) {
                    game_id_override = Some(gid.to_string());
                } else if let Some(appid) = map.get("app_id").and_then(|v| v.as_u64()) {
                    game_id_override = Some(format!("steam.{}", appid));
                }
            }
            map.get("achievements").unwrap_or(&root)
        }
        _ => &root,
    };

    let canonical_game_id = infer_game_id_near(source_path, game_id_override.as_deref());
    let label = source_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("achievements.json");

    match payload {
        serde_json::Value::Object(entries) => {
            let mut keys: Vec<&String> = entries.keys().collect();
            keys.sort();
            for ach_id in keys {
                let Some(entry_val) = entries.get(ach_id) else {
                    continue;
                };
                match entry_val {
                    serde_json::Value::Object(obj) => {
                        let earned = value_is_truthy(
                            obj.get("earned")
                                .or_else(|| obj.get("achieved"))
                                .or_else(|| obj.get("unlocked"))
                                .or_else(|| obj.get("Unlocked")),
                        );
                        if !earned {
                            skipped_locked += 1;
                            continue;
                        }
                        let ts = extract_timestamp_from_obj(obj);
                        records.push(build_goldberg_record(
                            &canonical_game_id,
                            ach_id,
                            ts,
                            artifact_hash,
                            label,
                            opts,
                        ));
                    }
                    serde_json::Value::Bool(b) => {
                        if !*b {
                            skipped_locked += 1;
                            continue;
                        }
                        records.push(build_goldberg_record(
                            &canonical_game_id,
                            ach_id,
                            None,
                            artifact_hash,
                            label,
                            opts,
                        ));
                    }
                    _ => {}
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                let Some(obj) = item.as_object() else {
                    continue;
                };
                let Some(ach_id) = obj
                    .get("name")
                    .or_else(|| obj.get("id"))
                    .or_else(|| obj.get("apiname"))
                    .or_else(|| obj.get("api_name"))
                    .and_then(|v| v.as_str())
                else {
                    continue;
                };
                let earned = value_is_truthy(
                    obj.get("earned")
                        .or_else(|| obj.get("achieved"))
                        .or_else(|| obj.get("unlocked"))
                        .or_else(|| obj.get("Unlocked")),
                );
                if !earned {
                    skipped_locked += 1;
                    continue;
                }
                let ts = extract_timestamp_from_obj(obj);
                records.push(build_goldberg_record(
                    &canonical_game_id,
                    ach_id,
                    ts,
                    artifact_hash,
                    label,
                    opts,
                ));
            }
        }
        _ => {
            return Err(format!(
                "Goldberg JSON '{}' must be an object or array",
                source_path.display()
            ));
        }
    }

    Ok(ParsedObservationSet {
        records,
        skipped_locked,
        source_kind: "goldberg_json".to_string(),
    })
}

pub fn parse_goldberg_ini_str(
    raw: &str,
    source_path: &Path,
    artifact_hash: &str,
    opts: &ImportOptions,
) -> Result<ParsedObservationSet, String> {
    let canonical_game_id = infer_game_id_near(source_path, opts.canonical_game_id.as_deref());
    let label = source_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("achievements.ini");

    let mut records = Vec::new();
    let mut skipped_locked = 0usize;

    let mut current_section: Option<String> = None;
    let mut unlocked: Option<bool> = None;
    let mut timestamp: Option<String> = None;

    let flush_section = |sec: Option<String>,
                         unl: Option<bool>,
                         ts: Option<String>,
                         recs: &mut Vec<ObservationRecord>,
                         skipped: &mut usize| {
        if let Some(section_name) = sec {
            let trimmed = section_name.trim();
            if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("SteamAchievements") {
                if unl == Some(true) {
                    recs.push(build_goldberg_record(
                        &canonical_game_id,
                        trimmed,
                        ts,
                        artifact_hash,
                        label,
                        opts,
                    ));
                } else if unl == Some(false) {
                    *skipped += 1;
                }
            }
        }
    };

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            flush_section(
                current_section.take(),
                unlocked.take(),
                timestamp.take(),
                &mut records,
                &mut skipped_locked,
            );
            current_section = Some(trimmed[1..trimmed.len() - 1].to_string());
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim();
            match key.as_str() {
                "achieved" | "unlocked" | "earned" | "state" => {
                    let is_true = matches!(val.to_ascii_lowercase().as_str(), "1" | "true" | "yes");
                    unlocked = Some(is_true);
                }
                "timeunlocked" | "unlocktime" | "earned_time" | "unlocked_at" | "timestamp" => {
                    let json_val = if let Ok(n) = val.parse::<i64>() {
                        serde_json::Value::Number(n.into())
                    } else {
                        serde_json::Value::String(val.to_string())
                    };
                    if let Some(ts) = parse_optional_timestamp(&json_val) {
                        timestamp = Some(ts);
                    }
                }
                _ => {}
            }
        }
    }

    flush_section(
        current_section,
        unlocked,
        timestamp,
        &mut records,
        &mut skipped_locked,
    );

    Ok(ParsedObservationSet {
        records,
        skipped_locked,
        source_kind: "goldberg_ini".to_string(),
    })
}

pub fn parse_goldberg_path(
    path: &Path,
    opts: &ImportOptions,
) -> Result<ParsedObservationSet, String> {
    let target = if path.is_dir() {
        let json = path.join("achievements.json");
        let ini = path.join("achievements.ini");
        if json.is_file() {
            json
        } else if ini.is_file() {
            ini
        } else {
            return Err(format!(
                "no Goldberg achievements.json or achievements.ini found in '{}'",
                path.display()
            ));
        }
    } else {
        path.to_path_buf()
    };

    let bytes = fs::read(&target)
        .map_err(|e| format!("failed reading Goldberg file '{}': {}", target.display(), e))?;
    let artifact_hash = sha256_hex(&bytes);
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("Goldberg file '{}' is not valid UTF-8", target.display()))?;

    let is_ini = target
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| e.eq_ignore_ascii_case("ini"))
        .unwrap_or(false);

    if is_ini {
        parse_goldberg_ini_str(&text, &target, &artifact_hash, opts)
    } else {
        parse_goldberg_json_str(&text, &target, &artifact_hash, opts)
    }
}
