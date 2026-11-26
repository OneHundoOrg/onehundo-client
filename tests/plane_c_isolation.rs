mod common;

use onehundo_client::core::config::{DaemonConfig, WatchedObservationDir};
use onehundo_client::core::engine::DaemonEngine;
use onehundo_client::core::types::{
    EvidenceClass, FairPlayViolation, IngestionPlane, LifecycleSignal, ProgressionMode, TrustTier,
};
use onehundo_client::diagnostics::server::start_health_http_server;
use onehundo_client::observation::external_importers::{
    build_save_file_observation, import_observation_file, import_observation_path_with_options,
    import_observation_records, parse_observation_json_str, sha256_hex, ImportOptions,
};
use onehundo_client::packs::matcher::MatchQuery;
use onehundo_client::storage::journal::fetch_pending_sync_events;
use onehundo_client::storage::state::{list_stats, list_trophies};
use onehundo_client::sync::http::http_request;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn build_synthetic_tropusr_dat(entries: &[(u32, u32, u64)]) -> Vec<u8> {
    let header_size = 0x30usize;
    let table_hdr_size = 32usize;
    let entry_size = 0x60usize;
    let data_offset = header_size + table_hdr_size;
    let total_len = data_offset + entries.len() * entry_size;
    let mut buf = vec![0u8; total_len];

    // Header: magic 0x818F54AD, version 1, tables_count at 0x10
    buf[0..4].copy_from_slice(&0x818F_54ADu32.to_be_bytes());
    buf[4..8].copy_from_slice(&1u32.to_be_bytes());
    buf[0x10..0x14].copy_from_slice(&1u32.to_be_bytes());

    // Single table descriptor at 0x30: type=6, entry_size=0x60, count, data_offset
    let th = header_size;
    buf[th..th + 4].copy_from_slice(&6u32.to_be_bytes());
    buf[th + 4..th + 8].copy_from_slice(&(entry_size as u32).to_be_bytes());
    buf[th + 12..th + 16].copy_from_slice(&(entries.len() as u32).to_be_bytes());
    buf[th + 16..th + 24].copy_from_slice(&(data_offset as u64).to_be_bytes());

    for (idx, &(trophy_id, unlocked, rtc_or_epoch)) in entries.iter().enumerate() {
        let e = data_offset + idx * entry_size;
        buf[e + 0x10..e + 0x14].copy_from_slice(&trophy_id.to_be_bytes());
        buf[e + 0x14..e + 0x18].copy_from_slice(&unlocked.to_be_bytes());
        buf[e + 0x20..e + 0x28].copy_from_slice(&rtc_or_epoch.to_be_bytes());
    }

    buf
}

fn build_synthetic_longshot_progress_sav(
    skulltulas: i32,
    max_hearts_quarters: i32,
    bool64s: &[u64],
    equip_primary: Option<&str>,
) -> Vec<u8> {
    let mut scalars: std::collections::BTreeMap<u32, u32> = std::collections::BTreeMap::new();
    scalars.insert(0xa779_21d7, skulltulas as u32);
    scalars.insert(0x31ab_5580, max_hearts_quarters as u32);

    let extra_entries = 1 + usize::from(equip_primary.is_some());
    let data_off = 0x28 + (scalars.len() + extra_entries) * 8;
    let mut tail = Vec::new();

    scalars.insert(0xa3db_7114, (data_off + tail.len()) as u32);
    for &b in bool64s {
        tail.extend_from_slice(&b.to_le_bytes());
    }
    tail.extend_from_slice(&0u64.to_le_bytes());

    if let Some(eq) = equip_primary {
        scalars.insert(0xfb11_7672, (data_off + tail.len()) as u32);
        let mut slot = [0u8; 64];
        let eb = eq.as_bytes();
        let copy_len = eb.len().min(63);
        slot[..copy_len].copy_from_slice(&eb[..copy_len]);
        tail.extend_from_slice(&slot);
    }

    let mut buf = vec![0u8; data_off + tail.len()];
    buf[0..4].copy_from_slice(&0x0102_0304u32.to_le_bytes());
    buf[4..8].copy_from_slice(&0x002d_80ebu32.to_le_bytes());
    buf[8..12].copy_from_slice(&(data_off as u32).to_le_bytes());

    let mut cursor = 0x28usize;
    for (h, v) in scalars {
        buf[cursor..cursor + 4].copy_from_slice(&h.to_le_bytes());
        buf[cursor + 4..cursor + 8].copy_from_slice(&v.to_le_bytes());
        cursor += 8;
    }
    buf[data_off..].copy_from_slice(&tail);
    buf
}

