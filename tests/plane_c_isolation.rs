mod common;

use onehundo_client::core::config::DaemonConfig;
use onehundo_client::core::engine::DaemonEngine;
use onehundo_client::core::types::{EvidenceClass, ProgressionMode};
use onehundo_client::observation::external_importers::{
    build_save_file_observation, import_observation_file, import_observation_records,
    parse_observation_json_str, sha256_hex,
};
use onehundo_client::storage::journal::fetch_pending_sync_events;
use onehundo_client::storage::state::{list_stats, list_trophies};
use std::fs;
use std::path::Path;

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
fn rejects_gameplay_evidence_classes_and_respects_spectator_isolation() {
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

    // 3. Static architectural source audit: observation/ MUST NEVER reference evaluation/
    let obs_src = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("daemon/observation/external-importers/mod.rs"),
    )
    .unwrap();
    assert!(
        !obs_src.contains("crate::evaluation") && !obs_src.contains("evaluation::"),
        "Plane C importer must not import or call crate::evaluation"
    );

    let _ = fs::remove_dir_all(&dir);
}
