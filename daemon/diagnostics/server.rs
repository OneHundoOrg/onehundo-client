use crate::core::engine::DaemonEngine;
use crate::core::types::{FairPlayViolation, ProgressionMode};
use crate::ui::game_health::render_game_health_html;
use crate::ui::pack_browser::render_pack_browser_html;
use serde::Deserialize;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[derive(Deserialize)]
struct FairPlayActionReq {
    session_id: String,
    violation: FairPlayViolation,
}

#[derive(Deserialize)]
struct SessionModeReq {
    mode: String,
    #[serde(default)]
    session_id: Option<String>,
}

#[derive(Deserialize)]
struct ObservationPathImportReq {
    path: String,
    #[serde(default)]
    canonical_game_id: Option<String>,
    #[serde(default)]
    progression_mode: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

pub fn start_health_http_server(
    engine: Arc<DaemonEngine>,
    bind_addr: &str,
    shutdown: Arc<AtomicBool>,
) -> Result<(SocketAddr, thread::JoinHandle<()>), String> {
    let listener = TcpListener::bind(bind_addr)
        .map_err(|e| format!("failed to bind Game Health server '{}': {}", bind_addr, e))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("failed setting nonblocking on health listener: {}", e))?;
    let local_addr = listener
        .local_addr()
        .map_err(|e| format!("failed reading health local_addr: {}", e))?;

    let handle = thread::spawn(move || {
        while !shutdown.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let eng = Arc::clone(&engine);
                    let sd = Arc::clone(&shutdown);
                    thread::spawn(move || handle_http_conn(stream, eng, sd));
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(_) => break,
            }
        }
    });

    Ok((local_addr, handle))
}

fn handle_observation_import_http(
    engine: &DaemonEngine,
    body: &str,
) -> (&'static str, &'static str, String) {
    if let Ok(req) = serde_json::from_str::<ObservationPathImportReq>(body) {
        let mode_override = match req.progression_mode.as_deref() {
            Some(m) => match ProgressionMode::from_str_strict(m) {
                Some(parsed) => Some(parsed),
                None => {
                    return (
                        "400 Bad Request",
                        "application/json; charset=utf-8",
                        serde_json::json!({
                            "error": format!("invalid progression_mode '{}' (expected STANDARD, ENCORE, or SPECTATOR)", m)
                        })
                        .to_string(),
                    );
                }
            },
            None => None,
        };
        return match engine.import_plane_c_path(
            Path::new(&req.path),
            req.canonical_game_id,
            mode_override,
            req.dry_run,
        ) {
            Ok(summary) => (
                "200 OK",
                "application/json; charset=utf-8",
                serde_json::to_string(&summary).unwrap_or_default(),
            ),
            Err(e) => (
                "400 Bad Request",
                "application/json; charset=utf-8",
                serde_json::json!({ "error": e }).to_string(),
            ),
        };
    }

    match engine.import_plane_c_json(body) {
        Ok(summary) => (
            "200 OK",
            "application/json; charset=utf-8",
            serde_json::to_string(&summary).unwrap_or_default(),
        ),
        Err(e) => (
            "400 Bad Request",
            "application/json; charset=utf-8",
            serde_json::json!({ "error": e }).to_string(),
        ),
    }
}

