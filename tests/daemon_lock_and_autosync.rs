mod common;

use onehundo_client::ipc::wire::{MSG_EVENT_ACK, MSG_EVENT_PUBLISH, MSG_GOODBYE, MSG_HELLO, MSG_HELLO_ACK};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn single_instance_lockfile_prevents_collision_and_background_autosync_flushes() {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_onehundod"));
    let dir = common::temp_test_dir("lock_autosync");
    let xdg_dir = dir.join("xdg");
    fs::create_dir_all(&xdg_dir).unwrap();
    let cfg_dir = dir.join("cfg");
    let data_dir = dir.join("data");
    fs::create_dir_all(&cfg_dir).unwrap();
    fs::create_dir_all(&data_dir).unwrap();
    fs::write(
        cfg_dir.join("credentials.json"),
        r#"{"api_key":"oh_live_test_autosync_key","device_id":null,"device_token":null}"#,
    )
    .unwrap();

    // Pick a free localhost port for Game Health and another for the mock platform
    let probe_health = TcpListener::bind("127.0.0.1:0").unwrap();
    let health_addr = probe_health.local_addr().unwrap();
    drop(probe_health);

    let mock_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mock_addr = mock_listener.local_addr().unwrap();
    mock_listener.set_nonblocking(true).unwrap();

    let stop_mock = Arc::new(AtomicBool::new(false));
    let stop_mock_srv = Arc::clone(&stop_mock);
    let sync_calls = Arc::new(AtomicUsize::new(0));
    let sync_calls_srv = Arc::clone(&sync_calls);

    let mock_srv = thread::spawn(move || {
        while !stop_mock_srv.load(Ordering::Relaxed) {
            match mock_listener.accept() {
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
                                "device_token": "dev_tok_autosync_test",
                                "registered_at": "2026-11-24T12:00:00Z"
                            })
                            .to_string(),
                        )
                    } else if path == "/v1/sync/events" {
                        sync_calls_srv.fetch_add(1, Ordering::Relaxed);
                        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
                        let results: Vec<serde_json::Value> = v["events"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|ev| {
                                serde_json::json!({
                                    "idempotency_key": ev["idempotency_key"],
                                    "receipt_status": "RECEIVED",
                                    "acceptance_status": "ACCEPTED",
                                    "verification_state": "VERIFIED"
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

    let pack = common::protocol_root().join("conformance/fixtures/packs/bbport_sample.onehundo");
    let platform_url = format!("http://{}", mock_addr);

    // 1. Start primary onehundod using default $XDG_RUNTIME_DIR socket + lockfile
    let mut child1 = Command::new(&bin)
        .env("XDG_RUNTIME_DIR", &xdg_dir)
        .env_remove("ONEHUNDO_SOCKET")
        .env_remove("ONEHUNDO_LOCK_FILE")
        .args([
            "--config-dir",
            cfg_dir.to_str().unwrap(),
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--pack",
            pack.to_str().unwrap(),
            "--health-addr",
            &health_addr.to_string(),
            "--platform-url",
            &platform_url,
            "--sync-interval-ms",
            "300",
            "--no-notify",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn primary onehundod");

    let expected_sock = xdg_dir.join("onehundo.sock");
    let expected_lock = xdg_dir.join("onehundod.lock");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if expected_sock.exists() && TcpStream::connect(health_addr).is_ok() {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    assert!(expected_sock.exists(), "socket did not appear at {}", expected_sock.display());
    assert!(expected_lock.exists(), "lockfile did not appear at {}", expected_lock.display());

    // 2. Attempt to launch a second instance with same XDG_RUNTIME_DIR -> must exit cleanly
    let dup = Command::new(&bin)
        .env("XDG_RUNTIME_DIR", &xdg_dir)
        .env_remove("ONEHUNDO_SOCKET")
        .env_remove("ONEHUNDO_LOCK_FILE")
        .args([
            "--config-dir",
            cfg_dir.to_str().unwrap(),
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--health-addr",
            &health_addr.to_string(),
            "--no-notify",
        ])
        .output()
        .expect("run duplicate onehundod");
    assert!(dup.status.success(), "duplicate onehundod should exit cleanly");
    let dup_err = String::from_utf8_lossy(&dup.stderr);
    assert!(
        dup_err.contains("already running"),
        "expected clear already-running message, got: {}",
        dup_err
    );

    // 3. Emit an event over Unix socket and verify background auto-sync flushes it WITHOUT sync-now
    {
        let mut stream = UnixStream::connect(&expected_sock).expect("connect to primary daemon");
        let session_id = "550e8400-e29b-41d4-a716-446655440077";
        let (h_ack, _) = common::send_and_recv(
            &mut stream,
            MSG_HELLO,
            0,
            &format!(
                r#"{{"canonical_game_id":"sony.bloodborne","producer_id":"bbport_pc_x64","session_id":"{}","plane":2,"progression_mode":"STANDARD","trust_tier":"VERIFIED_INTEGRATION"}}"#,
                session_id
            ),
        );
        assert_eq!(h_ack.msg_type, MSG_HELLO_ACK);

        let (ack1, _) = common::send_and_recv(
            &mut stream,
            MSG_EVENT_PUBLISH,
            1,
            &format!(
                r#"{{"event_type":"UNLOCK_REQUEST","canonical_game_id":"sony.bloodborne","producer_id":"bbport_pc_x64","session_id":"{}","sequence_number":1,"key":"ach_cleric_beast","int_value":1}}"#,
                session_id
            ),
        );
        assert_eq!(ack1.msg_type, MSG_EVENT_ACK);
        let _ = onehundo_client::ipc::wire::write_wire_frame(&mut stream, MSG_GOODBYE, 0, &[]);
    }

    let sync_deadline = Instant::now() + Duration::from_secs(5);
    let mut flushed = false;
    while Instant::now() < sync_deadline {
        if sync_calls.load(Ordering::Relaxed) >= 1 {
            let mut s = TcpStream::connect(health_addr).unwrap();
            s.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .unwrap();
            let mut resp = String::new();
            s.read_to_string(&mut resp).unwrap();
            if resp.contains("\"sync_queue_depth\": 0") && resp.contains("\"synced_events_count\": 1") {
                flushed = true;
                break;
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(flushed, "background auto-sync did not flush event automatically");

    // 4. Verify `onehundo-desktop status` and `onehundo-desktop stop` cleanly shut down the daemon
    let desktop_bin = PathBuf::from(env!("CARGO_BIN_EXE_onehundo-desktop"));
    let base_url = format!("http://{}", health_addr);
    let st = Command::new(&desktop_bin)
        .env("XDG_RUNTIME_DIR", &xdg_dir)
        .env_remove("ONEHUNDO_SOCKET")
        .env_remove("ONEHUNDO_LOCK_FILE")
        .args(["status", &base_url])
        .output()
        .expect("run onehundo-desktop status");
    assert!(st.status.success(), "status should exit 0 while running");
    let st_out = String::from_utf8_lossy(&st.stdout);
    assert!(st_out.contains("onehundod: running"), "got: {}", st_out);
    assert!(st_out.contains("platform_configured=true"), "got: {}", st_out);
    assert!(st_out.contains("device_registered=true"), "got: {}", st_out);

    let stop_cmd = Command::new(&desktop_bin)
        .env("XDG_RUNTIME_DIR", &xdg_dir)
        .env_remove("ONEHUNDO_SOCKET")
        .env_remove("ONEHUNDO_LOCK_FILE")
        .args(["stop", &base_url])
        .output()
        .expect("run onehundo-desktop stop");
    assert!(
        stop_cmd.status.success(),
        "stop failed: {}",
        String::from_utf8_lossy(&stop_cmd.stderr)
    );

    let exit_status = child1.wait().expect("wait primary onehundod");
    assert!(exit_status.success(), "onehundod should exit 0 on graceful stop");
    assert!(!expected_lock.exists(), "lockfile should be cleaned up on graceful exit");
    assert!(!expected_sock.exists(), "socket should be cleaned up on graceful exit");

    let st_after = Command::new(&desktop_bin)
        .env("XDG_RUNTIME_DIR", &xdg_dir)
        .env_remove("ONEHUNDO_SOCKET")
        .env_remove("ONEHUNDO_LOCK_FILE")
        .args(["status", &base_url])
        .output()
        .expect("run onehundo-desktop status after stop");
    assert_eq!(st_after.status.code(), Some(3));

    stop_mock.store(true, Ordering::Relaxed);
    let _ = mock_srv.join();
    let _ = fs::remove_dir_all(&dir);
}
