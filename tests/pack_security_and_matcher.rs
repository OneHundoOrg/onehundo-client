mod common;

use onehundo_client::packs::manifest::load_pack_from_zip_bytes;
use onehundo_client::packs::matcher::{MatchQuery, PackRegistry};
use std::fs;
use std::path::Path;

const VALID_MANIFEST: &str = r#"{
  "manifest_version": "0.1.0",
  "pack_id": "test.security.pack",
  "canonical_game_id": "test.game",
  "title": "Security Test Pack",
  "matchers": [
    {"matcher_type": "sidecar_uuid", "value": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"},
    {"matcher_type": "rom_hash", "value": "deadbeef01", "region": "USA"},
    {"matcher_type": "build_id", "value": "build-123"}
  ],
  "profiles": {"semantic": {"enabled": true, "producer_ids": ["test_prod"]}},
  "achievements": [{
    "canonical_achievement_id": "ach_one",
    "title": "One",
    "description": "Test",
    "points": 10,
    "supported_modes": ["STANDARD"],
    "minimum_trust_tier": "LOCAL"
  }]
}"#;

#[test]
fn validates_protocol_fixture_bbport_pack() {
    let pack_path = common::protocol_root()
        .join("conformance/fixtures/packs/bbport_sample.onehundo");
    let bytes = fs::read(&pack_path).expect("read bbport_sample.onehundo");
    let loaded = load_pack_from_zip_bytes(&bytes, &pack_path.display().to_string())
        .expect("bbport_sample.onehundo must validate");
    assert_eq!(loaded.manifest.canonical_game_id, "sony.bloodborne");
    assert_eq!(loaded.semantic_rules.len(), 1);
    assert_eq!(loaded.semantic_rules[0].achievement_id, "ach_cleric_beast");
}

#[test]
fn validates_bloodborne_bbport_production_pack() {
    let pack_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("packs/sony.bloodborne.bbport.onehundo");
    let bytes = fs::read(&pack_path).expect("read sony.bloodborne.bbport.onehundo");
    let loaded = load_pack_from_zip_bytes(&bytes, &pack_path.display().to_string())
        .expect("sony.bloodborne.bbport.onehundo must validate");
    assert_eq!(loaded.manifest.pack_id, "sony.bloodborne.bbport.pc");
    assert_eq!(loaded.manifest.canonical_game_id, "sony.bloodborne");
    assert_eq!(loaded.manifest.achievements.len(), 46);
    assert!(loaded.semantic_rules.len() >= 4);

    let mut reg = PackRegistry::new();
    reg.insert_pack(loaded);
    let (_, matched) = reg
        .match_pack(&MatchQuery {
            canonical_game_id: Some("sony.bloodborne".into()),
            sidecar_uuid: None,
            rom_hash: None,
            recomp_game_id: Some("bbport".into()),
            build_id: Some("bbport-0.7-v1.09".into()),
            asset_archive_hash: None,
            platform_id: Some("CUSA03173".into()),
            region: None,
        })
        .expect("bbport query must match sony.bloodborne.bbport.pc");
    assert_eq!(matched.pack_id, "sony.bloodborne.bbport.pc");
    assert_eq!(matched.canonical_game_id, "sony.bloodborne");
}

#[test]
fn validates_oot_longshot_production_pack() {
    let pack_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("packs/nintendo.oot.longshot.pc.onehundo");
    let bytes = fs::read(&pack_path).expect("read nintendo.oot.longshot.pc.onehundo");
    let loaded = load_pack_from_zip_bytes(&bytes, &pack_path.display().to_string())
        .expect("nintendo.oot.longshot.pc.onehundo must validate");
    assert_eq!(loaded.manifest.pack_id, "nintendo.oot.longshot.pc");
    assert_eq!(loaded.manifest.canonical_game_id, "nintendo.oot.longshot.pc");
    assert_eq!(
        loaded.mapped_oap_edition.as_deref(),
        Some("nintendo-provisional-oot-switch2")
    );
    assert_eq!(loaded.manifest.achievements.len(), 12);

    let mut reg = PackRegistry::new();
    reg.insert_pack(loaded);
    let (_, matched) = reg
        .match_pack(&MatchQuery {
            canonical_game_id: Some("nintendo.oot.longshot.pc".into()),
            sidecar_uuid: None,
            rom_hash: None,
            recomp_game_id: Some("longshot".into()),
            build_id: Some("longshot-nightly-805-6b928cb4".into()),
            asset_archive_hash: None,
            platform_id: Some("provisional:oot-switch2".into()),
            region: None,
        })
        .expect("longshot query must match nintendo.oot.longshot.pc");
    assert_eq!(matched.pack_id, "nintendo.oot.longshot.pc");
    assert_eq!(matched.canonical_game_id, "nintendo.oot.longshot.pc");
}

