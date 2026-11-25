//! Emulator Trophy Directory (`RPCS3` / `shadPS4`) Plane C importer.
//! Parses `TROPUSR.DAT` (big-endian PS3 trophy store), `TROP.XML`/`TROPUSR.XML`/`TROP.SFM`,
//! and JSON trophy exports (`trophies.json` / `TROPUSR.json`).
//! Strictly isolated from gameplay evaluators; all records emit `LOCALLY_OBSERVED`, `LOCAL`, `UNVERIFIED`.

use super::{
    normalize_canonical_id, parse_optional_timestamp, resolve_mode, ImportOptions,
    ObservationRecord, ParsedObservationSet,
};
use crate::core::types::{
    deterministic_uuid_from_seed, format_rfc3339_secs, now_rfc3339, sha256_hex, EvidenceClass,
    TrustTier, VerificationState,
};
use crate::packs::matcher::{MatchQuery, PackRegistry};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

const TROPUSR_MAGIC: u32 = 0x818F_54AD;
const CELL_RTC_UNIX_EPOCH_USEC: u64 = 62_135_596_800_000_000;

fn parse_cell_rtc_or_epoch(raw: u64) -> Option<String> {
    if raw == 0 {
        return None;
    }
    if raw >= CELL_RTC_UNIX_EPOCH_USEC {
        let unix_secs = ((raw - CELL_RTC_UNIX_EPOCH_USEC) / 1_000_000) as i64;
        return Some(format_rfc3339_secs(unix_secs));
    }
    if raw > 10_000_000_000 {
        // Milliseconds since unix epoch
        return Some(format_rfc3339_secs((raw / 1000) as i64));
    }
    Some(format_rfc3339_secs(raw as i64))
}

fn normalize_trophy_key(raw_id: &str) -> String {
    let trimmed = raw_id.trim();
    if let Ok(num) = trimmed.parse::<u32>() {
        format!("trophy_{}", num)
    } else {
        normalize_canonical_id(trimmed, "trophy")
    }
}

fn resolve_game_id(
    dir: &Path,
    npcommid: Option<&str>,
    packs: Option<&PackRegistry>,
    override_id: Option<&str>,
) -> String {
    if let Some(id) = override_id.map(str::trim).filter(|s| !s.is_empty()) {
        return id.to_string();
    }

    // Extract candidate platform_ids (NPWR..., CUSA..., BLUS...) from npcommid or path
    let mut candidates = Vec::new();
    if let Some(np) = npcommid.map(str::trim).filter(|s| !s.is_empty()) {
        candidates.push(np.to_string());
    }
    for ancestor in dir.ancestors().take(4) {
        if let Some(name) = ancestor.file_name().and_then(|s| s.to_str()) {
            let up = name.to_ascii_uppercase();
            if up.starts_with("CUSA")
                || up.starts_with("NPWR")
                || up.starts_with("BLUS")
                || up.starts_with("BLES")
                || up.starts_with("BCUS")
                || up.starts_with("BCES")
            {
                candidates.push(up);
            }
        }
    }

    if let Some(reg) = packs {
        for cand in &candidates {
            if let Some((_, matched)) = reg.match_pack(&MatchQuery {
                canonical_game_id: None,
                sidecar_uuid: None,
                rom_hash: None,
                recomp_game_id: None,
                build_id: None,
                asset_archive_hash: None,
                platform_id: Some(cand.clone()),
                region: None,
            }) {
                return matched.canonical_game_id;
            }
        }
    }

    if let Some(first) = candidates.first() {
        return format!("psn.{}", normalize_canonical_id(first, "title"));
    }

    "psn.trophy.local".to_string()
}

