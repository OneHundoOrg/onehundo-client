//! Declarative Plane C Save-File & Emulator SRAM Importer.
//! Parses pack-declared save watchers (`rules/save.json`: binary offsets/bitflags,
//! JSON dot-paths, INI/key-value pairs) with bounded reads and safe path containment.
//! Strictly emits `LOCALLY_OBSERVED` / `LOCAL` / `UNVERIFIED` provenance and
//! preserves missing unlock timestamps as `None` (`NULL`).

use super::{
    normalize_canonical_id, resolve_mode, validate_observation_record, ImportOptions,
    ObservationRecord, ParsedObservationSet,
};
use crate::core::types::{
    deterministic_uuid_from_seed, now_rfc3339, sha256_hex, EvidenceClass, TrustTier,
    VerificationState,
};
use crate::packs::manifest::{
    LoadedPack, SaveConditionRule, SaveStatRule, SaveWatcherRule,
};
use crate::packs::matcher::{glob_matches, MatchQuery, PackRegistry};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_DEFAULT_SAVE_BYTES: usize = 32 * 1024 * 1024;
const MAX_SCAN_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq)]
enum ScalarVal {
    Int(i64),
    Uint(u64),
    Float(f64),
    Bool(bool),
    Text(String),
}

/// Canonicalize `candidate` and guarantee it remains inside `root` (blocks `..` and symlink escapes).
pub fn safe_contained_path(root: &Path, candidate: &Path) -> Result<PathBuf, String> {
    let canon_root = fs::canonicalize(root)
        .map_err(|e| format!("failed canonicalizing watch root '{}': {}", root.display(), e))?;
    let canon_cand = fs::canonicalize(candidate)
        .map_err(|e| format!("failed canonicalizing path '{}': {}", candidate.display(), e))?;
    if !canon_cand.starts_with(&canon_root) {
        return Err(format!(
            "refusing path '{}' escaping watch root '{}'",
            candidate.display(),
            root.display()
        ));
    }
    Ok(canon_cand)
}

fn read_bounded_bytes(path: &Path, max_bytes: usize) -> Result<Vec<u8>, String> {
    let cap = max_bytes.clamp(1, MAX_DEFAULT_SAVE_BYTES);
    let mut file = File::open(path)
        .map_err(|e| format!("failed opening save file '{}': {}", path.display(), e))?;
    let mut buf = Vec::new();
    let read = file
        .by_ref()
        .take((cap + 1) as u64)
        .read_to_end(&mut buf)
        .map_err(|e| format!("failed reading save file '{}': {}", path.display(), e))?;
    if read > cap {
        return Err(format!(
            "save file '{}' exceeds bounded read limit ({} bytes)",
            path.display(),
            cap
        ));
    }
    Ok(buf)
}

fn decode_hex_bytes(hex: &str) -> Option<Vec<u8>> {
    let clean = hex.trim();
    if clean.len() % 2 != 0 || clean.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(clean.len() / 2);
    let bytes = clean.as_bytes();
    for i in (0..bytes.len()).step_by(2) {
        let pair = std::str::from_utf8(&bytes[i..i + 2]).ok()?;
        let val = u8::from_str_radix(pair, 16).ok()?;
        out.push(val);
    }
    Some(out)
}

fn check_magic(bytes: &[u8], watcher: &SaveWatcherRule) -> bool {
    let off = watcher.magic_offset.unwrap_or(0) as usize;
    if let Some(hex) = &watcher.magic_hex {
        let Some(expected) = decode_hex_bytes(hex) else {
            return false;
        };
        if off.saturating_add(expected.len()) > bytes.len() {
            return false;
        }
        if &bytes[off..off + expected.len()] != expected.as_slice() {
            return false;
        }
    }
    if let Some(ascii) = &watcher.magic_ascii {
        let expected = ascii.as_bytes();
        if off.saturating_add(expected.len()) > bytes.len() {
            return false;
        }
        if &bytes[off..off + expected.len()] != expected {
            return false;
        }
    }
    true
}

