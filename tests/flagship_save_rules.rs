mod common;

use onehundo_client::core::config::DaemonConfig;
use onehundo_client::core::engine::DaemonEngine;
use onehundo_client::core::types::ProgressionMode;
use onehundo_client::observation::external_importers::declarative_save::evaluate_save_file_with_pack;
use onehundo_client::observation::external_importers::{list_observation_history, ImportOptions};
use onehundo_client::packs::matcher::{MatchQuery, PackRegistry};
use onehundo_client::storage::state::{list_stats, list_trophies};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

fn init_engine_with_repo_packs(prefix: &str) -> (std::path::PathBuf, std::sync::Arc<DaemonEngine>) {
    let dir = common::temp_test_dir(prefix);
    let packs_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("packs");
    let mut cfg = DaemonConfig::default();
    cfg.config_dir = dir.join("cfg");
    cfg.data_dir = dir.join("data");
    cfg.packs_dirs = vec![packs_dir];
    cfg.notifications_enabled = false;
    let engine = DaemonEngine::initialize(cfg).expect("init engine");
    (dir, engine)
}

fn load_repo_registry() -> PackRegistry {
    let packs_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("packs");
    let mut reg = PackRegistry::new();
    let count = reg.discover_in_dir(&packs_dir);
    assert!(count >= 28, "expected >= 28 packs, got {}", count);
    assert!(
        reg.load_errors().is_empty(),
        "unexpected pack load errors: {:?}",
        reg.load_errors()
    );
    reg
}

fn assert_plane_c_local_provenance(
    engine: &DaemonEngine,
    canonical_game_id: &str,
    expected_ach_ids: &[&str],
) {
    let db = engine.db.lock().unwrap();
    let trophies = list_trophies(&db, Some(canonical_game_id)).unwrap();
    let actual: BTreeSet<String> = trophies
        .iter()
        .map(|t| t.canonical_achievement_id.clone())
        .collect();
    let expected: BTreeSet<String> = expected_ach_ids.iter().map(|s| (*s).to_string()).collect();
    assert_eq!(actual, expected, "achievement mismatch for {}", canonical_game_id);

    for t in &trophies {
        assert_eq!(t.progression_mode, "STANDARD");
        assert_eq!(t.trust_tier, "LOCAL");
        assert_eq!(t.evidence_class, "LOCALLY_OBSERVED");
        assert_eq!(t.verification_state, "UNVERIFIED");
    }

    let history = list_observation_history(&db, 100).unwrap();
    let game_hist: Vec<_> = history
        .into_iter()
        .filter(|row| row.canonical_game_id == canonical_game_id && row.progression_mode == "STANDARD")
        .collect();
    assert!(
        !game_hist.is_empty(),
        "expected observation_journal rows for {}",
        canonical_game_id
    );
    for row in &game_hist {
        assert_eq!(row.trust_tier, "LOCAL");
        assert_eq!(row.evidence_class, "LOCALLY_OBSERVED");
        assert_eq!(row.verification_state, "UNVERIFIED");
        assert_eq!(
            row.source_timestamp, None,
            "missing save timestamp must remain SQL NULL (never invented)"
        );
    }
}

