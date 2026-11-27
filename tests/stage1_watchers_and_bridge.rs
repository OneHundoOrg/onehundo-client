mod common;

use onehundo_client::core::config::DaemonConfig;
use onehundo_client::core::engine::DaemonEngine;
use onehundo_client::core::types::ProgressionMode;
use onehundo_client::diagnostics::server::start_health_http_server;
use onehundo_client::observation::external_importers::declarative_save::safe_contained_path;
use onehundo_client::packs::matcher::{MatchQuery, PackRegistry};
use onehundo_client::storage::state::{list_stats, list_trophies};
use onehundo_client::sync::http::http_request;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpStream, UdpSocket};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;

#[test]
fn discovers_all_stage1_starter_packs_with_clean_titles_and_multi_matchers() {
    let packs_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("packs");
    let mut reg = PackRegistry::new();
    let count = reg.discover_in_dir(&packs_dir);
    assert!(
        reg.load_errors().is_empty(),
        "unexpected pack load errors: {:?}",
        reg.load_errors()
    );
    assert!(
        count >= 28,
        "expected >= 28 Stage 1 + baseline packs in {:?}, got {}",
        packs_dir,
        count
    );

    // Every manifest title must remain clean and canonical (no runtime/launcher suffix in parentheses).
    for pack in reg.list() {
        if pack.manifest.pack_id == "sony.bloodborne.bbport.pc" {
            continue;
        }
        assert!(
            !pack.manifest.title.contains("(Longshot)")
                && !pack.manifest.title.contains("(Switch 2")
                && !pack.manifest.title.contains("(shadPS4")
                && !pack.manifest.title.contains("(bbport"),
            "pack '{}' has non-canonical runtime suffix in title '{}'",
            pack.manifest.pack_id,
            pack.manifest.title
        );
    }

    // Verify fractured-fork multi-matchers resolve properly:
    let (_, sotc) = reg
        .match_pack(&MatchQuery {
            platform_id: Some("BCUS98289".into()),
            launcher: Some("rpcs3".into()),
            ..Default::default()
        })
        .expect("Demon/SotC RPCS3 match");
    assert_eq!(sotc.canonical_game_id, "sony.shadow_of_the_colossus");

    let (_, oot_fork) = reg
        .match_pack(&MatchQuery {
            executable: Some("/opt/games/relink/relink_linux.x86_64".into()),
            os: Some("linux".into()),
            ..Default::default()
        })
        .expect("Ocarina of Time ReLink executable match");
    assert_eq!(oot_fork.canonical_game_id, "nintendo.oot.switch2");

    let (_, gta6) = reg
        .match_pack(&MatchQuery {
            save_path: Some("C:/Users/j/Documents/Rockstar Games/GTA VI/Profiles/SGTA60000".into()),
            os: Some("windows".into()),
            ..Default::default()
        })
        .expect("GTA6 save glob match");
    assert_eq!(gta6.canonical_game_id, "rockstar.gta6");
}

