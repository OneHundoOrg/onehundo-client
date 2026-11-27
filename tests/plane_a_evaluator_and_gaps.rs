mod common;

use onehundo_client::core::config::DaemonConfig;
use onehundo_client::core::engine::DaemonEngine;
use onehundo_client::core::types::{
    FairPlayViolation, IngestionPlane, LifecycleSignal, ProgressionMode, TrustTier,
};
use onehundo_client::diagnostics::server::start_health_http_server;
use onehundo_client::evaluation::memory_arena::MemoryArenaEvaluatorState;
use onehundo_client::ipc::shm_arena::{
    ArenaShmLayout, SnapshotSlotHeader, RING_SLOT_CAPACITY, RING_SLOT_COUNT,
};
use onehundo_client::ipc::unix_socket::start_unix_ipc_server;
use onehundo_client::packs::matcher::MatchQuery;
use onehundo_client::storage::state::list_trophies;
use std::fs;
use std::io::Read;
use std::net::TcpStream;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn shm_abi_layout_matches_c_header() {
    assert_eq!(std::mem::size_of::<SnapshotSlotHeader>(), 56);
    assert_eq!(
        std::mem::size_of::<ArenaShmLayout>(),
        16 + RING_SLOT_COUNT * (56 + RING_SLOT_CAPACITY)
    );
}

fn build_memory_test_pack_bytes() -> Vec<u8> {
    let manifest = serde_json::json!({
        "manifest_version": "0.1.0",
        "pack_id": "pack.emu.sample",
        "pack_version": "0.1.0",
        "canonical_game_id": "shared.emu.sample",
        "title": "Sample Emulator Pack",
        "matchers": [
            { "matcher_type": "rom_hash", "algorithm": "sha256", "value": "deadbeef01" }
        ],
        "profiles": {
            "memory": {
                "enabled": true,
                "architecture": "snes",
                "default_endianness": "little",
                "regions": [
                    { "region_name": "WRAM", "guest_base_address": "0x7E0000", "size_bytes": 4096 }
                ],
                "rules_file": "rules/memory.json"
            }
        },
        "achievements": [
            {
                "canonical_achievement_id": "ach_emu_stage1",
                "title": "Stage 1 Cleared",
                "description": "Cleared stage 1 in RAM",
                "points": 10,
                "supported_modes": ["STANDARD", "ENCORE"],
                "minimum_trust_tier": "LOCAL"
            },
            {
                "canonical_achievement_id": "ach_combo_hits",
                "title": "Steady Combo",
                "description": "Sustained bit0 and delta progression",
                "points": 25,
                "supported_modes": ["STANDARD", "ENCORE"],
                "minimum_trust_tier": "VERIFIED_INTEGRATION"
            }
        ]
    });

    let rules = serde_json::json!({
        "memory_rules": [
            {
                "achievement_id": "ach_emu_stage1",
                "conditions": [
                    { "region_id": 0, "address": "0x20", "size": "u8", "op": "==", "value": 1, "kind": "normal", "required_hits": 0 }
                ]
            },
            {
                "achievement_id": "ach_combo_hits",
                "conditions": [
                    { "region_id": 0, "address": "0x10", "size": "u16_le", "op": ">", "compare_delta": true, "kind": "normal", "required_hits": 2 },
                    { "region_id": 0, "address": "0x30", "size": "u8", "op": "==", "value": 99, "kind": "reset_if" },
                    { "region_id": 0, "address": "0x31", "size": "u8", "op": "==", "value": 1, "kind": "pause_if" }
                ],
                "alt_groups": [
                    [
                        { "region_id": 0, "address": "0x21", "size": "bit0", "op": "==", "value": 1, "kind": "normal" }
                    ]
                ]
            }
        ]
    });

    let m_bytes = serde_json::to_vec(&manifest).unwrap();
    let r_bytes = serde_json::to_vec(&rules).unwrap();
    common::build_test_zip(&[
        ("manifest.json", &m_bytes, 0),
        ("rules/memory.json", &r_bytes, 0),
    ])
}