#[test]
fn ship_of_harkinian_json_rules_unlock_idempotency_and_switch2_fallback() {
    let (dir, engine) = init_engine_with_repo_packs("flagship_soh");
    let reg = load_repo_registry();

    let oot_pack = reg
        .get("nintendo.oot.forks")
        .expect("nintendo.oot.forks pack must exist");
    assert_eq!(oot_pack.manifest.pack_version.as_deref(), Some("0.2.0"));
    assert_eq!(oot_pack.manifest.achievements.len(), 12);

    // Switch 2 fork fallback matchers still resolve cleanly, and placeholder template emits 0 unlocks.
    let (_, matched_relink) = reg
        .match_pack(&MatchQuery {
            recomp_game_id: Some("rekarina".into()),
            ..Default::default()
        })
        .expect("Switch 2 fork matcher resolution");
    assert_eq!(matched_relink.pack_id, "nintendo.oot.forks");

    let fork_dir = dir.join("relink_saves");
    fs::create_dir_all(&fork_dir).unwrap();
    let fork_bin = fork_dir.join("relink_save_01.bin");
    fs::write(&fork_bin, [0xFFu8; 64]).unwrap();
    let fork_eval = evaluate_save_file_with_pack(
        oot_pack,
        Some(&fork_dir),
        &fork_bin,
        &ImportOptions::default(),
    )
    .unwrap();
    assert!(
        fork_eval.records.is_empty(),
        "switch2_fork_save_template must remain placeholder_template: true and emit 0 unlocks"
    );

    // Full Ship of Harkinian Save/file1.sav (nested `save.*` JSON schema)
    let soh_save_dir = dir.join("ShipOfHarkinian/Save");
    fs::create_dir_all(&soh_save_dir).unwrap();
    let soh_file1 = soh_save_dir.join("file1.sav");

    // questItems bits: medallions (bits 0..5 = 0x3F), Song of Time (bit 16),
    // Kokiri Emerald (bit 18), Goron's Ruby (bit 19), Zora's Sapphire (bit 20).
    let quest_items: u32 = 0x3F | (1 << 16) | (1 << 18) | (1 << 19) | (1 << 20);
    // equipment bits: Kokiri Sword (bit 0 = 0x01), Master Sword (bit 1 = 0x02), Deku Shield (bit 4 = 0x10)
    let equipment: u32 = 0x01 | 0x02 | 0x10;
    let file1_json = serde_json::json!({
        "save": {
            "healthCapacity": 320,
            "isGameClear": true,
            "inventory": {
                "items": [0, 0, 0, 3, 0, 0, 0, 8],
                "equipment": equipment,
                "questItems": quest_items,
                "gsTokens": 100
            },
            "sohStats": {
                "heartPieces": 36
            },
            "flags": {
                "ganonDefeated": true
            }
        }
    });
    fs::write(&soh_file1, serde_json::to_string_pretty(&file1_json).unwrap()).unwrap();

    let first = engine
        .import_plane_c_path(
            &soh_file1,
            Some("nintendo.oot.switch2".into()),
            None,
            false,
        )
        .expect("import SoH file1.sav");
    assert_eq!(first.source_kind, "declarative_save");
    assert_eq!(first.imported, 17); // 12 achievements + 5 stats
    assert_eq!(first.deduplicated, 0);

    assert_plane_c_local_provenance(
        &engine,
        "nintendo.oot.switch2",
        &[
            "a01", "a02", "a03", "a04", "a05", "a06", "a07", "a08", "a09", "a10", "a11", "a13",
        ],
    );

    {
        let db = engine.db.lock().unwrap();
        let stats = list_stats(&db, Some("nintendo.oot.switch2")).unwrap();
        let gs = stats.iter().find(|s| s.stat_key == "gold_skulltulas").unwrap();
        let hc = stats.iter().find(|s| s.stat_key == "heart_capacity").unwrap();
        let hp = stats.iter().find(|s| s.stat_key == "heart_pieces").unwrap();
        let qi = stats.iter().find(|s| s.stat_key == "quest_items_mask").unwrap();
        let eq = stats.iter().find(|s| s.stat_key == "equipment_mask").unwrap();
        assert_eq!(gs.int_value, Some(100));
        assert_eq!(hc.int_value, Some(320));
        assert_eq!(hp.int_value, Some(36));
        assert_eq!(qi.int_value, Some(quest_items as i64));
        assert_eq!(eq.int_value, Some(equipment as i64));
    }

    // Re-importing exact same save must be idempotent
    let second = engine
        .import_plane_c_path(
            &soh_file1,
            Some("nintendo.oot.switch2".into()),
            None,
            false,
        )
        .expect("idempotent re-import");
    assert_eq!(second.imported, 0);
    assert_eq!(second.deduplicated, 17);

    // Also verify root-level `soh.json` schema evaluation with partial unlock progression
    let soh_root = soh_save_dir.join("soh.json");
    let root_json = serde_json::json!({
        "flags": { "kokiriLoadout": true },
        "questStatus": { "emerald": true, "ruby": true },
        "inventory": { "gsTokens": 55 },
        "healthCapacity": 240
    });
    fs::write(&soh_root, serde_json::to_string(&root_json).unwrap()).unwrap();
    let root_eval = evaluate_save_file_with_pack(
        oot_pack,
        Some(&soh_save_dir),
        &soh_root,
        &ImportOptions::default(),
    )
    .unwrap();
    let unlocked_root: BTreeSet<String> = root_eval
        .records
        .iter()
        .filter(|r| r.record_kind == "ACHIEVEMENT_EVIDENCE")
        .map(|r| r.key.clone())
        .collect();
    let expected_root: BTreeSet<String> = ["a01", "a02", "a03", "a07"]
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(unlocked_root, expected_root);

    let _ = fs::remove_dir_all(&dir);
}

