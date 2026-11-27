mod common;

use onehundo_client::core::config::DaemonConfig;
use onehundo_client::core::engine::DaemonEngine;
use onehundo_client::core::types::{IngestionPlane, ProgressionMode, TrustTier};
use onehundo_client::diagnostics::server::start_health_http_server;
use onehundo_client::evaluation::semantic_events::SemanticEventPayload;
use onehundo_client::packs::matcher::MatchQuery;
use onehundo_client::sync::http::http_request;
use onehundo_client::ui::desktop::backend::DesktopBackend;
use onehundo_client::ui::desktop::tray::{TrayModel, TrayStatus};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[test]
fn instant_game_active_notification_deduplicates_across_sessions_and_process_polls() {
    let dir = common::temp_test_dir("desktop_game_active");
    let packs_dir = dir.join("packs");
    fs::create_dir_all(&packs_dir).unwrap();

    let ls_src =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("packs/nintendo.oot.longshot.pc.onehundo");
    fs::copy(&ls_src, packs_dir.join("nintendo.oot.longshot.pc.onehundo")).unwrap();

    let mut cfg = DaemonConfig::default();
    cfg.config_dir = dir.join("cfg");
    cfg.data_dir = dir.join("data");
    cfg.socket_path = dir.join("onehundo.sock");
    cfg.packs_dirs = vec![packs_dir];
    cfg.notifications_enabled = false;

    let engine = DaemonEngine::initialize(cfg).expect("init engine");

    // 1. Launcher bridge start -> OneHundo Active dispatched once
    engine
        .report_launcher_game(
            "recomp_launcher",
            Some("nintendo.oot.longshot.pc".to_string()),
            None,
            Some("longshot".to_string()),
            None,
            None,
            None,
            true,
        )
        .expect("report active launcher game");

    let snap = engine.health_snapshot().unwrap();
    let active_notifs: Vec<_> = snap
        .recent_notifications
        .iter()
        .filter(|n| n.kind == "game_active")
        .collect();
    assert_eq!(active_notifs.len(), 1);
    assert_eq!(active_notifs[0].title, "OneHundo Active");
    assert!(
        active_notifs[0]
            .body
            .starts_with("Tracking The Legend of Zelda: Ocarina of Time"),
        "unexpected body: {}",
        active_notifs[0].body
    );

    // 2. Repeated launcher heartbeat & IPC session for same game while active -> still 1
    engine
        .report_launcher_game(
            "recomp_launcher",
            Some("nintendo.oot.longshot.pc".to_string()),
            None,
            Some("longshot".to_string()),
            None,
            None,
            None,
            true,
        )
        .expect("repeat launcher heartbeat");
    let (sess1, matched) = engine.start_session(
        "sess-oot-1".to_string(),
        "longshot_ns2".to_string(),
        "nintendo.oot.longshot.pc".to_string(),
        MatchQuery {
            recomp_game_id: Some("longshot".to_string()),
            ..Default::default()
        },
        IngestionPlane::SemanticEvent,
        ProgressionMode::Standard,
        TrustTier::VerifiedIntegration,
    );
    assert!(matched.is_some());

    let snap2 = engine.health_snapshot().unwrap();
    assert_eq!(
        snap2
            .recent_notifications
            .iter()
            .filter(|n| n.kind == "game_active")
            .count(),
        1
    );
    assert!(
        snap2
            .library_games
            .iter()
            .any(|g| g.canonical_game_id == "nintendo.oot.longshot.pc" && g.active)
    );

    // 3. Ending both launcher active state & session re-arms notification
    engine
        .report_launcher_game(
            "recomp_launcher",
            Some("nintendo.oot.longshot.pc".to_string()),
            None,
            Some("longshot".to_string()),
            None,
            None,
            None,
            false,
        )
        .expect("report launcher inactive");
    engine.end_session(&sess1.session_id);

    let _ = engine.start_session(
        "sess-oot-2".to_string(),
        "longshot_ns2".to_string(),
        "nintendo.oot.longshot.pc".to_string(),
        MatchQuery {
            recomp_game_id: Some("longshot".to_string()),
            ..Default::default()
        },
        IngestionPlane::SemanticEvent,
        ProgressionMode::Standard,
        TrustTier::VerifiedIntegration,
    );

    let snap3 = engine.health_snapshot().unwrap();
    assert_eq!(
        snap3
            .recent_notifications
            .iter()
            .filter(|n| n.kind == "game_active")
            .count(),
        2
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn desktop_backend_tray_autostart_auth_and_loopback_http() {
    let dir = common::temp_test_dir("desktop_backend");
    let packs_dir = dir.join("packs");
    fs::create_dir_all(&packs_dir).unwrap();
    let bb_src =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("packs/sony.bloodborne.bbport.onehundo");
    fs::copy(&bb_src, packs_dir.join("sony.bloodborne.bbport.onehundo")).unwrap();

    let mut cfg = DaemonConfig::default();
    cfg.config_dir = dir.join("cfg");
    cfg.data_dir = dir.join("data");
    cfg.socket_path = dir.join("onehundo.sock");
    cfg.packs_dirs = vec![packs_dir];
    cfg.notifications_enabled = false;
    cfg.platform_url = Some("https://onehundo.org".to_string());

    let engine = DaemonEngine::initialize(cfg).expect("init engine");
    let autostart_file = dir.join("autostart/onehundo.desktop");
    let backend = DesktopBackend::from_test_engine(Arc::clone(&engine), autostart_file.clone())
        .expect("create test backend");

    // 1. Initial tray state: SignInNeeded -> signed in -> Paused -> Quit
    let snap0 = backend.fetch_snapshot().unwrap();
    let mut tray = TrayModel::new(
        snap0.tracking_paused,
        snap0.account_signed_in,
        backend.is_launch_at_login(),
        true,
    );
    assert_eq!(tray.status, TrayStatus::SignInNeeded);
    assert!(!tray.handle_window_close_requested()); // minimizes to tray
    assert!(!tray.window_visible);
    tray.handle_open_requested();
    assert!(tray.window_visible);

    // 2. Autostart toggle creates & removes valid desktop file
    assert!(!backend.is_launch_at_login());
    assert!(backend.set_launch_at_login(true).unwrap());
    assert!(backend.is_launch_at_login());
    let desktop_contents = fs::read_to_string(&autostart_file).unwrap();
    assert!(desktop_contents.contains("--minimized"));
    assert!(!backend.set_launch_at_login(false).unwrap());
    assert!(!autostart_file.exists());

    // 3. Account save stores 0600 credentials and never leaks key in snapshots
    let secret = "oh_live_super_secret_desktop_key_12345";
    backend
        .save_account(secret.to_string(), Some("playerone".to_string()))
        .expect("save account");
    let mode = fs::metadata(dir.join("cfg/credentials.json"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);

    let snap1 = backend.fetch_snapshot().unwrap();
    assert!(snap1.account_signed_in);
    assert_eq!(snap1.account_username.as_deref(), Some("playerone"));
    let snap1_json = serde_json::to_string(&snap1).unwrap();
    assert!(!snap1_json.contains(secret));

    tray.sync_flags(
        snap1.tracking_paused,
        snap1.account_signed_in,
        false,
        None,
    );
    assert_eq!(tray.status, TrayStatus::TrackingActive);

    // 4. Pause & resume gating over loopback HTTP + CSRF refusal
    let shutdown = Arc::new(AtomicBool::new(false));
    let (addr, handle) =
        start_health_http_server(Arc::clone(&engine), "127.0.0.1:0", Arc::clone(&shutdown))
            .expect("bind loopback server");
    let base = format!("http://{}", addr);

    // Cross-origin pause must be rejected (403)
    {
        let mut s = TcpStream::connect(addr).unwrap();
        let body = r#"{"paused":true}"#;
        let req = format!(
            "POST /session/pause HTTP/1.1\r\nHost: {}\r\nOrigin: https://evil.example\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            addr,
            body.len(),
            body
        );
        s.write_all(req.as_bytes()).unwrap();
        let mut raw_resp = String::new();
        let _ = s.read_to_string(&mut raw_resp);
        assert!(raw_resp.starts_with("HTTP/1.1 403"), "got: {}", raw_resp);
    }

    // Authorized loopback pause works
    let pause_resp = http_request(
        "POST",
        &format!("{}/session/pause", base),
        None,
        Some(r#"{"paused":true}"#),
        None,
    )
    .unwrap();
    assert_eq!(pause_resp.status, 200);
    assert!(backend.fetch_snapshot().unwrap().tracking_paused);

    // While paused, unlock attempts do not record trophies
    let (sess_paused, _) = engine.start_session(
        "sess-paused".to_string(),
        "bbport".to_string(),
        "sony.bloodborne".to_string(),
        MatchQuery {
            recomp_game_id: Some("bbport".to_string()),
            ..Default::default()
        },
        IngestionPlane::SemanticEvent,
        ProgressionMode::Standard,
        TrustTier::Local,
    );
    let ev1 = SemanticEventPayload {
        event_type: "UNLOCK_REQUEST".to_string(),
        canonical_game_id: "sony.bloodborne".to_string(),
        producer_id: "bbport".to_string(),
        session_id: sess_paused.session_id,
        sequence_number: 1,
        idempotency_key: None,
        timestamp_nanos: None,
        key: "a02".to_string(),
        int_value: None,
        float_value: None,
        string_value: None,
        progression_mode: Some("STANDARD".to_string()),
        trust_tier: Some("LOCAL".to_string()),
        evidence_class: None,
        verification_state: None,
    };
    let blocked = engine.handle_plane_b_event(None, &ev1, "{}").unwrap();
    assert!(blocked.newly_unlocked.is_empty());
    assert_eq!(engine.health_snapshot().unwrap().total_journal_events, 0);

    // Resume via backend and verify unlock succeeds
    assert!(!backend.set_tracking_paused(false).unwrap());
    let mut ev2 = ev1;
    ev2.sequence_number = 2;
    let resumed = engine.handle_plane_b_event(None, &ev2, "{}").unwrap();
    assert_eq!(resumed.newly_unlocked.len(), 1);

    let snap2 = backend.fetch_snapshot().unwrap();
    assert!(snap2.recent_trophies.iter().any(|t| t.achievement_title == "Yharnam Sunrise"));

    let auth_status = http_request("GET", &format!("{}/auth/status", base), None, None, None)
        .unwrap();
    assert_eq!(auth_status.status, 200);
    assert!(auth_status.body.contains("\"signed_in\":true"));
    assert!(!auth_status.body.contains(secret));

    backend.sign_out().unwrap();
    assert!(!backend.fetch_snapshot().unwrap().account_signed_in);

    tray.handle_quit_requested();
    assert!(tray.handle_window_close_requested());

    shutdown.store(true, Ordering::SeqCst);
    let _ = handle.join();
    backend.shutdown();
    let _ = fs::remove_dir_all(&dir);
}
