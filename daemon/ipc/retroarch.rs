//! RetroArch Network Command UDP bridge (`127.0.0.1:55355` by default).
//! Fails open cleanly when RetroArch is not running or has `network_cmd_enable = false`.

use std::net::UdpSocket;
use std::time::Duration;

const DEFAULT_UDP_TIMEOUT_MS: u64 = 150;
const MAX_READ_CHUNK_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetroArchStatus {
    pub state: String,
    pub system: Option<String>,
    pub content: Option<String>,
    pub content_crc: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RetroArchClient {
    pub addr: String,
    pub timeout: Duration,
}

impl RetroArchClient {
    pub fn new(addr: impl Into<String>) -> Self {
        Self {
            addr: addr.into(),
            timeout: Duration::from_millis(DEFAULT_UDP_TIMEOUT_MS),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    fn send_cmd(&self, cmd: &str) -> Result<String, String> {
        let sock = UdpSocket::bind("127.0.0.1:0")
            .map_err(|e| format!("failed binding ephemeral UDP socket: {}", e))?;
        sock.set_read_timeout(Some(self.timeout))
            .map_err(|e| format!("failed setting UDP read timeout: {}", e))?;
        sock.set_write_timeout(Some(self.timeout))
            .map_err(|e| format!("failed setting UDP write timeout: {}", e))?;
        sock.connect(&self.addr)
            .map_err(|e| format!("failed connecting to RetroArch at '{}': {}", self.addr, e))?;

        let payload = if cmd.ends_with('\n') {
            cmd.to_string()
        } else {
            format!("{}\n", cmd)
        };
        sock.send(payload.as_bytes())
            .map_err(|e| format!("failed sending '{}' to RetroArch: {}", cmd, e))?;

        let mut buf = [0u8; 8192];
        let n = sock
            .recv(&mut buf)
            .map_err(|e| format!("no RetroArch response for '{}': {}", cmd, e))?;
        Ok(String::from_utf8_lossy(&buf[..n]).trim().to_string())
    }

    pub fn version(&self) -> Result<String, String> {
        let resp = self.send_cmd("VERSION")?;
        if resp.is_empty() {
            return Err("empty VERSION reply".into());
        }
        Ok(resp.strip_prefix("VERSION ").unwrap_or(&resp).trim().to_string())
    }

    pub fn get_status(&self) -> Result<RetroArchStatus, String> {
        let resp = self.send_cmd("GET_STATUS")?;
        parse_get_status_reply(&resp)
    }

    pub fn read_core_memory(&self, base_addr: u64, size: usize) -> Result<Vec<u8>, String> {
        if size == 0 {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity(size);
        let mut offset = 0usize;
        while offset < size {
            let chunk = (size - offset).min(MAX_READ_CHUNK_BYTES);
            let addr = base_addr.saturating_add(offset as u64);
            let resp = self.send_cmd(&format!("READ_CORE_MEMORY {:x} {}", addr, chunk))?;
            let bytes = parse_read_core_memory_reply(&resp, chunk)?;
            out.extend_from_slice(&bytes);
            offset += chunk;
        }
        Ok(out)
    }
}

pub fn parse_get_status_reply(reply: &str) -> Result<RetroArchStatus, String> {
    let trimmed = reply.trim();
    let rest = trimmed.strip_prefix("GET_STATUS ").unwrap_or(trimmed).trim();
    if rest.is_empty() {
        return Err("empty GET_STATUS response".into());
    }
    let (state, detail) = match rest.split_once(' ') {
        Some((s, d)) => (s.trim().to_ascii_uppercase(), d.trim()),
        None => (rest.to_ascii_uppercase(), ""),
    };
    if state == "CONTENTLESS" || detail.is_empty() {
        return Ok(RetroArchStatus {
            state,
            system: None,
            content: None,
            content_crc: None,
        });
    }

    let parts: Vec<&str> = detail.split(',').map(str::trim).collect();
    let system = parts.first().filter(|s| !s.is_empty()).map(|s| (*s).to_string());
    let content = parts.get(1).filter(|s| !s.is_empty()).map(|s| (*s).to_string());
    let crc = parts.get(2).and_then(|raw| {
        let cleaned = raw
            .trim()
            .strip_prefix("crc32=")
            .or_else(|| raw.trim().strip_prefix("CRC32="))
            .unwrap_or(raw.trim());
        let cleaned = cleaned.strip_prefix("0x").or_else(|| cleaned.strip_prefix("0X")).unwrap_or(cleaned);
        if cleaned.is_empty() {
            None
        } else {
            Some(cleaned.to_ascii_lowercase())
        }
    });

    Ok(RetroArchStatus {
        state,
        system,
        content,
        content_crc: crc,
    })
}

pub fn parse_read_core_memory_reply(reply: &str, expected_len: usize) -> Result<Vec<u8>, String> {
    let trimmed = reply.trim();
    let mut tokens = trimmed.split_whitespace();
    let head = tokens.next().unwrap_or("");
    if !head.eq_ignore_ascii_case("READ_CORE_MEMORY") {
        return Err(format!("unexpected READ_CORE_MEMORY reply '{}'", trimmed));
    }
    let _addr = tokens
        .next()
        .ok_or_else(|| "missing address in READ_CORE_MEMORY reply".to_string())?;
    let rem: Vec<&str> = tokens.collect();
    if rem.is_empty() || rem[0] == "-1" {
        return Err("RetroArch core memory read unavailable (-1)".into());
    }
    let mut out = Vec::with_capacity(rem.len());
    for tok in rem {
        let byte = u8::from_str_radix(tok.trim(), 16)
            .map_err(|_| format!("invalid hex byte '{}' in RetroArch reply", tok))?;
        out.push(byte);
    }
    if out.len() < expected_len {
        return Err(format!(
            "short RetroArch core memory read: expected {} bytes, got {}",
            expected_len,
            out.len()
        ));
    }
    out.truncate(expected_len);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn parses_status_and_reads_chunked_core_memory_over_udp() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let mut mem = vec![0u8; 600];
            mem[0x10] = 0xAB;
            mem[599] = 0xCD;
            let mut buf = [0u8; 512];
            for _ in 0..4 {
                let (n, src) = server.recv_from(&mut buf).unwrap();
                let cmd = String::from_utf8_lossy(&buf[..n]).trim().to_string();
                let reply = if cmd == "VERSION" {
                    "1.20.0\n".to_string()
                } else if cmd == "GET_STATUS" {
                    "GET_STATUS PLAYING snes9x,Super_Metroid,crc32=D63ED5F8\n".to_string()
                } else if let Some(rest) = cmd.strip_prefix("READ_CORE_MEMORY ") {
                    let mut sp = rest.split_whitespace();
                    let off = usize::from_str_radix(sp.next().unwrap(), 16).unwrap();
                    let len: usize = sp.next().unwrap().parse().unwrap();
                    let hex: Vec<String> = mem[off..off + len]
                        .iter()
                        .map(|b| format!("{:02X}", b))
                        .collect();
                    format!("READ_CORE_MEMORY {:x} {}\n", off, hex.join(" "))
                } else {
                    "ERR\n".to_string()
                };
                server.send_to(reply.as_bytes(), src).unwrap();
            }
        });

        let client = RetroArchClient::new(addr.to_string()).with_timeout(Duration::from_millis(500));
        assert_eq!(client.version().unwrap(), "1.20.0");
        let st = client.get_status().unwrap();
        assert_eq!(st.state, "PLAYING");
        assert_eq!(st.system.as_deref(), Some("snes9x"));
        assert_eq!(st.content_crc.as_deref(), Some("d63ed5f8"));

        let data = client.read_core_memory(0, 600).unwrap();
        assert_eq!(data.len(), 600);
        assert_eq!(data[0x10], 0xAB);
        assert_eq!(data[599], 0xCD);
        handle.join().unwrap();
    }
}