fn build_sm64_eeprom_slot0(big_endian: bool, full_120: bool) -> Vec<u8> {
    let mut buf = vec![0u8; 0x38];
    // 0x08..0x0C: flags (caps bits 1..3 = 0x0E; Bowser keys bits 4..5 = 0x30; Toad/MIPS bits 24..28 = 0x1F000000)
    let flags: u32 = if full_120 {
        0x1F00_003E
    } else {
        0x0000_0002 // only Wing Cap switch + no Bowser keys
    };
    let flag_bytes = if big_endian {
        flags.to_be_bytes()
    } else {
        flags.to_le_bytes()
    };
    buf[0x08..0x0C].copy_from_slice(&flag_bytes);

    if full_120 {
        // 15 main courses (0x0C..0x1A): 7 stars each = 0x7F -> 105 stars
        for byte in &mut buf[0x0C..0x1B] {
            *byte = 0x7F;
        }
        // 9 bonus/secret courses (0x1B..0x23): BitDW(1), BitFS(1), BitS(1), PSS(2), CotMC(1), TotWC(1), VCUtM(1), WMotR(1), SA(1) = 10 stars
        buf[0x1B] = 0x01;
        buf[0x1C] = 0x01;
        buf[0x1D] = 0x01;
        buf[0x1E] = 0x03;
        buf[0x1F] = 0x01;
        buf[0x20] = 0x01;
        buf[0x21] = 0x01;
        buf[0x22] = 0x01;
        buf[0x23] = 0x01;
        buf[0x25] = 148; // BoB coin high score
        buf[0x33] = 122; // Rainbow Ride coin high score
    } else {
        // Only BoB star 1
        buf[0x0C] = 0x01;
        buf[0x25] = 12;
    }

    // 0x34..0x36: EEPROM signature magic (`0x4441` 'DA' BE vs `0x41 0x44` LE PC port)
    let sig: u16 = 0x4441;
    let sig_bytes = if big_endian {
        sig.to_be_bytes()
    } else {
        sig.to_le_bytes()
    };
    buf[0x34..0x36].copy_from_slice(&sig_bytes);
    buf
}