fn build_trophy_record(
    canonical_game_id: &str,
    raw_trophy_id: &str,
    trophy_name: Option<&str>,
    source_timestamp: Option<String>,
    artifact_hash: &str,
    source_label: &str,
    opts: &ImportOptions,
) -> ObservationRecord {
    let key = normalize_trophy_key(raw_trophy_id);
    let mode = resolve_mode(opts);
    let seed = format!(
        "trophy_dir:{}:{}:{}:{}",
        canonical_game_id,
        mode.as_str(),
        key,
        raw_trophy_id.trim()
    );
    let mut notes = format!(
        "Observed from emulator trophy export '{}' (trophy_id='{}'",
        source_label, raw_trophy_id
    );
    if let Some(name) = trophy_name.filter(|s| !s.is_empty()) {
        notes.push_str(&format!(", name='{}'", name));
    }
    notes.push(')');

    ObservationRecord {
        schema_version: "0.1.0".to_string(),
        observation_id: deterministic_uuid_from_seed(&seed),
        canonical_game_id: canonical_game_id.to_string(),
        observer_id: "plane_c.emulator_trophy_dir".to_string(),
        source_type: "psn_trophy".to_string(),
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
        provenance_notes: Some(notes),
    }
}

fn read_be_u32(bytes: &[u8], off: usize) -> Option<u32> {
    let slice = bytes.get(off..off + 4)?;
    Some(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_be_u64(bytes: &[u8], off: usize) -> Option<u64> {
    let slice = bytes.get(off..off + 8)?;
    Some(u64::from_be_bytes([
        slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
    ]))
}

pub fn parse_tropusr_dat_bytes(
    bytes: &[u8],
    dir: &Path,
    npcommid: Option<&str>,
    trophy_names: &BTreeMap<String, String>,
    packs: Option<&PackRegistry>,
    opts: &ImportOptions,
) -> Result<ParsedObservationSet, String> {
    if bytes.len() < 0x30 {
        return Err("TROPUSR.DAT too short for header".to_string());
    }
    let magic = read_be_u32(bytes, 0).ok_or_else(|| "missing TROPUSR.DAT magic".to_string())?;
    if magic != TROPUSR_MAGIC {
        return Err(format!(
            "invalid TROPUSR.DAT magic 0x{:08x} (expected 0x{:08x})",
            magic, TROPUSR_MAGIC
        ));
    }

    let tables_count = read_be_u32(bytes, 0x10).unwrap_or(0) as usize;
    let artifact_hash = sha256_hex(bytes);
    let canonical_game_id = resolve_game_id(dir, npcommid, packs, opts.canonical_game_id.as_deref());

    let mut records = Vec::new();
    let mut skipped_locked = 0usize;
    let mut found_type6 = false;

    for t_idx in 0..tables_count.min(32) {
        let th_off = 0x30 + t_idx * 32;
        let Some(table_type) = read_be_u32(bytes, th_off) else {
            break;
        };
        let entry_size = read_be_u32(bytes, th_off + 4).unwrap_or(0) as usize;
        let entries_count = read_be_u32(bytes, th_off + 12).unwrap_or(0) as usize;
        let data_offset = read_be_u64(bytes, th_off + 16).unwrap_or(0) as usize;

        if table_type != 6 {
            continue;
        }
        found_type6 = true;
        if entry_size < 0x28 {
            return Err(format!("invalid TROPUSR.DAT type 6 entry_size {}", entry_size));
        }

        for e_idx in 0..entries_count {
            let e_off = data_offset.saturating_add(e_idx.saturating_mul(entry_size));
            if e_off.saturating_add(entry_size) > bytes.len() {
                break;
            }
            let trophy_id = read_be_u32(bytes, e_off + 0x10).unwrap_or(e_idx as u32);
            let state = read_be_u32(bytes, e_off + 0x14).unwrap_or(0);
            if state == 0 {
                skipped_locked += 1;
                continue;
            }
            let ts_raw = read_be_u64(bytes, e_off + 0x20).unwrap_or(0);
            let ts = parse_cell_rtc_or_epoch(ts_raw);
            let raw_id_str = trophy_id.to_string();
            let name = trophy_names.get(&raw_id_str).map(String::as_str);

            records.push(build_trophy_record(
                &canonical_game_id,
                &raw_id_str,
                name,
                ts,
                &artifact_hash,
                "TROPUSR.DAT",
                opts,
            ));
        }
    }

    if !found_type6 {
        return Err("TROPUSR.DAT contains no trophy info table (type 6)".to_string());
    }

    Ok(ParsedObservationSet {
        records,
        skipped_locked,
        source_kind: "rpcs3_tropusr_dat".to_string(),
    })
}

fn extract_xml_tag_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = xml.find(&open)? + open.len();
    let end_rel = xml[start..].find(&close)?;
    let val = xml[start..start + end_rel].trim();
    if val.is_empty() {
        None
    } else {
        Some(val.to_string())
    }
}

fn extract_xml_attr(attrs: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let needle = format!("{}={}", name, quote);
        if let Some(pos) = attrs.find(&needle) {
            let rest = &attrs[pos + needle.len()..];
            if let Some(end) = rest.find(quote) {
                return Some(rest[..end].to_string());
            }
        }
    }
    None
}

