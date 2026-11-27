// Regression tests for the local attack surface of `onehundod`:
// - the Game Health / control HTTP API on 127.0.0.1 refuses cross-origin and DNS-rebinding requests,
// - the IPC Unix socket is owner-only (0600) regardless of umask,
// - credentials (API key, device token) are written 0600 from the start and atomically.

use onehundo_client::core::config::{
    load_credentials, save_credentials, write_secret_file, Credentials, DaemonConfig,
};
use onehundo_client::core::engine::DaemonEngine;
use onehundo_client::diagnostics::server::{local_request_allowed, start_health_http_server};
use onehundo_client::ipc::unix_socket::start_unix_ipc_server;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "onehundo-sec-{}-{}-{}",
        label,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn engine_in(dir: &PathBuf) -> Arc<DaemonEngine> {
    let mut cfg = DaemonConfig::default();
    cfg.config_dir = dir.join("cfg");
    cfg.data_dir = dir.join("data");
    cfg.packs_dirs = vec![];
    cfg.notifications_enabled = false;
    DaemonEngine::initialize(cfg).expect("init daemon")
}

fn raw_request(addr: SocketAddr, request: &str) -> String {
    let mut s = TcpStream::connect(addr).expect("connect");
    s.write_all(request.as_bytes()).unwrap();
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    resp
}

const BOUND: &str = "127.0.0.1:19850";

fn head(lines: &[&str]) -> String {
    let mut h = String::from("POST /shutdown HTTP/1.1\r\n");
    for l in lines {
        h.push_str(l);
        h.push_str("\r\n");
    }
    h
}

#[test]
fn local_request_guard_accepts_cli_and_same_origin_ui() {
    let bound: SocketAddr = BOUND.parse().unwrap();
    for ok in [
        head(&["Host: 127.0.0.1:19850"]),
        head(&["Host: localhost"]),
        head(&["host: LOCALHOST:19850"]),
        head(&["Host: [::1]:19850"]),
        head(&["Host: 127.0.0.1:19850", "Origin: http://127.0.0.1:19850"]),
        head(&["Host: localhost:19850", "Origin: http://localhost:19850", "Sec-Fetch-Site: same-origin"]),
    ] {
        assert_eq!(local_request_allowed(&ok, bound), Ok(()), "{ok:?}");
    }
}

#[test]
fn local_request_guard_refuses_cross_origin_and_rebinding() {
    let bound: SocketAddr = BOUND.parse().unwrap();
    for bad in [
        // DNS rebinding: an attacker hostname that now resolves to 127.0.0.1.
        head(&["Host: evil.example:19850"]),
        head(&["Host: 127.0.0.1.evil.example"]),
        // No Host at all.
        head(&[]),
        // Cross-origin browser POSTs (no-cors "simple" requests still carry Origin).
        head(&["Host: 127.0.0.1:19850", "Origin: https://evil.example"]),
        head(&["Host: 127.0.0.1:19850", "Origin: null"]),
        // Another local web server (e.g. a dev server on :3000) is another origin.
        head(&["Host: 127.0.0.1:19850", "Origin: http://localhost:3000"]),
        head(&["Host: 127.0.0.1:19850", "Origin: https://127.0.0.1:19850"]),
        head(&["Host: 127.0.0.1:19850", "Sec-Fetch-Site: cross-site"]),
    ] {
        assert!(local_request_allowed(&bad, bound).is_err(), "should refuse {bad:?}");
    }
    // Bound to a wildcard address, a LAN IP in Host is still not trusted.
    let wildcard: SocketAddr = "0.0.0.0:19850".parse().unwrap();
    assert!(local_request_allowed(&head(&["Host: 192.168.1.20:19850"]), wildcard).is_err());
    // Bound to a specific IP, that IP is accepted.
    let lan: SocketAddr = "192.168.1.20:19850".parse().unwrap();
    assert_eq!(local_request_allowed(&head(&["Host: 192.168.1.20:19850"]), lan), Ok(()));
}

