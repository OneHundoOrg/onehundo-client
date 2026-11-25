use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

pub const PROTOCOL_VERSION: &str = "0.1.0";

/// Ingestion plane matching `onehundo_ingestion_plane_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum IngestionPlane {
    MemoryArena,
    SemanticEvent,
    ExternalObservation,
}

impl IngestionPlane {
    pub fn from_wire_u32(val: u32) -> Option<Self> {
        match val {
            1 => Some(Self::MemoryArena),
            2 => Some(Self::SemanticEvent),
            3 => Some(Self::ExternalObservation),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MemoryArena => "MEMORY_ARENA",
            Self::SemanticEvent => "SEMANTIC_EVENT",
            Self::ExternalObservation => "EXTERNAL_OBSERVATION",
        }
    }
}

/// Orthogonal Dimension 1: Gameplay progression mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProgressionMode {
    Standard,
    Encore,
    Spectator,
}

impl ProgressionMode {
    pub fn from_wire_u32(val: u32) -> Option<Self> {
        match val {
            1 => Some(Self::Standard),
            2 => Some(Self::Encore),
            3 => Some(Self::Spectator),
            _ => None,
        }
    }

    pub fn from_str_strict(s: &str) -> Option<Self> {
        match s {
            "STANDARD" => Some(Self::Standard),
            "ENCORE" => Some(Self::Encore),
            "SPECTATOR" => Some(Self::Spectator),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Standard => "STANDARD",
            Self::Encore => "ENCORE",
            Self::Spectator => "SPECTATOR",
        }
    }
}

impl fmt::Display for ProgressionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Orthogonal Dimension 2: Trust tier of the runtime integration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TrustTier {
    Local,
    VerifiedIntegration,
    Competitive,
}

impl TrustTier {
    pub fn from_wire_u32(val: u32) -> Option<Self> {
        match val {
            1 => Some(Self::Local),
            2 => Some(Self::VerifiedIntegration),
            3 => Some(Self::Competitive),
            _ => None,
        }
    }

    pub fn from_str_strict(s: &str) -> Option<Self> {
        match s {
            "LOCAL" => Some(Self::Local),
            "VERIFIED_INTEGRATION" => Some(Self::VerifiedIntegration),
            "COMPETITIVE" => Some(Self::Competitive),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Local => "LOCAL",
            Self::VerifiedIntegration => "VERIFIED_INTEGRATION",
            Self::Competitive => "COMPETITIVE",
        }
    }
}

impl fmt::Display for TrustTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Orthogonal Dimension 3: Evidence provenance class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EvidenceClass {
    DirectIntegrationEvent,
    RuntimeObserved,
    PlatformConfirmed,
    LocallyObserved,
    Inferred,
}

impl EvidenceClass {
    pub fn from_wire_u32(val: u32) -> Option<Self> {
        match val {
            1 => Some(Self::DirectIntegrationEvent),
            2 => Some(Self::RuntimeObserved),
            3 => Some(Self::PlatformConfirmed),
            4 => Some(Self::LocallyObserved),
            5 => Some(Self::Inferred),
            _ => None,
        }
    }

    pub fn from_str_strict(s: &str) -> Option<Self> {
        match s {
            "DIRECT_INTEGRATION_EVENT" => Some(Self::DirectIntegrationEvent),
            "RUNTIME_OBSERVED" => Some(Self::RuntimeObserved),
            "PLATFORM_CONFIRMED" => Some(Self::PlatformConfirmed),
            "LOCALLY_OBSERVED" => Some(Self::LocallyObserved),
            "INFERRED" => Some(Self::Inferred),
            _ => None,
        }
    }

    pub fn is_valid_plane_c(&self) -> bool {
        matches!(
            self,
            Self::PlatformConfirmed | Self::LocallyObserved | Self::Inferred
        )
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DirectIntegrationEvent => "DIRECT_INTEGRATION_EVENT",
            Self::RuntimeObserved => "RUNTIME_OBSERVED",
            Self::PlatformConfirmed => "PLATFORM_CONFIRMED",
            Self::LocallyObserved => "LOCALLY_OBSERVED",
            Self::Inferred => "INFERRED",
        }
    }
}

impl fmt::Display for EvidenceClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Orthogonal Dimension 4: Verification outcome state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VerificationState {
    Pending,
    Verified,
    Unverified,
    Rejected,
}

impl VerificationState {
    pub fn from_str_strict(s: &str) -> Option<Self> {
        match s {
            "PENDING" => Some(Self::Pending),
            "VERIFIED" => Some(Self::Verified),
            "UNVERIFIED" => Some(Self::Unverified),
            "REJECTED" => Some(Self::Rejected),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Verified => "VERIFIED",
            Self::Unverified => "UNVERIFIED",
            Self::Rejected => "REJECTED",
        }
    }
}

