use super::manifest::{load_pack_from_zip_bytes, LoadedPack, MatcherType};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct MatchQuery {
    pub canonical_game_id: Option<String>,
    pub sidecar_uuid: Option<String>,
    pub rom_hash: Option<String>,
    pub recomp_game_id: Option<String>,
    pub build_id: Option<String>,
    pub asset_archive_hash: Option<String>,
    pub platform_id: Option<String>,
    pub region: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PackMatchResult {
    pub pack_id: String,
    pub canonical_game_id: String,
    pub matched_by: String,
    pub precedence_rank: u8,
}

#[derive(Debug, Default)]
pub struct PackRegistry {
    packs_by_id: HashMap<String, LoadedPack>,
    load_errors: Vec<(String, String)>,
}

impl PackRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn discover_in_dir(&mut self, dir: &Path) -> usize {
        let mut count = 0;
        let entries = match fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => return 0,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                    if ext == "onehundo" {
                        if let Err(e) = self.load_pack_file(&path) {
                            self.load_errors.push((path.display().to_string(), e));
                        } else {
                            count += 1;
                        }
                    }
                }
            }
        }
        count
    }

    pub fn load_pack_file(&mut self, path: &Path) -> Result<String, String> {
        let bytes = fs::read(path)
            .map_err(|e| format!("failed to read '{}': {}", path.display(), e))?;
        let pack = load_pack_from_zip_bytes(&bytes, &path.display().to_string())?;
        let id = pack.manifest.pack_id.clone();
        self.packs_by_id.insert(id.clone(), pack);
        Ok(id)
    }

    pub fn insert_pack(&mut self, pack: LoadedPack) {
        self.packs_by_id.insert(pack.manifest.pack_id.clone(), pack);
    }

    pub fn get(&self, pack_id: &str) -> Option<&LoadedPack> {
        self.packs_by_id.get(pack_id)
    }

    pub fn list(&self) -> Vec<&LoadedPack> {
        let mut list: Vec<&LoadedPack> = self.packs_by_id.values().collect();
        list.sort_by(|a, b| a.manifest.pack_id.cmp(&b.manifest.pack_id));
        list
    }

    pub fn load_errors(&self) -> &[(String, String)] {
        &self.load_errors
    }

    /// Deterministic matcher precedence:
    /// 1. sidecar_uuid
    /// 2. rom_hash
    /// 3. recomp_game_id
    /// 4. build_id
    /// 5. asset_archive_hash
    /// 6. platform_id
    /// 7. canonical_game_id fallback (rank 100)
    /// Region matches get a tie-breaker bonus (rank * 2 vs rank * 2 + 1).
    pub fn match_pack(&self, query: &MatchQuery) -> Option<(&LoadedPack, PackMatchResult)> {
        let mut best: Option<(u16, &LoadedPack, PackMatchResult)> = None;

        for pack in self.packs_by_id.values() {
            for m in &pack.manifest.matchers {
                let q_val = match m.matcher_type {
                    MatcherType::SidecarUuid => query.sidecar_uuid.as_deref(),
                    MatcherType::RomHash => query.rom_hash.as_deref(),
                    MatcherType::RecompGameId => query.recomp_game_id.as_deref(),
                    MatcherType::BuildId => query.build_id.as_deref(),
                    MatcherType::AssetArchiveHash => query.asset_archive_hash.as_deref(),
                    MatcherType::PlatformId => query.platform_id.as_deref(),
                };

                if let Some(qv) = q_val {
                    if qv.eq_ignore_ascii_case(&m.value) {
                        let region_ok = match (&query.region, &m.region) {
                            (Some(qr), Some(mr)) => qr.eq_ignore_ascii_case(mr),
                            (Some(_), None) => true,
                            (None, _) => true,
                        };
                        if !region_ok {
                            continue;
                        }
                        let region_exact = matches!((&query.region, &m.region), (Some(qr), Some(mr)) if qr.eq_ignore_ascii_case(mr));
                        let rank = m.matcher_type.precedence_rank();
                        let score = (rank as u16) * 2 + if region_exact { 0 } else { 1 };
                        let candidate = PackMatchResult {
                            pack_id: pack.manifest.pack_id.clone(),
                            canonical_game_id: pack.manifest.canonical_game_id.clone(),
                            matched_by: format!("{:?}", m.matcher_type),
                            precedence_rank: rank,
                        };
                        if best
                            .as_ref()
                            .map(|(bs, bp, _)| {
                                (score, pack.manifest.pack_id.as_str())
                                    < (*bs, bp.manifest.pack_id.as_str())
                            })
                            .unwrap_or(true)
                        {
                            best = Some((score, pack, candidate));
                        }
                    }
                }
            }

            if let Some(gid) = &query.canonical_game_id {
                if pack.manifest.canonical_game_id == *gid {
                    let score = 200u16;
                    let candidate = PackMatchResult {
                        pack_id: pack.manifest.pack_id.clone(),
                        canonical_game_id: pack.manifest.canonical_game_id.clone(),
                        matched_by: "canonical_game_id".to_string(),
                        precedence_rank: 100,
                    };
                    if best
                        .as_ref()
                        .map(|(bs, bp, _)| {
                            (score, pack.manifest.pack_id.as_str())
                                < (*bs, bp.manifest.pack_id.as_str())
                        })
                        .unwrap_or(true)
                    {
                        best = Some((score, pack, candidate));
                    }
                }
            }
        }

        best.map(|(_, p, r)| (p, r))
    }
}