#[test]
fn imports_protocol_observation_fixture_and_isolates_from_evaluators() {
    let dir = common::temp_test_dir("plane_c");
    let mut cfg = DaemonConfig::default();
    cfg.config_dir = dir.join("cfg");
    cfg.data_dir = dir.join("data");
    cfg.packs_dirs = vec![];
    cfg.notifications_enabled = false;

    let engine = DaemonEngine::initialize(cfg).expect("init engine");
    let fixture =
        common::protocol_root().join("conformance/fixtures/observation/steam_local_save.oh-obs.json");

    let summary = {
        let mut db = engine.db.lock().unwrap();
        import_observation_file(&mut db, &fixture).expect("import protocol fixture")
    };
    assert_eq!(summary.imported, 2);
    assert_eq!(summary.deduplicated, 0);

    // Re-importing same observation_id must deduplicate safely
    let summary_dup = {
        let mut db = engine.db.lock().unwrap();
        import_observation_file(&mut db, &fixture).expect("re-import dedup")
    };
    assert_eq!(summary_dup.imported, 0);
    assert_eq!(summary_dup.deduplicated, 2);

    // Check outbox has OBSERVATION_IMPORT with preserved provenance
    let db = engine.db.lock().unwrap();
    let outbox = fetch_pending_sync_events(&db, 20).unwrap();
    assert_eq!(outbox.len(), 2);
    assert!(outbox.iter().all(|e| e.event_kind == "OBSERVATION_IMPORT"));
    assert_eq!(outbox[0].evidence_class, "LOCALLY_OBSERVED");
    assert_eq!(outbox[1].evidence_class, "PLATFORM_CONFIRMED");

    let trophies = list_trophies(&db, None).unwrap();
    assert_eq!(trophies.len(), 1);
    assert_eq!(trophies[0].canonical_achievement_id, "ach_tutorial_complete");
    assert_eq!(trophies[0].evidence_class, "LOCALLY_OBSERVED");
    assert_eq!(trophies[0].verification_state, "UNVERIFIED");
    // Second record is SPECTATOR mode, so stat_state must not be mutated
    assert!(list_stats(&db, Some("shared.steam.sample")).unwrap().is_empty());

    drop(db);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn rejects_gameplay_evidence_classes_and_audits_all_observation_sources() {
    let dir = common::temp_test_dir("plane_c_guard");
    let mut cfg = DaemonConfig::default();
    cfg.config_dir = dir.join("cfg");
    cfg.data_dir = dir.join("data");
    cfg.packs_dirs = vec![];
    cfg.notifications_enabled = false;
    let engine = DaemonEngine::initialize(cfg).expect("init engine");

    // 1. DIRECT_INTEGRATION_EVENT and RUNTIME_OBSERVED must be rejected by Plane C
    for forbidden in [
        EvidenceClass::DirectIntegrationEvent,
        EvidenceClass::RuntimeObserved,
    ] {
        let bad_json = serde_json::json!({
            "schema_version": "0.1.0",
            "observation_id": "22222222-2222-4222-8222-222222222222",
            "canonical_game_id": "sony.bloodborne",
            "observer_id": "test_save_watcher",
            "source_type": "save_file",
            "observed_at": "2026-11-24T12:00:00Z",
            "record_kind": "ACHIEVEMENT_EVIDENCE",
            "key": "ach_cleric_beast",
            "unlocked": true,
            "progression_mode": "STANDARD",
            "trust_tier": "LOCAL",
            "evidence_class": forbidden.as_str(),
            "verification_state": "UNVERIFIED"
        })
        .to_string();
        let err = parse_observation_json_str(&bad_json)
            .expect_err("Plane C must reject gameplay evidence_class");
        assert!(err.contains("forbidden"));
    }

    // 2. Save-file helper hashes file cleanly + SPECTATOR mode must not mutate local trophy_state
    let save_file = dir.join("slot0.sav");
    fs::write(&save_file, b"BLOODBORNE_SAVE_PAYLOAD_V1").unwrap();
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );

    let mut obs = build_save_file_observation(
        "sony.bloodborne",
        "save_watcher",
        &save_file,
        "ach_gascoigne",
        true,
    )
    .expect("build save obs");
    obs.progression_mode = ProgressionMode::Spectator;

    {
        let mut db = engine.db.lock().unwrap();
        let res = import_observation_records(&mut db, &[obs]).unwrap();
        assert_eq!(res.imported, 1);
        assert!(list_trophies(&db, Some("sony.bloodborne"))
            .unwrap()
            .is_empty());
    }

    // 3. Static architectural source audit across ALL Plane C modules:
    //    daemon/observation/ MUST NEVER reference crate::evaluation or evaluation::
    let obs_dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("daemon/observation/external-importers");
    let mut audited = 0usize;
    for entry in fs::read_dir(&obs_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            let src = fs::read_to_string(&path).unwrap();
            assert!(
                !src.contains("crate::evaluation") && !src.contains("evaluation::"),
                "Plane C module {} must not reference crate::evaluation",
                path.display()
            );
            audited += 1;
        }
    }
    assert!(
        audited >= 6,
        "expected mod + 5 submodule files audited (incl. longshot.rs)"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn goldberg_rpcs3_shadps4_and_generic_save_importers_preserve_null_timestamps_and_deduplicate() {
    let dir = common::temp_test_dir("plane_c_importers");
    let mut cfg = DaemonConfig::default();
    cfg.config_dir = dir.join("cfg");
    cfg.data_dir = dir.join("data");
    cfg.packs_dirs = vec![];
    cfg.notifications_enabled = false;
    let engine = DaemonEngine::initialize(cfg).expect("init");

    // A. Goldberg achievements.json (mixed missing timestamp = null, valid epoch, locked skipped)
    let gb_dir = dir.join("goldberg/400");
    fs::create_dir_all(&gb_dir).unwrap();
    let gb_json = gb_dir.join("achievements.json");
    fs::write(
        &gb_json,
        r#"{
            "ACH_PORTAL_1": { "earned": true, "earned_time": 1700000123 },
            "ACH_NO_TIME": { "earned": true, "earned_time": 0 },
            "ACH_LOCKED": { "earned": false, "earned_time": 0 }
        }"#,
    )
    .unwrap();

    let gb_summary = engine
        .import_plane_c_path(&gb_json, None, None, false)
        .expect("import goldberg json");
    assert_eq!(gb_summary.imported, 2);
    assert_eq!(gb_summary.skipped_locked, 1);
    assert!(gb_summary.records.iter().all(|r| {
        r.canonical_game_id == "steam.400"
            && r.evidence_class == EvidenceClass::LocallyObserved
            && r.trust_tier == TrustTier::Local
            && r.source_type == "steam_api"
    }));
    let no_time_rec = gb_summary
        .records
        .iter()
        .find(|r| r.key == "ach_no_time")
        .unwrap();
    assert_eq!(no_time_rec.source_timestamp, None);
    let timed_rec = gb_summary
        .records
        .iter()
        .find(|r| r.key == "ach_portal_1")
        .unwrap();
    assert_eq!(
        timed_rec.source_timestamp.as_deref(),
        Some("2023-11-14T22:15:23Z")
    );

    // Incremental unlock in the SAME achievements.json must deduplicate prior 2 and import 1 new
    fs::write(
        &gb_json,
        r#"{
            "ACH_PORTAL_1": { "earned": true, "earned_time": 1700000123 },
            "ACH_NO_TIME": { "earned": true, "earned_time": 0 },
            "ACH_LOCKED": { "earned": true, "earned_time": 0 }
        }"#,
    )
    .unwrap();
    let gb_inc = engine
        .import_plane_c_path(&gb_json, None, None, false)
        .expect("incremental goldberg");
    assert_eq!(gb_inc.imported, 1);
    assert_eq!(gb_inc.deduplicated, 2);

    // B. Goldberg achievements.ini
    let ini_dir = dir.join("goldberg_ini");
    fs::create_dir_all(&ini_dir).unwrap();
    fs::write(ini_dir.join("steam_appid.txt"), "620\n").unwrap();
    let ini_file = ini_dir.join("achievements.ini");
    fs::write(
        &ini_file,
        "[SteamAchievements]\nCount=2\n\n[ACH_COOP]\nAchieved=1\nTimeUnlocked=0\n\n[ACH_TURRET]\nAchieved=1\nTimeUnlocked=1700000500\n",
    )
    .unwrap();
    let ini_sum = engine
        .import_plane_c_path(&ini_file, None, None, false)
        .expect("import goldberg ini");
    assert_eq!(ini_sum.imported, 2);
    assert!(ini_sum
        .records
        .iter()
        .any(|r| r.key == "ach_coop" && r.source_timestamp.is_none()));
    assert!(ini_sum
        .records
        .iter()
        .any(|r| r.key == "ach_turret" && r.source_timestamp.as_deref() == Some("2023-11-14T22:21:40Z")));

    // C. RPCS3 TROPUSR.DAT + TROP.SFM (with binary header before <trophyconf)
    let rpcs3_dir = dir.join("rpcs3/NPWR07268_00");
    fs::create_dir_all(&rpcs3_dir).unwrap();
    let mut sfm = vec![0xAAu8; 0x40];
    sfm.extend_from_slice(
        b"<?xml version=\"1.0\"?><trophyconf version=\"1.1\"><npcommid>NPWR07268_00</npcommid><trophy id=\"000\"><name>First Spark</name></trophy></trophyconf>",
    );
    fs::write(rpcs3_dir.join("TROP.SFM"), sfm).unwrap();
    let cell_rtc_tick: u64 = 62_135_596_800_000_000 + (1_700_000_200 * 1_000_000);
    let dat = build_synthetic_tropusr_dat(&[
        (0, 1, 0),             // unlocked, timestamp missing -> None
        (1, 1, cell_rtc_tick), // unlocked, CellRtcTick -> RFC3339
        (2, 0, 0),             // locked -> skipped
    ]);
    fs::write(rpcs3_dir.join("TROPUSR.DAT"), dat).unwrap();

    let rpcs3_sum = engine
        .import_plane_c_path(&rpcs3_dir, None, None, false)
        .expect("import rpcs3 dir");
    assert_eq!(rpcs3_sum.imported, 2);
    assert_eq!(rpcs3_sum.skipped_locked, 1);
    assert_eq!(rpcs3_sum.records[0].canonical_game_id, "psn.npwr07268_00");
    assert_eq!(rpcs3_sum.records[0].source_type, "psn_trophy");
    assert_eq!(rpcs3_sum.records[0].source_timestamp, None);
    assert_eq!(
        rpcs3_sum.records[1].source_timestamp.as_deref(),
        Some("2023-11-14T22:16:40Z")
    );

    // Re-importing same RPCS3 dir is idempotent
    let rpcs3_dup = engine
        .import_plane_c_path(&rpcs3_dir, None, None, false)
        .expect("reimport rpcs3");
    assert_eq!(rpcs3_dup.imported, 0);
    assert_eq!(rpcs3_dup.deduplicated, 2);

    // D. shadPS4 TROP.XML directory
    let shad_dir = dir.join("shadps4/CUSA03173");
    fs::create_dir_all(&shad_dir).unwrap();
    fs::write(
        shad_dir.join("TROP.XML"),
        r#"<trophyconf><npcommid>CUSA03173</npcommid><trophy id="021" unlocked="true" unlocktime="0"><name>Cleric Beast</name></trophy><trophy id="022" unlocked="false"/></trophyconf>"#,
    )
    .unwrap();
    let shad_sum = engine
        .import_plane_c_path(&shad_dir, Some("sony.bloodborne".into()), None, false)
        .expect("import shadps4");
    assert_eq!(shad_sum.imported, 1);
    assert_eq!(shad_sum.skipped_locked, 1);
    assert_eq!(shad_sum.records[0].key, "trophy_21");
    assert_eq!(shad_sum.records[0].source_timestamp, None);

    // E. Generic local save export JSON
    let gen_file = dir.join("save_export.json");
    fs::write(
        &gen_file,
        r#"{
            "canonical_game_id": "indie.save_test",
            "achievements": [
                { "id": "first_clear", "unlocked": true, "unlocked_at": null },
                { "id": "locked_one", "unlocked": false }
            ],
            "stats": [{ "key": "deaths", "int_value": 7 }]
        }"#,
    )
    .unwrap();
    let gen_sum = engine
        .import_plane_c_path(&gen_file, None, None, false)
        .expect("import generic export");
    assert_eq!(gen_sum.imported, 2);
    assert_eq!(gen_sum.skipped_locked, 1);

    // Check SQL NULL preservation in observation_journal and int_value=1 on ACHIEVEMENT_EVIDENCE outbox
    {
        let db = engine.db.lock().unwrap();
        let null_ts_count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM observation_journal WHERE source_timestamp IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        // ach_no_time, ach_locked (incremental), ach_coop, rpcs3 trophy_0, shadps4 trophy_21, generic first_clear & deaths
        assert_eq!(null_ts_count, 7);

        let outbox = fetch_pending_sync_events(&db, 50).unwrap();
        assert_eq!(outbox.len(), 10);
        assert!(outbox
            .iter()
            .filter(|e| e.key == "trophy_21" || e.key == "first_clear" || e.key == "ach_no_time")
            .all(|e| e.int_value == Some(1) && e.evidence_class == "LOCALLY_OBSERVED"));
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn directory_watcher_and_spectator_mode_isolation_and_game_health_http_ui() {
    let dir = common::temp_test_dir("plane_c_ui_watch");
    let watch_dir = dir.join("watched");
    let packs_dir = dir.join("packs");
    fs::create_dir_all(&watch_dir).unwrap();
    fs::create_dir_all(&packs_dir).unwrap();

    // Copy production Bloodborne pack so /ui/packs, /packs, and / render real SHA-256 + OAP edition
    let bb_src =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("packs/sony.bloodborne.bbport.onehundo");
    fs::copy(&bb_src, packs_dir.join("sony.bloodborne.bbport.onehundo")).unwrap();

    let mut cfg = DaemonConfig::default();
    cfg.config_dir = dir.join("cfg");
    cfg.data_dir = dir.join("data");
    cfg.packs_dirs = vec![packs_dir.clone()];
    cfg.watched_observation_dirs = vec![WatchedObservationDir {
        path: watch_dir.clone(),
        canonical_game_id: Some("steam.watched.demo".into()),
    }];
    cfg.notifications_enabled = false;

    let engine = DaemonEngine::initialize(cfg).expect("init engine");

    // 1. Watcher imports dropped Goldberg file idempotently
    let watched_gb = watch_dir.join("achievements.json");
    fs::write(
        &watched_gb,
        r#"{ "ACH_WATCHED_1": { "earned": true, "earned_time": 0 } }"#,
    )
    .unwrap();
    let poll1 = engine.poll_watched_observation_dirs();
    assert_eq!(poll1.files_changed, 1);
    assert_eq!(poll1.imported, 1);

    let poll2 = engine.poll_watched_observation_dirs();
    assert_eq!(poll2.files_changed, 0);
    assert_eq!(poll2.imported, 0);

    // 2. Start a VERIFIED_INTEGRATION session, downgrade via SaveStateLoad + record FRAME_GAPs
    let (sess, _) = engine.start_session(
        "".into(),
        "test_emu".into(),
        "sony.bloodborne".into(),
        MatchQuery {
            canonical_game_id: Some("sony.bloodborne".into()),
            sidecar_uuid: None,
            rom_hash: None,
            recomp_game_id: None,
            build_id: None,
            asset_archive_hash: None,
            platform_id: None,
            region: None,
        },
        IngestionPlane::MemoryArena,
        ProgressionMode::Standard,
        TrustTier::VerifiedIntegration,
    );
    let _ = engine.apply_fair_play_violation(&sess.session_id, FairPlayViolation::SaveStateLoad);
    let _ = engine.handle_lifecycle_signal(&sess.session_id, LifecycleSignal::FrameGap, 3);

    // 3. Start local Game Health HTTP server
    let shutdown = Arc::new(AtomicBool::new(false));
    let (addr, http_handle) =
        start_health_http_server(Arc::clone(&engine), "127.0.0.1:0", Arc::clone(&shutdown))
            .expect("start health server");
    let base = format!("http://{}", addr);

    // Verify GET / renders session mode, trust tier, downgrade reason, FRAME_GAP counters, packs, and interactive forms
    let html_resp = http_request("GET", &format!("{}/", base), None, None, None).unwrap();
    assert_eq!(html_resp.status, 200);
    assert!(html_resp.body.contains("OneHundo Game Health Diagnostics"));
    assert!(html_resp.body.contains("STANDARD"));
    assert!(html_resp.body.contains("LOCAL"));
    assert!(html_resp.body.contains("save_state_load"));
    assert!(html_resp.body.contains("shm_frame_gaps_reported"));
    assert!(html_resp.body.contains("ipc_frame_gaps_reported: 3"));
    assert!(html_resp.body.contains("sony.bloodborne.bbport.pc"));
    assert!(html_resp.body.contains("psn-provisional-bloodborne-ps4"));
    assert!(html_resp.body.contains("Sync Now"));
    assert!(html_resp.body.contains("/session/mode"));
    assert!(html_resp.body.contains("/v1/observation/import"));
    assert!(html_resp.body.contains("null (not invented)"));

    // Verify GET /ui/packs & GET /packs
    let packs_html = http_request("GET", &format!("{}/ui/packs", base), None, None, None).unwrap();
    assert_eq!(packs_html.status, 200);
    assert!(packs_html.body.contains("sony.bloodborne.bbport.pc"));
    assert!(packs_html.body.contains("psn-provisional-bloodborne-ps4"));

    let packs_json = http_request("GET", &format!("{}/packs", base), None, None, None).unwrap();
    assert_eq!(packs_json.status, 200);
    let pv: serde_json::Value = serde_json::from_str(&packs_json.body).unwrap();
    assert_eq!(
        pv["pack_summaries"][0]["mapped_oap_edition"].as_str(),
        Some("psn-provisional-bloodborne-ps4")
    );
    assert_eq!(
        pv["pack_summaries"][0]["pack_sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );

    // 4. Switch to SPECTATOR via POST /session/mode and import via POST /v1/observation/import
    let mode_resp = http_request(
        "POST",
        &format!("{}/session/mode", base),
        None,
        Some(r#"{"mode":"SPECTATOR"}"#),
        None,
    )
    .unwrap();
    assert_eq!(mode_resp.status, 200);
    let mv: serde_json::Value = serde_json::from_str(&mode_resp.body).unwrap();
    assert_eq!(mv["active_progression_mode"].as_str(), Some("SPECTATOR"));

    let spec_file = dir.join("spectator_obs.json");
    fs::write(
        &spec_file,
        r#"{ "canonical_game_id": "sony.bloodborne", "achievements": [{ "id": "ach_spectator_blocked", "unlocked": true }] }"#,
    )
    .unwrap();
    let imp_body = serde_json::json!({
        "path": spec_file.display().to_string()
    })
    .to_string();
    let imp_resp = http_request(
        "POST",
        &format!("{}/v1/observation/import", base),
        None,
        Some(&imp_body),
        None,
    )
    .unwrap();
    assert_eq!(imp_resp.status, 200);
    let iv: serde_json::Value = serde_json::from_str(&imp_resp.body).unwrap();
    assert_eq!(iv["imported"].as_u64(), Some(1));
    assert_eq!(iv["records"][0]["progression_mode"].as_str(), Some("SPECTATOR"));

    // Persistent trophy_state MUST NOT be mutated during SPECTATOR mode
    {
        let db = engine.db.lock().unwrap();
        assert!(list_trophies(&db, Some("sony.bloodborne")).unwrap().is_empty());
    }

    // Health JSON confirms SPECTATOR mode & ipc_frame_gaps_reported == 3
    let health_resp = http_request("GET", &format!("{}/health", base), None, None, None).unwrap();
    assert_eq!(health_resp.status, 200);
    let hv: serde_json::Value = serde_json::from_str(&health_resp.body).unwrap();
    assert_eq!(hv["active_progression_mode"].as_str(), Some("SPECTATOR"));
    assert_eq!(hv["plane_a"]["rings"][0]["ipc_frame_gaps_reported"].as_u64(), Some(3));

    // 5. Test onehundo-desktop binary subcommands (packs, mode, import-obs --dry-run, import-obs)
    let desktop_bin = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/onehundo-desktop");
    if desktop_bin.exists() {
        let packs_cmd = Command::new(&desktop_bin)
            .args(["packs", "--url", &base])
            .output()
            .expect("run onehundo-desktop packs");
        assert!(packs_cmd.status.success());
        let packs_out = String::from_utf8_lossy(&packs_cmd.stdout);
        assert!(packs_out.contains("sony.bloodborne.bbport.pc"));

        let mode_cmd = Command::new(&desktop_bin)
            .args(["mode", "ENCORE", "--url", &base])
            .output()
            .expect("run onehundo-desktop mode");
        assert!(mode_cmd.status.success());
        assert_eq!(engine.current_progression_mode(), ProgressionMode::Encore);

        // --dry-run must NOT mutate SQLite
        let cli_data = dir.join("cli_data");
        let dry_file = dir.join("cli_obs.json");
        fs::write(
            &dry_file,
            r#"{ "canonical_game_id": "indie.cli_game", "achievements": [{ "id": "ach_cli_one", "unlocked": true }] }"#,
        )
        .unwrap();
        let dry_cmd = Command::new(&desktop_bin)
            .args([
                "import-obs",
                dry_file.to_str().unwrap(),
                "--data-dir",
                cli_data.to_str().unwrap(),
                "--dry-run",
            ])
            .output()
            .expect("run import-obs --dry-run");
        assert!(dry_cmd.status.success());
        assert!(!cli_data.join("onehundo_state.db").exists());

        // Real offline import via CLI
        let imp_cmd = Command::new(&desktop_bin)
            .args([
                "import-obs",
                dry_file.to_str().unwrap(),
                "--data-dir",
                cli_data.to_str().unwrap(),
            ])
            .output()
            .expect("run import-obs");
        assert!(imp_cmd.status.success());
        assert!(String::from_utf8_lossy(&imp_cmd.stdout).contains("Imported 1 observation(s)"));
    }

    // Ensure direct dry_run API preserves non-mutation too
    let dry_res = {
        let mut db = engine.db.lock().unwrap();
        import_observation_path_with_options(
            &mut db,
            &spec_file,
            None,
            &ImportOptions {
                canonical_game_id: Some("sony.bloodborne".into()),
                override_progression_mode: Some(ProgressionMode::Standard),
                dry_run: true,
            },
        )
        .unwrap()
    };
    assert!(dry_res.dry_run);
    assert_eq!(dry_res.records.len(), 1);
    {
        let db = engine.db.lock().unwrap();
        assert!(list_trophies(&db, Some("sony.bloodborne")).unwrap().is_empty());
    }

    shutdown.store(true, Ordering::SeqCst);
    let _ = http_handle.join();
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn longshot_binary_save_watcher_and_spectator_non_mutation() {
    let dir = common::temp_test_dir("plane_c_longshot");
    let watch_dir = dir.join("longshot_save/5a3c9e1f0b7d2468/slot_00");
    let packs_dir = dir.join("packs");
    fs::create_dir_all(&watch_dir).unwrap();
    fs::create_dir_all(&packs_dir).unwrap();

    let ls_src =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("packs/nintendo.oot.longshot.pc.onehundo");
    fs::copy(&ls_src, packs_dir.join("nintendo.oot.longshot.pc.onehundo")).unwrap();

    let mut cfg = DaemonConfig::default();
    cfg.config_dir = dir.join("cfg");
    cfg.data_dir = dir.join("data");
    cfg.packs_dirs = vec![packs_dir];
    cfg.watched_observation_dirs = vec![WatchedObservationDir {
        path: dir.join("longshot_save"),
        canonical_game_id: None,
    }];
    cfg.notifications_enabled = false;

    let engine = DaemonEngine::initialize(cfg).expect("init longshot engine");

    // 1. Watcher picks up nested progress.sav under watched root cleanly
    let sav_path = watch_dir.join("progress.sav");
    let sav_v1 = build_synthetic_longshot_progress_sav(
        16,
        16, // 4 hearts = 3 base + 1 Deku container + 0 heart pieces
        &[0xf914_fa36_7471_a3e3],
        None,
    );
    fs::write(&sav_path, &sav_v1).unwrap();

    let poll1 = engine.poll_watched_observation_dirs();
    assert_eq!(poll1.files_changed, 1);
    assert_eq!(poll1.imported, 4); // a01 + skulltulas_collected + heart_pieces + bosses_defeated

    let poll2 = engine.poll_watched_observation_dirs();
    assert_eq!(poll2.files_changed, 0);
    assert_eq!(poll2.imported, 0);

    {
        let db = engine.db.lock().unwrap();
        let trophies = list_trophies(&db, Some("nintendo.oot.longshot.pc")).unwrap();
        assert_eq!(trophies.len(), 1);
        assert_eq!(trophies[0].canonical_achievement_id, "a01");
        assert_eq!(trophies[0].evidence_class, "LOCALLY_OBSERVED");
        assert_eq!(trophies[0].verification_state, "UNVERIFIED");

        let stats = list_stats(&db, Some("nintendo.oot.longshot.pc")).unwrap();
        assert_eq!(
            stats
                .iter()
                .find(|s| s.stat_key == "skulltulas_collected")
                .and_then(|s| s.int_value),
            Some(16)
        );
        assert_eq!(
            stats
                .iter()
                .find(|s| s.stat_key == "bosses_defeated")
                .and_then(|s| s.int_value),
            Some(1)
        );
        assert_eq!(
            stats
                .iter()
                .find(|s| s.stat_key == "heart_pieces")
                .and_then(|s| s.int_value),
            Some(0)
        );

        let null_ts_count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM observation_journal WHERE canonical_game_id = 'nintendo.oot.longshot.pc' AND source_timestamp IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(null_ts_count, 4);
    }

    // 2. Pack registration maps nintendo.oot.longshot.pc -> nintendo-provisional-oot-switch2
    {
        let reg = engine.packs.lock().unwrap();
        let pack = reg.get("nintendo.oot.longshot.pc").expect("longshot pack loaded");
        assert_eq!(
            pack.mapped_oap_edition.as_deref(),
            Some("nintendo-provisional-oot-switch2")
        );
    }

    // 3. SPECTATOR mode must NOT mutate trophy_state or stat_state when updated save arrives
    engine.set_progression_mode(None, ProgressionMode::Spectator);
    let sav_v2 = build_synthetic_longshot_progress_sav(
        100,
        24,
        &[0xf914_fa36_7471_a3e3, 0xff11_593a_2277_8e7e],
        Some("Weapon_Sword_Master"),
    );
    fs::write(&sav_path, &sav_v2).unwrap();
    let poll_spec = engine.poll_watched_observation_dirs();
    assert_eq!(poll_spec.files_changed, 1);

    {
        let db = engine.db.lock().unwrap();
        let trophies = list_trophies(&db, Some("nintendo.oot.longshot.pc")).unwrap();
        assert_eq!(trophies.len(), 1); // still only a01 from STANDARD
        let stats = list_stats(&db, Some("nintendo.oot.longshot.pc")).unwrap();
        assert_eq!(
            stats
                .iter()
                .find(|s| s.stat_key == "skulltulas_collected")
                .and_then(|s| s.int_value),
            Some(16)
        );
    }

    let _ = fs::remove_dir_all(&dir);
}
