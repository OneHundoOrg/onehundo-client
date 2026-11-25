use crate::core::types::{
    now_unix_secs, AcceptanceStatus, EventKind, EvidenceClass, ProgressionMode, ReceiptStatus,
    TrustTier, VerificationState,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEventInput {
    pub producer_id: String,
    pub session_id: String,
    pub sequence_number: i64,
    pub idempotency_key: String,
    pub canonical_game_id: String,
    pub event_kind: EventKind,
    pub key: String,
    pub int_value: Option<i64>,
    pub float_value: Option<f64>,
    pub string_value: Option<String>,
    pub occurred_at: String,
    pub progression_mode: ProgressionMode,
    pub trust_tier: TrustTier,
    pub evidence_class: EvidenceClass,
    pub verification_state: VerificationState,
    pub raw_payload: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingSyncRow {
    pub id: i64,
    pub idempotency_key: String,
    pub canonical_game_id: String,
    pub producer_id: String,
    pub session_id: String,
    pub sequence_number: i64,
    pub event_kind: String,
    pub key: String,
    pub int_value: Option<i64>,
    pub float_value: Option<f64>,
    pub string_value: Option<String>,
    pub occurred_at: String,
    pub progression_mode: String,
    pub trust_tier: String,
    pub evidence_class: String,
    pub verification_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalStats {
    pub total_events: u64,
    pub pending_sync_count: u64,
    pub synced_count: u64,
    pub rejected_count: u64,
    pub total_frame_gaps: u64,
    pub observation_count: u64,
    pub last_receipt_status: Option<String>,
    pub last_acceptance_status: Option<String>,
}

pub fn open_and_init_db(db_path: &Path) -> Result<Connection, String> {
    if let Some(parent) = db_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                format!("failed to create DB directory '{}': {}", parent.display(), e)
            })?;
        }
    }
    let conn = Connection::open(db_path)
        .map_err(|e| format!("failed to open SQLite '{}': {}", db_path.display(), e))?;
    conn.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = FULL;
        PRAGMA foreign_keys = ON;
        PRAGMA busy_timeout = 5000;

        CREATE TABLE IF NOT EXISTS event_journal (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            producer_id         TEXT NOT NULL,
            session_id          TEXT NOT NULL,
            sequence_number     INTEGER NOT NULL,
            idempotency_key     TEXT NOT NULL,
            canonical_game_id   TEXT NOT NULL,
            event_kind          TEXT NOT NULL,
            key                 TEXT NOT NULL,
            int_value           INTEGER,
            float_value         REAL,
            string_value        TEXT,
            occurred_at         TEXT NOT NULL,
            progression_mode    TEXT NOT NULL,
            trust_tier          TEXT NOT NULL,
            evidence_class      TEXT NOT NULL,
            verification_state  TEXT NOT NULL,
            raw_payload         TEXT NOT NULL,
            sync_state          TEXT NOT NULL DEFAULT 'PENDING',
            receipt_status      TEXT,
            acceptance_status   TEXT,
            rejection_reason    TEXT,
            synced_at           INTEGER,
            created_at          INTEGER NOT NULL,
            UNIQUE(producer_id, session_id, sequence_number)
        );

        CREATE INDEX IF NOT EXISTS idx_event_journal_sync
            ON event_journal (sync_state, id);

        CREATE TABLE IF NOT EXISTS stat_state (
            canonical_game_id   TEXT NOT NULL,
            progression_mode    TEXT NOT NULL,
            stat_key            TEXT NOT NULL,
            int_value           INTEGER,
            float_value         REAL,
            string_value        TEXT,
            updated_at          INTEGER NOT NULL,
            PRIMARY KEY (canonical_game_id, progression_mode, stat_key)
        );

        CREATE TABLE IF NOT EXISTS trophy_state (
            canonical_game_id        TEXT NOT NULL,
            canonical_achievement_id TEXT NOT NULL,
            progression_mode         TEXT NOT NULL,
            first_unlocked_at        TEXT NOT NULL,
            last_unlocked_at         TEXT NOT NULL,
            replay_count             INTEGER NOT NULL DEFAULT 1,
            trust_tier               TEXT NOT NULL,
            evidence_class           TEXT NOT NULL,
            verification_state       TEXT NOT NULL,
            PRIMARY KEY (canonical_game_id, canonical_achievement_id, progression_mode)
        );

        CREATE TABLE IF NOT EXISTS encore_runs (
            id                       INTEGER PRIMARY KEY AUTOINCREMENT,
            canonical_game_id        TEXT NOT NULL,
            canonical_achievement_id TEXT NOT NULL,
            session_id               TEXT NOT NULL,
            unlocked_at              TEXT NOT NULL,
            trust_tier               TEXT NOT NULL,
            evidence_class           TEXT NOT NULL,
            verification_state       TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS frame_gap_events (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id          TEXT NOT NULL,
            producer_id         TEXT NOT NULL,
            canonical_game_id   TEXT NOT NULL,
            frame_gap_count     INTEGER NOT NULL,
            sequence_number     INTEGER NOT NULL,
            reason              TEXT NOT NULL,
            created_at          INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS observation_journal (
            observation_id       TEXT PRIMARY KEY,
            canonical_game_id    TEXT NOT NULL,
            observer_id          TEXT NOT NULL,
            source_type          TEXT NOT NULL,
            observed_at          TEXT NOT NULL,
            source_timestamp     TEXT,
            artifact_hash_sha256 TEXT,
            record_kind          TEXT NOT NULL,
            key                  TEXT NOT NULL,
            unlocked             INTEGER,
            int_value            INTEGER,
            float_value          REAL,
            string_value         TEXT,
            progression_mode     TEXT NOT NULL,
            trust_tier           TEXT NOT NULL,
            evidence_class       TEXT NOT NULL,
            verification_state   TEXT NOT NULL,
            provenance_notes     TEXT,
            raw_json             TEXT NOT NULL,
            created_at           INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS plane_a_save_state (
            canonical_game_id   TEXT NOT NULL,
            session_id          TEXT NOT NULL,
            state_json          TEXT NOT NULL,
            updated_at          INTEGER NOT NULL,
            PRIMARY KEY (canonical_game_id, session_id)
        );
        ",
    )
    .map_err(|e| format!("failed to initialize SQLite schema: {}", e))?;

    Ok(conn)
}

pub fn insert_journal_event_tx(
    tx: &rusqlite::Transaction<'_>,
    ev: &JournalEventInput,
) -> Result<bool, String> {
    let now = now_unix_secs();
    let rows = tx
        .execute(
            "INSERT OR IGNORE INTO event_journal (
                producer_id, session_id, sequence_number, idempotency_key,
                canonical_game_id, event_kind, key, int_value, float_value, string_value,
                occurred_at, progression_mode, trust_tier, evidence_class, verification_state,
                raw_payload, sync_state, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 'PENDING', ?17)",
            params![
                ev.producer_id,
                ev.session_id,
                ev.sequence_number,
                ev.idempotency_key,
                ev.canonical_game_id,
                ev.event_kind.as_str(),
                ev.key,
                ev.int_value,
                ev.float_value,
                ev.string_value,
                ev.occurred_at,
                ev.progression_mode.as_str(),
                ev.trust_tier.as_str(),
                ev.evidence_class.as_str(),
                ev.verification_state.as_str(),
                ev.raw_payload,
                now
            ],
        )
        .map_err(|e| format!("failed inserting event_journal row: {}", e))?;
    Ok(rows > 0)
}

