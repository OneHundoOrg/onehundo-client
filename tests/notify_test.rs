mod common;

use onehundo_client::core::config::DaemonConfig;
use onehundo_client::core::engine::DaemonEngine;
use onehundo_client::diagnostics::server::start_health_http_server;
use onehundo_client::storage::state::{list_stats, list_trophies};
use onehundo_client::sync::http::http_request;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[test]
fn notify_test_http_and_cli_are_non_destructive() {
    let dir = common::temp_test_dir("notify_test");
    let packs_dir = dir.join("packs");
    fs::create_dir_all(&packs_dir).unwrap();

    let ls_src =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("packs/nintendo.oot.longshot.pc.onehundo");
    fs::copy(&ls_src, packs_dir.join("nintendo.oot.longshot.pc.onehundo")).unwrap();

    let mut cfg = DaemonConfig::default();
    cfg.config_dir = dir.join("cfg");
    cfg.data_dir = dir.join("data");
    cfg.packs_dirs = vec![packs_dir];
    cfg.notifications_enabled = false;

    let engine = DaemonEngine::initialize(cfg).expect("init engine");
    let shutdown = Arc::new(AtomicBool::new(false));
    let (addr, handle) =
        start_health_http_server(Arc::clone(&engine), "127.0.0.1:0", Arc::clone(&shutdown))
            .expect("bind test health server");
    let base = format!("http://{}", addr);

    // 1. Default POST /notify-test resolves installed OoT pack without mutating SQLite
    let resp = http_request("POST", &format!("{}/notify-test", base), None, Some("{}"), None)
        .expect("POST /notify-test");
    assert_eq!(resp.status, 200);
    let json: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(json["mode"], "daemon");
    assert_eq!(json["os_toasts_enabled"], false);
    assert_eq!(json["dispatched"], false);
    assert_eq!(
        json["event"]["app_name"],
        "The Legend of Zelda: Ocarina of Time"
    );
    assert_eq!(json["event"]["title"], "🏆 It works, it works!");
    assert_eq!(json["event"]["kind"], "test_notification");

    // 2. CLI `onehundo-desktop notify-test --url ...` with custom overrides
    let desktop_bin = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().and_then(|d| d.parent()).map(|d| d.join("onehundo-desktop")))
        .filter(|p| p.exists());
    if let Some(bin) = desktop_bin {
        let out = Command::new(&bin)
            .args([
                "notify-test",
                "--url",
                &base,
                "--game",
                "Custom Test Game",
                "--title",
                "Smoke Check",
                "--body",
                "Popup preview body",
                "--json",
            ])
            .output()
            .expect("run onehundo-desktop notify-test");
        assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
        let cli_json: serde_json::Value =
            serde_json::from_slice(&out.stdout).expect("valid notify-test JSON");
        assert_eq!(cli_json["event"]["app_name"], "Custom Test Game");
        assert_eq!(cli_json["event"]["title"], "🏆 Smoke Check");
        assert_eq!(cli_json["event"]["body"], "Popup preview body");
    }

    // 3. Verify zero SQLite mutation across trophies, stats, event_journal, and observation_journal
    let snap = engine.health_snapshot().unwrap();
    assert!(!snap.recent_notifications.is_empty());
    assert_eq!(snap.total_journal_events, 0);
    assert_eq!(snap.observations_imported, 0);
    {
        let db = engine.db.lock().unwrap();
        assert!(list_trophies(&db, None).unwrap().is_empty());
        assert!(list_stats(&db, None).unwrap().is_empty());
    }

    shutdown.store(true, Ordering::SeqCst);
    let _ = handle.join();
    let _ = fs::remove_dir_all(&dir);
}