fn extract_binary_scalar(
    bytes: &[u8],
    offset: usize,
    value_type: &str,
    endianness: &str,
) -> Option<ScalarVal> {
    let big = endianness.eq_ignore_ascii_case("BIG");
    match value_type {
        "u8" => bytes.get(offset).map(|&b| ScalarVal::Uint(b as u64)),
        "i8" => bytes.get(offset).map(|&b| ScalarVal::Int(b as i8 as i64)),
        "bool" => bytes.get(offset).map(|&b| ScalarVal::Bool(b != 0)),
        "u16" => {
            let s: [u8; 2] = bytes.get(offset..offset + 2)?.try_into().ok()?;
            Some(ScalarVal::Uint(if big {
                u16::from_be_bytes(s)
            } else {
                u16::from_le_bytes(s)
            } as u64))
        }
        "i16" => {
            let s: [u8; 2] = bytes.get(offset..offset + 2)?.try_into().ok()?;
            Some(ScalarVal::Int(if big {
                i16::from_be_bytes(s)
            } else {
                i16::from_le_bytes(s)
            } as i64))
        }
        "u32" => {
            let s: [u8; 4] = bytes.get(offset..offset + 4)?.try_into().ok()?;
            Some(ScalarVal::Uint(if big {
                u32::from_be_bytes(s)
            } else {
                u32::from_le_bytes(s)
            } as u64))
        }
        "i32" => {
            let s: [u8; 4] = bytes.get(offset..offset + 4)?.try_into().ok()?;
            Some(ScalarVal::Int(if big {
                i32::from_be_bytes(s)
            } else {
                i32::from_le_bytes(s)
            } as i64))
        }
        "u64" | "bitmask" => {
            if offset + 8 <= bytes.len() {
                let s: [u8; 8] = bytes.get(offset..offset + 8)?.try_into().ok()?;
                Some(ScalarVal::Uint(if big {
                    u64::from_be_bytes(s)
                } else {
                    u64::from_le_bytes(s)
                }))
            } else if value_type == "bitmask" && offset + 4 <= bytes.len() {
                let s: [u8; 4] = bytes.get(offset..offset + 4)?.try_into().ok()?;
                Some(ScalarVal::Uint(if big {
                    u32::from_be_bytes(s)
                } else {
                    u32::from_le_bytes(s)
                } as u64))
            } else if value_type == "bitmask" && offset < bytes.len() {
                Some(ScalarVal::Uint(bytes[offset] as u64))
            } else {
                None
            }
        }
        "i64" => {
            let s: [u8; 8] = bytes.get(offset..offset + 8)?.try_into().ok()?;
            Some(ScalarVal::Int(if big {
                i64::from_be_bytes(s)
            } else {
                i64::from_le_bytes(s)
            }))
        }
        "f32" => {
            let s: [u8; 4] = bytes.get(offset..offset + 4)?.try_into().ok()?;
            Some(ScalarVal::Float(if big {
                f32::from_be_bytes(s)
            } else {
                f32::from_le_bytes(s)
            } as f64))
        }
        "f64" => {
            let s: [u8; 8] = bytes.get(offset..offset + 8)?.try_into().ok()?;
            Some(ScalarVal::Float(if big {
                f64::from_be_bytes(s)
            } else {
                f64::from_le_bytes(s)
            }))
        }
        "string" => {
            let end = bytes.len().min(offset.saturating_add(256));
            let slice = bytes.get(offset..end)?;
            let nul_pos = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
            Some(ScalarVal::Text(
                String::from_utf8_lossy(&slice[..nul_pos]).to_string(),
            ))
        }
        _ => None,
    }
}

