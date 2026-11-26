//! Longshot (*The Legend of Zelda: Ocarina of Time* Nintendo Switch 2 -> PC)
//! binary save (`progress.sav`) Plane C importer.
//! Strictly isolated from `daemon/evaluation/`: emits `LOCALLY_OBSERVED` / `LOCAL` /
//! `UNVERIFIED` provenance and preserves missing timestamps as `None` (`NULL`).

use super::{
    resolve_mode, validate_observation_record, ImportOptions, ObservationRecord,
    ParsedObservationSet,
};
use crate::core::types::{
    deterministic_uuid_from_seed, now_rfc3339, sha256_hex, EvidenceClass, TrustTier,
    VerificationState,
};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub const DEFAULT_CANONICAL_GAME_ID: &str = "nintendo.oot.longshot.pc";
const LONGSHOT_SAVE_MAGIC: u32 = 0x0102_0304;
const HEADER_BYTES: usize = 0x28;
const MAX_SCAN_DEPTH: usize = 8;

// Verified MurmurHash3 (x86-32, seed 0) GameDataList hashes in Longshot `progress.sav`
const HASH_INTRO_DEKU_NAVI: u32 = 0xb041_ae87; // HavePlayedEvent.Dm_Kokiri_0030 (Bool == 1)
const HASH_SKULLTULAS_COLLECTED: u32 = 0x0473_5978; // PouchData.Item_Token_Gold.StockNum
const HASH_MAX_LIFE: u32 = 0xfbe0_1da1; // PlayerStatus.MaxLife (Quarter-hearts: 12 = 3 hearts, 80 = 20 hearts)
const HASH_HEART_PIECE_STOCK: u32 = 0x51ec_97b3; // PouchData.Obj_Heart_Piece.StockNum (0..3 remainder)
const HASH_BOOL64_TRUE_LIST: u32 = 0xa3db_7114; // Null-terminated LE u64 array of set Bool64bitKey flags
const HASH_DEFEATED_ENEMIES: u32 = 0x9152_eef8; // UInt64Array (u32 count + u64[])
const HASH_CURRENT_MAP: u32 = 0x1d61_89da; // String64 (e.g. "KokiriForest_Child")
const HASH_ALT_MAP: u32 = 0x4a6b_db7d; // String64
const HASH_EQUIP_PRIMARY: u32 = 0xfb11_7672; // String64
const HASH_EQUIP_SECONDARY: u32 = 0xa95a_3521; // String64
const HASH_IS_GET_MASTER_SWORD: u32 = 0x7ae3_8e5b; // IsGet.Weapon_Sword_Master (Bool == 1)
const HASH_STOCK_MASTER_SWORD: u32 = 0xf1c4_5294; // PouchData.Weapon_Sword_Master.StockNum (Int > 0)
const HASH_IS_GAME_CLEAR: u32 = 0xb496_2fee; // IsGameClear (Bool == 1)
const HASH_DEFEATED_GANON: u32 = 0x6d83_4709; // DefeatedEnemyNum.Enemy_DungeonBoss_Ganon (Int > 0)

// Exact `Bool64bitKey` hashes for dungeon boss Heart Containers from `Banc/*_Static.bcett.byml.zs`
const BOOL64_DEKU_TREE_HEART: u64 = 0xf914_fa36_7471_a3e3; // Queen Gohma / Kokiri's Emerald
const BOOL64_DODONGO_CAVERN_HEART: u64 = 0x5262_cea5_6c53_554d; // King Dodongo / Goron's Ruby
const BOOL64_JABU_JABU_HEART: u64 = 0xf45f_5319_d88e_7ac7; // Barinade / Zora's Sapphire
const BOOL64_FOREST_TEMPLE_HEART: u64 = 0xff11_593a_2277_8e7e; // Phantom Ganon / Forest Medallion
const BOOL64_FIRE_TEMPLE_HEART: u64 = 0x8389_cafb_1277_d318; // Volvagia / Fire Medallion
const BOOL64_WATER_TEMPLE_HEART: u64 = 0xc835_e1d0_3656_bf46; // Morpha / Water Medallion
const BOOL64_SHADOW_TEMPLE_HEART: u64 = 0xf43c_cb26_0f55_76dc; // Bongo Bongo / Shadow Medallion
const BOOL64_SPIRIT_TEMPLE_HEART: u64 = 0x4181_1f12_5b1e_edd5; // Twinrova / Spirit Medallion