#[test]
fn super_mario_64_eeprom_binary_le_be_srm_and_corrupt_magic() {
    let (dir, engine) = init_engine_with_repo_packs("flagship_sm64");
    let reg = load_repo_registry();
    let sm64_pack = reg
        .get("nintendo.sm64.recomp")
        .expect("nintendo.sm64.recomp pack must exist");
    assert_eq!(sm64_pack.manifest.pack_version.as_deref(), Some("0.2.0"));

    let save_dir = dir.join("sm64_saves");
    fs::create_dir_all(&save_dir).unwrap();
    let pc_bin = save_dir.join("sm64_save_file.bin");

    // Step 1: Early progress (1 star in BoB, no all caps/keys/120)
    fs::write(&pc_bin, build_sm64_eeprom_slot0(false, false)).unwrap();
    let partial = evaluate_save_file_with_pack(
        sm64_pack,
        Some(&save_dir),
        &pc_bin,
        &ImportOptions::default(),
    )
    .unwrap();
    let partial_achs: Vec<_> = partial
        .records
        .iter()
        .filter(|r| r.record_kind == "ACHIEVEMENT_EVIDENCE")
        .map(|r| r.key.as_str())
        .collect();
    assert_eq!(partial_achs, vec!["ach_first_star"]);

    // Step 2: Full 120-star little-endian PC save imported through DaemonEngine
    fs::write(&pc_bin, build_sm64_eeprom_slot0(false, true)).unwrap();
    let full_res = engine
        .import_plane_c_path(&pc_bin, None, None, false)
        .expect("import SM64 PC binary save");
    assert_eq!(full_res.source_kind, "declarative_save");
    assert_eq!(full_res.imported, 8); // 5 achievements + 3 stats
    assert_eq!(full_res.deduplicated, 0);

    assert_plane_c_local_provenance(
        &engine,
        "nintendo.super_mario_64",
        &[
            "ach_first_star",
            "ach_all_caps",
            "ach_bowser_keys",
            "ach_castle_secrets",
            "ach_120_stars",
        ],
    );
    {
        let db = engine.db.lock().unwrap();
        let stats = list_stats(&db, Some("nintendo.super_mario_64")).unwrap();
        assert_eq!(
            stats.iter().find(|s| s.stat_key == "save_flags").unwrap().int_value,
            Some(0x1F00_003E)
        );
        assert_eq!(
            stats.iter().find(|s| s.stat_key == "bob_coin_score").unwrap().int_value,
            Some(148)
        );
        assert_eq!(
            stats.iter().find(|s| s.stat_key == "rr_coin_score").unwrap().int_value,
            Some(122)
        );
    }

    // Step 3: Idempotent re-import
    let dup_res = engine
        .import_plane_c_path(&pc_bin, None, None, false)
        .unwrap();
    assert_eq!(dup_res.imported, 0);
    assert_eq!(dup_res.deduplicated, 8);

    // Step 4: Big-endian N64 save & RetroArch `.srm`
    let n64_dir = dir.join("n64_be");
    fs::create_dir_all(&n64_dir).unwrap();
    let be_bin = n64_dir.join("sm64_save_file.bin");
    fs::write(&be_bin, build_sm64_eeprom_slot0(true, true)).unwrap();
    let be_eval = evaluate_save_file_with_pack(
        sm64_pack,
        Some(&n64_dir),
        &be_bin,
        &ImportOptions::default(),
    )
    .unwrap();
    assert_eq!(
        be_eval
            .records
            .iter()
            .filter(|r| r.record_kind == "ACHIEVEMENT_EVIDENCE")
            .count(),
        5
    );

    let srm_path = n64_dir.join("Super Mario 64 (USA).srm");
    fs::write(&srm_path, build_sm64_eeprom_slot0(true, true)).unwrap();
    let srm_eval = evaluate_save_file_with_pack(
        sm64_pack,
        Some(&n64_dir),
        &srm_path,
        &ImportOptions::default(),
    )
    .unwrap();
    assert_eq!(
        srm_eval
            .records
            .iter()
            .filter(|r| r.record_kind == "ACHIEVEMENT_EVIDENCE")
            .count(),
        5
    );

    // Step 5: Corrupted magic must emit zero achievements
    let mut bad_buf = build_sm64_eeprom_slot0(false, true);
    bad_buf[0x34] = 0xFF;
    bad_buf[0x35] = 0xFF;
    let bad_path = dir.join("bad_sm64/sm64_save_file.bin");
    fs::create_dir_all(bad_path.parent().unwrap()).unwrap();
    fs::write(&bad_path, &bad_buf).unwrap();
    let bad_eval = evaluate_save_file_with_pack(
        sm64_pack,
        Some(bad_path.parent().unwrap()),
        &bad_path,
        &ImportOptions::default(),
    )
    .unwrap();
    assert!(
        bad_eval.records.is_empty(),
        "corrupt SM64 magic must never emit records"
    );

    let _ = fs::remove_dir_all(&dir);
}

