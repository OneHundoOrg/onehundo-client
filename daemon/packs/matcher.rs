use super::manifest::{load_pack_from_zip_bytes, LoadedPack, MatcherType, PackSummary};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct MatchQuery {
    pub canonical_game_id: Option<String>,
    pub sidecar_uuid: Option<String>,
    pub rom_hash: Option<String>,
    pub recomp_game_id: Option<String>,
    pub build_id: Option<String>,
    pub asset_archive_hash: Option<String>,
    pub platform_id: Option<String>,
    pub executable: Option<String>,
    pub app_id: Option<String>,
    pub save_path: Option<String>,
    pub region: Option<String>,
    pub os: Option<String>,
    pub launcher: Option<String>,
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

/// Pure-Rust case-insensitive glob matcher supporting `**`, `*`, and `?` over normalized `/` paths.
pub fn glob_matches(pattern: &str, text: &str) -> bool {
    let p_norm = pattern.replace('\\', "/").to_ascii_lowercase();
    let t_norm = text.replace('\\', "/").to_ascii_lowercase();
    let p_bytes: Vec<char> = p_norm.chars().collect();
    let t_bytes: Vec<char> = t_norm.chars().collect();
    glob_chars_match(&p_bytes, &t_bytes)
}

fn glob_chars_match(p: &[char], t: &[char]) -> bool {
    if p.is_empty() {
        return t.is_empty();
    }
    if p[0] == '*' {
        // Collapse consecutive `*` so `**` matches across `/` as well as `*`.
        let is_double = p.get(1) == Some(&'*');
        let mut next_p = 1;
        while next_p < p.len() && p[next_p] == '*' {
            next_p += 1;
        }
        // `**/` optionally skips zero or more directory segments.
        if is_double && p.get(next_p) == Some(&'/') && glob_chars_match(&p[next_p + 1..], t) {
            return true;
        }
        if glob_chars_match(&p[next_p..], t) {
            return true;
        }
        if !t.is_empty() && (is_double || t[0] != '/') {
            return glob_chars_match(p, &t[1..]);
        }
        return false;
    }
    if t.is_empty() {
        return false;
    }
    if p[0] == '?' || p[0] == t[0] {
        return glob_chars_match(&p[1..], &t[1..]);
    }
    false
}

fn executable_matches(matcher_val: &str, query_val: &str) -> bool {
    let q_norm = query_val.replace('\\', "/");
    let m_norm = matcher_val.replace('\\', "/");
    if q_norm.eq_ignore_ascii_case(&m_norm) {
        return true;
    }
    let q_base = q_norm.rsplit('/').next().unwrap_or(&q_norm);
    let m_base = m_norm.rsplit('/').next().unwrap_or(&m_norm);
    if m_norm.contains('*') || m_norm.contains('?') {
        return glob_matches(&m_norm, &q_norm) || glob_matches(m_base, q_base);
    }
    q_base.eq_ignore_ascii_case(m_base)
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

    /// Look up by `pack_id` first, then deterministically by `canonical_game_id`.
    pub fn find_by_game_id(&self, id: &str) -> Option<&LoadedPack> {
        if let Some(exact) = self.packs_by_id.get(id) {
            return Some(exact);
        }
        self.packs_for_game_id(id).into_iter().next()
    }

    /// Return all packs matching either `pack_id` or `canonical_game_id` sorted deterministically.
    pub fn packs_for_game_id(&self, game_or_pack_id: &str) -> Vec<&LoadedPack> {
        let mut list: Vec<&LoadedPack> = self
            .packs_by_id
            .values()
            .filter(|p| {
                p.manifest.canonical_game_id == game_or_pack_id
                    || p.manifest.pack_id == game_or_pack_id
            })
            .collect();
        list.sort_by(|a, b| a.manifest.pack_id.cmp(&b.manifest.pack_id));
        list
    }

    pub fn list(&self) -> Vec<&LoadedPack> {
        let mut list: Vec<&LoadedPack> = self.packs_by_id.values().collect();
        list.sort_by(|a, b| a.manifest.pack_id.cmp(&b.manifest.pack_id));
        list
    }

    pub fn summaries(&self) -> Vec<PackSummary> {
        self.list().into_iter().map(LoadedPack::summary).collect()
    }

    pub fn load_errors(&self) -> &[(String, String)] {
        &self.load_errors
    }

    /// Collect all non-empty `save_watchers[].file_glob` and `observation.sources[].file_glob` patterns.
    pub fn all_watch_file_globs(&self) -> Vec<String> {
        let mut globs = Vec::new();
        for p in self.list() {
            for w in &p.save_watchers {
                if !w.placeholder_template && !globs.contains(&w.file_glob) {
                    globs.push(w.file_glob.clone());
                }
            }
            if let Some(obs) = &p.manifest.profiles.observation {
                for s in &obs.sources {
                    if s.source_type == "trophy_dir" {
                        continue;
                    }
                    if let Some(fg) = &s.file_glob {
                        if fg == "**/savedata/**" {
                            continue;
                        }
                        if !globs.contains(fg) {
                            globs.push(fg.clone());
                        }
                    }
                }
            }
        }
        globs
    }

    /// Deterministic matcher precedence:
    /// 1. sidecar_uuid
    /// 2. rom_hash
    /// 3. recomp_game_id
    /// 4. build_id
    /// 5. asset_archive_hash
    /// 6. platform_id
    /// 7. app_id
    /// 8. executable
    /// 9. save_path_glob
    /// 100. canonical_game_id fallback
    /// Exact region/OS/launcher matches receive tie-breaker priority over wildcard matchers.
    pub fn match_pack(&self, query: &MatchQuery) -> Option<(&LoadedPack, PackMatchResult)> {
        let mut best: Option<(u16, &LoadedPack, PackMatchResult)> = None;

        for pack in self.packs_by_id.values() {
            for m in &pack.manifest.matchers {
                let (q_opt, value_ok) = match m.matcher_type {
                    MatcherType::SidecarUuid => (
                        query.sidecar_uuid.as_deref(),
                        query
                            .sidecar_uuid
                            .as_deref()
                            .map(|qv| qv.eq_ignore_ascii_case(&m.value))
                            .unwrap_or(false),
                    ),
                    MatcherType::RomHash => (
                        query.rom_hash.as_deref(),
                        query
                            .rom_hash
                            .as_deref()
                            .map(|qv| qv.eq_ignore_ascii_case(&m.value))
                            .unwrap_or(false),
                    ),
                    MatcherType::RecompGameId => (
                        query.recomp_game_id.as_deref(),
                        query
                            .recomp_game_id
                            .as_deref()
                            .map(|qv| qv.eq_ignore_ascii_case(&m.value))
                            .unwrap_or(false),
                    ),
                    MatcherType::BuildId => (
                        query.build_id.as_deref(),
                        query
                            .build_id
                            .as_deref()
                            .map(|qv| qv.eq_ignore_ascii_case(&m.value))
                            .unwrap_or(false),
                    ),
                    MatcherType::AssetArchiveHash => (
                        query.asset_archive_hash.as_deref(),
                        query
                            .asset_archive_hash
                            .as_deref()
                            .map(|qv| qv.eq_ignore_ascii_case(&m.value))
                            .unwrap_or(false),
                    ),
                    MatcherType::PlatformId => (
                        query.platform_id.as_deref(),
                        query
                            .platform_id
                            .as_deref()
                            .map(|qv| qv.eq_ignore_ascii_case(&m.value))
                            .unwrap_or(false),
                    ),
                    MatcherType::AppId => (
                        query.app_id.as_deref(),
                        query
                            .app_id
                            .as_deref()
                            .map(|qv| qv.eq_ignore_ascii_case(&m.value))
                            .unwrap_or(false),
                    ),
                    MatcherType::Executable => (
                        query.executable.as_deref(),
                        query
                            .executable
                            .as_deref()
                            .map(|qv| executable_matches(&m.value, qv))
                            .unwrap_or(false),
                    ),
                    MatcherType::SavePathGlob => (
                        query.save_path.as_deref(),
                        query
                            .save_path
                            .as_deref()
                            .map(|qv| glob_matches(&m.value, qv))
                            .unwrap_or(false),
                    ),
                };

                if q_opt.is_some() && value_ok {
                    let region_ok = match (&query.region, &m.region) {
                        (Some(qr), Some(mr)) => qr.eq_ignore_ascii_case(mr),
                        (Some(_), None) | (None, _) => true,
                    };
                    let os_ok = match (&query.os, &m.os) {
                        (Some(qo), Some(mo)) => {
                            mo.eq_ignore_ascii_case("any") || qo.eq_ignore_ascii_case(mo)
                        }
                        _ => true,
                    };
                    let launcher_ok = match (&query.launcher, &m.launcher) {
                        (Some(ql), Some(ml)) => ql.eq_ignore_ascii_case(ml),
                        _ => true,
                    };
                    if !region_ok || !os_ok || !launcher_ok {
                        continue;
                    }
                    let specificity_penalty: u16 =
                        if matches!((&query.region, &m.region), (Some(qr), Some(mr)) if qr.eq_ignore_ascii_case(mr)) {
                            0
                        } else {
                            1
                        } + if matches!((&query.os, &m.os), (Some(qo), Some(mo)) if qo.eq_ignore_ascii_case(mo)) {
                            0
                        } else {
                            1
                        } + if matches!((&query.launcher, &m.launcher), (Some(ql), Some(ml)) if ql.eq_ignore_ascii_case(ml)) {
                            0
                        } else {
                            1
                        };
                    let rank = m.matcher_type.precedence_rank();
                    let score = (rank as u16) * 8 + specificity_penalty;
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

            if let Some(gid) = &query.canonical_game_id {
                if pack.manifest.canonical_game_id == *gid {
                    let score = 100u16 * 8;
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
