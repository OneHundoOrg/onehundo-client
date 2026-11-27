use crate::core::config::generate_uuid_v4;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct BrowserAuthResult {
    pub api_key: String,
    pub username: Option<String>,
}

pub struct PendingBrowserAuth {
    pub auth_url: String,
    pub fallback_url: String,
    pub cancel_flag: Arc<AtomicBool>,
    pub rx: Receiver<Result<BrowserAuthResult, String>>,
}

fn url_encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() * 2);
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn decode_component(raw: &str) -> String {
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

pub fn parse_callback_query(request_target: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    let Some((_, qs)) = request_target.split_once('?') else {
        return params;
    };
    for pair in qs.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        params.insert(decode_component(k), decode_component(v));
    }
    params
}

pub fn validate_callback_params(
    expected_state: &str,
    params: &HashMap<String, String>,
) -> Result<BrowserAuthResult, String> {
    if let Some(err) = params.get("error").filter(|e| !e.trim().is_empty()) {
        return Err(format!("Sign-in cancelled or denied ({})", err));
    }
    let received_state = params
        .get("state")
        .map(|s| s.as_str())
        .unwrap_or("");
    if received_state != expected_state {
        return Err("Sign-in verification state mismatch".to_string());
    }
    let token = params
        .get("token")
        .or_else(|| params.get("api_key"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Missing token in sign-in callback".to_string())?;
    let username = params
        .get("username")
        .map(|s| s.trim().trim_start_matches('@').to_string())
        .filter(|s| !s.is_empty());
    Ok(BrowserAuthResult {
        api_key: token,
        username,
    })
}

fn callback_html(ok: bool, headline: &str, detail: &str) -> String {
    let badge = if ok { "CONNECTED" } else { "SIGN-IN ERROR" };
    let border = if ok { "#E3C46A" } else { "#FF6B5E" };
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
        <title>OneHundo Desktop Sign-In</title>\
        <style>\
        body{{margin:0;min-height:100vh;display:grid;place-items:center;background:#0D0D0E;color:#EDEBE6;font-family:Inter,system-ui,sans-serif;}}\
        .card{{max-width:460px;padding:28px;background:#17181A;border:1px solid #2C2E31;border-top:2px solid {border};}}\
        .tag{{display:inline-block;padding:3px 8px;background:#202124;border:1px solid #2C2E31;font-size:11px;letter-spacing:.08em;color:{border};margin-bottom:14px;}}\
        h1{{margin:0 0 10px;font-size:22px;font-weight:700;}}\
        p{{margin:0;color:#A9A7A1;line-height:1.5;font-size:14px;}}\
        </style></head><body><main class=\"card\">\
        <div class=\"tag\">{badge}</div>\
        <h1>{headline}</h1><p>{detail}</p></main></body></html>"
    )
}

pub fn open_in_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    };

    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not launch system browser: {}", e))?;
    Ok(())
}

/// Starts a loopback HTTP listener on `127.0.0.1:0` and launches `<platform_url>/desktop-auth`
/// in the system browser for one-click sign-in.
pub fn start_browser_auth(platform_url: &str, device_name: &str) -> Result<PendingBrowserAuth, String> {
    let base = platform_url.trim().trim_end_matches('/');
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Failed binding loopback callback listener: {}", e))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("Failed configuring loopback listener: {}", e))?;
    let local_addr = listener
        .local_addr()
        .map_err(|e| format!("Failed reading loopback listener address: {}", e))?;

    let state = generate_uuid_v4();
    let redirect_uri = format!("http://127.0.0.1:{}/callback", local_addr.port());
    let auth_url = format!(
        "{}/desktop-auth?redirect_uri={}&state={}&device={}",
        base,
        url_encode(&redirect_uri),
        url_encode(&state),
        url_encode(device_name),
    );
    let fallback_url = format!("{}/me/account#api-keys", base);

    open_in_browser(&auth_url)?;

    let (tx, rx) = mpsc::channel();
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let cancel_worker = Arc::clone(&cancel_flag);
    let expected_state = state;

    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(300);
        while Instant::now() < deadline && !cancel_worker.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut buf = [0u8; 8192];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let first_line = req.lines().next().unwrap_or("");
                    let mut parts = first_line.split_whitespace();
                    let _method = parts.next();
                    let target = parts.next().unwrap_or("/");
                    if !target.starts_with("/callback") {
                        let body = "Not Found";
                        let resp = format!(
                            "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(resp.as_bytes());
                        continue;
                    }

                    let params = parse_callback_query(target);
                    let result = validate_callback_params(&expected_state, &params);
                    let (status, html) = match &result {
                        Ok(res) => {
                            let who = res
                                .username
                                .as_deref()
                                .map(|u| format!("Signed in as @{}", u))
                                .unwrap_or_else(|| "Signed in to OneHundo".to_string());
                            (
                                "200 OK",
                                callback_html(
                                    true,
                                    &who,
                                    "OneHundo Desktop is connected. You can close this browser tab and return to the app.",
                                ),
                            )
                        }
                        Err(err) => (
                            "400 Bad Request",
                            callback_html(
                                false,
                                "Desktop sign-in could not be completed",
                                err,
                            ),
                        ),
                    };
                    let resp = format!(
                        "HTTP/1.1 {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{}",
                        status,
                        html.len(),
                        html
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                    let _ = tx.send(result);
                    return;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    let _ = tx.send(Err(format!("Sign-in listener error: {}", e)));
                    return;
                }
            }
        }
        if !cancel_worker.load(Ordering::Relaxed) {
            let _ = tx.send(Err("Browser sign-in timed out after 5 minutes".to_string()));
        }
    });

    Ok(PendingBrowserAuth {
        auth_url,
        fallback_url,
        cancel_flag,
        rx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_callback_state_and_extracts_token_and_username() {
        let params = parse_callback_query(
            "/callback?token=oh_live_abc123&username=player_one&state=expected-state",
        );
        let ok = validate_callback_params("expected-state", &params).expect("valid callback");
        assert_eq!(ok.api_key, "oh_live_abc123");
        assert_eq!(ok.username.as_deref(), Some("player_one"));

        assert!(validate_callback_params("wrong-state", &params).is_err());

        let missing = parse_callback_query("/callback?state=expected-state");
        assert!(validate_callback_params("expected-state", &missing).is_err());
    }
}