fn build_bb_sprj0005_userdata(magic: &[u8; 8], version: u32, boss_mask: u32) -> Vec<u8> {
    let mut buf = vec![0u8; 64];
    buf[0x00..0x08].copy_from_slice(magic);
    buf[0x08..0x0C].copy_from_slice(&version.to_le_bytes());
    buf[0x0C..0x10].copy_from_slice(&3_600_000u32.to_le_bytes()); // playtime_ms
    buf[0x10..0x14].copy_from_slice(&42_500u32.to_le_bytes()); // blood_echoes_held
    buf[0x14..0x18].copy_from_slice(&15u32.to_le_bytes()); // insight
    buf[0x18..0x1C].copy_from_slice(&boss_mask.to_le_bytes());
    buf
}

#[test]
fn bloodborne_sprj0005_userdata_binary_and_spectator_isolation() {
    let (dir, engine) = init_engine_with_repo_packs("flagship_bb");

    let bb_dir = dir.join("shadPS4/user/savedata/1/SPRJ0005");
    fs::create_dir_all(&bb_dir).unwrap();
    let userdata0 = bb_dir.join("userdata0000");
    fs::write(&userdata0, build_bb_sprj0005_userdata(b"SPRJ0005", 0x6E, 0x07)).unwrap();

    // Non-matching magic/version file in sibling slot must be ignored safely
    let userdata1 = bb_dir.join("userdata0001");
    fs::write(&userdata1, build_bb_sprj0005_userdata(b"NOTBB000", 0x6E, 0x07)).unwrap();

    // 1. SPECTATOR mode journals records without mutating trophy_state or stat_state
    let spec_res = engine
        .import_plane_c_path(
            &userdata0,
            Some("sony.bloodborne".into()),
            Some(ProgressionMode::Spectator),
            false,
        )
        .expect("spectator import userdata0000");
    assert_eq!(spec_res.imported, 6); // 3 achievements + 3 stats journaled
    {
        let db = engine.db.lock().unwrap();
        assert!(list_trophies(&db, Some("sony.bloodborne")).unwrap().is_empty());
        assert!(list_stats(&db, Some("sony.bloodborne")).unwrap().is_empty());
    }

    // 2. Register watch folder and poll in STANDARD mode
    engine
        .add_watch_override(Some("sony.bloodborne".into()), &bb_dir)
        .unwrap();
    let poll = engine.poll_watched_observation_dirs();
    assert!(poll.errors.is_empty(), "poll errors: {:?}", poll.errors);
    assert_eq!(poll.files_scanned, 2);
    assert_eq!(poll.imported, 6); // Only userdata0000 unlocked; userdata0001 skipped cleanly

    assert_plane_c_local_provenance(
        &engine,
        "sony.bloodborne",
        &["ach_cleric_beast", "ach_father_gascoigne", "ach_bloodborne_plat"],
    );
    {
        let db = engine.db.lock().unwrap();
        let stats = list_stats(&db, Some("sony.bloodborne")).unwrap();
        assert_eq!(
            stats.iter().find(|s| s.stat_key == "playtime_ms").unwrap().int_value,
            Some(3_600_000)
        );
        assert_eq!(
            stats
                .iter()
                .find(|s| s.stat_key == "blood_echoes_held")
                .unwrap()
                .int_value,
            Some(42_500)
        );
        assert_eq!(
            stats.iter().find(|s| s.stat_key == "insight").unwrap().int_value,
            Some(15)
        );
    }

    // 3. Re-poll sees zero changed files; direct re-import deduplicates completely
    let repoll = engine.poll_watched_observation_dirs();
    assert_eq!(repoll.files_changed, 0);
    assert_eq!(repoll.imported, 0);

    let reimport = engine
        .import_plane_c_path(&userdata0, Some("sony.bloodborne".into()), None, false)
        .unwrap();
    assert_eq!(reimport.imported, 0);
    assert_eq!(reimport.deduplicated, 6);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn opengoal_jak1_binary_and_unverified_template_guard() {
    let (dir, engine) = init_engine_with_repo_packs("flagship_jak1");
    let reg = load_repo_registry();

    let jak1_dir = dir.join("OpenGOAL/jak1/saves");
    fs::create_dir_all(&jak1_dir).unwrap();
    let jak1_save = jak1_dir.join("jak1_slot0.bin");

    let mut jak1_buf = vec![0u8; 64];
    jak1_buf[0x00..0x04].copy_from_slice(b"OGJ1");
    jak1_buf[0x04..0x08].copy_from_slice(&0x68u32.to_le_bytes()); // SAVE_VERSION 104
    jak1_buf[0x10..0x14].copy_from_slice(&101u32.to_le_bytes()); // power_cells
    jak1_buf[0x14..0x18].copy_from_slice(&2000u32.to_le_bytes()); // precursor_orbs
    jak1_buf[0x18..0x1C].copy_from_slice(&112u32.to_le_bytes()); // scout_flies
    fs::write(&jak1_save, &jak1_buf).unwrap();

    // Auto-resolve pack from save path (`canonical_game_id: None`)
    let res = engine
        .import_plane_c_path(&jak1_save, None, None, false)
        .expect("import OpenGOAL Jak 1 save");
    assert_eq!(res.source_kind, "declarative_save");
    assert_eq!(res.imported, 7); // 4 achievements + 3 stats
    assert_eq!(res.deduplicated, 0);

    assert_plane_c_local_provenance(
        &engine,
        "opengoal.jak_and_daxter",
        &[
            "ach_power_cell_1",
            "ach_101_cells",
            "ach_all_scout_flies",
            "ach_precursor_orbs_2000",
        ],
    );
    {
        let db = engine.db.lock().unwrap();
        let stats = list_stats(&db, Some("opengoal.jak_and_daxter")).unwrap();
        assert_eq!(
            stats.iter().find(|s| s.stat_key == "power_cells").unwrap().int_value,
            Some(101)
        );
        assert_eq!(
            stats
                .iter()
                .find(|s| s.stat_key == "precursor_orbs")
                .unwrap()
                .int_value,
            Some(2000)
        );
        assert_eq!(
            stats.iter().find(|s| s.stat_key == "scout_flies").unwrap().int_value,
            Some(112)
        );
    }

    // Idempotent re-import
    let dup = engine
        .import_plane_c_path(&jak1_save, Some("opengoal.jak_and_daxter".into()), None, false)
        .unwrap();
    assert_eq!(dup.imported, 0);
    assert_eq!(dup.deduplicated, 7);

    // Unverified Stage 1 pack (Jak II) stays placeholder_template: true and must emit 0 unlocks
    let jak2_pack = reg
        .get("opengoal.jak2.starter")
        .expect("opengoal.jak2.starter pack");
    let jak2_dir = dir.join("OpenGOAL/jak2/saves");
    fs::create_dir_all(&jak2_dir).unwrap();
    let jak2_save = jak2_dir.join("jak2_slot0.bin");
    fs::write(&jak2_save, [0xAAu8; 64]).unwrap();
    let jak2_eval = evaluate_save_file_with_pack(
        jak2_pack,
        Some(&jak2_dir),
        &jak2_save,
        &ImportOptions::default(),
    )
    .unwrap();
    assert!(
        jak2_eval.records.is_empty(),
        "placeholder_template packs must emit 0 unlocks"
    );

    let _ = fs::remove_dir_all(&dir);
}