// Explicit completion marker hash (Bool64 sentinel supported in synthetic/legacy binary saves)
const BOOL64_GANON_DEFEATED: u64 = 0x4741_4e4f_4e5f_434c; // "GANON_CL"

struct DungeonMilestoneRule {
    ach_id: &'static str,
    title: &'static str,
    is_adult_medallion: bool,
    bool64_heart: u64,
    is_get_item: u32,
    stock_item: u32,
    defeated_boss: u32,
}

const DUNGEON_MILESTONES: &[DungeonMilestoneRule] = &[
    DungeonMilestoneRule {
        ach_id: "a02",
        title: "Kokiri's Emerald",
        is_adult_medallion: false,
        bool64_heart: BOOL64_DEKU_TREE_HEART,
        is_get_item: 0x110b_83ec,   // IsGet.Item_Stone_Forest
        stock_item: 0xcbd1_e58c,    // PouchData.Item_Stone_Forest.StockNum
        defeated_boss: 0xee85_0ad3, // DefeatedEnemyNum.Enemy_DungeonBoss_Gohma
    },
    DungeonMilestoneRule {
        ach_id: "a03",
        title: "Goron's Ruby",
        is_adult_medallion: false,
        bool64_heart: BOOL64_DODONGO_CAVERN_HEART,
        is_get_item: 0x8908_d6ad,   // IsGet.Item_Stone_Fire
        stock_item: 0x7d69_ad47,    // PouchData.Item_Stone_Fire.StockNum
        defeated_boss: 0x1da4_58d0, // DefeatedEnemyNum.Enemy_DungeonBoss_Dodongo
    },
    DungeonMilestoneRule {
        ach_id: "a04",
        title: "Zora's Sapphire",
        is_adult_medallion: false,
        bool64_heart: BOOL64_JABU_JABU_HEART,
        is_get_item: 0xeec3_83f5,   // IsGet.Item_Stone_Water
        stock_item: 0xb8e3_bf45,    // PouchData.Item_Stone_Water.StockNum
        defeated_boss: 0x7253_79e3, // DefeatedEnemyNum.Enemy_DungeonBoss_Barinade
    },
    DungeonMilestoneRule {
        ach_id: "a06",
        title: "Forest Medallion",
        is_adult_medallion: true,
        bool64_heart: BOOL64_FOREST_TEMPLE_HEART,
        is_get_item: 0xfcd8_d653,   // IsGet.Item_Medal_Forest
        stock_item: 0x692d_57cb,    // PouchData.Item_Medal_Forest.StockNum
        defeated_boss: 0xc917_5b2a, // DefeatedEnemyNum.Enemy_DungeonBoss_PhantomGanon
    },
    DungeonMilestoneRule {
        ach_id: "a07",
        title: "Fire Medallion",
        is_adult_medallion: true,
        bool64_heart: BOOL64_FIRE_TEMPLE_HEART,
        is_get_item: 0x103a_0dc7,   // IsGet.Item_Medal_Fire
        stock_item: 0x5ce2_54a6,    // PouchData.Item_Medal_Fire.StockNum
        defeated_boss: 0xf6bb_b871, // DefeatedEnemyNum.Enemy_DungeonBoss_Volvagia
    },
    DungeonMilestoneRule {
        ach_id: "a08",
        title: "Water Medallion",
        is_adult_medallion: true,
        bool64_heart: BOOL64_WATER_TEMPLE_HEART,
        is_get_item: 0xcf64_e780,   // IsGet.Item_Medal_Water
        stock_item: 0x0151_a68f,    // PouchData.Item_Medal_Water.StockNum
        defeated_boss: 0x8846_1890, // DefeatedEnemyNum.Enemy_DungeonBoss_Morpha
    },
    DungeonMilestoneRule {
        ach_id: "a09",
        title: "Shadow Medallion",
        is_adult_medallion: true,
        bool64_heart: BOOL64_SHADOW_TEMPLE_HEART,
        is_get_item: 0x12d6_3ac6,   // IsGet.Item_Medal_Shadow
        stock_item: 0x204d_2063,    // PouchData.Item_Medal_Shadow.StockNum
        defeated_boss: 0xb439_6bb4, // DefeatedEnemyNum.Enemy_DungeonBoss_BongoBongo
    },
    DungeonMilestoneRule {
        ach_id: "a10",
        title: "Spirit Medallion",
        is_adult_medallion: true,
        bool64_heart: BOOL64_SPIRIT_TEMPLE_HEART,
        is_get_item: 0xe269_df38,   // IsGet.Item_Medal_Spirit
        stock_item: 0x0c7a_5fd0,    // PouchData.Item_Medal_Spirit.StockNum
        defeated_boss: 0x76eb_9d9e, // DefeatedEnemyNum.Enemy_DungeonBoss_Twinrova
    },
];