fn parse_sfm_metadata(dir: &Path) -> (Option<String>, BTreeMap<String, String>) {
    let mut npcommid = None;
    let mut names = BTreeMap::new();

    for candidate in ["TROP.SFM", "TROPCONF.SFM", "TROP.XML", "TROPUSR.XML", "trophies.xml"] {
        let p = dir.join(candidate);
        let Ok(bytes) = fs::read(&p) else {
            continue;
        };
        let lossy = String::from_utf8_lossy(&bytes);
        let xml_slice = if let Some(idx) = lossy.find("<trophyconf") {
            &lossy[idx..]
        } else {
            lossy.as_ref()
        };
        if npcommid.is_none() {
            npcommid = extract_xml_tag_text(xml_slice, "npcommid");
        }
        // Extract `<trophy id="...">...<name>...</name>...</trophy>` if present
        let mut cursor = xml_slice;
        while let Some(start) = cursor.find("<trophy ") {
            let after_tag = &cursor[start + 8..];
            let Some(gt) = after_tag.find('>') else {
                break;
            };
            let attrs = &after_tag[..gt];
            let id = extract_xml_attr(attrs, "id")
                .or_else(|| extract_xml_attr(attrs, "trophy_id"))
                .and_then(|v| v.trim().parse::<u32>().ok().map(|n| n.to_string()).or(Some(v)));
            let body_start = &after_tag[gt + 1..];
            if !attrs.ends_with('/') {
                if let Some(end_trophy) = body_start.find("</trophy>") {
                    let body = &body_start[..end_trophy];
                    if let (Some(tid), Some(tname)) = (id, extract_xml_tag_text(body, "name")) {
                        names.entry(tid).or_insert(tname);
                    }
                    cursor = &body_start[end_trophy + 9..];
                    continue;
                }
            }
            cursor = body_start;
        }
    }

    (npcommid, names)
}