fn lookup_json_path<'a>(root: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut cur = root;
    for raw_seg in path.split('.') {
        if raw_seg.is_empty() {
            return None;
        }
        if let Some(bracket_idx) = raw_seg.find('[') {
            let key = &raw_seg[..bracket_idx];
            if !key.is_empty() {
                cur = cur.get(key)?;
            }
            let mut rest = &raw_seg[bracket_idx..];
            while let Some(inside) = rest.strip_prefix('[') {
                let end = inside.find(']')?;
                let idx: usize = inside[..end].parse().ok()?;
                cur = cur.get(idx)?;
                rest = &inside[end + 1..];
            }
        } else {
            cur = cur.get(raw_seg)?;
        }
    }
    Some(cur)
}

fn json_to_scalar(val: &serde_json::Value, value_type: &str) -> Option<ScalarVal> {
    match val {
        serde_json::Value::Bool(b) => {
            if value_type == "bool" {
                Some(ScalarVal::Bool(*b))
            } else {
                Some(ScalarVal::Uint(if *b { 1 } else { 0 }))
            }
        }
        serde_json::Value::Number(n) => match value_type {
            "f32" | "f64" => n.as_f64().map(ScalarVal::Float),
            "i8" | "i16" | "i32" | "i64" => n
                .as_i64()
                .map(ScalarVal::Int)
                .or_else(|| n.as_f64().map(|f| ScalarVal::Int(f as i64))),
            "bool" => n.as_i64().map(|v| ScalarVal::Bool(v != 0)),
            _ => n
                .as_u64()
                .map(ScalarVal::Uint)
                .or_else(|| n.as_i64().map(ScalarVal::Int)),
        },
        serde_json::Value::String(s) => match value_type {
            "string" => Some(ScalarVal::Text(s.clone())),
            "bool" => Some(ScalarVal::Bool(
                s.eq_ignore_ascii_case("true") || s == "1",
            )),
            "u8" | "u16" | "u32" | "u64" | "bitmask" => s.trim().parse::<u64>().ok().map(ScalarVal::Uint),
            "i8" | "i16" | "i32" | "i64" => s.trim().parse::<i64>().ok().map(ScalarVal::Int),
            "f32" | "f64" => s.trim().parse::<f64>().ok().map(ScalarVal::Float),
            _ => Some(ScalarVal::Text(s.clone())),
        },
        _ => None,
    }
}

fn parse_ini_kv(text: &str) -> HashMap<(String, String), String> {
    let mut map = HashMap::new();
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_ascii_lowercase();
            continue;
        }
        if let Some((k, v)) = line.split_once('=').or_else(|| line.split_once(':')) {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim().trim_matches('"').to_string();
            map.insert((section.clone(), key.clone()), val.clone());
            if !section.is_empty() {
                map.entry((String::new(), key)).or_insert(val);
            }
        }
    }
    map
}

fn ini_to_scalar(
    map: &HashMap<(String, String), String>,
    section: Option<&str>,
    key: &str,
    value_type: &str,
) -> Option<ScalarVal> {
    let sec = section.unwrap_or("").trim().to_ascii_lowercase();
    let k = key.trim().to_ascii_lowercase();
    let raw = map
        .get(&(sec, k.clone()))
        .or_else(|| map.get(&(String::new(), k)))?;
    match value_type {
        "bool" => Some(ScalarVal::Bool(
            raw.eq_ignore_ascii_case("true")
                || raw == "1"
                || raw.eq_ignore_ascii_case("yes"),
        )),
        "u8" | "u16" | "u32" | "u64" | "bitmask" => {
            if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
                u64::from_str_radix(hex, 16).ok().map(ScalarVal::Uint)
            } else {
                raw.parse::<u64>().ok().map(ScalarVal::Uint)
            }
        }
        "i8" | "i16" | "i32" | "i64" => raw.parse::<i64>().ok().map(ScalarVal::Int),
        "f32" | "f64" => raw.parse::<f64>().ok().map(ScalarVal::Float),
        _ => Some(ScalarVal::Text(raw.clone())),
    }
}

fn scalar_as_f64(s: &ScalarVal) -> Option<f64> {
    match s {
        ScalarVal::Uint(u) => Some(*u as f64),
        ScalarVal::Int(i) => Some(*i as f64),
        ScalarVal::Float(f) => Some(*f),
        ScalarVal::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        ScalarVal::Text(t) => t.trim().parse::<f64>().ok(),
    }
}

