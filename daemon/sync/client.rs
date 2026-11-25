use crate::core::config::{generate_uuid_v4, redact_secrets, save_credentials, Credentials};
use crate::core::types::{AcceptanceStatus, ReceiptStatus, VerificationState, PROTOCOL_VERSION};
use crate::storage::journal::{fetch_pending_sync_events, mark_event_sync_outcome};
use crate::sync::http::http_request;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
struct DeviceRegisterRequest<'a> {
    device_uuid: &'a str,
    device_name: &'a str,
    client_version: &'a str,
    platform_os: &'a str,
    architecture: &'a str,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceRegisterResponse {
    pub device_id: String,
    pub device_token: String,
    pub registered_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PackResolveResponse {
    pub canonical_game_id: String,
    pub pack_id: String,
    pub pack_version: String,
    pub download_url: String,
    pub pack_sha256: String,
    #[serde(default)]
    pub signature_key_id: Option<String>,
}

/// Exact OpenAPI 3.1 `SyncEventEnvelope` (`additionalProperties: false`).
#[derive(Debug, Clone, Serialize)]
struct SyncEventEnvelope<'a> {
    idempotency_key: &'a str,
    canonical_game_id: &'a str,
    producer_id: &'a str,
    session_id: &'a str,
    sequence_number: i64,
    event_kind: &'a str,
    key: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    int_value: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    float_value: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    string_value: Option<&'a str>,
    occurred_at: &'a str,
    progression_mode: &'a str,
    trust_tier: &'a str,
    evidence_class: &'a str,
    verification_state: &'a str,
}

#[derive(Debug, Clone, Serialize)]
struct SyncEventsRequest<'a> {
    client_batch_id: &'a str,
    events: Vec<SyncEventEnvelope<'a>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SyncEventResult {
    pub idempotency_key: String,
    pub receipt_status: ReceiptStatus,
    pub acceptance_status: AcceptanceStatus,
    pub verification_state: VerificationState,
    #[serde(default)]
    pub rejection_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SyncEventsResponse {
    pub client_batch_id: String,
    pub batch_receipt_status: ReceiptStatus,
    pub results: Vec<SyncEventResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncBatchSummary {
    pub synced_count: usize,
    pub batch_receipt_status: Option<String>,
    pub last_acceptance_status: Option<String>,
}

fn current_platform_os() -> &'static str {
    match std::env::consts::OS {
        "linux" => "linux",
        "windows" => "windows",
        "macos" => "macos",
        _ => "bsd",
    }
}

fn current_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        "riscv64" => "riscv64",
        "x86" => "x86",
        _ => "x86_64",
    }
}

pub fn register_device(
    base_url: &str,
    device_uuid: &str,
    device_name: &str,
    config_dir: &Path,
    creds: &mut Credentials,
) -> Result<DeviceRegisterResponse, String> {
    let url = format!("{}/v1/devices/register", base_url.trim_end_matches('/'));
    let req_body = DeviceRegisterRequest {
        device_uuid,
        device_name,
        client_version: PROTOCOL_VERSION,
        platform_os: current_platform_os(),
        architecture: current_arch(),
    };
    let json = serde_json::to_string(&req_body).map_err(|e| e.to_string())?;
    let resp = http_request("POST", &url, creds.api_key.as_deref(), Some(&json), Some(creds))?;
    if resp.status != 201 && resp.status != 200 {
        return Err(redact_secrets(
            &format!("device registration failed (HTTP {}): {}", resp.status, resp.body),
            Some(creds),
        ));
    }
    let parsed: DeviceRegisterResponse = serde_json::from_str(&resp.body)
        .map_err(|e| format!("invalid registerDevice response JSON: {}", e))?;
    creds.device_id = Some(parsed.device_id.clone());
    creds.device_token = Some(parsed.device_token.clone());
    save_credentials(config_dir, creds)?;
    Ok(parsed)
}