#[test]
fn rejects_zip_slip_symlink_and_executables() {
    let zip_slip = common::build_test_zip(&[
        ("manifest.json", VALID_MANIFEST.as_bytes(), 0o100644 << 16),
        ("../../etc/passwd.json", b"{}", 0o100644 << 16),
    ]);
    let err = load_pack_from_zip_bytes(&zip_slip, "slip.onehundo").unwrap_err();
    assert!(err.contains("traversal"), "unexpected: {}", err);

    let symlink = common::build_test_zip(&[
        ("manifest.json", VALID_MANIFEST.as_bytes(), 0o100644 << 16),
        ("assets/evil.json", b"/etc/passwd", 0o120777 << 16),
    ]);
    let err = load_pack_from_zip_bytes(&symlink, "sym.onehundo").unwrap_err();
    assert!(err.contains("symbolic link"), "unexpected: {}", err);

    let exe = common::build_test_zip(&[
        ("manifest.json", VALID_MANIFEST.as_bytes(), 0o100644 << 16),
        ("payload.sh", b"#!/bin/sh", 0o100755 << 16),
    ]);
    let err = load_pack_from_zip_bytes(&exe, "exe.onehundo").unwrap_err();
    assert!(err.contains("disallowed file extension"), "unexpected: {}", err);
}

#[test]
fn enforces_deterministic_matcher_precedence() {
    let pack_a = load_pack_from_zip_bytes(
        &common::build_test_zip(&[("manifest.json", VALID_MANIFEST.as_bytes(), 0o100644 << 16)]),
        "a.onehundo",
    )
    .unwrap();

    let manifest_b = r#"{
      "manifest_version": "0.1.0",
      "pack_id": "test.rom.priority",
      "canonical_game_id": "test.game.b",
      "title": "ROM Priority Pack",
      "matchers": [
        {"matcher_type": "rom_hash", "value": "cafebabe", "region": "EUR"}
      ],
      "profiles": {"semantic": {"enabled": true, "producer_ids": ["p2"]}},
      "achievements": [{
        "canonical_achievement_id": "ach_two",
        "title": "Two",
        "description": "Test",
        "points": 10,
        "supported_modes": ["STANDARD"],
        "minimum_trust_tier": "LOCAL"
      }]
    }"#;
    let pack_b = load_pack_from_zip_bytes(
        &common::build_test_zip(&[("manifest.json", manifest_b.as_bytes(), 0o100644 << 16)]),
        "b.onehundo",
    )
    .unwrap();

    let mut reg = PackRegistry::new();
    reg.insert_pack(pack_a);
    reg.insert_pack(pack_b);

    // Query matches both: sidecar_uuid on pack_a (rank 1) beats rom_hash on pack_b (rank 2)
    let (_, matched) = reg
        .match_pack(&MatchQuery {
            canonical_game_id: None,
            sidecar_uuid: Some("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into()),
            rom_hash: Some("cafebabe".into()),
            recomp_game_id: None,
            build_id: None,
            asset_archive_hash: None,
            platform_id: None,
            region: Some("EUR".into()),
        })
        .expect("should match pack");
    assert_eq!(matched.pack_id, "test.security.pack");
    assert_eq!(matched.precedence_rank, 1);

    // Wrong region on rom_hash does not match pack_b
    assert!(reg
        .match_pack(&MatchQuery {
            canonical_game_id: None,
            sidecar_uuid: None,
            rom_hash: Some("cafebabe".into()),
            recomp_game_id: None,
            build_id: None,
            asset_archive_hash: None,
            platform_id: None,
            region: Some("JPN".into()),
        })
        .is_none());
}
