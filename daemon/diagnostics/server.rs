use crate::core::engine::DaemonEngine;
use crate::core::types::{FairPlayViolation, ProgressionMode};
use crate::observation::external_importers::list_observation_history;
use crate::storage::state::{list_stats, list_trophies};
use crate::ui::game_health::render_game_health_html;
use crate::ui::pack_browser::render_pack_browser_with_state_html;
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

#[derive(Deserialize, Default)]
struct NotifyTestReq {
    #[serde(default)]
    canonical_game_id: Option<String>,
    #[serde(default)]
    game_title: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Deserialize)]
struct WatchAddReq {
    #[serde(default)]
    canonical_game_id: Option<String>,
    path: String,
}

#[derive(Deserialize)]
struct WatchRemoveReq {
    path: String,
}

#[derive(Deserialize)]
struct LauncherReportReq {
    launcher: String,
    #[serde(default)]
    canonical_game_id: Option<String>,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    executable: Option<String>,
    #[serde(default)]
    install_dir: Option<String>,
    #[serde(default)]
    save_dir: Option<String>,
    #[serde(default)]
    os: Option<String>,
    #[serde(default = "default_true_opt")]
    active: bool,
}

fn default_true_opt() -> bool {
    true
}

fn decode_query_component(raw: &str) -> String {
    let mut out = Vec::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
        } else if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
                    i += 3;
                    continue;
                }
            }
            out.push(bytes[i]);
            i += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn parse_query_params(raw_path: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let Some((_, qs)) = raw_path.split_once('?') else {
        return map;
    };
    for pair in qs.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        map.insert(decode_query_component(k), decode_query_component(v));
    }
    map
}

/// Header value for `name` in a raw request head (case-insensitive), first occurrence.
fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        if k.trim().eq_ignore_ascii_case(name) {
            Some(v.trim())
        } else {
            None
        }
    })
}

/// Splits `host[:port]` (including bracketed IPv6 `[::1]:19850`) into host and optional port.
fn split_host_port(value: &str) -> Option<(&str, Option<&str>)> {
    if let Some(rest) = value.strip_prefix('[') {
        let end = rest.find(']')?;
        let host = &value[..end + 2];
        let after = &rest[end + 1..];
        return match after.strip_prefix(':') {
            Some(port) => Some((host, Some(port))),
            None if after.is_empty() => Some((host, None)),
            None => None,
        };
    }
    match value.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => Some((h, Some(p))),
        Some(_) => None,
        None => Some((value, None)),
    }
}

/// Host names the Game Health server answers to: loopback names, plus the exact address it is
/// bound to when that is a specific (non-wildcard) IP.
fn host_allowed(host: &str, bound: SocketAddr) -> bool {
    let host = host.to_ascii_lowercase();
    if host == "localhost" || host == "127.0.0.1" || host == "[::1]" {
        return true;
    }
    let ip = bound.ip();
    if ip.is_unspecified() {
        return false;
    }
    let bound_host = match ip {
        std::net::IpAddr::V4(v4) => v4.to_string(),
        std::net::IpAddr::V6(v6) => format!("[{}]", v6),
    };
    host == bound_host
}

/// Defends the loopback control API against cross-origin browser requests and DNS rebinding:
/// - `Host` must be present and name a loopback host (or the specific bound IP), so a page served
///   from an attacker's domain that re-resolves to 127.0.0.1 is refused.
/// - `Origin`, when sent (browsers always send it on cross-origin and on POST), must be exactly
///   `http://<allowed host>:<bound port>`: another site, another local port, or `null` is refused.
/// - `Sec-Fetch-Site: cross-site` is refused outright.
/// CLI clients (`onehundo-desktop`, curl) send `Host` and no `Origin`, and are unaffected.
pub fn local_request_allowed(head: &str, bound: SocketAddr) -> Result<(), &'static str> {
    let host = header_value(head, "host").ok_or("missing Host header")?;
    let (host_name, _) = split_host_port(host).ok_or("malformed Host header")?;
    if !host_allowed(host_name, bound) {
        return Err("Host is not a loopback address");
    }
    if let Some(site) = header_value(head, "sec-fetch-site") {
        if site.eq_ignore_ascii_case("cross-site") {
            return Err("cross-site request refused");
        }
    }
    if let Some(origin) = header_value(head, "origin") {
        let rest = origin
            .strip_prefix("http://")
            .ok_or("cross-origin request refused")?;
        let (o_host, o_port) = split_host_port(rest).ok_or("cross-origin request refused")?;
        let port_ok = o_port.and_then(|p| p.parse::<u16>().ok()) == Some(bound.port());
        if !port_ok || !host_allowed(o_host, bound) {
            return Err("cross-origin request refused");
        }
    }
    Ok(())
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
                    thread::spawn(move || handle_http_conn(stream, eng, sd, local_addr));
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

fn write_response(stream: &mut TcpStream, status: &str, ctype: &str, body: &str) {
    // nosniff + no framing: the control center's buttons can't be clickjacked from another page.
    let header = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nContent-Security-Policy: frame-ancestors 'none'\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n",
        status,
        ctype,
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
}