fn scalar_as_u64(s: &ScalarVal) -> Option<u64> {
    match s {
        ScalarVal::Uint(u) => Some(*u),
        ScalarVal::Int(i) if *i >= 0 => Some(*i as u64),
        ScalarVal::Bool(b) => Some(if *b { 1 } else { 0 }),
        ScalarVal::Float(f) if *f >= 0.0 => Some(*f as u64),
        ScalarVal::Text(t) => t.trim().parse::<u64>().ok(),
        _ => None,
    }
}

fn eval_condition(
    watcher: &SaveWatcherRule,
    bytes: &[u8],
    json_root: Option<&serde_json::Value>,
    ini_map: Option<&HashMap<(String, String), String>>,
    cond: &SaveConditionRule,
) -> bool {
    let default_endian = watcher.endianness.as_deref().unwrap_or("LITTLE");
    let endian = cond.endianness.as_deref().unwrap_or(default_endian);

    let extracted = match watcher.format.as_str() {
        "binary" => cond
            .offset
            .and_then(|off| extract_binary_scalar(bytes, off as usize, &cond.value_type, endian)),
        "json" => cond
            .path
            .as_deref()
            .and_then(|p| json_root.and_then(|r| lookup_json_path(r, p)))
            .and_then(|v| json_to_scalar(v, &cond.value_type)),
        "ini" | "kv" => cond
            .key
            .as_deref()
            .and_then(|k| ini_map.and_then(|m| ini_to_scalar(m, cond.section.as_deref(), k, &cond.value_type))),
        _ => None,
    };

    let Some(mut actual) = extracted else {
        return false;
    };

    if let Some(mask_val) = cond.mask.as_ref().and_then(|m| m.as_u64()) {
        if let Some(u) = scalar_as_u64(&actual) {
            actual = ScalarVal::Uint(u & mask_val);
        }
    }

    match cond.op.as_str() {
        "nonzero" => match &actual {
            ScalarVal::Uint(u) => *u != 0,
            ScalarVal::Int(i) => *i != 0,
            ScalarVal::Float(f) => *f != 0.0,
            ScalarVal::Bool(b) => *b,
            ScalarVal::Text(t) => !t.is_empty(),
        },
        "bit_set" => {
            let Some(bit) = cond.bit.filter(|&b| b < 64) else {
                return false;
            };
            scalar_as_u64(&actual)
                .map(|u| (u & (1u64 << bit)) != 0)
                .unwrap_or(false)
        }
        "bit_clear" => {
            let Some(bit) = cond.bit.filter(|&b| b < 64) else {
                return false;
            };
            scalar_as_u64(&actual)
                .map(|u| (u & (1u64 << bit)) == 0)
                .unwrap_or(false)
        }
        "contains" => {
            let Some(needle) = cond.value.as_ref().and_then(|v| v.as_str()) else {
                return false;
            };
            match &actual {
                ScalarVal::Text(t) => t.contains(needle),
                _ => false,
            }
        }
        "==" | "!=" | ">=" | ">" | "<=" | "<" => {
            let Some(expected) = cond.value.as_ref() else {
                return false;
            };
            if matches!(&actual, ScalarVal::Bool(_)) || expected.is_boolean() {
                let ab = match &actual {
                    ScalarVal::Bool(b) => *b,
                    ScalarVal::Uint(u) => *u != 0,
                    ScalarVal::Int(i) => *i != 0,
                    _ => return false,
                };
                let eb = expected.as_bool().unwrap_or_else(|| {
                    expected.as_i64().map(|v| v != 0).unwrap_or(false)
                });
                return match cond.op.as_str() {
                    "==" => ab == eb,
                    "!=" => ab != eb,
                    _ => false,
                };
            }
            if let ScalarVal::Text(ref at) = actual {
                if let Some(et) = expected.as_str() {
                    return match cond.op.as_str() {
                        "==" => at == et,
                        "!=" => at != et,
                        _ => false,
                    };
                }
            }
            let (Some(af), Some(ef)) = (
                scalar_as_f64(&actual),
                expected
                    .as_f64()
                    .or_else(|| expected.as_u64().map(|u| u as f64)),
            ) else {
                return false;
            };
            match cond.op.as_str() {
                "==" => (af - ef).abs() < 1e-9,
                "!=" => (af - ef).abs() >= 1e-9,
                ">=" => af >= ef,
                ">" => af > ef,
                "<=" => af <= ef,
                "<" => af < ef,
                _ => false,
            }
        }
        _ => false,
    }
}