#[derive(Debug, Clone)]
pub struct ParsedLongshotSave {
    pub unlocked_achievements: Vec<(&'static str, &'static str)>,
    pub skulltulas_collected: i64,
    pub heart_pieces: i64,
    pub bosses_defeated: i64,
    pub max_hearts_quarters: i32,
    pub dungeon_containers_taken: usize,
    pub intro_awakened: bool,
    pub ganon_defeated: bool,
    pub master_sword: bool,
}

fn read_u32_le(bytes: &[u8], off: usize) -> Option<u32> {
    let slice: [u8; 4] = bytes.get(off..off + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(slice))
}

fn read_u64_le(bytes: &[u8], off: usize) -> Option<u64> {
    let slice: [u8; 8] = bytes.get(off..off + 8)?.try_into().ok()?;
    Some(u64::from_le_bytes(slice))
}

fn read_cstr_64(bytes: &[u8], off: usize) -> Option<String> {
    if off >= bytes.len() {
        return None;
    }
    let end = (off + 64).min(bytes.len());
    let slice = &bytes[off..end];
    let nul = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
    if nul == 0 {
        return None;
    }
    let raw = &slice[..nul];
    if !raw.iter().all(|&b| (0x20..0x7f).contains(&b)) {
        return None;
    }
    std::str::from_utf8(raw).ok().map(str::to_string)
}

fn collect_tail_strings(bytes: &[u8], data_off: usize, off: usize, out: &mut HashSet<String>) {
    if off < data_off || off >= bytes.len() {
        return;
    }
    if let Some(s) = read_cstr_64(bytes, off) {
        out.insert(s);
        return;
    }
    // Check String64Array layout: u32 count followed by `count * 64` bytes
    if let Some(count) = read_u32_le(bytes, off) {
        let count_usize = count as usize;
        if (1..=64).contains(&count_usize) {
            let needed = off.saturating_add(4).saturating_add(count_usize * 64);
            if needed <= bytes.len() {
                for idx in 0..count_usize {
                    let elem_off = off + 4 + idx * 64;
                    if let Some(s) = read_cstr_64(bytes, elem_off) {
                        out.insert(s);
                    }
                }
            }
        }
    }
}

