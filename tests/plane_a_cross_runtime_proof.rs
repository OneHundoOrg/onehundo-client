mod common;

use onehundo_client::core::config::{save_credentials, Credentials, DaemonConfig};
use onehundo_client::core::engine::DaemonEngine;
use onehundo_client::core::types::{
    EventKind, IngestionPlane, LifecycleSignal, ProgressionMode, TrustTier,
};
use onehundo_client::diagnostics::server::start_health_http_server;
use onehundo_client::evaluation::memory_arena::{
    parse_rcheevos_memaddr, MemoryArenaEvaluatorState,
};
use onehundo_client::ipc::unix_socket::start_unix_ipc_server;
use onehundo_client::packs::manifest::{LoadedPack, MemoryConditionSpec, MemoryRule};
use onehundo_client::packs::matcher::MatchQuery;
use onehundo_client::storage::journal::{fetch_pending_sync_events, query_journal_stats};
use onehundo_client::storage::state::{list_stats, list_trophies};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

fn compile_c99(src: &Path, out_bin: &Path) {
    let proto_root = common::protocol_root();
    let transport_c = proto_root.join("reference/c/transport_posix.c");
    let out = Command::new("gcc")
        .args([
            "-std=c99",
            "-pedantic",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-I",
            proto_root.join("include").to_str().unwrap(),
            "-I",
            proto_root.join("reference/c").to_str().unwrap(),
            src.to_str().unwrap(),
            transport_c.to_str().unwrap(),
            "-lrt",
            "-o",
            out_bin.to_str().unwrap(),
        ])
        .output()
        .expect("invoke gcc");
    assert!(
        out.status.success(),
        "gcc {} failed:\n{}",
        src.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn rcheevos_evaluator_advanced_ops_memaddr_parser_and_excessive_gap_downgrade() {
    // 1. Parse RA memaddr string including AddSource, SubSource, Prior, Once/Hits, ResetIf, Measured, AltGroups
    let (core, alts) = parse_rcheevos_memaddr(
        "A:0xH04_0xH05>=5_M:0xX08>=10_R:0xH0C=99_P:0xL10=3S0xM12=1(2)S0xN12=1",
    )
    .expect("parse memaddr");
    assert_eq!(core.len(), 5);
    assert_eq!(core[0].kind, "add_source");
    assert_eq!(core[0].size, "u8");
    assert_eq!(core[1].op, ">=");
    assert_eq!(core[1].value, Some(5));
    assert_eq!(core[2].kind, "measured");
    assert_eq!(core[2].size, "u32_le");
    assert_eq!(core[3].kind, "reset_if");
    assert_eq!(core[4].kind, "pause_if");
    assert_eq!(core[4].size, "lower4");
    assert_eq!(alts.len(), 2);
    assert_eq!(alts[0][0].size, "bit0");
    assert_eq!(alts[0][0].required_hits, 2);
    assert_eq!(alts[1][0].size, "bit1");

    // 2. Test Prior vs Delta, AddHits, OrNext, big-endian u32_be, and ExcessiveFrameGap downgrade
    let dir = common::temp_test_dir("rcheevos_ops");
    let mut cfg = DaemonConfig::default();
    cfg.config_dir = dir.join("cfg");
    cfg.data_dir = dir.join("data");
    cfg.packs_dirs = vec![];
    cfg.notifications_enabled = false;
    cfg.plane_a_max_frame_gaps_before_downgrade = 6;

    let engine = DaemonEngine::initialize(cfg).expect("init engine");

    let manifest_json = serde_json::json!({
        "manifest_version": "0.1.0",
        "pack_id": "test.rcheevos.ops",
        "pack_version": "0.1.0",
        "canonical_game_id": "test.rcheevos.game",
        "title": "Rcheevos Ops Test",
        "matchers": [{ "matcher_type": "rom_hash", "value": "abc123" }],
        "profiles": {
            "memory": {
                "enabled": true,
                "architecture": "mips64",
                "default_endianness": "BIG",
                "regions": [{ "region_name": "RDRAM", "guest_base_address": "0x80000000", "size_bytes": 4096 }]
            }
        },
        "achievements": [
            {
                "canonical_achievement_id": "ach_prior_transition",
                "title": "Prior Value Checked",
                "description": "Triggers using Prior + OrNext + AddHits + u32_be",
                "points": 15,
                "supported_modes": ["STANDARD"],
                "minimum_trust_tier": "LOCAL"
            }
        ]
    })
    .to_string();
    let manifest =
        onehundo_client::packs::manifest::validate_and_parse_manifest(&manifest_json, None).unwrap();

    let rule = MemoryRule {
        achievement_id: "ach_prior_transition".to_string(),
        memaddr: None,
        conditions: vec![
            // Prior value at 0x00 (u32_be) must equal 100 while current equals 200
            MemoryConditionSpec {
                region_id: 0,
                address: "0x00".into(),
                size: "u32_be".into(),
                op: "==".into(),
                value: Some(100),
                compare_delta: false,
                kind: "normal".into(),
                required_hits: 0,
                lhs_type: Some("prior".into()),
                rhs_type: Some("const".into()),
                rhs_region_id: None,
                rhs_address: None,
                rhs_size: None,
            },
            MemoryConditionSpec {
                region_id: 0,
                address: "0x00".into(),
                size: "u32_be".into(),
                op: "==".into(),
                value: Some(200),
                compare_delta: false,
                kind: "normal".into(),
                required_hits: 0,
                lhs_type: Some("mem".into()),
                rhs_type: Some("const".into()),
                rhs_region_id: None,
                rhs_address: None,
                rhs_size: None,
            },
            // OrNext: either 0x04 bitcount >= 3 OR 0x05 upper4 == 0xA
            MemoryConditionSpec {
                region_id: 0,
                address: "0x04".into(),
                size: "bitcount".into(),
                op: ">=".into(),
                value: Some(3),
                compare_delta: false,
                kind: "or_next".into(),
                required_hits: 0,
                lhs_type: None,
                rhs_type: None,
                rhs_region_id: None,
                rhs_address: None,
                rhs_size: None,
            },
            MemoryConditionSpec {
                region_id: 0,
                address: "0x05".into(),
                size: "upper4".into(),
                op: "==".into(),
                value: Some(0xA),
                compare_delta: false,
                kind: "once".into(),
                required_hits: 1,
                lhs_type: None,
                rhs_type: None,
                rhs_region_id: None,
                rhs_address: None,
                rhs_size: None,
            },
        ],
        alt_groups: vec![],
    };

    engine.packs.lock().unwrap().insert_pack(LoadedPack {
        manifest,
        semantic_rules: vec![],
        memory_rules: vec![rule],
        memory_stats: vec![],
        save_watchers: vec![],
        pack_sha256: "0".repeat(64),
        mapped_oap_edition: None,
        source_path: "in-memory".into(),
    });

    let sid = "aaaaaaaa-1111-4111-8111-111111111111".to_string();
    engine.start_session(
        sid.clone(),
        "mupen64".into(),
        "test.rcheevos.game".into(),
        MatchQuery {
            canonical_game_id: Some("test.rcheevos.game".into()),
            sidecar_uuid: None,
            rom_hash: Some("abc123".into()),
            recomp_game_id: None,
            build_id: None,
            asset_archive_hash: None,
            platform_id: None,
            region: None,
            ..Default::default()
        },
        IngestionPlane::MemoryArena,
        ProgressionMode::Standard,
        TrustTier::VerifiedIntegration,
    );
    engine.register_plane_a_region(&sid, 0, 0);

    let mut ram = vec![0u8; 64];
    ram[0..4].copy_from_slice(&100u32.to_be_bytes());
    let f1 = engine
        .handle_plane_a_frame(&sid, 1, LifecycleSignal::NormalFrame, 0, &ram)
        .unwrap();
    assert!(f1.unlocked_achievements.is_empty());

    // Transition 100 -> 200 on frame 2, keep 200 on frame 3 (Delta=200, Prior STILL=100!), satisfy OrNext on frame 3
    ram[0..4].copy_from_slice(&200u32.to_be_bytes());
    let f2 = engine
        .handle_plane_a_frame(&sid, 2, LifecycleSignal::NormalFrame, 0, &ram)
        .unwrap();
    assert!(f2.unlocked_achievements.is_empty());

    ram[0x04] = 0b0000_0111; // bitcount == 3 satisfies OrNext -> Once triggers!
    let f3 = engine
        .handle_plane_a_frame(&sid, 3, LifecycleSignal::NormalFrame, 0, &ram)
        .unwrap();
    assert_eq!(f3.unlocked_achievements, vec!["ach_prior_transition"]);

    // Verify simultaneous skipped + explicit FrameGap does NOT double-count (seq 3 -> 7 = 3 gaps, not 6)
    let f_gap = engine
        .handle_plane_a_frame(&sid, 7, LifecycleSignal::FrameGap, 3, &ram)
        .unwrap();
    assert!(f_gap.frame_gap_detected);
    assert_eq!(f_gap.gap_count, 3);
    // Below threshold (3 < 6) -> still VERIFIED_INTEGRATION
    let health_mid = engine.health_snapshot().unwrap();
    assert_eq!(health_mid.sessions[0].active_trust_tier, TrustTier::VerifiedIntegration);

    // Exceed threshold (3 + 4 = 7 >= 6) -> automatic downgrade to LOCAL (excessive_frame_gap)
    let f_gap2 = engine
        .handle_plane_a_frame(&sid, 8, LifecycleSignal::FrameGap, 4, &ram)
        .unwrap();
    assert_eq!(f_gap2.gap_count, 4);
    let health_after = engine.health_snapshot().unwrap();
    let sess = &health_after.sessions[0];
    assert_eq!(sess.active_trust_tier, TrustTier::Local);
    assert_eq!(sess.downgrade_reason.as_deref(), Some("excessive_frame_gap"));

    // Verify MemoryArenaEvaluatorState unused warning is avoided
    let _ = MemoryArenaEvaluatorState::new("x", "y");
    let _ = HashMap::<u32, usize>::new();
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn single_onehundo_pack_produces_identical_runtime_observed_unlocks_across_emulator_and_static_recomp() {
    let dir = common::temp_test_dir("cross_runtime_proof");
    let client_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let pack_path = client_root.join("packs/onehundo.plane_a.cross_runtime.onehundo");
    assert!(pack_path.exists(), "missing cross-runtime pack {:?}", pack_path);

    let emu_bin = dir.join("plane_a_emulator_harness");
    let recomp_bin = dir.join("plane_a_recomp_harness");
    compile_c99(
        &client_root.join("integrations/emulators/plane_a_emulator_harness.c"),
        &emu_bin,
    );
    compile_c99(
        &client_root.join("integrations/static-recompilations/plane_a_recomp_harness.c"),
        &recomp_bin,
    );

    // Run both runtimes against the EXACT SAME `.onehundo` pack in isolated daemons
    let mut results = Vec::new();
    for (label, bin, args) in [
        (
            "emulator",
            &emu_bin,
            vec!["--scenario", "cross-runtime", "--mode", "standard"],
        ),
        ("recomp", &recomp_bin, vec!["--mode", "standard"]),
    ] {
        let sub = dir.join(label);
        let sock_path = sub.join("onehundo.sock");
        let mut cfg = DaemonConfig::default();
        cfg.config_dir = sub.join("cfg");
        cfg.data_dir = sub.join("data");
        cfg.packs_dirs = vec![];
        cfg.notifications_enabled = false;
        cfg.shm_poll_interval_ms = 1;

        let engine = DaemonEngine::initialize(cfg).expect("init daemon");
        let loaded_id = engine.load_pack_path(&pack_path).expect("load pack");
        assert_eq!(loaded_id, "onehundo.plane_a.cross_runtime");

        // Verify the exact same pack matches both ROM hash (emulator) and recomp_game_id / build_id (recomp)
        {
            let packs = engine.packs.lock().unwrap();
            let (_, m_emu) = packs
                .match_pack(&MatchQuery {
                    canonical_game_id: None,
                    sidecar_uuid: None,
                    rom_hash: Some("chronos-emu-rom-sha256-0001".into()),
                    recomp_game_id: None,
                    build_id: None,
                    asset_archive_hash: None,
                    platform_id: None,
                    region: Some("USA".into()),
                    ..Default::default()
                })
                .expect("rom_hash match");
            assert_eq!(m_emu.pack_id, "onehundo.plane_a.cross_runtime");

            let (_, m_recomp) = packs
                .match_pack(&MatchQuery {
                    canonical_game_id: None,
                    sidecar_uuid: None,
                    rom_hash: None,
                    recomp_game_id: Some("chronos-recomp".into()),
                    build_id: Some("chronos-recomp-v1".into()),
                    asset_archive_hash: None,
                    platform_id: None,
                    region: None,
                    ..Default::default()
                })
                .expect("recomp_game_id match");
            assert_eq!(m_recomp.pack_id, "onehundo.plane_a.cross_runtime");
        }

        let shutdown = Arc::new(AtomicBool::new(false));
        let ipc_h = start_unix_ipc_server(Arc::clone(&engine), &sock_path, Arc::clone(&shutdown))
            .expect("start ipc");
        let (http_addr, http_h) =
            start_health_http_server(Arc::clone(&engine), "127.0.0.1:0", Arc::clone(&shutdown))
                .expect("start health http");

        let run = Command::new(bin)
            .arg(sock_path.to_str().unwrap())
            .args(&args)
            .output()
            .expect("run harness");
        assert!(
            run.status.success(),
            "{} failed: {}",
            label,
            String::from_utf8_lossy(&run.stderr)
        );

        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            let db = engine.db.lock().unwrap();
            let trophies = list_trophies(&db, Some("shared.cross_runtime.chronos")).unwrap();
            if trophies.len() == 3 {
                break;
            }
            drop(db);
            thread::sleep(Duration::from_millis(15));
        }

        let (mut trophy_ids, stats_map, pending_events): (Vec<String>, Vec<(String, Option<i64>)>, Vec<(String, String, String)>) = {
            let db = engine.db.lock().unwrap();
            let trophies = list_trophies(&db, Some("shared.cross_runtime.chronos")).unwrap();
            for t in &trophies {
                assert_eq!(t.evidence_class, "RUNTIME_OBSERVED");
                assert_eq!(t.trust_tier, "VERIFIED_INTEGRATION");
                assert_eq!(t.verification_state, "VERIFIED");
                assert_eq!(t.progression_mode, "STANDARD");
            }
            let mut tids: Vec<String> = trophies
                .iter()
                .map(|t| t.canonical_achievement_id.clone())
                .collect();
            tids.sort();

            let stats = list_stats(&db, Some("shared.cross_runtime.chronos"))
                .unwrap()
                .into_iter()
                .map(|s| (s.stat_key, s.int_value))
                .collect();

            let pending = fetch_pending_sync_events(&db, 50)
                .unwrap()
                .into_iter()
                .map(|e| (e.event_kind, e.key, e.evidence_class))
                .collect();

            (tids, stats, pending)
        };

        assert_eq!(
            trophy_ids,
            vec![
                "ach_boss_vanquished",
                "ach_phase_flurry",
                "ach_relic_hoard"
            ],
            "{} runtime missed expected achievements",
            label
        );
        assert!(
            stats_map.contains(&("bosses_defeated".to_string(), Some(1))),
            "{} missing bosses_defeated=1 in {:?}",
            label,
            stats_map
        );
        assert!(
            stats_map.contains(&("score_total".to_string(), Some(2500))),
            "{} missing score_total=2500 in {:?}",
            label,
            stats_map
        );
        assert!(pending_events
            .iter()
            .all(|(_, _, ev_cls)| ev_cls == "RUNTIME_OBSERVED"));
        assert!(pending_events
            .iter()
            .any(|(kind, key, _)| kind == "UNLOCK_REQUEST" && key == "ach_boss_vanquished"));
        assert!(pending_events
            .iter()
            .any(|(kind, key, _)| kind == "STAT_INC" && key == "bosses_defeated"));

        // Verify /health JSON surfaces Plane A SHM telemetry & zero CRC errors
        let mut hs = std::net::TcpStream::connect(http_addr).unwrap();
        hs.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut h_resp = String::new();
        hs.read_to_string(&mut h_resp).unwrap();
        assert!(h_resp.contains("200 OK"));
        let (_, json_str) = h_resp.split_once("\r\n\r\n").unwrap();
        let health: serde_json::Value = serde_json::from_str(json_str).unwrap();
        assert!(health["plane_a"]["total_snapshots_consumed"].as_u64().unwrap() >= 5);
        assert_eq!(health["plane_a"]["total_crc_errors"].as_u64().unwrap(), 0);
        assert_eq!(health["plane_a"]["rings"][0]["unlocked_rules_count"].as_u64().unwrap(), 3);

        trophy_ids.sort();
        results.push((trophy_ids, stats_map));

        shutdown.store(true, Ordering::Relaxed);
        let _ = ipc_h.join();
        let _ = http_h.join();
    }

    // Core Cross-Runtime Proof: both Emulator & Static Recomp produced byte-for-byte identical progression
    assert_eq!(results[0], results[1]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn plane_a_contention_increments_frame_gaps_and_surfaces_in_health() {
    let dir = common::temp_test_dir("plane_a_contention");
    let client_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let pack_path = client_root.join("packs/onehundo.plane_a.cross_runtime.onehundo");
    let emu_bin = dir.join("plane_a_emulator_harness");
    compile_c99(
        &client_root.join("integrations/emulators/plane_a_emulator_harness.c"),
        &emu_bin,
    );

    let sock_path = dir.join("onehundo_contention.sock");
    let mut cfg = DaemonConfig::default();
    cfg.config_dir = dir.join("cfg");
    cfg.data_dir = dir.join("data");
    cfg.packs_dirs = vec![];
    cfg.notifications_enabled = false;
    cfg.shm_poll_interval_ms = 25; // Intentionally slow consumer to force 4-slot ring saturation
    cfg.plane_a_max_frame_gaps_before_downgrade = 5;

    let engine = DaemonEngine::initialize(cfg).unwrap();
    engine.load_pack_path(&pack_path).unwrap();

    let shutdown = Arc::new(AtomicBool::new(false));
    let ipc_h =
        start_unix_ipc_server(Arc::clone(&engine), &sock_path, Arc::clone(&shutdown)).unwrap();
    let (http_addr, http_h) =
        start_health_http_server(Arc::clone(&engine), "127.0.0.1:0", Arc::clone(&shutdown))
            .unwrap();

    // Burst 80 frames with 0us delay -> 4-slot SHM overflows repeatedly without blocking game thread
    let out = Command::new(&emu_bin)
        .args([
            sock_path.to_str().unwrap(),
            "--scenario",
            "contention",
            "--frames",
            "80",
            "--sleep-us",
            "0",
        ])
        .output()
        .expect("run contention harness");
    assert!(out.status.success());

    // Wait for daemon to finish draining & recording FRAME_GAPs
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        let h = engine.health_snapshot().unwrap();
        if h.total_frame_gaps >= 5 && h.sessions.iter().any(|s| s.is_downgraded()) {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }

    let mut hs = std::net::TcpStream::connect(http_addr).unwrap();
    hs.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut h_resp = String::new();
    hs.read_to_string(&mut h_resp).unwrap();
    let (_, json_str) = h_resp.split_once("\r\n\r\n").unwrap();
    let health: serde_json::Value = serde_json::from_str(json_str).unwrap();

    let total_gaps = health["total_frame_gaps"].as_u64().unwrap();
    assert!(
        total_gaps >= 5,
        "expected >=5 FRAME_GAPs under ring contention, got {}",
        total_gaps
    );
    assert_eq!(health["sessions"][0]["evaluation_incomplete"].as_bool(), Some(true));
    assert_eq!(health["sessions"][0]["active_trust_tier"].as_str(), Some("LOCAL"));
    assert_eq!(
        health["sessions"][0]["downgrade_reason"].as_str(),
        Some("excessive_frame_gap")
    );
    assert!(
        health["plane_a"]["rings"][0]["producer_frame_gaps_reported"]
            .as_u64()
            .unwrap()
            >= 5
    );

    shutdown.store(true, Ordering::Relaxed);
    let _ = ipc_h.join();
    let _ = http_h.join();
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn plane_a_spectator_non_mutation_and_save_state_downgrade_over_shm() {
    let dir = common::temp_test_dir("plane_a_mode_trust");
    let client_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let pack_path = client_root.join("packs/onehundo.plane_a.cross_runtime.onehundo");
    let emu_bin = dir.join("plane_a_emulator_harness");
    let recomp_bin = dir.join("plane_a_recomp_harness");
    compile_c99(
        &client_root.join("integrations/emulators/plane_a_emulator_harness.c"),
        &emu_bin,
    );
    compile_c99(
        &client_root.join("integrations/static-recompilations/plane_a_recomp_harness.c"),
        &recomp_bin,
    );

    // 1. SPECTATOR mode via static recomp harness: evaluates all frames, zero SQLite mutation
    {
        let sub = dir.join("spectator");
        let sock_path = sub.join("spec.sock");
        let mut cfg = DaemonConfig::default();
        cfg.config_dir = sub.join("cfg");
        cfg.data_dir = sub.join("data");
        cfg.packs_dirs = vec![];
        cfg.notifications_enabled = false;
        cfg.shm_poll_interval_ms = 1;

        let engine = DaemonEngine::initialize(cfg).unwrap();
        engine.load_pack_path(&pack_path).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let ipc_h =
            start_unix_ipc_server(Arc::clone(&engine), &sock_path, Arc::clone(&shutdown)).unwrap();

        let out = Command::new(&recomp_bin)
            .args([sock_path.to_str().unwrap(), "--mode", "spectator"])
            .output()
            .unwrap();
        assert!(out.status.success());

        thread::sleep(Duration::from_millis(80));
        let db = engine.db.lock().unwrap();
        assert!(list_trophies(&db, Some("shared.cross_runtime.chronos"))
            .unwrap()
            .is_empty());
        assert!(list_stats(&db, Some("shared.cross_runtime.chronos"))
            .unwrap()
            .is_empty());
        assert_eq!(query_journal_stats(&db).unwrap().total_events, 0);
        drop(db);

        let snap = engine.health_snapshot().unwrap();
        assert!(snap.sessions[0].frame_snapshots_evaluated >= 5);
        assert_eq!(snap.sessions[0].progression_mode, ProgressionMode::Spectator);

        shutdown.store(true, Ordering::Relaxed);
        let _ = ipc_h.join();
    }

    // 2. SAVE_STATE_LOAD via emulator harness: downgrades VERIFIED_INTEGRATION -> LOCAL before boss defeat
    {
        let sub = dir.join("saveload");
        let sock_path = sub.join("save.sock");
        let mut cfg = DaemonConfig::default();
        cfg.config_dir = sub.join("cfg");
        cfg.data_dir = sub.join("data");
        cfg.packs_dirs = vec![];
        cfg.notifications_enabled = false;
        cfg.shm_poll_interval_ms = 1;

        let engine = DaemonEngine::initialize(cfg).unwrap();
        engine.load_pack_path(&pack_path).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let ipc_h =
            start_unix_ipc_server(Arc::clone(&engine), &sock_path, Arc::clone(&shutdown)).unwrap();

        let out = Command::new(&emu_bin)
            .args([sock_path.to_str().unwrap(), "--scenario", "save-load"])
            .output()
            .unwrap();
        assert!(out.status.success());

        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            let db = engine.db.lock().unwrap();
            let trophies = list_trophies(&db, Some("shared.cross_runtime.chronos")).unwrap();
            if trophies
                .iter()
                .any(|t| t.canonical_achievement_id == "ach_boss_vanquished")
            {
                break;
            }
            drop(db);
            thread::sleep(Duration::from_millis(15));
        }

        let db = engine.db.lock().unwrap();
        let trophies = list_trophies(&db, Some("shared.cross_runtime.chronos")).unwrap();
        let boss = trophies
            .iter()
            .find(|t| t.canonical_achievement_id == "ach_boss_vanquished")
            .expect("ach_boss_vanquished unlocked after save-state load");
        assert_eq!(boss.trust_tier, "LOCAL");
        assert_eq!(boss.verification_state, "UNVERIFIED");
        assert_eq!(boss.evidence_class, "RUNTIME_OBSERVED");
        drop(db);

        let snap = engine.health_snapshot().unwrap();
        assert_eq!(snap.sessions[0].active_trust_tier, TrustTier::Local);
        assert_eq!(
            snap.sessions[0].downgrade_reason.as_deref(),
            Some("save_state_load")
        );

        shutdown.store(true, Ordering::Relaxed);
        let _ = ipc_h.join();
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn plane_a_runtime_observed_end_to_end_cloud_sync_payload() {
    let dir = common::temp_test_dir("plane_a_cloud_sync");
    let client_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let pack_path = client_root.join("packs/onehundo.plane_a.cross_runtime.onehundo");
    let recomp_bin = dir.join("plane_a_recomp_harness");
    compile_c99(
        &client_root.join("integrations/static-recompilations/plane_a_recomp_harness.c"),
        &recomp_bin,
    );

    // Start mock platform `/v1/devices/register` + `/v1/sync/events`
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mock_addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();

    let captured_events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let captured_ref = Arc::clone(&captured_events);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_srv = Arc::clone(&stop);

    let srv = thread::spawn(move || {
        while !stop_srv.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut buf = vec![0u8; 64 * 1024];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        continue;
                    }
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let (head, mut body) = req
                        .split_once("\r\n\r\n")
                        .map(|(h, b)| (h.to_string(), b.to_string()))
                        .unwrap_or((req, String::new()));
                    let clen: usize = head
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.trim()
                                .eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    while body.len() < clen {
                        let mut extra = [0u8; 4096];
                        match stream.read(&mut extra) {
                            Ok(0) | Err(_) => break,
                            Ok(m) => body.push_str(&String::from_utf8_lossy(&extra[..m])),
                        }
                    }

                    let path = head
                        .lines()
                        .next()
                        .unwrap_or("")
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("");
                    let (status, resp_json) = if path == "/v1/devices/register" {
                        (
                            "201 Created",
                            serde_json::json!({
                                "device_id": "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
                                "device_token": "dev_tok_plane_a_sync",
                                "registered_at": "2026-11-25T12:00:00Z"
                            })
                            .to_string(),
                        )
                    } else if path == "/v1/sync/events" {
                        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
                        let evs = v["events"].as_array().unwrap();
                        captured_ref.lock().unwrap().extend(evs.iter().cloned());
                        let results: Vec<serde_json::Value> = evs
                            .iter()
                            .map(|ev| {
                                serde_json::json!({
                                    "idempotency_key": ev["idempotency_key"],
                                    "receipt_status": "RECEIVED",
                                    "acceptance_status": "ACCEPTED",
                                    "verification_state": ev["verification_state"]
                                })
                            })
                            .collect();
                        (
                            "200 OK",
                            serde_json::json!({
                                "client_batch_id": v["client_batch_id"],
                                "batch_receipt_status": "RECEIVED",
                                "results": results
                            })
                            .to_string(),
                        )
                    } else {
                        ("404 Not Found", "{}".to_string())
                    };
                    let http = format!(
                        "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        status,
                        resp_json.len(),
                        resp_json
                    );
                    let _ = stream.write_all(http.as_bytes());
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });

    let cfg_dir = dir.join("cfg");
    save_credentials(
        &cfg_dir,
        &Credentials {
            api_key: Some("oh_live_plane_a_test".into()),
            device_id: None,
            device_token: None,
        },
    )
    .unwrap();

    let sock_path = dir.join("sync.sock");
    let mut cfg = DaemonConfig::default();
    cfg.config_dir = cfg_dir;
    cfg.data_dir = dir.join("data");
    cfg.packs_dirs = vec![];
    cfg.platform_url = Some(format!("http://{}", mock_addr));
    cfg.notifications_enabled = false;
    cfg.shm_poll_interval_ms = 1;

    let engine = DaemonEngine::initialize(cfg).unwrap();
    engine.load_pack_path(&pack_path).unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    let ipc_h =
        start_unix_ipc_server(Arc::clone(&engine), &sock_path, Arc::clone(&shutdown)).unwrap();

    let out = Command::new(&recomp_bin)
        .arg(sock_path.to_str().unwrap())
        .output()
        .unwrap();
    assert!(out.status.success());

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        let db = engine.db.lock().unwrap();
        if list_trophies(&db, Some("shared.cross_runtime.chronos"))
            .unwrap()
            .len()
            == 3
        {
            break;
        }
        drop(db);
        thread::sleep(Duration::from_millis(15));
    }

    let summary = engine
        .sync_if_pending()
        .expect("sync_if_pending")
        .expect("synced batch");
    assert_eq!(summary.batch_receipt_status.as_deref(), Some("RECEIVED"));
    assert_eq!(summary.last_acceptance_status.as_deref(), Some("ACCEPTED"));
    assert!(summary.synced_count >= 5);

    let evs = captured_events.lock().unwrap();
    assert!(evs.iter().all(|e| {
        e["evidence_class"] == "RUNTIME_OBSERVED"
            && e["canonical_game_id"] == "shared.cross_runtime.chronos"
    }));
    assert!(evs.iter().any(|e| {
        e["event_kind"] == EventKind::UnlockRequest.as_str()
            && e["key"] == "ach_boss_vanquished"
    }));
    assert!(evs.iter().any(|e| {
        e["event_kind"] == EventKind::StatInc.as_str() && e["key"] == "bosses_defeated"
    }));

    assert_eq!(engine.pending_sync_count().unwrap(), 0);

    shutdown.store(true, Ordering::Relaxed);
    stop.store(true, Ordering::Relaxed);
    let _ = ipc_h.join();
    let _ = srv.join();
    let _ = fs::remove_dir_all(&dir);
}