pub fn resolve_pack_remote(
    base_url: &str,
    creds: &Credentials,
    matcher_type: &str,
    value: &str,
    region: Option<&str>,
) -> Result<Option<PackResolveResponse>, String> {
    let bearer = creds
        .active_bearer()
        .ok_or_else(|| "missing API key or device token for /v1/packs/resolve".to_string())?;
    let mut url = format!(
        "{}/v1/packs/resolve?matcher_type={}&value={}",
        base_url.trim_end_matches('/'),
        url_encode(matcher_type),
        url_encode(value)
    );
    if let Some(r) = region {
        url.push_str(&format!("&region={}", url_encode(r)));
    }
    let resp = http_request("GET", &url, Some(bearer), None, Some(creds))?;
    if resp.status == 404 {
        return Ok(None);
    }
    if resp.status != 200 {
        return Err(redact_secrets(
            &format!("pack resolve failed (HTTP {}): {}", resp.status, resp.body),
            Some(creds),
        ));
    }
    let parsed: PackResolveResponse = serde_json::from_str(&resp.body)
        .map_err(|e| format!("invalid PackResolveResponse JSON: {}", e))?;
    Ok(Some(parsed))
}

/// Sync up to `batch_size` pending rows from SQLite `event_journal` to `POST /v1/sync/events`.
/// Marks rows synced ONLY after receiving HTTP 200 with durable `receipt_status`.
pub fn sync_pending_batch(
    conn: &mut Connection,
    base_url: &str,
    creds: &Credentials,
    batch_size: usize,
) -> Result<SyncBatchSummary, String> {
    let bearer = creds
        .active_bearer()
        .ok_or_else(|| "no API key or device token configured for cloud sync".to_string())?;

    let rows = fetch_pending_sync_events(conn, batch_size.clamp(1, 500))?;
    if rows.is_empty() {
        return Ok(SyncBatchSummary {
            synced_count: 0,
            batch_receipt_status: None,
            last_acceptance_status: None,
        });
    }

    let batch_id = generate_uuid_v4();
    let envelopes: Vec<SyncEventEnvelope<'_>> = rows
        .iter()
        .map(|r| SyncEventEnvelope {
            idempotency_key: &r.idempotency_key,
            canonical_game_id: &r.canonical_game_id,
            producer_id: &r.producer_id,
            session_id: &r.session_id,
            sequence_number: r.sequence_number,
            event_kind: &r.event_kind,
            key: &r.key,
            int_value: r.int_value,
            float_value: r.float_value,
            string_value: r.string_value.as_deref(),
            occurred_at: &r.occurred_at,
            progression_mode: &r.progression_mode,
            trust_tier: &r.trust_tier,
            evidence_class: &r.evidence_class,
            verification_state: &r.verification_state,
        })
        .collect();

    let req = SyncEventsRequest {
        client_batch_id: &batch_id,
        events: envelopes,
    };
    let json = serde_json::to_string(&req).map_err(|e| e.to_string())?;
    let url = format!("{}/v1/sync/events", base_url.trim_end_matches('/'));
    let resp = http_request("POST", &url, Some(bearer), Some(&json), Some(creds))?;

    if resp.status != 200 {
        return Err(redact_secrets(
            &format!("sync failed (HTTP {}): {}", resp.status, resp.body),
            Some(creds),
        ));
    }

    let parsed: SyncEventsResponse = serde_json::from_str(&resp.body)
        .map_err(|e| format!("invalid SyncEventsResponse JSON: {}", e))?;

    let outcomes: Vec<(
        String,
        ReceiptStatus,
        AcceptanceStatus,
        VerificationState,
        Option<String>,
    )> = parsed
        .results
        .iter()
        .map(|r| {
            (
                r.idempotency_key.clone(),
                r.receipt_status,
                r.acceptance_status,
                r.verification_state,
                r.rejection_reason.clone(),
            )
        })
        .collect();

    let last_acceptance = parsed
        .results
        .last()
        .map(|r| r.acceptance_status.as_str().to_string());
    let count = outcomes.len();

    mark_event_sync_outcome(conn, &outcomes)?;

    Ok(SyncBatchSummary {
        synced_count: count,
        batch_receipt_status: Some(parsed.batch_receipt_status.as_str().to_string()),
        last_acceptance_status: last_acceptance,
    })
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}