/// Parses a single Longshot `progress.sav` byte slice without touching any gameplay evaluator.
pub fn parse_longshot_save_bytes(bytes: &[u8]) -> Result<ParsedLongshotSave, String> {
    if bytes.len() < HEADER_BYTES {
        return Err(format!(
            "Longshot save truncated ({} bytes < 40)",
            bytes.len()
        ));
    }
    let magic = read_u32_le(bytes, 0).ok_or("missing Longshot header magic")?;
    if magic != LONGSHOT_SAVE_MAGIC {
        return Err(format!(
            "invalid Longshot progress.sav magic 0x{:08x} (expected 0x01020304)",
            magic
        ));
    }
    let data_off = read_u32_le(bytes, 8).ok_or("missing Longshot data_off")? as usize;
    if data_off < HEADER_BYTES || data_off % 8 != 0 || data_off > bytes.len() {
        return Err(format!(
            "invalid Longshot data_off {} for {} byte save",
            data_off,
            bytes.len()
        ));
    }

    let mut entries: BTreeMap<u32, u32> = BTreeMap::new();
    let mut off = HEADER_BYTES;
    while off + 8 <= data_off {
        let hash = read_u32_le(bytes, off).unwrap();
        let val = read_u32_le(bytes, off + 4).unwrap();
        entries.insert(hash, val);
        off += 8;
    }

    let skulltulas_raw = entries
        .get(&HASH_SKULLTULAS_COLLECTED)
        .copied()
        .map(|v| v as i32)
        .unwrap_or(0)
        .max(0);
    let max_hearts_quarters = entries
        .get(&HASH_MAX_LIFE)
        .copied()
        .map(|v| v as i32)
        .unwrap_or(12)
        .max(0);
    let heart_piece_remainder = entries
        .get(&HASH_HEART_PIECE_STOCK)
        .copied()
        .map(|v| (v as i32) & 3)
        .unwrap_or(0);

    // Parse Null-terminated u64 Bool64bitKey list at HASH_BOOL64_TRUE_LIST
    let mut bool64_flags = HashSet::new();
    if let Some(&list_off_u32) = entries.get(&HASH_BOOL64_TRUE_LIST) {
        let mut cursor = list_off_u32 as usize;
        if cursor >= data_off {
            while let Some(v) = read_u64_le(bytes, cursor) {
                if v == 0 {
                    break;
                }
                bool64_flags.insert(v);
                cursor += 8;
            }
        }
    }

    // Parse UInt64Array at HASH_DEFEATED_ENEMIES (kept for backwards compatibility)
    let mut defeated_enemy_hashes = HashSet::new();
    if let Some(&enemy_off_u32) = entries.get(&HASH_DEFEATED_ENEMIES) {
        let enemy_off = enemy_off_u32 as usize;
        if enemy_off >= data_off {
            if let Some(count) = read_u32_le(bytes, enemy_off) {
                let cap = (count as usize).min(512);
                for i in 0..cap {
                    if let Some(h) = read_u64_le(bytes, enemy_off + 4 + i * 8) {
                        if h != 0 {
                            defeated_enemy_hashes.insert(h);
                        }
                    }
                }
            }
        }
    }

    // Parse map/equipped/pouch String64 + String64Array values
    let mut tail_strings = HashSet::new();
    for (&hash, &val_or_off) in &entries {
        if hash == HASH_BOOL64_TRUE_LIST || hash == HASH_DEFEATED_ENEMIES {
            continue;
        }
        collect_tail_strings(bytes, data_off, val_or_off as usize, &mut tail_strings);
    }

    let current_map = entries
        .get(&HASH_CURRENT_MAP)
        .or_else(|| entries.get(&HASH_ALT_MAP))
        .and_then(|&o| {
            let o = o as usize;
            if o >= data_off {
                read_cstr_64(bytes, o)
            } else {
                None
            }
        });

    let mut unlocked_achievements: Vec<(&'static str, &'static str)> = Vec::new();

    // a01: "It works, it works!" — Deku Tree / Navi opening cutscene completed
    let intro_awakened = entries.get(&HASH_INTRO_DEKU_NAVI).copied().unwrap_or(0) != 0;
    if intro_awakened {
        unlocked_achievements.push(("a01", "It works, it works!"));
    }

    let mut dungeon_bosses_cleared = 0usize;
    let mut dungeon_containers_taken = 0usize;
    let mut adult_medallions_count = 0usize;

    for rule in DUNGEON_MILESTONES {
        let heart_taken = bool64_flags.contains(&rule.bool64_heart);
        if heart_taken {
            dungeon_containers_taken += 1;
        }
        let boss_killed = entries.get(&rule.defeated_boss).copied().unwrap_or(0) as i32 > 0;
        let relic_obtained = entries.get(&rule.is_get_item).copied().unwrap_or(0) != 0
            || entries.get(&rule.stock_item).copied().unwrap_or(0) as i32 > 0;

        if heart_taken || boss_killed || relic_obtained {
            dungeon_bosses_cleared += 1;
            if rule.is_adult_medallion {
                adult_medallions_count += 1;
            }
            unlocked_achievements.push((rule.ach_id, rule.title));
        }
    }

    let has_master_sword_flag = entries
        .get(&HASH_IS_GET_MASTER_SWORD)
        .copied()
        .unwrap_or(0)
        != 0
        || entries
            .get(&HASH_STOCK_MASTER_SWORD)
            .copied()
            .unwrap_or(0) as i32
            > 0;
    let has_master_sword_str = tail_strings
        .iter()
        .any(|s| s == "Weapon_Sword_Master" || s == "Item_Sword_Master")
        || [HASH_EQUIP_PRIMARY, HASH_EQUIP_SECONDARY]
            .iter()
            .filter_map(|k| entries.get(k))
            .filter_map(|&o| {
                let o = o as usize;
                if o >= data_off {
                    read_cstr_64(bytes, o)
                } else {
                    None
                }
            })
            .any(|s| s == "Weapon_Sword_Master" || s == "Item_Sword_Master");

    // Pulling the Master Sword is required for Adult Temple progression in vanilla OoT.
    let master_sword =
        has_master_sword_flag || has_master_sword_str || adult_medallions_count > 0;
    if master_sword {
        unlocked_achievements.push(("a05", "Master Sword"));
    }

    let in_final_battle = matches!(
        current_map.as_deref(),
        Some("FinalBattleField") | Some("CrumblingGanonCastle")
    );
    let ganon_defeated = entries.get(&HASH_IS_GAME_CLEAR).copied().unwrap_or(0) != 0
        || entries.get(&HASH_DEFEATED_GANON).copied().unwrap_or(0) as i32 > 0
        || bool64_flags.contains(&BOOL64_GANON_DEFEATED)
        || defeated_enemy_hashes.contains(&BOOL64_GANON_DEFEATED)
        || tail_strings
            .iter()
            .any(|s| s == "Ganon_Defeated" || s == "GameClear")
        || (adult_medallions_count == 5 && in_final_battle);

    if ganon_defeated {
        unlocked_achievements.push(("a11", "Ganon Defeated"));
    }

    if skulltulas_raw >= 100 {
        unlocked_achievements.push(("a12", "100 Gold Skulltulas"));
    }

    if max_hearts_quarters >= 80 {
        unlocked_achievements.push(("a13", "All Heart Containers"));
    }

    // Sort canonically a01..a13
    unlocked_achievements.sort_by_key(|(id, _)| *id);

    // Heart Pieces formula derived from MaxLife quarter-hearts + loose piece stock:
    // total_hearts = starting 3 + taken dungeon boss containers (0..=8) + pieces / 4.
    let total_whole_hearts = (max_hearts_quarters / 4).max(0);
    let non_dungeon_extra_hearts =
        total_whole_hearts.saturating_sub(3 + dungeon_containers_taken as i32);
    let heart_pieces = ((non_dungeon_extra_hearts * 4) + heart_piece_remainder).clamp(0, 36) as i64;
    let bosses_defeated = (dungeon_bosses_cleared + usize::from(ganon_defeated)) as i64;

    Ok(ParsedLongshotSave {
        unlocked_achievements,
        skulltulas_collected: skulltulas_raw as i64,
        heart_pieces,
        bosses_defeated,
        max_hearts_quarters,
        dungeon_containers_taken,
        intro_awakened,
        ganon_defeated,
        master_sword,
    })
}

