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
}

impl FairPlayViolation {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SaveStateLoad => "save_state_load",
            Self::CheatToggle => "cheat_toggle",
            Self::MemoryPoke => "memory_poke",
            Self::DebugConsoleOverride => "debug_console_override",
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