#[test]
fn plane_a_rules_save_rewind_trust_downgrade_and_frame_gaps() {
    let dir = common::temp_test_dir("plane_a_unit");
    let pack_path = dir.join("shared_emu.onehundo");
    fs::write(&pack_path, build_memory_test_pack_bytes()).unwrap();

    let mut cfg = DaemonConfig::default();
    cfg.config_dir = dir.join("cfg");
    cfg.data_dir = dir.join("data");
    cfg.packs_dirs = vec![];
    cfg.notifications_enabled = false;
    let engine = DaemonEngine::initialize(cfg).expect("init");
    engine.load_pack_path(&pack_path).expect("load memory pack");

    let sid = "11111111-1111-4111-8111-111111111111".to_string();
    engine.start_session(
        sid.clone(),
        "ares_snes_core".into(),
        "shared.emu.sample".into(),
        MatchQuery {
            canonical_game_id: Some("shared.emu.sample".into()),
            sidecar_uuid: None,
            rom_hash: Some("deadbeef01".into()),
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

    let mut ram = vec![0u8; 256];
    ram[0x21] = 1; // alt group bit0 = 1

    // Frame 1: initialize delta baseline (0x10 == 10)
    ram[0x10] = 10;
    let f1 = engine
        .handle_plane_a_frame(&sid, 1, LifecycleSignal::NormalFrame, 0, &ram)
        .unwrap();
    assert!(!f1.frame_gap_detected);
    assert!(f1.unlocked_achievements.is_empty());

    // Frame 2: delta increases (10 -> 11) => 1 hit
    ram[0x10] = 11;
    let f2 = engine
        .handle_plane_a_frame(&sid, 2, LifecycleSignal::NormalFrame, 0, &ram)
        .unwrap();
    assert!(f2.unlocked_achievements.is_empty());

    // Frame 3: pause_if active (0x31 = 1) even if delta increases -> must NOT unlock
    ram[0x10] = 12;
    ram[0x31] = 1;
    let f3 = engine
        .handle_plane_a_frame(&sid, 3, LifecycleSignal::NormalFrame, 0, &ram)
        .unwrap();
    assert!(f3.unlocked_achievements.is_empty());
    ram[0x31] = 0;

    // Frame 4: reset_if active (0x30 = 99) -> clears hits back to 0
    ram[0x30] = 99;
    let _ = engine
        .handle_plane_a_frame(&sid, 4, LifecycleSignal::NormalFrame, 0, &ram)
        .unwrap();
    ram[0x30] = 0;

    // Sequence gap test: jump seq 4 -> 8 => records gap of 3 and marks evaluation incomplete
    ram[0x10] = 15;
    let f_gap = engine
        .handle_plane_a_frame(&sid, 8, LifecycleSignal::NormalFrame, 0, &ram)
        .unwrap();
    assert!(f_gap.frame_gap_detected);
    assert_eq!(f_gap.gap_count, 3);
    assert!(f_gap.evaluation_incomplete);

    // Explicit FRAME_GAP signal
    let f_explicit_gap = engine
        .handle_plane_a_frame(&sid, 9, LifecycleSignal::FrameGap, 2, &ram)
        .unwrap();
    assert!(f_explicit_gap.frame_gap_detected);
    assert_eq!(f_explicit_gap.gap_count, 2);

    // Hit 2 again -> ach_combo_hits unlocks (verified before save-state load)
    ram[0x10] = 20;
    let f10 = engine
        .handle_plane_a_frame(&sid, 10, LifecycleSignal::NormalFrame, 0, &ram)
        .unwrap();
    assert!(f10.unlocked_achievements.contains(&"ach_combo_hits".to_string()));

    // SAVE_STATE_LOAD at frame 11 => rewinds progress AND downgrades VERIFIED_INTEGRATION -> LOCAL
    let f11 = engine
        .handle_plane_a_frame(&sid, 11, LifecycleSignal::SaveStateLoad, 0, &ram)
        .unwrap();
    assert!(f11.rewound_on_save_load);

    let health = engine.health_snapshot().unwrap();
    assert_eq!(health.total_frame_gaps, 5); // 3 skipped + 2 explicit
    let s = health.sessions.iter().find(|s| s.session_id == sid).unwrap();
    assert_eq!(s.active_trust_tier, TrustTier::Local);
    assert!(s.is_downgraded());
    assert!(s.evaluation_incomplete);

    // Also test explicit FairPlayViolation serialization on MemoryArenaEvaluatorState
    let state = MemoryArenaEvaluatorState::new("shared.emu.sample", &sid);
    let ser = state.serialize_progress().unwrap();
    let restored = MemoryArenaEvaluatorState::restore_progress(&ser).unwrap();
    assert_eq!(restored.canonical_game_id, "shared.emu.sample");

    let _ = engine.apply_fair_play_violation(&sid, FairPlayViolation::DebugConsoleOverride);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn live_c99_plane_a_emulator_harness_over_posix_shm_and_unix_socket() {
    let dir = common::temp_test_dir("plane_a_c_e2e");
    let pack_path = dir.join("shared_emu.onehundo");
    fs::write(&pack_path, build_memory_test_pack_bytes()).unwrap();

    let sock_path = dir.join("onehundo_a.sock");
    let bin_path = dir.join("plane_a_emulator_harness");

    let client_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let proto_root = common::protocol_root();
    let harness_c = client_root.join("integrations/emulators/plane_a_emulator_harness.c");
    let transport_c = proto_root.join("reference/c/transport_posix.c");

    let compile = Command::new("gcc")
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
            harness_c.to_str().unwrap(),
            transport_c.to_str().unwrap(),
            "-lrt",
            "-o",
            bin_path.to_str().unwrap(),
        ])
        .output()
        .expect("invoke gcc");
    assert!(
        compile.status.success(),
        "gcc failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let mut cfg = DaemonConfig::default();
    cfg.config_dir = dir.join("cfg");
    cfg.data_dir = dir.join("data");
    cfg.packs_dirs = vec![];
    cfg.notifications_enabled = false;
    cfg.shm_poll_interval_ms = 1;

    let engine = DaemonEngine::initialize(cfg).expect("init daemon");
    engine.load_pack_path(&pack_path).expect("load pack");

    let shutdown = Arc::new(AtomicBool::new(false));
    let ipc_handle =
        start_unix_ipc_server(Arc::clone(&engine), &sock_path, Arc::clone(&shutdown))
            .expect("start unix ipc");
    let (health_addr, http_handle) =
        start_health_http_server(Arc::clone(&engine), "127.0.0.1:0", Arc::clone(&shutdown))
            .expect("start health http");

    let run = Command::new(&bin_path)
        .arg(sock_path.to_str().unwrap())
        .output()
        .expect("run plane_a_emulator_harness");
    assert!(
        run.status.success(),
        "plane_a_emulator_harness exited {}: {}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );

    // Poll until daemon processes the SHM frames & unlocks ach_emu_stage1
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut unlocked = false;
    while Instant::now() < deadline {
        let db = engine.db.lock().unwrap();
        let trophies = list_trophies(&db, Some("shared.emu.sample")).unwrap();
        if trophies
            .iter()
            .any(|t| t.canonical_achievement_id == "ach_emu_stage1")
        {
            unlocked = true;
            break;
        }
        drop(db);
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        unlocked,
        "ach_emu_stage1 was not unlocked via live POSIX SHM Plane A harness"
    );

    let snap = engine.health_snapshot().unwrap();
    let sess = snap
        .sessions
        .iter()
        .find(|s| s.canonical_game_id == "shared.emu.sample")
        .expect("Plane A session in Game Health");
    assert!(
        sess.frame_snapshots_evaluated >= 4,
        "expected >=4 SHM snapshots evaluated, got {}",
        sess.frame_snapshots_evaluated
    );

    // Verify /health JSON HTTP response contains frame_snapshots_evaluated
    let mut stream = TcpStream::connect(health_addr).expect("connect /health");
    use std::io::Write;
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut resp = String::new();
    stream.read_to_string(&mut resp).unwrap();
    assert!(resp.contains("200 OK"));
    assert!(resp.contains("shared.emu.sample"));

    shutdown.store(true, Ordering::Relaxed);
    let _ = ipc_handle.join();
    let _ = http_handle.join();
    let _ = fs::remove_dir_all(&dir);
}
