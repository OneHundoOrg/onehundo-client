use crate::core::types::{is_valid_canonical_id, sha256_hex, ProgressionMode, TrustTier};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatcherType {
    RomHash,
    RecompGameId,
    BuildId,
    PlatformId,
    SidecarUuid,
    AssetArchiveHash,
}

impl MatcherType {
    /// Deterministic precedence order (lower rank = higher priority).
    pub fn precedence_rank(&self) -> u8 {
        match self {
            Self::SidecarUuid => 1,
            Self::RomHash => 2,
            Self::RecompGameId => 3,
            Self::BuildId => 4,
            Self::AssetArchiveHash => 5,
            Self::PlatformId => 6,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityMatcher {
    pub matcher_type: MatcherType,
    #[serde(default)]
    pub algorithm: Option<String>,
    pub value: String,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub revision: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRegionSpec {
    pub region_name: String,
    pub guest_base_address: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryProfile {
    pub enabled: bool,
    pub architecture: String,
    #[serde(default)]
    pub default_endianness: Option<String>,
    pub regions: Vec<MemoryRegionSpec>,
    #[serde(default)]
    pub rules_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredStat {
    pub key: String,
    pub value_type: String,
    #[serde(default)]
    pub monotonic: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticProfile {
    pub enabled: bool,
    pub producer_ids: Vec<String>,
    #[serde(default)]
    pub declared_stats: Vec<DeclaredStat>,
    #[serde(default)]
    pub rules_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationSourceSpec {
    pub source_type: String,
    pub evidence_class: String,
    #[serde(default)]
    pub file_glob: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationProfile {
    pub enabled: bool,
    pub sources: Vec<ObservationSourceSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfilesBlock {
    #[serde(default)]
    pub memory: Option<MemoryProfile>,
    #[serde(default)]
    pub semantic: Option<SemanticProfile>,
    #[serde(default)]
    pub observation: Option<ObservationProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AchievementDefinition {
    pub canonical_achievement_id: String,
    pub title: String,
    pub description: String,
    pub points: u32,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub icon_path: Option<String>,
    pub supported_modes: Vec<ProgressionMode>,
    pub minimum_trust_tier: TrustTier,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WasmSandboxMetadata {
    pub module_path: String,
    pub sha256: String,
    pub max_memory_bytes: u64,
    pub max_fuel_per_frame: u64,
    #[serde(default)]
    pub allow_network: Option<bool>,
    #[serde(default)]
    pub allow_filesystem: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureBlock {
    pub algorithm: String,
    pub key_id: String,
    pub signature_base64: String,
    pub signed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackManifest {
    pub manifest_version: String,
    pub pack_id: String,
    #[serde(default)]
    pub pack_version: Option<String>,
    pub canonical_game_id: String,
    pub title: String,
    #[serde(default)]
    pub authors: Vec<String>,
    pub matchers: Vec<IdentityMatcher>,
    pub profiles: ProfilesBlock,
    pub achievements: Vec<AchievementDefinition>,
    #[serde(default)]
    pub wasm_sandbox: Option<WasmSandboxMetadata>,
    #[serde(default)]
    pub signature: Option<SignatureBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticRule {
    pub achievement_id: String,
    pub event: String,
    #[serde(default)]
    pub stat: Option<String>,
    #[serde(default)]
    pub op: Option<String>,
    #[serde(default)]
    pub threshold: Option<i64>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConditionSpec {
    #[serde(default)]
    pub region_id: u32,
    /// Hex or integer guest offset/address
    #[serde(default)]
    pub address: String,
    /// "u8", "u16_le", "u16_be", "u24_le", "u24_be", "u32_le", "u32_be",
    /// "i8", "i16_le", "i16_be", "i32_le", "i32_be", "lower4", "upper4",
    /// "bitcount", "bit0".."bit7"
    #[serde(default = "default_mem_size")]
    pub size: String,
    /// "==", "!=", "<", "<=", ">", ">="
    #[serde(default = "default_mem_op")]
    pub op: String,
    /// Compare against constant or rhs operand
    #[serde(default)]
    pub value: Option<i64>,
    #[serde(default)]
    pub compare_delta: bool,
    /// "normal", "once", "reset_if", "pause_if", "add_source", "sub_source",
    /// "add_hits", "sub_hits", "and_next", "or_next", "reset_next_if", "measured", "measured_if"
    #[serde(default = "default_cond_type")]
    pub kind: String,
    #[serde(default)]
    pub required_hits: u32,
    /// LHS operand source: "mem" (default), "delta", "prior", "const", "invert"
    #[serde(default)]
    pub lhs_type: Option<String>,
    /// RHS operand source: "const" (default unless compare_delta), "mem", "delta", "prior", "invert"
    #[serde(default)]
    pub rhs_type: Option<String>,
    #[serde(default)]
    pub rhs_region_id: Option<u32>,
    #[serde(default)]
    pub rhs_address: Option<String>,
    #[serde(default)]
    pub rhs_size: Option<String>,
}

fn default_mem_size() -> String {
    "u8".to_string()
}
fn default_mem_op() -> String {
    "==".to_string()
}
fn default_cond_type() -> String {
    "normal".to_string()
}
fn default_stat_event_kind() -> String {
    "STAT_SET".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRule {
    pub achievement_id: String,
    /// Optional RetroAchievements (`rcheevos`) `memaddr` condition string (e.g. `0xH0020=1_d0xH0010=5.2.`)
    #[serde(default)]
    pub memaddr: Option<String>,
    #[serde(default)]
    pub conditions: Vec<MemoryConditionSpec>,
    /// Optional alternative condition groups (OR across groups, AND inside group)
    #[serde(default)]
    pub alt_groups: Vec<Vec<MemoryConditionSpec>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStatRule {
    pub stat_key: String,
    /// "STAT_SET" or "STAT_INC"
    #[serde(default = "default_stat_event_kind")]
    pub event_kind: String,
    #[serde(default)]
    pub region_id: u32,
    pub address: String,
    #[serde(default = "default_mem_size")]
    pub size: String,
    /// Skip emitting value 0 on initial baseline frame
    #[serde(default)]
    pub only_when_nonzero: bool,
    /// Optional guard conditions that must evaluate to true before updating this stat
    #[serde(default)]
    pub conditions: Vec<MemoryConditionSpec>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RulesDocument {
    #[serde(default)]
    pub rules: Vec<SemanticRule>,
    #[serde(default)]
    pub memory_rules: Vec<MemoryRule>,
    #[serde(default)]
    pub memory_stats: Vec<MemoryStatRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackSummary {
    pub pack_id: String,
    pub pack_version: Option<String>,
    pub canonical_game_id: String,
    pub title: String,
    pub mapped_oap_edition: Option<String>,
    pub achievement_count: usize,
    pub pack_sha256: String,
    pub enabled_planes: Vec<String>,
    pub signed: bool,
    pub source_path: String,
}

#[derive(Debug, Clone)]
pub struct LoadedPack {
    pub manifest: PackManifest,
    pub semantic_rules: Vec<SemanticRule>,
    pub memory_rules: Vec<MemoryRule>,
    pub memory_stats: Vec<MemoryStatRule>,
    pub pack_sha256: String,
    pub mapped_oap_edition: Option<String>,
    pub source_path: String,
}

impl LoadedPack {
    pub fn summary(&self) -> PackSummary {
        let mut enabled_planes = Vec::new();
        if self
            .manifest
            .profiles
            .memory
            .as_ref()
            .map(|m| m.enabled)
            .unwrap_or(false)
        {
            enabled_planes.push("Plane A (Memory)".to_string());
        }
        if self
            .manifest
            .profiles
            .semantic
            .as_ref()
            .map(|s| s.enabled)
            .unwrap_or(false)
        {
            enabled_planes.push("Plane B (Semantic)".to_string());
        }
        if self
            .manifest
            .profiles
            .observation
            .as_ref()
            .map(|o| o.enabled)
            .unwrap_or(false)
        {
            enabled_planes.push("Plane C (Observation)".to_string());
        }
        PackSummary {
            pack_id: self.manifest.pack_id.clone(),
            pack_version: self.manifest.pack_version.clone(),
            canonical_game_id: self.manifest.canonical_game_id.clone(),
            title: self.manifest.title.clone(),
            mapped_oap_edition: self.mapped_oap_edition.clone(),
            achievement_count: self.manifest.achievements.len(),
            pack_sha256: self.pack_sha256.clone(),
            enabled_planes,
            signed: self.manifest.signature.is_some(),
            source_path: self.source_path.clone(),
        }
    }
}

pub fn derive_mapped_oap_edition(manifest: &PackManifest) -> Option<String> {
    if manifest.canonical_game_id == "sony.bloodborne"
        || manifest
            .matchers
            .iter()
            .any(|m| m.matcher_type == MatcherType::PlatformId && m.value.eq_ignore_ascii_case("CUSA03173"))
    {
        return Some("psn-provisional-bloodborne-ps4".to_string());
    }
    if manifest.canonical_game_id == "shared.cross_runtime.chronos" {
        return Some("ra-chronos".to_string());
    }
    manifest
        .matchers
        .iter()
        .find(|m| m.matcher_type == MatcherType::PlatformId)
        .map(|m| format!("platform_id:{}", m.value))
}

pub fn validate_and_parse_manifest(
    manifest_json: &str,
    archive_entries: Option<&BTreeMap<String, Vec<u8>>>,
) -> Result<PackManifest, String> {
    let manifest: PackManifest = serde_json::from_str(manifest_json)
        .map_err(|e| format!("manifest.json schema parse error: {}", e))?;

    if manifest.manifest_version != "0.1.0" {
        return Err(format!(
            "unsupported manifest_version '{}'",
            manifest.manifest_version
        ));
    }
    if !is_valid_canonical_id(&manifest.pack_id) {
        return Err(format!("invalid pack_id '{}'", manifest.pack_id));
    }
    if !is_valid_canonical_id(&manifest.canonical_game_id) {
        return Err(format!(
            "invalid canonical_game_id '{}'",
            manifest.canonical_game_id
        ));
    }
    if manifest.title.trim().is_empty() || manifest.title.len() > 200 {
        return Err("manifest title must be 1..=200 chars".into());
    }
    if manifest.matchers.is_empty() {
        return Err("manifest matchers must not be empty".into());
    }
    for (idx, m) in manifest.matchers.iter().enumerate() {
        if m.value.is_empty() || m.value.len() > 256 {
            return Err(format!("matchers[{}].value invalid length", idx));
        }
    }
    if manifest.profiles.memory.is_none()
        && manifest.profiles.semantic.is_none()
        && manifest.profiles.observation.is_none()
    {
        return Err("manifest.profiles must define at least one profile".into());
    }
    if let Some(mem) = &manifest.profiles.memory {
        if mem.regions.is_empty() {
            return Err("manifest.profiles.memory.regions must not be empty".into());
        }
    }
    if let Some(sem) = &manifest.profiles.semantic {
        if sem.producer_ids.is_empty() {
            return Err("manifest.profiles.semantic.producer_ids must not be empty".into());
        }
    }
    if let Some(obs) = &manifest.profiles.observation {
        if obs.sources.is_empty() {
            return Err("manifest.profiles.observation.sources must not be empty".into());
        }
    }
    if manifest.achievements.is_empty() {
        return Err("manifest.achievements must not be empty".into());
    }
    for (idx, ach) in manifest.achievements.iter().enumerate() {
        if !is_valid_canonical_id(&ach.canonical_achievement_id) {
            return Err(format!(
                "achievements[{}].canonical_achievement_id '{}' invalid",
                idx, ach.canonical_achievement_id
            ));
        }
        if ach.points > 1000 {
            return Err(format!("achievements[{}].points exceeds 1000", idx));
        }
        if ach.supported_modes.is_empty() {
            return Err(format!(
                "achievements[{}].supported_modes must not be empty",
                idx
            ));
        }
        if let (Some(icon_path), Some(entries)) = (&ach.icon_path, archive_entries) {
            if !entries.contains_key(icon_path) {
                return Err(format!(
                    "achievements[{}].icon_path '{}' missing from archive",
                    idx, icon_path
                ));
            }
        }
    }
    if let Some(wasm) = &manifest.wasm_sandbox {
        if !wasm.module_path.starts_with("rules/") || !wasm.module_path.ends_with(".wasm") {
            return Err("wasm_sandbox.module_path must match rules/*.wasm".into());
        }
        if !(65536..=67_108_864).contains(&wasm.max_memory_bytes) {
            return Err("wasm_sandbox.max_memory_bytes out of bounds".into());
        }
        if wasm.allow_network == Some(true) || wasm.allow_filesystem == Some(true) {
            return Err("wasm_sandbox cannot allow network or filesystem".into());
        }
    }
    if let Some(sig) = &manifest.signature {
        if sig.algorithm != "ed25519" {
            return Err(format!("unsupported signature algorithm '{}'", sig.algorithm));
        }
    }
    Ok(manifest)
}

pub fn load_pack_from_zip_bytes(bytes: &[u8], source_path: &str) -> Result<LoadedPack, String> {
    let entries = super::zip_safe::read_and_validate_zip(bytes)?;
    let manifest_bytes = entries
        .get("manifest.json")
        .ok_or_else(|| "pack archive missing required root 'manifest.json'".to_string())?;
    let manifest_str = std::str::from_utf8(manifest_bytes)
        .map_err(|_| "manifest.json is not valid UTF-8".to_string())?;
    let manifest = validate_and_parse_manifest(manifest_str, Some(&entries))?;

    let mut semantic_rules = Vec::new();
    let mut memory_rules = Vec::new();
    let mut memory_stats = Vec::new();

    if let Some(sem) = &manifest.profiles.semantic {
        if let Some(rules_path) = &sem.rules_file {
            if let Some(raw) = entries.get(rules_path) {
                let s = std::str::from_utf8(raw)
                    .map_err(|_| format!("rules file '{}' not valid UTF-8", rules_path))?;
                let doc: RulesDocument = serde_json::from_str(s)
                    .map_err(|e| format!("failed parsing rules file '{}': {}", rules_path, e))?;
                semantic_rules.extend(doc.rules);
            }
        }
    }
    if let Some(mem) = &manifest.profiles.memory {
        if let Some(rules_path) = &mem.rules_file {
            if let Some(raw) = entries.get(rules_path) {
                let s = std::str::from_utf8(raw)
                    .map_err(|_| format!("rules file '{}' not valid UTF-8", rules_path))?;
                let doc: RulesDocument = serde_json::from_str(s)
                    .map_err(|e| format!("failed parsing rules file '{}': {}", rules_path, e))?;
                memory_rules.extend(doc.memory_rules);
                memory_stats.extend(doc.memory_stats);
            }
        }
    }

    let mapped_oap_edition = derive_mapped_oap_edition(&manifest);
    let pack_sha256 = sha256_hex(bytes);

    Ok(LoadedPack {
        manifest,
        semantic_rules,
        memory_rules,
        memory_stats,
        pack_sha256,
        mapped_oap_edition,
        source_path: source_path.to_string(),
    })
}