pub fn parse_trophy_xml_bytes(
    bytes: &[u8],
    source_path: &Path,
    packs: Option<&PackRegistry>,
    opts: &ImportOptions,
) -> Result<ParsedObservationSet, String> {
    let artifact_hash = sha256_hex(bytes);
    let lossy = String::from_utf8_lossy(bytes);
    let xml = if let Some(idx) = lossy.find("<trophyconf") {
        &lossy[idx..]
    } else {
        lossy.as_ref()
    };

    // Guard against XML entity expansion / DTD tricks (no external parser used anyway)
    let dir = source_path.parent().unwrap_or(source_path);
    let (sfm_np, sfm_names) = parse_sfm_metadata(dir);
    let npcommid = extract_xml_tag_text(xml, "npcommid").or(sfm_np);
    let canonical_game_id = resolve_game_id(
        dir,
        npcommid.as_deref(),
        packs,
        opts.canonical_game_id.as_deref(),
    );
    let label = source_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("TROP.XML");

    let mut records = Vec::new();
    let mut skipped_locked = 0usize;
    let mut cursor = xml;

    while let Some(pos) = cursor.find("<trophy") {
        let after = &cursor[pos + 7..];
        let Some(first_ch) = after.chars().next() else {
            break;
        };
        if !first_ch.is_whitespace() && first_ch != '>' {
            cursor = after;
            continue;
        }
        let Some(gt) = after.find('>') else {
            break;
        };
        let attrs = &after[..gt];
        let self_closing = attrs.trim_end().ends_with('/');
        let rest = &after[gt + 1..];

        let (body, next_cursor) = if self_closing {
            ("", rest)
        } else if let Some(close_idx) = rest.find("</trophy>") {
            (&rest[..close_idx], &rest[close_idx + 9..])
        } else {
            ("", rest)
        };

        let raw_id = extract_xml_attr(attrs, "id")
            .or_else(|| extract_xml_attr(attrs, "trophy_id"))
            .or_else(|| extract_xml_tag_text(body, "id"));

        if let Some(tid) = raw_id {
            let unlocked_str = extract_xml_attr(attrs, "unlocked")
                .or_else(|| extract_xml_attr(attrs, "unlockedstate"))
                .or_else(|| extract_xml_attr(attrs, "state"))
                .or_else(|| extract_xml_tag_text(body, "unlocked"));

            let is_unlocked = unlocked_str
                .as_deref()
                .map(|s| matches!(s.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
                .unwrap_or(false);

            if !is_unlocked {
                skipped_locked += 1;
            } else {
                let ts_raw = extract_xml_attr(attrs, "timestamp")
                    .or_else(|| extract_xml_attr(attrs, "unlocktime"))
                    .or_else(|| extract_xml_attr(attrs, "unlocked_at"))
                    .or_else(|| extract_xml_tag_text(body, "timestamp"))
                    .or_else(|| extract_xml_tag_text(body, "unlocktime"));

                let ts = ts_raw.and_then(|s| {
                    if let Ok(num) = s.trim().parse::<u64>() {
                        parse_cell_rtc_or_epoch(num)
                    } else {
                        parse_optional_timestamp(&serde_json::Value::String(s))
                    }
                });

                let norm_num = tid
                    .trim()
                    .parse::<u32>()
                    .ok()
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| tid.clone());
                let tname = extract_xml_tag_text(body, "name")
                    .or_else(|| sfm_names.get(&norm_num).cloned());

                records.push(build_trophy_record(
                    &canonical_game_id,
                    &tid,
                    tname.as_deref(),
                    ts,
                    &artifact_hash,
                    label,
                    opts,
                ));
            }
        }

        cursor = next_cursor;
    }

    Ok(ParsedObservationSet {
        records,
        skipped_locked,
        source_kind: "shadps4_trophy_xml".to_string(),
    })
}

pub fn parse_trophy_json_bytes(
    bytes: &[u8],
    source_path: &Path,
    packs: Option<&PackRegistry>,
    opts: &ImportOptions,
) -> Result<ParsedObservationSet, String> {
    let artifact_hash = sha256_hex(bytes);
    let text = std::str::from_utf8(bytes)
        .map_err(|_| format!("trophy JSON '{}' is not UTF-8", source_path.display()))?;
    let val: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| format!("invalid trophy JSON '{}': {}", source_path.display(), e))?;

    let dir = source_path.parent().unwrap_or(source_path);
    let (sfm_np, sfm_names) = parse_sfm_metadata(dir);

    let (npcommid, game_override, arr) = match &val {
        serde_json::Value::Object(obj) => {
            let np = obj
                .get("npcommid")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .or(sfm_np);
            let gid = opts.canonical_game_id.clone().or_else(|| {
                obj.get("canonical_game_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            });
            let list = obj
                .get("trophies")
                .or_else(|| obj.get("achievements"))
                .and_then(|v| v.as_array())
                .ok_or_else(|| "trophy JSON object missing 'trophies' array".to_string())?;
            (np, gid, list)
        }
        serde_json::Value::Array(list) => (sfm_np, opts.canonical_game_id.clone(), list),
        _ => return Err("trophy JSON must be an object or array".to_string()),
    };

    let canonical_game_id = resolve_game_id(dir, npcommid.as_deref(), packs, game_override.as_deref());
    let label = source_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("trophies.json");

    let mut records = Vec::new();
    let mut skipped_locked = 0usize;

    for item in arr {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let raw_id = obj
            .get("id")
            .or_else(|| obj.get("trophy_id"))
            .map(|v| match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                _ => String::new(),
            })
            .filter(|s| !s.is_empty());
        let Some(tid) = raw_id else {
            continue;
        };

        let unlocked = match obj.get("unlocked").or_else(|| obj.get("earned")) {
            Some(serde_json::Value::Bool(b)) => *b,
            Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
            Some(serde_json::Value::String(s)) => {
                matches!(s.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes")
            }
            _ => false,
        };
        if !unlocked {
            skipped_locked += 1;
            continue;
        }

        let ts = ["timestamp", "unlock_time", "unlocked_at", "unlocktime"]
            .iter()
            .find_map(|f| {
                obj.get(*f).and_then(|v| {
                    if let Some(u) = v.as_u64() {
                        parse_cell_rtc_or_epoch(u)
                    } else {
                        parse_optional_timestamp(v)
                    }
                })
            });

        let norm_num = tid
            .trim()
            .parse::<u32>()
            .ok()
            .map(|n| n.to_string())
            .unwrap_or_else(|| tid.clone());
        let name = obj
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| sfm_names.get(&norm_num).cloned());

        records.push(build_trophy_record(
            &canonical_game_id,
            &tid,
            name.as_deref(),
            ts,
            &artifact_hash,
            label,
            opts,
        ));
    }

    Ok(ParsedObservationSet {
        records,
        skipped_locked,
        source_kind: "shadps4_trophy_json".to_string(),
    })
}

pub fn parse_trophy_directory(
    dir: &Path,
    packs: Option<&PackRegistry>,
    opts: &ImportOptions,
) -> Result<ParsedObservationSet, String> {
    let dat = dir.join("TROPUSR.DAT");
    if dat.is_file() {
        let bytes = fs::read(&dat)
            .map_err(|e| format!("failed reading '{}': {}", dat.display(), e))?;
        let (npcommid, names) = parse_sfm_metadata(dir);
        return parse_tropusr_dat_bytes(&bytes, dir, npcommid.as_deref(), &names, packs, opts);
    }

    for xml_name in ["TROPUSR.XML", "TROP.XML", "trophies.xml", "TROP.SFM"] {
        let p = dir.join(xml_name);
        if p.is_file() {
            let bytes = fs::read(&p)
                .map_err(|e| format!("failed reading '{}': {}", p.display(), e))?;
            return parse_trophy_xml_bytes(&bytes, &p, packs, opts);
        }
    }

    for json_name in ["TROPUSR.json", "trophies.json", "trophy.json"] {
        let p = dir.join(json_name);
        if p.is_file() {
            let bytes = fs::read(&p)
                .map_err(|e| format!("failed reading '{}': {}", p.display(), e))?;
            return parse_trophy_json_bytes(&bytes, &p, packs, opts);
        }
    }

    Err(format!(
        "no RPCS3/shadPS4 trophy files (TROPUSR.DAT, TROP.XML, trophies.json) found in '{}'",
        dir.display()
    ))
}

pub fn parse_trophy_file(
    path: &Path,
    packs: Option<&PackRegistry>,
    opts: &ImportOptions,
) -> Result<ParsedObservationSet, String> {
    let fname = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let bytes = fs::read(path)
        .map_err(|e| format!("failed reading trophy file '{}': {}", path.display(), e))?;

    if fname == "tropusr.dat" {
        let dir = path.parent().unwrap_or(path);
        let (npcommid, names) = parse_sfm_metadata(dir);
        parse_tropusr_dat_bytes(&bytes, dir, npcommid.as_deref(), &names, packs, opts)
    } else if fname.ends_with(".xml") || fname.ends_with(".sfm") {
        parse_trophy_xml_bytes(&bytes, path, packs, opts)
    } else {
        parse_trophy_json_bytes(&bytes, path, packs, opts)
    }
}