#[test]
fn declarative_binary_json_ini_and_template_placeholder_safety() {
    let dir = common::temp_test_dir("decl_save");
    let mut cfg = DaemonConfig::default();
    cfg.config_dir = dir.join("cfg");
    cfg.data_dir = dir.join("data");
    cfg.packs_dirs = vec![];
    cfg.notifications_enabled = false;
    let engine = DaemonEngine::initialize(cfg).expect("init engine");

    let manifest = r#"{
      "manifest_version": "0.1.0",
      "pack_id": "test.declarative.formats",
      "canonical_game_id": "test.decl_formats",
      "title": "Declarative Test Game",
      "matchers": [
        {"matcher_type": "executable", "value": "*DeclGame*"}
      ],
      "profiles": {
        "observation": {
          "enabled": true,
          "rules_file": "rules/save.json",
          "sources": [
            {"source_type": "save_file", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/save_*.bin"},
            {"source_type": "save_file", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/state.json"},
            {"source_type": "save_file", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/prefs.ini"},
            {"source_type": "save_file", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/future_*.sav"}
          ]
        }
      },
      "achievements": [
        {"canonical_achievement_id": "ach_bin_bit", "title": "Binary Bit", "description": "d", "points": 10, "supported_modes": ["STANDARD", "SPECTATOR"], "minimum_trust_tier": "LOCAL"},
        {"canonical_achievement_id": "ach_json_boss", "title": "JSON Boss", "description": "d", "points": 10, "supported_modes": ["STANDARD"], "minimum_trust_tier": "LOCAL"},
        {"canonical_achievement_id": "ach_ini_clear", "title": "INI Clear", "description": "d", "points": 10, "supported_modes": ["STANDARD"], "minimum_trust_tier": "LOCAL"},
        {"canonical_achievement_id": "ach_template_never", "title": "Template Never", "description": "d", "points": 10, "supported_modes": ["STANDARD"], "minimum_trust_tier": "LOCAL"}
      ]
    }"#;

    let save_rules = r#"{
      "schema_version": "0.1.0",
      "canonical_game_id": "test.decl_formats",
      "watchers": [
        {
          "id": "bin_w",
          "source_type": "save_file",
          "format": "binary",
          "file_glob": "**/save_*.bin",
          "magic_ascii": "OHSV",
          "magic_offset": 0,
          "endianness": "BIG",
          "max_file_size_bytes": 1024,
          "achievement_rules": [
            {
              "achievement_id": "ach_bin_bit",
              "conditions": [
                {"offset": 4, "value_type": "u16", "op": ">=", "value": 258},
                {"offset": 8, "value_type": "u8", "bit": 3, "op": "bit_set"}
              ]
            }
          ],
          "stat_rules": [
            {"stat_key": "stars_collected", "offset": 12, "value_type": "u32", "endianness": "LITTLE"}
          ]
        },
        {
          "id": "json_w",
          "source_type": "save_file",
          "format": "json",
          "file_glob": "**/state.json",
          "achievement_rules": [
            {
              "achievement_id": "ach_json_boss",
              "conditions": [
                {"path": "slots[0].bosses.colossus", "value_type": "bool", "op": "==", "value": true}
              ]
            }
          ]
        },
        {
          "id": "ini_w",
          "source_type": "save_file",
          "format": "ini",
          "file_glob": "**/prefs.ini",
          "achievement_rules": [
            {
              "achievement_id": "ach_ini_clear",
              "conditions": [
                {"section": "Progress", "key": "CampaignClear", "value_type": "i64", "op": "==", "value": 1}
              ]
            }
          ]
        },
        {
          "id": "tmpl_w",
          "source_type": "save_file",
          "format": "template",
          "file_glob": "**/future_*.sav",
          "placeholder_template": true,
          "notes": "Must never unlock even if a matching file exists",
          "achievement_rules": [
            {
              "achievement_id": "ach_template_never",
              "conditions": [{"offset": 0, "value_type": "u8", "op": "nonzero"}]
            }
          ]
        }
      ]
    }"#;

    let pack_bytes = common::build_test_zip(&[
        ("manifest.json", manifest.as_bytes(), 0o100644 << 16),
        ("rules/save.json", save_rules.as_bytes(), 0o100644 << 16),
    ]);
    let pack_path = dir.join("decl.onehundo");
    fs::write(&pack_path, pack_bytes).unwrap();
    engine.load_pack_path(&pack_path).unwrap();

    let save_dir = dir.join("saves");
    fs::create_dir_all(&save_dir).unwrap();

    // Write valid binary save (`OHSV`, u16 BE = 300 at 4, bit 3 set at 8, u32 LE = 77 at 12)
    let mut bin_buf = vec![0u8; 32];
    bin_buf[0..4].copy_from_slice(b"OHSV");
    bin_buf[4..6].copy_from_slice(&300u16.to_be_bytes());
    bin_buf[8] = 1 << 3;
    bin_buf[12..16].copy_from_slice(&77u32.to_le_bytes());
    let bin_path = save_dir.join("save_slot1.bin");
    fs::write(&bin_path, &bin_buf).unwrap();

    let json_path = save_dir.join("state.json");
    fs::write(&json_path, r#"{"slots":[{"bosses":{"colossus":true}}]}"#).unwrap();

    let ini_path = save_dir.join("prefs.ini");
    fs::write(&ini_path, "[Progress]\nCampaignClear=1\n").unwrap();

    let tmpl_path = save_dir.join("future_slot.sav");
    fs::write(&tmpl_path, &[0xFF; 16]).unwrap();

    // Check SPECTATOR non-mutation first
    let spec_res = engine
        .import_plane_c_path(&bin_path, Some("test.decl_formats".into()), Some(ProgressionMode::Spectator), false)
        .unwrap();
    assert_eq!(spec_res.imported, 2); // ach + stat journaled, zero mutation in trophy_state
    {
        let db = engine.db.lock().unwrap();
        assert!(list_trophies(&db, Some("test.decl_formats")).unwrap().is_empty());
    }

    // Bind watch folder via Manual Watch Override and scan!
    engine
        .add_watch_override(Some("test.decl_formats".into()), &save_dir)
        .unwrap();
    let poll = engine.poll_watched_observation_dirs();
    assert!(poll.errors.is_empty(), "poll errors: {:?}", poll.errors);
    assert!(poll.imported >= 4);

    {
        let db = engine.db.lock().unwrap();
        let trophies = list_trophies(&db, Some("test.decl_formats")).unwrap();
        let ids: Vec<&str> = trophies.iter().map(|t| t.canonical_achievement_id.as_str()).collect();
        assert!(ids.contains(&"ach_bin_bit"));
        assert!(ids.contains(&"ach_json_boss"));
        assert!(ids.contains(&"ach_ini_clear"));
        assert!(!ids.contains(&"ach_template_never"), "placeholder template must never emit unlocks");
        for t in &trophies {
            assert_eq!(t.trust_tier, "LOCAL");
            assert_eq!(t.evidence_class, "LOCALLY_OBSERVED");
            assert_eq!(t.verification_state, "UNVERIFIED");
        }
        let stats = list_stats(&db, Some("test.decl_formats")).unwrap();
        let stars = stats.iter().find(|s| s.stat_key == "stars_collected").unwrap();
        assert_eq!(stars.int_value, Some(77));
    }

    // Security 1: Bounded read check rejects files exceeding max_file_size_bytes (1024)
    let big_bin = save_dir.join("save_oversized.bin");
    fs::write(&big_bin, vec![0x41; 4096]).unwrap();
    let big_err = engine
        .import_plane_c_path(&big_bin, Some("test.decl_formats".into()), None, true)
        .unwrap_err();
    assert!(big_err.contains("bounded read limit"), "unexpected: {}", big_err);

    // Security 2: Symlink escape check rejects outside target
    #[cfg(unix)]
    {
        let outside = dir.join("outside_secret.txt");
        fs::write(&outside, b"secret").unwrap();
        let sym = save_dir.join("escape.bin");
        std::os::unix::fs::symlink(&outside, &sym).unwrap();
        let sym_err = safe_contained_path(&save_dir, &sym).unwrap_err();
        assert!(sym_err.contains("escaping watch root"), "unexpected: {}", sym_err);
    }

    // Security 3: config.json created with 0600 permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(dir.join("cfg/config.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn manual_watch_override_http_cli_and_launcher_bridge_with_retroarch_udp() {
    let dir = common::temp_test_dir("bridge_e2e");
    let packs_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("packs");

    // Ephemeral RetroArch UDP mock server responding with SNES Super Metroid / Bridge CRC `d63ed5f8`
    let ra_server = UdpSocket::bind("127.0.0.1:0").unwrap();
    let ra_addr = ra_server.local_addr().unwrap();
    let ra_stop = Arc::new(AtomicBool::new(false));
    let ra_stop_bg = Arc::clone(&ra_stop);
    let ra_thread = thread::spawn(move || {
        ra_server
            .set_read_timeout(Some(std::time::Duration::from_millis(50)))
            .unwrap();
        let mut ram = vec![0u8; 512];
        ram[0x10] = 1; // Triggers ach_ra_boss_clear in retroarch.multicore.bridge.onehundo!
        let mut buf = [0u8; 512];
        while !ra_stop_bg.load(std::sync::atomic::Ordering::Relaxed) {
            if let Ok((n, src)) = ra_server.recv_from(&mut buf) {
                let cmd = String::from_utf8_lossy(&buf[..n]).trim().to_string();
                let reply = if cmd == "VERSION" {
                    "1.20.0\n".to_string()
                } else if cmd == "GET_STATUS" {
                    "GET_STATUS PLAYING snes9x,RetroArch_Bridge_Test,crc32=d63ed5f8\n".to_string()
                } else if let Some(rest) = cmd.strip_prefix("READ_CORE_MEMORY ") {
                    let mut sp = rest.split_whitespace();
                    let addr = usize::from_str_radix(sp.next().unwrap(), 16).unwrap();
                    let len: usize = sp.next().unwrap().parse().unwrap();
                    let start = addr.saturating_sub(0x7e0000);
                    let slice = &ram[start..(start + len).min(ram.len())];
                    let hex: Vec<String> = slice.iter().map(|b| format!("{:02X}", b)).collect();
                    format!("READ_CORE_MEMORY {:x} {}\n", addr, hex.join(" "))
                } else {
                    "ERR\n".to_string()
                };
                let _ = ra_server.send_to(reply.as_bytes(), src);
            }
        }
    });

    let mut cfg = DaemonConfig::default();
    cfg.config_dir = dir.join("cfg");
    cfg.data_dir = dir.join("data");
    cfg.packs_dirs = vec![packs_dir];
    cfg.retroarch_udp_addr = Some(ra_addr.to_string());
    cfg.notifications_enabled = false;
    let engine = DaemonEngine::initialize(cfg).expect("init engine");

    let shutdown = Arc::new(AtomicBool::new(false));
    let (http_addr, http_handle) =
        start_health_http_server(Arc::clone(&engine), "127.0.0.1:0", Arc::clone(&shutdown))
            .expect("start http server");
    let base = format!("http://{}", http_addr);

    // 1. Cross-origin guard must reject attacker Origin on POST /watches with 403
    let mut raw = TcpStream::connect(http_addr).unwrap();
    let evil_req = format!(
        "POST /watches HTTP/1.1\r\nHost: {}\r\nOrigin: https://evil.example\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}",
        http_addr
    );
    raw.write_all(evil_req.as_bytes()).unwrap();
    let mut evil_resp = String::new();
    let _ = raw.read_to_string(&mut evil_resp);
    assert!(evil_resp.starts_with("HTTP/1.1 403"), "expected 403, got:\n{}", evil_resp);

    // 2. Manual watch override HTTP endpoints (`POST /watches`, `GET /watches`, `POST /watches/remove`)
    let watch_folder = dir.join("portable_saves");
    fs::create_dir_all(&watch_folder).unwrap();
    let add_body = serde_json::json!({
        "canonical_game_id": "sony.bloodborne",
        "path": watch_folder.display().to_string()
    })
    .to_string();
    let add_resp = http_request("POST", &format!("{}/watches", base), None, Some(&add_body), None).unwrap();
    assert_eq!(add_resp.status, 200);

    let list_resp = http_request("GET", &format!("{}/watches", base), None, None, None).unwrap();
    assert_eq!(list_resp.status, 200);
    assert!(list_resp.body.contains("sony.bloodborne"));

    let rm_body = serde_json::json!({ "path": watch_folder.display().to_string() }).to_string();
    let rm_resp = http_request("POST", &format!("{}/watches/remove", base), None, Some(&rm_body), None).unwrap();
    assert_eq!(rm_resp.status, 200);

    // 3. CLI `watch-add` + `watches` + `watch-remove` roundtrip via onehundo-desktop binary
    let desktop_bin = env!("CARGO_BIN_EXE_onehundo-desktop");
    let cli_add = Command::new(desktop_bin)
        .args([
            "watch-add",
            "nintendo.oot.switch2",
            watch_folder.to_str().unwrap(),
            "--url",
            &base,
        ])
        .output()
        .expect("run watch-add");
    assert!(cli_add.status.success(), "{}", String::from_utf8_lossy(&cli_add.stderr));

    let cli_list = Command::new(desktop_bin)
        .args(["watches", "--url", &base, "--json"])
        .output()
        .expect("run watches");
    assert!(cli_list.status.success());
    assert!(String::from_utf8_lossy(&cli_list.stdout).contains("nintendo.oot.switch2"));

    // 4. Launcher bridge (`POST /v1/launchers/report` & `GET /v1/launchers/status`)
    let launcher_save = dir.join("playnite_save_oot");
    fs::create_dir_all(&launcher_save).unwrap();
    fs::write(
        launcher_save.join("soh_save_slot1.json"),
        r#"{"questStatus":{"emerald":true},"inventory":{"gsTokens":42}}"#,
    )
    .unwrap();
    let report_body = serde_json::json!({
        "launcher": "playnite",
        "executable": "C:/Games/Longshot/longshot.exe",
        "save_dir": launcher_save.display().to_string(),
        "os": "windows",
        "active": true
    })
    .to_string();
    let rep_resp = http_request(
        "POST",
        &format!("{}/v1/launchers/report", base),
        None,
        Some(&report_body),
        None,
    )
    .unwrap();
    assert_eq!(rep_resp.status, 200);
    assert!(rep_resp.body.contains("nintendo.oot.forks"));

    // Poll watches -> imports Ship of Harkinian JSON rule (`a02` + stat `gold_skulltulas`)
    let poll_resp = http_request("POST", &format!("{}/watches/poll", base), None, Some("{}"), None).unwrap();
    assert_eq!(poll_resp.status, 200);

    let st_resp = http_request(
        "GET",
        &format!("{}/v1/launchers/status?canonical_game_id=nintendo.oot.switch2", base),
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(st_resp.status, 200);
    let st_json: serde_json::Value = serde_json::from_str(&st_resp.body).unwrap();
    assert_eq!(st_json["games"][0]["unlocked_count"], 1);
    assert_eq!(st_json["games"][0]["total_achievements"], 12);
    assert_eq!(st_json["games"][0]["completion_percent"], 8);

    // 5. RetroArch UDP bridge polls mock server and unlocks `ach_ra_boss_clear` cleanly!
    engine.poll_retroarch_once().unwrap();
    {
        let db = engine.db.lock().unwrap();
        let ra_trophies = list_trophies(&db, Some("retroarch.multicore")).unwrap();
        assert_eq!(ra_trophies.len(), 1);
        assert_eq!(ra_trophies[0].canonical_achievement_id, "ach_ra_boss_clear");
        assert_eq!(ra_trophies[0].trust_tier, "LOCAL");
        assert_eq!(ra_trophies[0].verification_state, "UNVERIFIED");
    }

    ra_stop.store(true, std::sync::atomic::Ordering::SeqCst);
    shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = ra_thread.join();
    let _ = http_handle.join();
    let _ = fs::remove_dir_all(&dir);
}
