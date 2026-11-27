use crate::core::engine::DaemonEngine;
use crate::core::types::{
    FairPlayViolation, IngestionPlane, LifecycleSignal, ProgressionMode, TrustTier,
};
use crate::evaluation::semantic_events::SemanticEventPayload;
use crate::ipc::shm_arena::ShmArenaConsumer;
use crate::ipc::wire::{
    read_wire_frame, write_wire_frame, MSG_ARENA_ACK, MSG_ARENA_REGISTER, MSG_EVENT_ACK,
    MSG_EVENT_PUBLISH, MSG_GOODBYE, MSG_HELLO, MSG_HELLO_ACK, MSG_LIFECYCLE_SIGNAL,
};
use crate::packs::matcher::MatchQuery;
use serde::Deserialize;
use std::fs;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct HelloPayload {
    canonical_game_id: String,
    producer_id: String,
    #[serde(default)]
    session_id: String,
    #[serde(default = "default_plane")]
    plane: u32,
    #[serde(default)]
    progression_mode: Option<serde_json::Value>,
    #[serde(default)]
    trust_tier: Option<serde_json::Value>,
    #[serde(default)]
    sidecar_uuid: Option<String>,
    #[serde(default)]
    rom_hash: Option<String>,
    #[serde(default)]
    recomp_game_id: Option<String>,
    #[serde(default)]
    build_id: Option<String>,
    #[serde(default)]
    platform_id: Option<String>,
    #[serde(default)]
    region: Option<String>,
}

fn default_plane() -> u32 {
    2
}

#[derive(Debug, Deserialize)]
struct ArenaRegisterPayload {
    shm_name: String,
    #[serde(default)]
    region_id: u32,
    #[serde(default)]
    offset_in_arena: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct LifecyclePayload {
    signal: serde_json::Value,
    #[serde(default)]
    frame_gap_count: u32,
    #[serde(default)]
    cheat_toggle: Option<bool>,
}

fn parse_mode(v: Option<&serde_json::Value>) -> ProgressionMode {
    match v {
        Some(serde_json::Value::Number(n)) => n
            .as_u64()
            .and_then(|u| ProgressionMode::from_wire_u32(u as u32))
            .unwrap_or(ProgressionMode::Standard),
        Some(serde_json::Value::String(s)) => {
            ProgressionMode::from_str_strict(s).unwrap_or(ProgressionMode::Standard)
        }
        _ => ProgressionMode::Standard,
    }
}

fn parse_trust(v: Option<&serde_json::Value>) -> TrustTier {
    match v {
        Some(serde_json::Value::Number(n)) => n
            .as_u64()
            .and_then(|u| TrustTier::from_wire_u32(u as u32))
            .unwrap_or(TrustTier::Local),
        Some(serde_json::Value::String(s)) => {
            TrustTier::from_str_strict(s).unwrap_or(TrustTier::Local)
        }
        _ => TrustTier::Local,
    }
}

fn parse_signal(v: &serde_json::Value) -> Option<LifecycleSignal> {
    match v {
        serde_json::Value::Number(n) => n
            .as_u64()
            .and_then(|u| LifecycleSignal::from_wire_u32(u as u32)),
        serde_json::Value::String(s) => LifecycleSignal::from_str_strict(s),
        _ => None,
    }
}

pub fn start_unix_ipc_server(
    engine: Arc<DaemonEngine>,
    socket_path: &Path,
    shutdown: Arc<AtomicBool>,
) -> Result<thread::JoinHandle<()>, String> {
    if let Some(parent) = socket_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::remove_file(socket_path);

    let listener = UnixListener::bind(socket_path)
        .map_err(|e| format!("failed binding Unix socket '{}': {}", socket_path.display(), e))?;
    // Owner-only, whatever the umask: connecting needs write permission on the socket file, so
    // other local users (notably under the shared /tmp fallback) can't submit events or read state.
    restrict_socket_permissions(socket_path)?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("failed setting nonblocking on UnixListener: {}", e))?;

    let handle = thread::spawn(move || {
        while !shutdown.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let eng = Arc::clone(&engine);
                    let sd = Arc::clone(&shutdown);
                    thread::spawn(move || handle_ipc_client(stream, eng, sd));
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break,
            }
        }
    });

    Ok(handle)
}