fn handle_http_conn(
    mut stream: TcpStream,
    engine: Arc<DaemonEngine>,
    shutdown: Arc<AtomicBool>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let mut buf = vec![0u8; 64 * 1024];
    let n = match stream.read(&mut buf) {
        Ok(0) | Err(_) => return,
        Ok(n) => n,
    };
    let req_str = String::from_utf8_lossy(&buf[..n]);
    let (head, body_part) = req_str
        .split_once("\r\n\r\n")
        .or_else(|| req_str.split_once("\n\n"))
        .unwrap_or((&req_str, ""));

    let first_line = head.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let raw_path = parts.next().unwrap_or("/");
    let path = raw_path.split('?').next().unwrap_or("/");

    // Read remaining content-length if POST
    let mut full_body = body_part.to_string();
    let content_len: usize = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            if k.trim().eq_ignore_ascii_case("content-length") {
                v.trim().parse().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    while full_body.len() < content_len && full_body.len() < 1024 * 1024 {
        let mut extra = [0u8; 4096];
        match stream.read(&mut extra) {
            Ok(0) | Err(_) => break,
            Ok(m) => full_body.push_str(&String::from_utf8_lossy(&extra[..m])),
        }
    }

    let mut trigger_shutdown = false;
    let (status, ctype, resp_body) = match (method, path) {
        ("GET", "/") => match engine.health_snapshot() {
            Ok(h) => (
                "200 OK",
                "text/html; charset=utf-8",
                render_game_health_html(&h),
            ),
            Err(e) => ("500 Internal Server Error", "text/plain", e),
        },
        ("GET", "/ui/packs") => {
            let packs_guard = engine.packs.lock().unwrap();
            let list = packs_guard.list();
            let errs = packs_guard.load_errors();
            (
                "200 OK",
                "text/html; charset=utf-8",
                render_pack_browser_html(&list, errs),
            )
        }
        ("GET", "/health") => match engine.health_snapshot() {
            Ok(h) => (
                "200 OK",
                "application/json; charset=utf-8",
                serde_json::to_string_pretty(&h).unwrap_or_default(),
            ),
            Err(e) => (
                "500 Internal Server Error",
                "application/json",
                serde_json::json!({ "error": e }).to_string(),
            ),
        },
        ("GET", "/packs") => {
            let packs_guard = engine.packs.lock().unwrap();
            let manifests: Vec<_> = packs_guard.list().iter().map(|p| &p.manifest).collect();
            let summaries = packs_guard.summaries();
            let body = serde_json::json!({
                "packs": manifests,
                "pack_summaries": summaries,
                "errors": packs_guard.load_errors(),
            })
            .to_string();
            ("200 OK", "application/json; charset=utf-8", body)
        }
        ("GET", "/progression") => match engine.progression_json(None) {
            Ok(v) => ("200 OK", "application/json; charset=utf-8", v.to_string()),
            Err(e) => (
                "500 Internal Server Error",
                "application/json",
                serde_json::json!({ "error": e }).to_string(),
            ),
        },
        ("POST", "/observations/import") | ("POST", "/v1/observation/import") => {
            handle_observation_import_http(&engine, &full_body)
        }
        ("POST", "/sync/trigger") | ("POST", "/sync-now") => match engine.sync_now() {
            Ok(summary) => (
                "200 OK",
                "application/json; charset=utf-8",
                serde_json::to_string(&summary).unwrap_or_default(),
            ),
            Err(e) => (
                "502 Bad Gateway",
                "application/json; charset=utf-8",
                serde_json::json!({ "error": e }).to_string(),
            ),
        },
        ("POST", "/session/mode") => match serde_json::from_str::<SessionModeReq>(&full_body) {
            Ok(req) => match ProgressionMode::from_str_strict(req.mode.trim()) {
                Some(mode) => {
                    let updated_sessions =
                        engine.set_progression_mode(req.session_id.as_deref(), mode);
                    (
                        "200 OK",
                        "application/json; charset=utf-8",
                        serde_json::json!({
                            "active_progression_mode": engine.current_progression_mode().as_str(),
                            "updated_sessions": updated_sessions,
                        })
                        .to_string(),
                    )
                }
                None => (
                    "400 Bad Request",
                    "application/json; charset=utf-8",
                    serde_json::json!({
                        "error": format!("invalid mode '{}' (expected STANDARD, ENCORE, or SPECTATOR)", req.mode)
                    })
                    .to_string(),
                ),
            },
            Err(e) => (
                "400 Bad Request",
                "application/json; charset=utf-8",
                serde_json::json!({ "error": e.to_string() }).to_string(),
            ),
        },
        ("POST", "/shutdown") => {
            trigger_shutdown = true;
            (
                "200 OK",
                "application/json; charset=utf-8",
                r#"{"status":"stopping"}"#.to_string(),
            )
        }
        ("POST", "/sessions/fair-play-violation") => {
            match serde_json::from_str::<FairPlayActionReq>(&full_body) {
                Ok(req) => {
                    let downgraded = engine
                        .apply_fair_play_violation(&req.session_id, req.violation)
                        .is_some();
                    (
                        "200 OK",
                        "application/json; charset=utf-8",
                        serde_json::json!({ "downgraded": downgraded }).to_string(),
                    )
                }
                Err(e) => (
                    "400 Bad Request",
                    "application/json; charset=utf-8",
                    serde_json::json!({ "error": e.to_string() }).to_string(),
                ),
            }
        }
        _ => (
            "404 Not Found",
            "application/json; charset=utf-8",
            r#"{"error":"not found"}"#.to_string(),
        ),
    };

    let header = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        status,
        ctype,
        resp_body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(resp_body.as_bytes());
    let _ = stream.flush();
    if trigger_shutdown {
        shutdown.store(true, Ordering::SeqCst);
    }
}