impl fmt::Display for VerificationState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Plane A runtime lifecycle signals matching `onehundo_lifecycle_signal_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LifecycleSignal {
    NormalFrame,
    SaveStateLoad,
    SoftReset,
    HardReset,
    FrameGap,
}

impl LifecycleSignal {
    pub fn from_wire_u32(val: u32) -> Option<Self> {
        match val {
            1 => Some(Self::NormalFrame),
            2 => Some(Self::SaveStateLoad),
            3 => Some(Self::SoftReset),
            4 => Some(Self::HardReset),
            5 => Some(Self::FrameGap),
            _ => None,
        }
    }

    pub fn from_str_strict(s: &str) -> Option<Self> {
        match s {
            "NORMAL_FRAME" => Some(Self::NormalFrame),
            "SAVE_STATE_LOAD" => Some(Self::SaveStateLoad),
            "SOFT_RESET" => Some(Self::SoftReset),
            "HARD_RESET" => Some(Self::HardReset),
            "FRAME_GAP" => Some(Self::FrameGap),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NormalFrame => "NORMAL_FRAME",
            Self::SaveStateLoad => "SAVE_STATE_LOAD",
            Self::SoftReset => "SOFT_RESET",
            Self::HardReset => "HARD_RESET",
            Self::FrameGap => "FRAME_GAP",
        }
    }
}

/// Cloud sync receipt status (orthogonal to acceptance_status).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReceiptStatus {
    Received,
    Duplicate,
}

impl ReceiptStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Received => "RECEIVED",
            Self::Duplicate => "DUPLICATE",
        }
    }
}

/// Cloud sync acceptance status (orthogonal to receipt_status).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AcceptanceStatus {
    Accepted,
    PendingReview,
    Rejected,
}

impl AcceptanceStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Accepted => "ACCEPTED",
            Self::PendingReview => "PENDING_REVIEW",
            Self::Rejected => "REJECTED",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EventKind {
    StatSet,
    StatInc,
    StateObserve,
    UnlockRequest,
    ObservationImport,
}

impl EventKind {
    pub fn from_str_strict(s: &str) -> Option<Self> {
        match s {
            "STAT_SET" => Some(Self::StatSet),
            "STAT_INC" => Some(Self::StatInc),
            "STATE_OBSERVE" => Some(Self::StateObserve),
            "UNLOCK_REQUEST" => Some(Self::UnlockRequest),
            "OBSERVATION_IMPORT" => Some(Self::ObservationImport),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::StatSet => "STAT_SET",
            Self::StatInc => "STAT_INC",
            Self::StateObserve => "STATE_OBSERVE",
            Self::UnlockRequest => "UNLOCK_REQUEST",
            Self::ObservationImport => "OBSERVATION_IMPORT",
        }
    }
}

/// Fair-play violations triggering automatic VERIFIED_INTEGRATION -> LOCAL downgrade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FairPlayViolation {
    SaveStateLoad,
    CheatToggle,
    MemoryPoke,
    DebugConsoleOverride,
    ExcessiveFrameGap,
}

impl FairPlayViolation {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SaveStateLoad => "save_state_load",
            Self::CheatToggle => "cheat_toggle",
            Self::MemoryPoke => "memory_poke",
            Self::DebugConsoleOverride => "debug_console_override",
            Self::ExcessiveFrameGap => "excessive_frame_gap",
        }
    }
}

pub fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub fn now_unix_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

/// Format unix epoch seconds as RFC 3339 UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`)
/// using Howard Hinnant's civil-from-days algorithm (zero external dependencies).
pub fn format_rfc3339_secs(unix_secs: i64) -> String {
    let secs = unix_secs.max(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;
    let second = rem % 60;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + if m <= 2 { 1 } else { 0 };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, m, d, hour, minute, second
    )
}

pub fn now_rfc3339() -> String {
    format_rfc3339_secs(now_unix_secs())
}

pub fn is_valid_canonical_id(s: &str) -> bool {
    if s.len() < 2 || s.len() > 128 {
        return false;
    }
    let bytes = s.as_bytes();
    if !matches!(bytes[0], b'a'..=b'z' | b'0'..=b'9') {
        return false;
    }
    bytes
        .iter()
        .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
}

pub fn is_valid_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 36 {
        return false;
    }
    for (i, &ch) in b.iter().enumerate() {
        if matches!(i, 8 | 13 | 18 | 23) {
            if ch != b'-' {
                return false;
            }
        } else if !ch.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

/// Pure-Rust SHA-256 digest returning 32 raw bytes.
pub fn sha256_bytes(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let bit_len = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for (idx, word) in h.iter().enumerate() {
        out[idx * 4..idx * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// Pure-Rust SHA-256 hex digest helper.
pub fn sha256_hex(data: &[u8]) -> String {
    let bytes = sha256_bytes(data);
    let mut out = String::with_capacity(64);
    for b in bytes {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

/// Deterministic RFC 4122 v5-shaped UUID derived from SHA-256 of `seed`,
/// ensuring idempotent re-imports produce identical `observation_id`s.
pub fn deterministic_uuid_from_seed(seed: &str) -> String {
    let digest = sha256_bytes(seed.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}
