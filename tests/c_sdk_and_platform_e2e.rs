mod common;

use onehundo_client::core::config::{redact_secrets, Credentials, DaemonConfig};
use onehundo_client::core::engine::DaemonEngine;
use onehundo_client::diagnostics::server::start_health_http_server;
use onehundo_client::ipc::unix_socket::start_unix_ipc_server;
use onehundo_client::storage::journal::{
    fetch_pending_sync_events, insert_journal_event_tx, query_journal_stats, JournalEventInput,
};
use onehundo_client::storage::state::list_trophies;
use onehundo_client::sync::client::{register_device, resolve_pack_remote, sync_pending_batch};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn c_sdk_bbport_live_demo_and_onehundo_quick_fail_open() {
    let dir = common::temp_test_dir("c_sdk_e2e");
    let sock_path = dir.join("onehundo_bb.sock");
    let bbport_bin = dir.join("bbport_live_demo");
    let quick_bin = dir.join("quick_fail_open");

    let client_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let proto_root = common::protocol_root();

    // 1. Compile bbport_live_demo.c with pinned onehundo-protocol/reference/c/transport_posix.c
    let compile_bb = Command::new("gcc")
        .args([
            "-std=c99",
            "-pedantic",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-I",
            proto_root.join("include").to_str().unwrap(),
            proto_root.join("reference/c/transport_posix.c").to_str().unwrap(),
            client_root
                .join("integrations/native-ports/bbport/bbport_live_demo.c")
                .to_str()
                .unwrap(),
            "-lrt",
            "-o",
            bbport_bin.to_str().unwrap(),
        ])
        .output()
        .expect("compile bbport_live_demo");
    assert!(
        compile_bb.status.success(),
        "gcc bbport_live_demo failed: {}",
        String::from_utf8_lossy(&compile_bb.stderr)
    );

    // 2. Compile & run onehundo_quick.h test WITHOUT daemon running -> must fail-open & not block
    let quick_src = dir.join("quick_test.c");
    fs::write(
        &quick_src,
        format!(
            r#"
#include "{}/integrations/native-ports/quick/onehundo_quick.h"
#include <stdio.h>
int main(void) {{
    onehundo_context_t *oh = onehundo_quick_init("test.vibe_game", "quick_test", "/tmp/nonexistent_oh_999.sock");
    onehundo_quick_stat_inc(oh, "coins", 5);
    onehundo_quick_unlock(oh, "ach_first_coin");
    onehundo_quick_shutdown(oh);
    printf("quick_ok\n");
    return 0;
}}
"#,
            client_root.display()
        ),
    )
    .unwrap();
    let compile_quick = Command::new("gcc")
        .args([
            "-std=c99",
            "-pedantic",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-I",
            proto_root.join("include").to_str().unwrap(),
            quick_src.to_str().unwrap(),
            proto_root.join("reference/c/transport_posix.c").to_str().unwrap(),
            "-lrt",
            "-o",
            quick_bin.to_str().unwrap(),
        ])
        .output()
        .expect("compile onehundo_quick test");
    assert!(
        compile_quick.status.success(),
        "gcc onehundo_quick failed: {}",
        String::from_utf8_lossy(&compile_quick.stderr)
    );
    let quick_out = Command::new(&quick_bin).output().expect("run quick");
    assert!(quick_out.status.success());
    assert!(String::from_utf8_lossy(&quick_out.stdout).contains("quick_ok"));

    // 3. Boot daemon with bbport_sample.onehundo + IPC + Diagnostics UI HTTP server
    let mut cfg = DaemonConfig::default();
    cfg.config_dir = dir.join("cfg");
    cfg.data_dir = dir.join("data");
    cfg.packs_dirs = vec![];
    cfg.notifications_enabled = false;

    let engine = DaemonEngine::initialize(cfg).expect("init engine");
    let bb_pack = proto_root.join("conformance/fixtures/packs/bbport_sample.onehundo");
    engine.load_pack_path(&bb_pack).expect("load bbport pack");

    let shutdown = Arc::new(AtomicBool::new(false));
    let ipc_h =
        start_unix_ipc_server(Arc::clone(&engine), &sock_path, Arc::clone(&shutdown)).unwrap();
    let (http_addr, http_h) =
        start_health_http_server(Arc::clone(&engine), "127.0.0.1:0", Arc::clone(&shutdown))
            .unwrap();

    let bb_out = Command::new(&bbport_bin)
        .arg(sock_path.to_str().unwrap())
        .output()
        .expect("run bbport_live_demo");
    assert!(bb_out.status.success());
    let stdout = String::from_utf8_lossy(&bb_out.stdout);
    assert!(
        stdout.contains("connected=1 emitted=4 acked=4"),
        "expected acked=4 from bbport_live_demo, got: {}",
        stdout
    );

    // Check materialized trophy & pending sync outbox
    {
        let db = engine.db.lock().unwrap();
        let trophies = list_trophies(&db, Some("sony.bloodborne")).unwrap();
        assert_eq!(trophies.len(), 1);
        assert_eq!(trophies[0].canonical_achievement_id, "ach_cleric_beast");
        let stats = query_journal_stats(&db).unwrap();
        assert_eq!(stats.total_events, 4);
        assert_eq!(stats.pending_sync_count, 4);
    }

    // Check Game Health HTML & Pack Browser HTML WCAG UI endpoints
    for (route, expected_text) in [
        ("/", "Game Health Diagnostics"),
        ("/ui/packs", "OneHundo Pack Browser"),
        ("/health", "loaded_packs_count"),
        ("/progression", "ach_cleric_beast"),
    ] {
        let mut s = TcpStream::connect(http_addr).unwrap();
        let req = format!(
            "GET {} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            route
        );
        s.write_all(req.as_bytes()).unwrap();
        let mut resp = String::new();
        s.read_to_string(&mut resp).unwrap();
        assert!(resp.contains("200 OK"), "route {} failed: {}", route, resp);
        assert!(
            resp.contains(expected_text),
            "route {} missing '{}'",
            route,
            expected_text
        );
    }

    shutdown.store(true, Ordering::Relaxed);
    let _ = ipc_h.join();
    let _ = http_h.join();
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cloud_sync_openapi_contract_receipt_vs_acceptance_and_secret_redaction() {
    let dir = common::temp_test_dir("cloud_sync");
    let mut cfg = DaemonConfig::default();
    cfg.config_dir = dir.join("cfg");
    cfg.data_dir = dir.join("data");
    cfg.packs_dirs = vec![];
    cfg.notifications_enabled = false;
    let engine = DaemonEngine::initialize(cfg).expect("init");

    // Seed 2 pending journal events (with None optional fields to verify null-free JSON!)
    {
        let mut db = engine.db.lock().unwrap();
        let tx = db.transaction().unwrap();
        insert_journal_event_tx(
            &tx,
            &JournalEventInput {
                producer_id: "bbport_pc_x64".into(),
                session_id: "550e8400-e29b-41d4-a716-446655440000".into(),
                sequence_number: 1,
                idempotency_key: "bbport_pc_x64:550e8400-e29b-41d4-a716-446655440000:1".into(),
                canonical_game_id: "sony.bloodborne".into(),
                event_kind: onehundo_client::core::types::EventKind::UnlockRequest,
                key: "ach_cleric_beast".into(),
                int_value: None,
                float_value: None,
                string_value: None,
                occurred_at: "2026-11-24T12:00:00Z".into(),
                progression_mode: onehundo_client::core::types::ProgressionMode::Standard,
                trust_tier: onehundo_client::core::types::TrustTier::VerifiedIntegration,
                evidence_class: onehundo_client::core::types::EvidenceClass::DirectIntegrationEvent,
                verification_state: onehundo_client::core::types::VerificationState::Verified,
                raw_payload: "{}".into(),
            },
        )
        .unwrap();
        insert_journal_event_tx(
            &tx,
            &JournalEventInput {
                producer_id: "bbport_pc_x64".into(),
                session_id: "550e8400-e29b-41d4-a716-446655440000".into(),
                sequence_number: 2,
                idempotency_key: "bbport_pc_x64:550e8400-e29b-41d4-a716-446655440000:2".into(),
                canonical_game_id: "sony.bloodborne".into(),
                event_kind: onehundo_client::core::types::EventKind::StatInc,
                key: "bosses_defeated".into(),
                int_value: Some(1),
                float_value: None,
                string_value: None,
                occurred_at: "2026-11-24T12:00:01Z".into(),
                progression_mode: onehundo_client::core::types::ProgressionMode::Standard,
                trust_tier: onehundo_client::core::types::TrustTier::Local,
                evidence_class: onehundo_client::core::types::EvidenceClass::DirectIntegrationEvent,
                verification_state: onehundo_client::core::types::VerificationState::Unverified,
                raw_payload: "{}".into(),
            },
        )
        .unwrap();
        tx.commit().unwrap();
    }

    // Start mock OpenAPI `/v1/*` server enforcing strict schema rules
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mock_addr = listener.local_addr().unwrap();
    let captured_sync_body = Arc::new(Mutex::new(String::new()));
    let captured_ref = Arc::clone(&captured_sync_body);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_srv = Arc::clone(&stop);

    listener.set_nonblocking(true).unwrap();
    let srv = thread::spawn(move || {
        while !stop_srv.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut buf = vec![0u8; 32 * 1024];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        continue;
                    }
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let (head, mut body) = req
                        .split_once("\r\n\r\n")
                        .map(|(h, b)| (h.to_string(), b.to_string()))
                        .unwrap_or((req.clone(), String::new()));
                    let clen: usize = head
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.trim().eq_ignore_ascii_case("content-length").then(|| v.trim().parse().ok()).flatten()
                        })
                        .unwrap_or(0);
                    while body.len() < clen {
                        let mut extra = [0u8; 4096];
                        match stream.read(&mut extra) {
                            Ok(0) | Err(_) => break,
                            Ok(m) => body.push_str(&String::from_utf8_lossy(&extra[..m])),
                        }
                    }

                    let path = head.lines().next().unwrap_or("").split_whitespace().nth(1).unwrap_or("");
                    let (status, resp_json) = if path == "/v1/devices/register" {
                        assert!(head.contains("Authorization: Bearer oh_live_secret_key_123"));
                        (
                            "201 Created",
                            serde_json::json!({
                                "device_id": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
                                "device_token": "dev_tok_super_secret_xyz",
                                "registered_at": "2026-11-24T12:00:00Z"
                            })
                            .to_string(),
                        )
                    } else if path.starts_with("/v1/packs/resolve") {
                        assert!(head.contains("Authorization: Bearer dev_tok_super_secret_xyz"));
                        (
                            "200 OK",
                            serde_json::json!({
                                "canonical_game_id": "sony.bloodborne",
                                "pack_id": "pack.sony.bloodborne.bbport",
                                "pack_version": "0.1.0",
                                "download_url": "https://cdn.example/pack.onehundo",
                                "pack_sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                            })
                            .to_string(),
                        )
                    } else if path == "/v1/sync/events" {
                        assert!(head.contains("Authorization: Bearer dev_tok_super_secret_xyz"));
                        *captured_ref.lock().unwrap() = body.clone();
                        // Verify OpenAPI SyncEventsRequest & no `null` optional fields
                        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
                        let evs = v["events"].as_array().unwrap();
                        assert_eq!(evs.len(), 2);
                        assert!(evs[0].get("int_value").is_none());
                        assert!(evs[0].get("float_value").is_none());
                        assert!(evs[0].get("string_value").is_none());
                        (
                            "200 OK",
                            serde_json::json!({
                                "client_batch_id": v["client_batch_id"],
                                "batch_receipt_status": "RECEIVED",
                                "results": [
                                    {
                                        "idempotency_key": evs[0]["idempotency_key"],
                                        "receipt_status": "RECEIVED",
                                        "acceptance_status": "PENDING_REVIEW",
                                        "verification_state": "PENDING"
                                    },
                                    {
                                        "idempotency_key": evs[1]["idempotency_key"],
                                        "receipt_status": "DUPLICATE",
                                        "acceptance_status": "REJECTED",
                                        "verification_state": "REJECTED",
                                        "rejection_reason": "test_rejected_rule"
                                    }
                                ]
                            })
                            .to_string(),
                        )
                    } else {
                        ("404 Not Found", r#"{"error":"not_found"}"#.to_string())
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

    let base_url = format!("http://{}", mock_addr);
    let mut creds = Credentials {
        api_key: Some("oh_live_secret_key_123".into()),
        device_id: None,
        device_token: None,
    };

    // Debug impl and redact_secrets must never expose secrets
    let dbg = format!("{:?}", creds);
    assert!(!dbg.contains("oh_live_secret_key_123"));
    let scrubbed = redact_secrets(
        "Bearer oh_live_secret_key_123 / dev_tok_super_secret_xyz",
        Some(&creds),
    );
    assert!(!scrubbed.contains("oh_live_secret_key_123"));

    // 1. Register device
    let reg = register_device(
        &base_url,
        "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
        "test-linux-rig",
        &dir.join("cfg"),
        &mut creds,
    )
    .expect("register device");
    assert_eq!(reg.device_token, "dev_tok_super_secret_xyz");

    // 2. Resolve pack remotely
    let resolved = resolve_pack_remote(&base_url, &creds, "recomp_game_id", "bbport", None)
        .expect("resolve pack")
        .expect("pack found");
    assert_eq!(resolved.canonical_game_id, "sony.bloodborne");

    // 3. Before HTTP 500 or offline error, outbox stays PENDING
    let bad_sync = {
        let mut db = engine.db.lock().unwrap();
        sync_pending_batch(&mut db, "http://127.0.0.1:1", &creds, 50)
    };
    assert!(bad_sync.is_err());
    {
        let db = engine.db.lock().unwrap();
        assert_eq!(fetch_pending_sync_events(&db, 10).unwrap().len(), 2);
    }

    // 4. Sync succeeds against mock /v1/sync/events: RECEIVED/PENDING_REVIEW + DUPLICATE/REJECTED
    let summary = {
        let mut db = engine.db.lock().unwrap();
        sync_pending_batch(&mut db, &base_url, &creds, 50).expect("sync batch")
    };
    assert_eq!(summary.synced_count, 2);
    assert_eq!(summary.batch_receipt_status.as_deref(), Some("RECEIVED"));

    {
        let db = engine.db.lock().unwrap();
        assert!(fetch_pending_sync_events(&db, 10).unwrap().is_empty());
        let stats = query_journal_stats(&db).unwrap();
        assert_eq!(stats.synced_count, 2);
        assert_eq!(stats.rejected_count, 1);
    }

    stop.store(true, Ordering::Relaxed);
    let _ = srv.join();
    let _ = Instant::now();
    let _ = fs::remove_dir_all(&dir);
}