/// Sets the bound socket file to `0600`. Fails closed: a socket we can't restrict isn't served.
pub fn restrict_socket_permissions(socket_path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600)).map_err(|e| {
        let _ = fs::remove_file(socket_path);
        format!(
            "failed restricting Unix socket '{}' to 0600: {}",
            socket_path.display(),
            e
        )
    })
}

fn handle_ipc_client(
    mut stream: UnixStream,
    engine: Arc<DaemonEngine>,
    daemon_shutdown: Arc<AtomicBool>,
) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));

    let mut reader = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };

    let mut active_session_id: Option<String> = None;
    let client_stop = Arc::new(AtomicBool::new(false));
    let mut shm_thread: Option<thread::JoinHandle<()>> = None;

    while !daemon_shutdown.load(Ordering::Relaxed) && !client_stop.load(Ordering::Relaxed) {
        let frame = match read_wire_frame(&mut reader) {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(ref e)
                if e.contains("WouldBlock")
                    || e.contains("TimedOut")
                    || e.contains("Resource temporarily unavailable") =>
            {
                continue;
            }
            Err(e) => {
                eprintln!("[onehundod-ipc] closing client on frame error: {}", e);
                break;
            }
        };

        let (hdr, payload_bytes) = frame;
        let payload_str = String::from_utf8_lossy(&payload_bytes);

        match hdr.msg_type {
            MSG_HELLO => {
                if let Ok(hello) = serde_json::from_str::<HelloPayload>(&payload_str) {
                    let plane = IngestionPlane::from_wire_u32(hello.plane)
                        .unwrap_or(IngestionPlane::SemanticEvent);
                    let mode = parse_mode(hello.progression_mode.as_ref());
                    let trust = parse_trust(hello.trust_tier.as_ref());
                    let query = MatchQuery {
                        canonical_game_id: Some(hello.canonical_game_id.clone()),
                        sidecar_uuid: hello.sidecar_uuid,
                        rom_hash: hello.rom_hash,
                        recomp_game_id: hello.recomp_game_id,
                        build_id: hello.build_id,
                        asset_archive_hash: None,
                        platform_id: hello.platform_id,
                        region: hello.region,
                    };
                    let (info, matched) = engine.start_session(
                        hello.session_id,
                        hello.producer_id,
                        hello.canonical_game_id,
                        query,
                        plane,
                        mode,
                        trust,
                    );
                    active_session_id = Some(info.session_id.clone());
                    let ack_json = serde_json::json!({
                        "daemon_version": crate::core::types::PROTOCOL_VERSION,
                        "session_id": info.session_id,
                        "matched_pack_id": matched.map(|m| m.pack_id),
                        "progression_mode": info.progression_mode.as_str(),
                        "trust_tier": info.active_trust_tier.as_str(),
                    })
                    .to_string();
                    let _ = write_wire_frame(&mut stream, MSG_HELLO_ACK, 0, ack_json.as_bytes());
                }
            }
            MSG_ARENA_REGISTER => {
                if let Ok(reg) = serde_json::from_str::<ArenaRegisterPayload>(&payload_str) {
                    let sid = active_session_id
                        .clone()
                        .unwrap_or_else(crate::core::config::generate_uuid_v4);
                    engine.register_plane_a_region(
                        &sid,
                        reg.region_id,
                        reg.offset_in_arena.unwrap_or(0),
                    );

                    // Spawn SHM polling consumer thread if not yet running for this connection
                    if shm_thread.is_none() {
                        match ShmArenaConsumer::open(&reg.shm_name) {
                            Ok(mut consumer) => {
                                let eng = Arc::clone(&engine);
                                let stop_flag = Arc::clone(&client_stop);
                                let d_stop = Arc::clone(&daemon_shutdown);
                                let session_for_shm = sid.clone();
                                let poll_ms = engine.config.shm_poll_interval_ms.max(1);
                                eng.update_plane_a_ring_telemetry(
                                    &session_for_shm,
                                    consumer.ring_stats(true),
                                );
                                shm_thread = Some(thread::spawn(move || {
                                    while !stop_flag.load(Ordering::Relaxed)
                                        && !d_stop.load(Ordering::Relaxed)
                                    {
                                        let mut processed_any = false;
                                        while let Some(snap) = consumer.poll_next_slot() {
                                            processed_any = true;
                                            let _ = eng.handle_plane_a_frame(
                                                &session_for_shm,
                                                snap.sequence_number,
                                                snap.lifecycle_signal,
                                                snap.frame_gap_count,
                                                &snap.payload,
                                            );
                                        }
                                        if processed_any {
                                            eng.update_plane_a_ring_telemetry(
                                                &session_for_shm,
                                                consumer.ring_stats(true),
                                            );
                                        } else {
                                            thread::sleep(Duration::from_millis(poll_ms));
                                        }
                                    }
                                    // Drain any remaining READY slots before exiting on disconnect/GOODBYE
                                    while let Some(snap) = consumer.poll_next_slot() {
                                        let _ = eng.handle_plane_a_frame(
                                            &session_for_shm,
                                            snap.sequence_number,
                                            snap.lifecycle_signal,
                                            snap.frame_gap_count,
                                            &snap.payload,
                                        );
                                    }
                                    eng.update_plane_a_ring_telemetry(
                                        &session_for_shm,
                                        consumer.ring_stats(false),
                                    );
                                }));
                            }
                            Err(e) => {
                                eprintln!("[onehundod-ipc] SHM open failed: {}", e);
                            }
                        }
                    }

                    let ack = serde_json::json!({
                        "status": "ok",
                        "region_id": reg.region_id,
                    })
                    .to_string();
                    let _ =
                        write_wire_frame(&mut stream, MSG_ARENA_ACK, hdr.sequence_number, ack.as_bytes());
                }
            }
            MSG_EVENT_PUBLISH => {
                match serde_json::from_str::<SemanticEventPayload>(&payload_str) {
                    Ok(ev) => {
                        let ack_seq = if hdr.sequence_number > 0 {
                            hdr.sequence_number
                        } else {
                            ev.sequence_number as u64
                        };
                        match engine.handle_plane_b_event(
                            active_session_id.as_deref(),
                            &ev,
                            &payload_str,
                        ) {
                            Ok(outcome) => {
                                // COMMIT-BEFORE-ACK: transaction is already committed to SQLite WAL+FULL
                                let ack_json = serde_json::json!({
                                    "status": "ok",
                                    "sequence_number": outcome.sequence_number,
                                    "deduplicated": outcome.deduplicated,
                                    "unlocked": outcome.newly_unlocked.iter().map(|u| &u.canonical_achievement_id).collect::<Vec<_>>(),
                                })
                                .to_string();
                                let _ = write_wire_frame(
                                    &mut stream,
                                    MSG_EVENT_ACK,
                                    ack_seq,
                                    ack_json.as_bytes(),
                                );
                            }
                            Err(e) => {
                                eprintln!("[onehundod-ipc] Plane B commit failed: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("[onehundod-ipc] malformed EVENT_PUBLISH JSON: {}", e);
                    }
                }
            }
            MSG_LIFECYCLE_SIGNAL => {
                if let Ok(lp) = serde_json::from_str::<LifecyclePayload>(&payload_str) {
                    if let Some(sid) = active_session_id.as_deref() {
                        if lp.cheat_toggle == Some(true) {
                            let _ = engine
                                .apply_fair_play_violation(sid, FairPlayViolation::CheatToggle);
                        } else if let Some(sig) = parse_signal(&lp.signal) {
                            let _ = engine.handle_lifecycle_signal(sid, sig, lp.frame_gap_count);
                        }
                    }
                }
            }
            MSG_GOODBYE => {
                break;
            }
            other => {
                eprintln!("[onehundod-ipc] ignoring unknown wire msg_type 0x{:04x}", other);
            }
        }
    }

    client_stop.store(true, Ordering::Relaxed);
    if let Some(h) = shm_thread {
        let _ = h.join();
    }
    if let Some(sid) = active_session_id {
        engine.end_session(&sid);
    }
}