pub fn record_frame_gap_db(
    conn: &Connection,
    session_id: &str,
    producer_id: &str,
    canonical_game_id: &str,
    frame_gap_count: u64,
    sequence_number: u64,
    reason: &str,
) -> Result<(), String> {
    let now = now_unix_secs();
    conn.execute(
        "INSERT INTO frame_gap_events
         (session_id, producer_id, canonical_game_id, frame_gap_count, sequence_number, reason, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            session_id,
            producer_id,
            canonical_game_id,
            frame_gap_count as i64,
            sequence_number as i64,
            reason,
            now
        ],
    )
    .map_err(|e| format!("failed inserting frame_gap_events: {}", e))?;
    Ok(())
}

pub fn fetch_pending_sync_events(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<PendingSyncRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, idempotency_key, canonical_game_id, producer_id, session_id,
                    sequence_number, event_kind, key, int_value, float_value, string_value,
                    occurred_at, progression_mode, trust_tier, evidence_class, verification_state
             FROM event_journal
             WHERE sync_state = 'PENDING'
             ORDER BY id ASC
             LIMIT ?1",
        )
        .map_err(|e| format!("failed preparing outbox query: {}", e))?;

    let rows = stmt
        .query_map(params![limit as i64], |r| {
            Ok(PendingSyncRow {
                id: r.get(0)?,
                idempotency_key: r.get(1)?,
                canonical_game_id: r.get(2)?,
                producer_id: r.get(3)?,
                session_id: r.get(4)?,
                sequence_number: r.get(5)?,
                event_kind: r.get(6)?,
                key: r.get(7)?,
                int_value: r.get(8)?,
                float_value: r.get(9)?,
                string_value: r.get(10)?,
                occurred_at: r.get(11)?,
                progression_mode: r.get(12)?,
                trust_tier: r.get(13)?,
                evidence_class: r.get(14)?,
                verification_state: r.get(15)?,
            })
        })
        .map_err(|e| format!("failed executing outbox query: {}", e))?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("outbox row decode error: {}", e))?);
    }
    Ok(out)
}