fn collect_longshot_save_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    children.sort();
    for p in children {
        if p.is_dir() {
            collect_longshot_save_files(&p, depth + 1, out);
        } else if p.is_file()
            && p.file_name()
                .and_then(|s| s.to_str())
                .map(|n| n.eq_ignore_ascii_case("progress.sav"))
                .unwrap_or(false)
        {
            out.push(p);
        }
    }
}

pub fn directory_has_longshot_saves(dir: &Path) -> bool {
    let mut files = Vec::new();
    collect_longshot_save_files(dir, 0, &mut files);
    !files.is_empty()
}

fn modified_nanos(path: &Path) -> u128 {
    fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Parses either a single `progress.sav` file or a directory of Longshot save slots
/// into `LOCALLY_OBSERVED` / `LOCAL` / `UNVERIFIED` Plane C observation records.
/// Missing timestamps are NEVER fabricated (`source_timestamp` is always `None`).
pub fn parse_longshot_path(
    path: &Path,
    opts: &ImportOptions,
) -> Result<ParsedObservationSet, String> {
    let save_files = if path.is_file() {
        vec![path.to_path_buf()]
    } else if path.is_dir() {
        let mut files = Vec::new();
        collect_longshot_save_files(path, 0, &mut files);
        files
    } else {
        return Err(format!("Longshot path '{}' does not exist", path.display()));
    };

    if save_files.is_empty() {
        return Err(format!(
            "no Longshot progress.sav files found under '{}'",
            path.display()
        ));
    }

    let canonical_game_id = opts
        .canonical_game_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_CANONICAL_GAME_ID)
        .to_string();
    let mode = resolve_mode(opts);

    let mut all_parsed: Vec<(PathBuf, String, u128, ParsedLongshotSave)> = Vec::new();
    for file in save_files {
        let bytes = fs::read(&file)
            .map_err(|e| format!("failed reading Longshot save '{}': {}", file.display(), e))?;
        let hash = sha256_hex(&bytes);
        let mtime = modified_nanos(&file);
        let parsed = parse_longshot_save_bytes(&bytes)
            .map_err(|e| format!("{}: {}", file.display(), e))?;
        all_parsed.push((file, hash, mtime, parsed));
    }

    // Union unlocked achievements across all discovered save slots
    let mut union_unlocks: BTreeMap<&'static str, (&'static str, String, PathBuf)> = BTreeMap::new();
    for (file, hash, _, parsed) in &all_parsed {
        for &(ach_id, title) in &parsed.unlocked_achievements {
            union_unlocks
                .entry(ach_id)
                .or_insert_with(|| (title, hash.clone(), file.clone()));
        }
    }

    // Choose the primary slot for STAT_SNAPSHOT emission: prefer highest active progress,
    // breaking ties by newest filesystem modification time.
    let (primary_file, primary_hash, _, primary_save) = all_parsed
        .iter()
        .max_by_key(|(_, _, mtime, p)| {
            (
                p.bosses_defeated,
                p.skulltulas_collected,
                p.max_hearts_quarters,
                *mtime,
            )
        })
        .expect("non-empty save_files checked");

    let mut records = Vec::new();
    let total_milestones = 13usize;
    let skipped_locked = total_milestones.saturating_sub(union_unlocks.len());

    for (ach_id, (title, artifact_hash, src_file)) in union_unlocks {
        let seed = format!(
            "longshot:{}:{}:{}",
            canonical_game_id,
            mode.as_str(),
            ach_id
        );
        let rec = ObservationRecord {
            schema_version: "0.1.0".to_string(),
            observation_id: deterministic_uuid_from_seed(&seed),
            canonical_game_id: canonical_game_id.clone(),
            observer_id: "plane_c.longshot_save".to_string(),
            source_type: "save_file".to_string(),
            observed_at: now_rfc3339(),
            source_timestamp: None,
            artifact_hash_sha256: Some(artifact_hash),
            record_kind: "ACHIEVEMENT_EVIDENCE".to_string(),
            key: ach_id.to_string(),
            unlocked: Some(true),
            int_value: Some(1),
            float_value: None,
            string_value: None,
            progression_mode: mode,
            trust_tier: TrustTier::Local,
            evidence_class: EvidenceClass::LocallyObserved,
            verification_state: VerificationState::Unverified,
            provenance_notes: Some(format!(
                "Observed '{}' milestone from Longshot save '{}'",
                title,
                src_file.display()
            )),
        };
        validate_observation_record(&rec)?;
        records.push(rec);
    }

    for (stat_key, stat_val) in [
        ("skulltulas_collected", primary_save.skulltulas_collected),
        ("heart_pieces", primary_save.heart_pieces),
        ("bosses_defeated", primary_save.bosses_defeated),
    ] {
        let seed = format!(
            "longshot_stat:{}:{}:{}:{}",
            canonical_game_id,
            mode.as_str(),
            stat_key,
            stat_val
        );
        let rec = ObservationRecord {
            schema_version: "0.1.0".to_string(),
            observation_id: deterministic_uuid_from_seed(&seed),
            canonical_game_id: canonical_game_id.clone(),
            observer_id: "plane_c.longshot_save".to_string(),
            source_type: "save_file".to_string(),
            observed_at: now_rfc3339(),
            source_timestamp: None,
            artifact_hash_sha256: Some(primary_hash.clone()),
            record_kind: "STAT_SNAPSHOT".to_string(),
            key: stat_key.to_string(),
            unlocked: None,
            int_value: Some(stat_val),
            float_value: None,
            string_value: None,
            progression_mode: mode,
            trust_tier: TrustTier::Local,
            evidence_class: EvidenceClass::LocallyObserved,
            verification_state: VerificationState::Unverified,
            provenance_notes: Some(format!(
                "Observed stat '{}'={} from Longshot save '{}'",
                stat_key,
                stat_val,
                primary_file.display()
            )),
        };
        validate_observation_record(&rec)?;
        records.push(rec);
    }

    Ok(ParsedObservationSet {
        records,
        skipped_locked,
        source_kind: "longshot_save".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap as TestMap;

    pub(crate) fn build_synthetic_longshot_save(
        skulltulas: i32,
        max_hearts_quarters: i32,
        bool64s: &[u64],
        extra_scalars: &[(u32, u32)],
        equip_primary: Option<&str>,
        current_map: Option<&str>,
    ) -> Vec<u8> {
        let mut scalars: TestMap<u32, u32> = TestMap::new();
        scalars.insert(HASH_SKULLTULAS_COLLECTED, skulltulas as u32);
        scalars.insert(HASH_MAX_LIFE, max_hearts_quarters as u32);
        for &(k, v) in extra_scalars {
            scalars.insert(k, v);
        }

        // Build tail blobs after 8-byte-aligned entry table
        let extra_entries = 1
            + usize::from(equip_primary.is_some())
            + usize::from(current_map.is_some());
        let table_bytes = (scalars.len() + extra_entries) * 8;
        let data_off = HEADER_BYTES + table_bytes;

        let mut tail: Vec<u8> = Vec::new();

        // Bool64bitKey list at data_off
        let bool64_off = (data_off + tail.len()) as u32;
        scalars.insert(HASH_BOOL64_TRUE_LIST, bool64_off);
        for &b in bool64s {
            tail.extend_from_slice(&b.to_le_bytes());
        }
        tail.extend_from_slice(&0u64.to_le_bytes());

        if let Some(eq) = equip_primary {
            let eq_off = (data_off + tail.len()) as u32;
            scalars.insert(HASH_EQUIP_PRIMARY, eq_off);
            let mut slot = [0u8; 64];
            let eb = eq.as_bytes();
            let copy_len = eb.len().min(63);
            slot[..copy_len].copy_from_slice(&eb[..copy_len]);
            tail.extend_from_slice(&slot);
        }

        if let Some(m) = current_map {
            let m_off = (data_off + tail.len()) as u32;
            scalars.insert(HASH_CURRENT_MAP, m_off);
            let mut slot = [0u8; 64];
            let mb = m.as_bytes();
            let copy_len = mb.len().min(63);
            slot[..copy_len].copy_from_slice(&mb[..copy_len]);
            tail.extend_from_slice(&slot);
        }

        let mut buf = vec![0u8; data_off + tail.len()];
        buf[0..4].copy_from_slice(&LONGSHOT_SAVE_MAGIC.to_le_bytes());
        buf[4..8].copy_from_slice(&0x002d_80ebu32.to_le_bytes());
        buf[8..12].copy_from_slice(&(data_off as u32).to_le_bytes());

        let mut cursor = HEADER_BYTES;
        for (h, v) in scalars {
            buf[cursor..cursor + 4].copy_from_slice(&h.to_le_bytes());
            buf[cursor + 4..cursor + 8].copy_from_slice(&v.to_le_bytes());
            cursor += 8;
        }
        buf[data_off..].copy_from_slice(&tail);
        buf
    }

    #[test]
    fn parses_synthetic_longshot_save_milestones_and_stats_with_null_timestamp() {
        let bytes = build_synthetic_longshot_save(
            100,
            80,
            &[
                BOOL64_DEKU_TREE_HEART,
                BOOL64_FOREST_TEMPLE_HEART,
                BOOL64_FIRE_TEMPLE_HEART,
                BOOL64_WATER_TEMPLE_HEART,
                BOOL64_SHADOW_TEMPLE_HEART,
                BOOL64_SPIRIT_TEMPLE_HEART,
            ],
            &[
                (HASH_INTRO_DEKU_NAVI, 1),
                (HASH_IS_GET_MASTER_SWORD, 1),
                (HASH_IS_GAME_CLEAR, 1),
            ],
            Some("Weapon_Sword_Biggoron"),
            Some("KokiriForest_Child"),
        );

        let parsed = parse_longshot_save_bytes(&bytes).expect("parse synthetic save");
        let ids: Vec<&str> = parsed.unlocked_achievements.iter().map(|(id, _)| *id).collect();
        assert_eq!(
            ids,
            vec!["a01", "a02", "a05", "a06", "a07", "a08", "a09", "a10", "a11", "a12", "a13"]
        );
        assert_eq!(parsed.skulltulas_collected, 100);
        // 20 total hearts - 3 base - 6 dungeon boss heart containers = 11 heart containers from pieces => clamp(44, 0..=36) = 36
        assert_eq!(parsed.heart_pieces, 36);
        assert_eq!(parsed.bosses_defeated, 7);

        let tmp = std::env::temp_dir().join(format!(
            "onehundo_longshot_unit_{}",
            crate::core::config::generate_uuid_v4()
        ));
        fs::create_dir_all(&tmp).unwrap();
        let save_path = tmp.join("progress.sav");
        fs::write(&save_path, &bytes).unwrap();

        let res = parse_longshot_path(&save_path, &ImportOptions::default()).unwrap();
        assert_eq!(res.source_kind, "longshot_save");
        assert!(res.records.iter().all(|r| {
            r.canonical_game_id == DEFAULT_CANONICAL_GAME_ID
                && r.source_timestamp.is_none()
                && r.evidence_class == EvidenceClass::LocallyObserved
                && r.trust_tier == TrustTier::Local
                && r.verification_state == VerificationState::Unverified
        }));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn parses_real_100_percent_and_early_game_hash_layouts() {
        let full_save = build_synthetic_longshot_save(
            100,
            80,
            &[
                BOOL64_DEKU_TREE_HEART,
                BOOL64_DODONGO_CAVERN_HEART,
                BOOL64_JABU_JABU_HEART,
                BOOL64_FOREST_TEMPLE_HEART,
                BOOL64_FIRE_TEMPLE_HEART,
                BOOL64_WATER_TEMPLE_HEART,
                BOOL64_SHADOW_TEMPLE_HEART,
                BOOL64_SPIRIT_TEMPLE_HEART,
            ],
            &[
                (HASH_INTRO_DEKU_NAVI, 1),
                (HASH_IS_GET_MASTER_SWORD, 1),
                (HASH_STOCK_MASTER_SWORD, 1),
                (HASH_IS_GAME_CLEAR, 1),
                (HASH_DEFEATED_GANON, 1),
            ],
            None,
            None,
        );
        let full = parse_longshot_save_bytes(&full_save).unwrap();
        assert_eq!(full.unlocked_achievements.len(), 13);
        assert_eq!(full.unlocked_achievements[0], ("a01", "It works, it works!"));
        assert_eq!(full.unlocked_achievements[12], ("a13", "All Heart Containers"));
        assert_eq!(full.skulltulas_collected, 100);
        assert_eq!(full.heart_pieces, 36);
        assert_eq!(full.bosses_defeated, 9);

        // Early game with 40 rupees (NOT Skulltulas) + 12 quarter-hearts + Dm_Kokiri_0030 complete
        let early_save = build_synthetic_longshot_save(
            0,
            12,
            &[],
            &[
                (HASH_INTRO_DEKU_NAVI, 1),
                (0xa779_21d7, 40), // PlayerStatus.CurrentRupee must NOT leak into skulltulas_collected
                (0x31ab_5580, 62), // PlayerStatus.Life must NOT count as MaxLife
            ],
            None,
            None,
        );
        let early = parse_longshot_save_bytes(&early_save).unwrap();
        assert_eq!(early.unlocked_achievements, vec![("a01", "It works, it works!")]);
        assert_eq!(early.skulltulas_collected, 0);
        assert_eq!(early.heart_pieces, 0);
        assert_eq!(early.bosses_defeated, 0);
    }

    #[test]
    fn parses_launcher_v2_nested_profile_directory() {
        let base = std::env::temp_dir().join(format!(
            "onehundo_longshot_l2_{}",
            crate::core::config::generate_uuid_v4()
        ));
        let slot_dir = base
            .join("default")
            .join("saves")
            .join("save")
            .join("5a3c9e1f0b7d24680000000000000001")
            .join("slot_00");
        fs::create_dir_all(&slot_dir).unwrap();
        let bytes = build_synthetic_longshot_save(
            0,
            12,
            &[],
            &[(HASH_INTRO_DEKU_NAVI, 1)],
            None,
            None,
        );
        fs::write(slot_dir.join("progress.sav"), &bytes).unwrap();

        assert!(directory_has_longshot_saves(&base));
        let res = parse_longshot_path(&base, &ImportOptions::default()).unwrap();
        assert!(res
            .records
            .iter()
            .any(|r| r.record_kind == "ACHIEVEMENT_EVIDENCE" && r.key == "a01" && r.unlocked == Some(true)));

        let _ = fs::remove_dir_all(&base);
    }
}