fn extract_stat_value(
    watcher: &SaveWatcherRule,
    bytes: &[u8],
    json_root: Option<&serde_json::Value>,
    ini_map: Option<&HashMap<(String, String), String>>,
    stat: &SaveStatRule,
) -> Option<(Option<i64>, Option<f64>, Option<String>)> {
    let default_endian = watcher.endianness.as_deref().unwrap_or("LITTLE");
    let endian = stat.endianness.as_deref().unwrap_or(default_endian);

    let mut extracted = match watcher.format.as_str() {
        "binary" => stat
            .offset
            .and_then(|off| extract_binary_scalar(bytes, off as usize, &stat.value_type, endian))?,
        "json" => stat
            .path
            .as_deref()
            .and_then(|p| json_root.and_then(|r| lookup_json_path(r, p)))
            .and_then(|v| json_to_scalar(v, &stat.value_type))?,
        "ini" | "kv" => stat
            .key
            .as_deref()
            .and_then(|k| ini_map.and_then(|m| ini_to_scalar(m, stat.section.as_deref(), k, &stat.value_type)))?,
        _ => return None,
    };

    if let Some(mask_val) = stat.mask.as_ref().and_then(|m| m.as_u64()) {
        if let Some(u) = scalar_as_u64(&extracted) {
            extracted = ScalarVal::Uint(u & mask_val);
        }
    }
    if let Some(bit) = stat.bit.filter(|&b| b < 64) {
        if let Some(u) = scalar_as_u64(&extracted) {
            extracted = ScalarVal::Uint(if (u & (1u64 << bit)) != 0 { 1 } else { 0 });
        }
    }

    match extracted {
        ScalarVal::Int(i) => Some((Some(i), None, None)),
        ScalarVal::Uint(u) => Some((i64::try_from(u).ok(), None, None)),
        ScalarVal::Float(f) => Some((None, Some(f), None)),
        ScalarVal::Bool(b) => Some((Some(if b { 1 } else { 0 }), None, None)),
        ScalarVal::Text(t) => Some((None, None, Some(t))),
    }
}