/// Mark a local journal row synced ONLY after receiving HTTP 200 with durable receipt_status.
/// Preserves orthogonal receipt_status (`RECEIVED` | `DUPLICATE`) and acceptance_status (`ACCEPTED` | `PENDING_REVIEW` | `REJECTED`).
pub fn mark_event_sync_outcome(
    conn: &mut Connection,
    outcomes: &[(String, ReceiptStatus, AcceptanceStatus, VerificationState, Option<String>)],
) -> Result<(), String> {
    let now = now_unix_secs();
    let tx = conn
        .transaction()
        .map_err(|e| format!("failed opening sync outcome tx: {}", e))?;
    {
        let mut stmt = tx
            .prepare(
                "UPDATE event_journal
                 SET sync_state = 'SYNCED',
                     receipt_status = ?2,
                     acceptance_status = ?3,
                     verification_state = ?4,
                     rejection_reason = ?5,
                     synced_at = ?6
                 WHERE idempotency_key = ?1",
            )
            .map_err(|e| format!("failed preparing sync outcome update: {}", e))?;

        for (idemp, receipt, acceptance, ver_state, reason) in outcomes {
            stmt.execute(params![
                idemp,
                receipt.as_str(),
                acceptance.as_str(),
                ver_state.as_str(),
                reason,
                now
            ])
            .map_err(|e| format!("failed updating sync outcome: {}", e))?;
        }
    }
    tx.commit()
        .map_err(|e| format!("failed committing sync outcome tx: {}", e))?;
    Ok(())
}

pub fn query_journal_stats(conn: &Connection) -> Result<JournalStats, String> {
    let total_events: u64 = conn
        .query_row("SELECT COUNT(*) FROM event_journal", [], |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())? as u64;
    let pending_sync_count: u64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_journal WHERE sync_state = 'PENDING'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map_err(|e| e.to_string())? as u64;
    let synced_count: u64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_journal WHERE sync_state = 'SYNCED'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map_err(|e| e.to_string())? as u64;
    let rejected_count: u64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_journal WHERE acceptance_status = 'REJECTED'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map_err(|e| e.to_string())? as u64;
    let total_frame_gaps: u64 = conn
        .query_row(
            "SELECT COALESCE(SUM(frame_gap_count), 0) FROM frame_gap_events",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map_err(|e| e.to_string())? as u64;
    let observation_count: u64 = conn
        .query_row("SELECT COUNT(*) FROM observation_journal", [], |r| {
            r.get::<_, i64>(0)
        })
        .map_err(|e| e.to_string())? as u64;

    let last_receipt_status: Option<String> = conn
        .query_row(
            "SELECT receipt_status FROM event_journal WHERE receipt_status IS NOT NULL ORDER BY synced_at DESC, id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let last_acceptance_status: Option<String> = conn
        .query_row(
            "SELECT acceptance_status FROM event_journal WHERE acceptance_status IS NOT NULL ORDER BY synced_at DESC, id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    Ok(JournalStats {
        total_events,
        pending_sync_count,
        synced_count,
        rejected_count,
        total_frame_gaps,
        observation_count,
        last_receipt_status,
        last_acceptance_status,
    })
}