#[test]
fn health_server_refuses_cross_origin_shutdown_and_rebinding_reads() {
    let dir = temp_dir("http");
    let engine = engine_in(&dir);
    let shutdown = Arc::new(AtomicBool::new(false));
    let (addr, handle) =
        start_health_http_server(Arc::clone(&engine), "127.0.0.1:0", Arc::clone(&shutdown))
            .expect("start health http");

    // A malicious page's blind POST: refused, and the daemon keeps running.
    let csrf = raw_request(
        addr,
        "POST /shutdown HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: https://evil.example\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
    );
    assert!(csrf.starts_with("HTTP/1.1 403"), "{csrf}");
    assert!(!shutdown.load(Ordering::SeqCst), "cross-origin POST must not stop the daemon");

    let import = raw_request(
        addr,
        "POST /v1/observation/import HTTP/1.1\r\nHost: localhost\r\nOrigin: https://evil.example\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
    );
    assert!(import.starts_with("HTTP/1.1 403"), "{import}");

    // A DNS-rebinding page reading local progression: refused by Host.
    let rebound = raw_request(
        addr,
        "GET /progression HTTP/1.1\r\nHost: attacker.example\r\nConnection: close\r\n\r\n",
    );
    assert!(rebound.starts_with("HTTP/1.1 403"), "{rebound}");

    // The CLI and the UI itself still work, with hardening headers.
    let ok = raw_request(addr, "GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    assert!(ok.contains("X-Frame-Options: DENY"));
    assert!(ok.contains("X-Content-Type-Options: nosniff"));
    let same_origin = format!(
        "POST /session/mode HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\nOrigin: http://127.0.0.1:{p}\r\nContent-Type: application/json\r\nContent-Length: 19\r\nConnection: close\r\n\r\n{{\"mode\":\"STANDARD\"}}",
        p = addr.port()
    );
    let mode = raw_request(addr, &same_origin);
    assert!(mode.starts_with("HTTP/1.1 200"), "{mode}");

    let stop = raw_request(addr, "POST /shutdown HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
    assert!(stop.starts_with("HTTP/1.1 200"), "{stop}");
    let _ = handle.join();
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ipc_socket_is_owner_only_even_with_a_permissive_umask() {
    let dir = temp_dir("sock");
    let engine = engine_in(&dir);
    let sock = dir.join("onehundo.sock");
    // A permissive umask would otherwise leave the socket group/world-connectable.
    let old = unsafe { libc_umask(0o000) };
    let shutdown = Arc::new(AtomicBool::new(false));
    let started = start_unix_ipc_server(Arc::clone(&engine), &sock, Arc::clone(&shutdown));
    unsafe { libc_umask(old) };
    let handle = started.expect("start ipc");
    let mode = fs::metadata(&sock).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "socket mode was {mode:o}");
    shutdown.store(true, Ordering::SeqCst);
    let _ = handle.join();
    let _ = fs::remove_dir_all(&dir);
}

extern "C" {
    #[link_name = "umask"]
    fn libc_umask(mask: u32) -> u32;
}

#[test]
fn credentials_are_written_0600_and_replace_a_world_readable_file() {
    let dir = temp_dir("creds");
    let path = dir.join("credentials.json");
    fs::write(&path, "{}").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

    let creds = Credentials {
        api_key: Some("oh_live_test_secret".into()),
        device_id: None,
        device_token: Some("ohd_test_token".into()),
    };
    save_credentials(&dir, &creds).expect("save");
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "credentials mode was {mode:o}");
    let loaded = load_credentials(&dir);
    assert_eq!(loaded.api_key.as_deref(), Some("oh_live_test_secret"));
    assert_eq!(loaded.device_token.as_deref(), Some("ohd_test_token"));
    let leftovers: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "no temp file left behind");

    let other = dir.join("token");
    write_secret_file(&other, b"secret").unwrap();
    assert_eq!(fs::metadata(&other).unwrap().permissions().mode() & 0o777, 0o600);
    let _ = fs::remove_dir_all(&dir);
}
