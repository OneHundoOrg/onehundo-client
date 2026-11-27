mod common;

use onehundo_client::core::config::DaemonConfig;
use onehundo_client::core::engine::DaemonEngine;
use onehundo_client::core::types::{FairPlayViolation, IngestionPlane, ProgressionMode, TrustTier};
use onehundo_client::evaluation::semantic_events::SemanticEventPayload;
use onehundo_client::packs::matcher::MatchQuery;
use onehundo_client::storage::state::{list_encore_runs, list_stats, list_trophies};
use std::thread;
use std::time::Duration;

#[test]
fn enforces_spectator_non_mutation_encore_preservation_and_trust_downgrade() {
    let dir = common::temp_test_dir("modes");
    let mut cfg = DaemonConfig::default();
    cfg.config_dir = dir.join("cfg");
    cfg.data_dir = dir.join("data");
    cfg.packs_dirs = vec![];
    cfg.notifications_enabled = false;

    let engine = DaemonEngine::initialize(cfg).expect("init engine");
    let pack_path = common::protocol_root().join("conformance/fixtures/packs/bbport_sample.onehundo");
    engine.load_pack_path(&pack_path).expect("load pack");

    // 1. SPECTATOR session must NEVER mutate trophy_state or stat_state
    let spec_sid = "10000000-0000-4000-8000-000000000001".to_string();
    engine.start_session(
        spec_sid.clone(),
        "bbport_pc_x64".into(),
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
            ..Default::default()
        },
        IngestionPlane::SemanticEvent,
        ProgressionMode::Spectator,
        TrustTier::Local,
    );

    let spec_stat = SemanticEventPayload {
        event_type: "STAT_INC".into(),
        canonical_game_id: "sony.bloodborne".into(),
        producer_id: "bbport_pc_x64".into(),
        session_id: spec_sid.clone(),
        sequence_number: 1,
        idempotency_key: None,
        timestamp_nanos: None,
        key: "bosses_defeated".into(),
        int_value: Some(5),
        float_value: None,
        string_value: None,
        progression_mode: None,
        trust_tier: None,
        evidence_class: None,
        verification_state: None,
    };
    let spec_unlock = SemanticEventPayload {
        event_type: "UNLOCK_REQUEST".into(),
        canonical_game_id: "sony.bloodborne".into(),
        producer_id: "bbport_pc_x64".into(),
        session_id: spec_sid.clone(),
        sequence_number: 2,
        idempotency_key: None,
        timestamp_nanos: None,
        key: "ach_cleric_beast".into(),
        int_value: Some(1),
        float_value: None,
        string_value: None,
        progression_mode: None,
        trust_tier: None,
        evidence_class: None,
        verification_state: None,
    };
    let out1 = engine.handle_plane_b_event(Some(&spec_sid), &spec_stat, "{}").unwrap();
    let out2 = engine.handle_plane_b_event(Some(&spec_sid), &spec_unlock, "{}").unwrap();
    assert!(out1.updated_stat_int.is_none());
    assert!(out2.newly_unlocked.is_empty());

    {
        let db = engine.db.lock().unwrap();
        assert!(list_trophies(&db, Some("sony.bloodborne")).unwrap().is_empty());
        assert!(list_stats(&db, Some("sony.bloodborne")).unwrap().is_empty());
    }

    // 2. STANDARD session unlocks ach_cleric_beast as VERIFIED_INTEGRATION
    let std_sid = "10000000-0000-4000-8000-000000000002".to_string();
    engine.start_session(
        std_sid.clone(),
        "bbport_pc_x64".into(),
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
            ..Default::default()
        },
        IngestionPlane::SemanticEvent,
        ProgressionMode::Standard,
        TrustTier::VerifiedIntegration,
    );
    let std_unlock = SemanticEventPayload {
        session_id: std_sid.clone(),
        sequence_number: 1,
        ..spec_unlock.clone()
    };
    let std_out = engine.handle_plane_b_event(Some(&std_sid), &std_unlock, "{}").unwrap();
    assert_eq!(std_out.newly_unlocked.len(), 1);
    let std_first_ts = {
        let db = engine.db.lock().unwrap();
        let tr = list_trophies(&db, Some("sony.bloodborne")).unwrap();
        assert_eq!(tr.len(), 1);
        assert_eq!(tr[0].progression_mode, "STANDARD");
        assert_eq!(tr[0].verification_state, "VERIFIED");
        tr[0].first_unlocked_at.clone()
    };

    // Trigger fair-play violation -> automatic VERIFIED_INTEGRATION -> LOCAL downgrade
    let downgrade = engine.apply_fair_play_violation(&std_sid, FairPlayViolation::CheatToggle);
    assert!(downgrade.is_some());
    let snap = engine.health_snapshot().unwrap();
    let std_sess = snap.sessions.iter().find(|s| s.session_id == std_sid).unwrap();
    assert_eq!(std_sess.active_trust_tier, TrustTier::Local);
    assert!(std_sess.is_downgraded());
    assert_eq!(std_sess.downgrade_reason.as_deref(), Some("cheat_toggle"));

    // Subsequent stat/event in downgraded session runs cleanly in LOCAL (never deletes prior VERIFIED unlock)
    let after_cheat = SemanticEventPayload {
        event_type: "STAT_INC".into(),
        session_id: std_sid.clone(),
        sequence_number: 2,
        key: "bosses_defeated".into(),
        int_value: Some(1),
        ..spec_stat.clone()
    };
    let out_after = engine.handle_plane_b_event(Some(&std_sid), &after_cheat, "{}").unwrap();
    assert_eq!(out_after.updated_stat_int, Some(1));

    // 3. ENCORE mode preserves STANDARD first_unlocked_at and logs separate replay runs
    thread::sleep(Duration::from_millis(1100));
    let enc_sid = "10000000-0000-4000-8000-000000000003".to_string();
    engine.start_session(
        enc_sid.clone(),
        "bbport_pc_x64".into(),
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
            ..Default::default()
        },
        IngestionPlane::SemanticEvent,
        ProgressionMode::Encore,
        TrustTier::Local,
    );
    for seq in 1..=2 {
        let enc_ev = SemanticEventPayload {
            session_id: enc_sid.clone(),
            sequence_number: seq,
            ..spec_unlock.clone()
        };
        let _ = engine.handle_plane_b_event(Some(&enc_sid), &enc_ev, "{}").unwrap();
    }

    {
        let db = engine.db.lock().unwrap();
        let tr = list_trophies(&db, Some("sony.bloodborne")).unwrap();
        assert_eq!(tr.len(), 2); // STANDARD + ENCORE rows distinct
        let std_row = tr.iter().find(|t| t.progression_mode == "STANDARD").unwrap();
        let enc_row = tr.iter().find(|t| t.progression_mode == "ENCORE").unwrap();
        assert_eq!(std_row.first_unlocked_at, std_first_ts);
        assert_eq!(enc_row.replay_count, 2);

        let runs = list_encore_runs(&db).unwrap();
        assert_eq!(runs.len(), 2);
    }

    let _ = std::fs::remove_dir_all(&dir);
}