pub fn evaluate_save_file_with_pack(
    pack: &LoadedPack,
    watch_root: Option<&Path>,
    save_file: &Path,
    opts: &ImportOptions,
) -> Result<ParsedObservationSet, String> {
    if pack.save_watchers.is_empty() {
        return Ok(ParsedObservationSet {
            records: Vec::new(),
            skipped_locked: 0,
            source_kind: "declarative_save".to_string(),
        });
    }

    let safe_path = if let Some(root) = watch_root {
        safe_contained_path(root, save_file)?
    } else {
        fs::canonicalize(save_file).map_err(|e| {
            format!("failed canonicalizing save file '{}': {}", save_file.display(), e)
        })?
    };

    let rel_for_glob = match watch_root {
        Some(root) => {
            let canon_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
            safe_path
                .strip_prefix(&canon_root)
                .unwrap_or(&safe_path)
                .to_string_lossy()
                .replace('\\', "/")
        }
        None => safe_path.to_string_lossy().replace('\\', "/"),
    };
    let file_name = safe_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    let game_id = opts
        .canonical_game_id
        .clone()
        .unwrap_or_else(|| pack.manifest.canonical_game_id.clone());
    let mode = resolve_mode(opts);

    let mut records = Vec::new();
    let mut skipped_locked = 0usize;
    let mut cached_bytes: Option<(usize, Vec<u8>, String)> = None;

    for watcher in &pack.save_watchers {
        let glob_hit = glob_matches(&watcher.file_glob, &rel_for_glob)
            || glob_matches(&watcher.file_glob, file_name);
        if !glob_hit {
            continue;
        }
        // Never invent unlocks for template placeholders whose byte offsets are unverified.
        if watcher.placeholder_template || watcher.format == "template" {
            continue;
        }

        let max_sz = watcher
            .max_file_size_bytes
            .unwrap_or(MAX_DEFAULT_SAVE_BYTES as u64)
            .min(MAX_DEFAULT_SAVE_BYTES as u64) as usize;

        let need_reload = cached_bytes
            .as_ref()
            .map(|(sz, _, _)| *sz < max_sz)
            .unwrap_or(true);
        if need_reload {
            let raw = read_bounded_bytes(&safe_path, max_sz)?;
            let hash = sha256_hex(&raw);
            cached_bytes = Some((max_sz, raw, hash));
        }

        let (_, bytes, artifact_hash) = cached_bytes.as_ref().unwrap();
        if !check_magic(bytes, watcher) {
            continue;
        }

        let json_root = if watcher.format == "json" {
            let Ok(val) = serde_json::from_slice::<serde_json::Value>(bytes) else {
                continue;
            };
            Some(val)
        } else {
            None
        };
        let ini_map = if watcher.format == "ini" || watcher.format == "kv" {
            let text = String::from_utf8_lossy(bytes);
            Some(parse_ini_kv(&text))
        } else {
            None
        };

        for rule in &watcher.achievement_rules {
            if rule.conditions.is_empty() {
                continue;
            }
            let passed = if rule.all_of {
                rule.conditions
                    .iter()
                    .all(|c| eval_condition(watcher, bytes, json_root.as_ref(), ini_map.as_ref(), c))
            } else {
                rule.conditions
                    .iter()
                    .any(|c| eval_condition(watcher, bytes, json_root.as_ref(), ini_map.as_ref(), c))
            };
            if !passed {
                skipped_locked += 1;
                continue;
            }

            let key = normalize_canonical_id(&rule.achievement_id, "ach");
            let observer_id = format!("decl_save_{}", watcher.id);
            let seed = format!(
                "decl_ach:{}:{}:{}:{}:{}:unlocked",
                game_id, mode.as_str(), pack.manifest.pack_id, watcher.id, key
            );
            let rec = ObservationRecord {
                schema_version: "0.1.0".to_string(),
                observation_id: deterministic_uuid_from_seed(&seed),
                canonical_game_id: game_id.clone(),
                observer_id,
                source_type: watcher.source_type.clone(),
                observed_at: now_rfc3339(),
                source_timestamp: None, // Never invent missing save unlock timestamps
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
                    "Declarative {} watcher '{}' matched pack '{}' on '{}'",
                    watcher.format,
                    watcher.id,
                    pack.manifest.pack_id,
                    safe_path.display()
                )),
            };
            validate_observation_record(&rec)?;
            records.push(rec);
        }

        for stat in &watcher.stat_rules {
            let Some((int_value, float_value, string_value)) =
                extract_stat_value(watcher, bytes, json_root.as_ref(), ini_map.as_ref(), stat)
            else {
                continue;
            };
            let val_sig = format!(
                "{:?}:{:?}:{:?}",
                int_value, float_value, string_value
            );
            let key = normalize_canonical_id(&stat.stat_key, "stat");
            let observer_id = format!("decl_save_{}", watcher.id);
            let seed = format!(
                "decl_stat:{}:{}:{}:{}:{}:{}:{}",
                game_id, mode.as_str(), pack.manifest.pack_id, watcher.id, key, val_sig, artifact_hash
            );
            let rec = ObservationRecord {
                schema_version: "0.1.0".to_string(),
                observation_id: deterministic_uuid_from_seed(&seed),
                canonical_game_id: game_id.clone(),
                observer_id,
                source_type: watcher.source_type.clone(),
                observed_at: now_rfc3339(),
                source_timestamp: None,
                artifact_hash_sha256: Some(artifact_hash.clone()),
                record_kind: "STAT_SNAPSHOT".to_string(),
                key,
                unlocked: None,
                int_value,
                float_value,
                string_value,
                progression_mode: mode,
                trust_tier: TrustTier::Local,
                evidence_class: EvidenceClass::LocallyObserved,
                verification_state: VerificationState::Unverified,
                provenance_notes: Some(format!(
                    "Declarative {} stat '{}' extracted via watcher '{}'",
                    watcher.format, stat.stat_key, watcher.id
                )),
            };
            validate_observation_record(&rec)?;
            records.push(rec);
        }
    }

    Ok(ParsedObservationSet {
        records,
        skipped_locked,
        source_kind: "declarative_save".to_string(),
    })
}

