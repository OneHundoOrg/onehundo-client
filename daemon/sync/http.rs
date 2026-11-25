use crate::core::config::{redact_secrets, Credentials};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

pub fn http_request(
    method: &str,
    url: &str,
    bearer_token: Option<&str>,
    json_body: Option<&str>,
    creds_for_redaction: Option<&Credentials>,
) -> Result<HttpResponse, String> {
    if url.starts_with("http://") {
        tcp_http_request(method, url, bearer_token, json_body)
            .map_err(|e| redact_secrets(&e, creds_for_redaction))
    } else if url.starts_with("https://") {
        curl_https_request(method, url, bearer_token, json_body)
            .map_err(|e| redact_secrets(&e, creds_for_redaction))
    } else {
        Err(format!("unsupported URL scheme in '{}'", url))
    }
}

fn tcp_http_request(
    method: &str,
    url: &str,
    bearer_token: Option<&str>,
    json_body: Option<&str>,
) -> Result<HttpResponse, String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| "expected http:// URL".to_string())?;
    let (host_port, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    let addr = if host_port.contains(':') {
        host_port.to_string()
    } else {
        format!("{}:80", host_port)
    };

    let mut stream = TcpStream::connect(&addr)
        .map_err(|e| format!("TCP connect to '{}' failed: {}", addr, e))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let mut req = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAccept: application/json\r\n",
        method, path, host_port
    );
    if let Some(tok) = bearer_token {
        req.push_str(&format!("Authorization: Bearer {}\r\n", tok));
    }
    if let Some(body) = json_body {
        req.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    req.push_str("\r\n");

    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("HTTP write header failed: {}", e))?;
    if let Some(body) = json_body {
        stream
            .write_all(body.as_bytes())
            .map_err(|e| format!("HTTP write body failed: {}", e))?;
    }
    stream
        .flush()
        .map_err(|e| format!("HTTP flush failed: {}", e))?;

    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|e| format!("HTTP read failed: {}", e))?;

    parse_http_response(&raw)
}

fn parse_http_response(raw: &[u8]) -> Result<HttpResponse, String> {
    let text = String::from_utf8_lossy(raw);
    let (head, body_raw) = text
        .split_once("\r\n\r\n")
        .or_else(|| text.split_once("\n\n"))
        .ok_or_else(|| "malformed HTTP response (missing header separator)".to_string())?;

    let mut lines = head.lines();
    let status_line = lines
        .next()
        .ok_or_else(|| "missing HTTP status line".to_string())?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("invalid HTTP status line '{}'", status_line))?;

    let is_chunked = lines.any(|l| {
        let lower = l.to_ascii_lowercase();
        lower.starts_with("transfer-encoding:") && lower.contains("chunked")
    });

    let body = if is_chunked {
        decode_chunked(body_raw)?
    } else {
        body_raw.to_string()
    };

    Ok(HttpResponse { status, body })
}

fn decode_chunked(mut s: &str) -> Result<String, String> {
    let mut out = String::new();
    loop {
        let (len_line, rest) = match s.split_once("\r\n") {
            Some(pair) => pair,
            None => break,
        };
        let hex = len_line.split(';').next().unwrap_or("").trim();
        let len = usize::from_str_radix(hex, 16)
            .map_err(|_| format!("invalid chunked hex length '{}'", hex))?;
        if len == 0 {
            break;
        }
        if rest.len() < len {
            out.push_str(rest);
            break;
        }
        out.push_str(&rest[..len]);
        s = rest[len..].strip_prefix("\r\n").unwrap_or(&rest[len..]);
    }
    Ok(out)
}

/// HTTPS transport via `curl --config -` so Authorization tokens are passed via stdin
/// (never visible in process command-line arguments or logs).
fn curl_https_request(
    method: &str,
    url: &str,
    bearer_token: Option<&str>,
    json_body: Option<&str>,
) -> Result<HttpResponse, String> {
    let mut cfg = String::new();
    cfg.push_str("silent\nshow-error\n");
    cfg.push_str(&format!("request = \"{}\"\n", method));
    cfg.push_str(&format!("url = \"{}\"\n", url.replace('"', "\\\"")));
    cfg.push_str("write-out = \"\\n__OH_STATUS__:%{http_code}\"\n");
    cfg.push_str("header = \"Accept: application/json\"\n");
    if let Some(tok) = bearer_token {
        cfg.push_str(&format!(
            "header = \"Authorization: Bearer {}\"\n",
            tok.replace('"', "")
        ));
    }
    if json_body.is_some() {
        cfg.push_str("header = \"Content-Type: application/json\"\n");
    }

    let mut cmd = Command::new("curl");
    cmd.args(["--config", "-"]);
    if let Some(body) = json_body {
        cmd.args(["--data-raw", body]);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn curl for HTTPS sync: {}", e))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(cfg.as_bytes())
            .map_err(|e| format!("failed writing curl config: {}", e))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("curl wait failed: {}", e))?;
    if !output.status.success() {
        let err_str = String::from_utf8_lossy(&output.stderr);
        return Err(format!("HTTPS request failed: {}", err_str.trim()));
    }
    let full = String::from_utf8_lossy(&output.stdout);
    let (body, marker) = full
        .rsplit_once("\n__OH_STATUS__:")
        .ok_or_else(|| "missing status marker from curl".to_string())?;
    let status: u16 = marker
        .trim()
        .parse()
        .map_err(|_| "invalid HTTP status code from curl".to_string())?;
    Ok(HttpResponse {
        status,
        body: body.to_string(),
    })
}