fn handle_http_conn(
    mut stream: TcpStream,
    engine: Arc<DaemonEngine>,
    shutdown: Arc<AtomicBool>,
    bound: SocketAddr,
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

    if let Err(reason) = local_request_allowed(head, bound) {
        write_response(
            &mut stream,
            "403 Forbidden",
            "application/json; charset=utf-8",
            &serde_json::json!({ "error": reason }).to_string(),
        );
        return;
    }

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
            let (observations, trophies, stats) = {
                let db = engine.db.lock().unwrap();
                (
                    list_observation_history(&db, 200).unwrap_or_default(),
                    list_trophies(&db, None).unwrap_or_default(),
                    list_stats(&db, None).unwrap_or_default(),
                )
            };
            let active_mode = engine.current_progression_mode();
            let packs_guard = engine.packs.lock().unwrap();
            let list = packs_guard.list();
            let errs = packs_guard.load_errors();
            (
                "200 OK",
                "text/html; charset=utf-8",
                render_pack_browser_with_state_html(
                    &list,
                    errs,
                    &observations,
                    &trophies,
                    &stats,
                    active_mode,
                ),
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
        ("GET", "/watches") => {
            let dirs = engine.list_watch_overrides();
            (
                "200 OK",
                "application/json; charset=utf-8",
                serde_json::json!({ "watched_dirs": dirs }).to_string(),
            )
        }
        ("POST", "/watches") => match serde_json::from_str::<WatchAddReq>(&full_body) {
            Ok(req) => match engine.add_watch_override(req.canonical_game_id, Path::new(&req.path)) {
                Ok(added) => (
                    "200 OK",
                    "application/json; charset=utf-8",
                    serde_json::json!({
                        "watched_dir": added,
                        "watched_dirs": engine.list_watch_overrides(),
                    })
                    .to_string(),
                ),
                Err(e) => (
                    "400 Bad Request",
                    "application/json; charset=utf-8",
                    serde_json::json!({ "error": e }).to_string(),
                ),
            },
            Err(e) => (
                "400 Bad Request",
                "application/json; charset=utf-8",
                serde_json::json!({ "error": e.to_string() }).to_string(),
            ),
        },
        ("POST", "/watches/remove") => match serde_json::from_str::<WatchRemoveReq>(&full_body) {
            Ok(req) => match engine.remove_watch_override(Path::new(&req.path)) {
                Ok(removed) => (
                    "200 OK",
                    "application/json; charset=utf-8",
                    serde_json::json!({
                        "removed": removed,
                        "watched_dirs": engine.list_watch_overrides(),
                    })
                    .to_string(),
                ),
                Err(e) => (
                    "500 Internal Server Error",
                    "application/json; charset=utf-8",
                    serde_json::json!({ "error": e }).to_string(),
                ),
            },
            Err(e) => (
                "400 Bad Request",
                "application/json; charset=utf-8",
                serde_json::json!({ "error": e.to_string() }).to_string(),
            ),
        },
        ("POST", "/watches/poll") => {
            let summary = engine.poll_watched_observation_dirs();
            (
                "200 OK",
                "application/json; charset=utf-8",
                serde_json::to_string(&summary).unwrap_or_default(),
            )
        }
        ("POST", "/v1/launchers/report") | ("POST", "/launchers/report") => {
            match serde_json::from_str::<LauncherReportReq>(&full_body) {
                Ok(req) => match engine.report_launcher_game(
                    &req.launcher,
                    req.canonical_game_id,
                    req.app_id,
                    req.executable,
                    req.install_dir,
                    req.save_dir,
                    req.os,
                    req.active,
                ) {
                    Ok(telemetry) => (
                        "200 OK",
                        "application/json; charset=utf-8",
                        serde_json::json!({ "game": telemetry }).to_string(),
                    ),
                    Err(e) => (
                        "400 Bad Request",
                        "application/json; charset=utf-8",
                        serde_json::json!({ "error": e }).to_string(),
                    ),
                },
                Err(e) => (
                    "400 Bad Request",
                    "application/json; charset=utf-8",
                    serde_json::json!({ "error": e.to_string() }).to_string(),
                ),
            }
        }
        ("GET", "/v1/launchers/status") | ("GET", "/launchers/status") => {
            let q = parse_query_params(raw_path);
            let games = engine.query_launcher_status(
                q.get("canonical_game_id").map(String::as_str).filter(|s| !s.is_empty()),
                q.get("app_id").map(String::as_str).filter(|s| !s.is_empty()),
                q.get("executable").map(String::as_str).filter(|s| !s.is_empty()),
            );
            let retroarch = engine.retroarch_telemetry.lock().unwrap().clone();
            (
                "200 OK",
                "application/json; charset=utf-8",
                serde_json::json!({
                    "games": games,
                    "retroarch": retroarch,
                })
                .to_string(),
            )
        }
        ("POST", "/observations/import") | ("POST", "/v1/observation/import") => {
            handle_observation_import_http(&engine, &full_body)
        }
        ("POST", "/notify-test") => {
            let trimmed = full_body.trim();
            let req_res = if trimmed.is_empty() {
                Ok(NotifyTestReq::default())
            } else {
                serde_json::from_str::<NotifyTestReq>(trimmed)
            };
            match req_res {
                Ok(req) => {
                    let report = engine.trigger_test_notification(
                        req.canonical_game_id.as_deref(),
                        req.game_title.as_deref(),
                        req.title.as_deref(),
                        req.body.as_deref(),
                    );
                    let status = if report.error.is_some() && report.os_toasts_enabled {
                        "500 Internal Server Error"
                    } else {
                        "200 OK"
                    };
                    let body = serde_json::json!({
                        "mode": "daemon",
                        "dispatched": report.dispatched,
                        "os_toasts_enabled": report.os_toasts_enabled,
                        "error": report.error,
                        "event": report.event,
                    })
                    .to_string();
                    (status, "application/json; charset=utf-8", body)
                }
                Err(e) => (
                    "400 Bad Request",
                    "application/json; charset=utf-8",
                    serde_json::json!({ "error": e.to_string() }).to_string(),
                ),
            }
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

    write_response(&mut stream, status, ctype, &resp_body);
    if trigger_shutdown {
        shutdown.store(true, Ordering::SeqCst);
    }
}