fn collect_files_recursive(root: &Path, dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    children.sort();
    for p in children {
        // Reject symlinks pointing outside watch root
        let Ok(meta) = fs::symlink_metadata(&p) else {
            continue;
        };
        if meta.file_type().is_symlink() && safe_contained_path(root, &p).is_err() {
            continue;
        }
        if p.is_dir() {
            collect_files_recursive(root, &p, depth + 1, out);
        } else if p.is_file() {
            out.push(p);
        }
    }
}

/// Try matching candidate save files or directory against pack-declared save watchers.
/// Returns `Ok(None)` when no loaded pack has matching non-template declarative rules for the path.
pub fn try_parse_with_packs(
    path: &Path,
    packs: &PackRegistry,
    opts: &ImportOptions,
) -> Result<Option<ParsedObservationSet>, String> {
    let candidate_packs: Vec<&LoadedPack> = if let Some(gid) = &opts.canonical_game_id {
        packs.packs_for_game_id(gid)
    } else {
        let mut selected: Vec<&LoadedPack> = Vec::new();
        let norm_path = path.to_string_lossy().replace('\\', "/");
        if let Some((matched, _)) = packs.match_pack(&MatchQuery {
            save_path: Some(norm_path),
            ..Default::default()
        }) {
            if !matched.save_watchers.is_empty() {
                selected.push(matched);
            }
        }
        for p in packs.list() {
            if !p.save_watchers.is_empty()
                && !selected.iter().any(|s| s.manifest.pack_id == p.manifest.pack_id)
            {
                selected.push(p);
            }
        }
        selected
    };

    if candidate_packs.is_empty() {
        return Ok(None);
    }

    let (watch_root, files): (Option<&Path>, Vec<PathBuf>) = if path.is_dir() {
        let mut f = Vec::new();
        collect_files_recursive(path, path, 0, &mut f);
        (Some(path), f)
    } else {
        (None, vec![path.to_path_buf()])
    };

    let mut combined = Vec::new();
    let mut skipped_locked = 0usize;
    let mut matched_any_watcher = false;

    for file in files {
        let fname = file.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let rel = match watch_root {
            Some(r) => file
                .strip_prefix(r)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/"),
            None => file.to_string_lossy().replace('\\', "/"),
        };

        for pack in &candidate_packs {
            if !pack.save_watchers.iter().any(|w| {
                glob_matches(&w.file_glob, &rel) || glob_matches(&w.file_glob, fname)
            }) {
                continue;
            }
            matched_any_watcher = true;
            let parsed = evaluate_save_file_with_pack(pack, watch_root, &file, opts)?;
            skipped_locked += parsed.skipped_locked;
            combined.extend(parsed.records);
        }
    }

    if !matched_any_watcher {
        return Ok(None);
    }

    Ok(Some(ParsedObservationSet {
        records: combined,
        skipped_locked,
        source_kind: "declarative_save".to_string(),
    }))
}
