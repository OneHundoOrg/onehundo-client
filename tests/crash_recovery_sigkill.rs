mod common;

use onehundo_client::ipc::wire::{MSG_EVENT_ACK, MSG_EVENT_PUBLISH, MSG_GOODBYE, MSG_HELLO, MSG_HELLO_ACK};
use onehundo_client::storage::journal::{open_and_init_db, query_journal_stats};
use onehundo_client::storage::state::{list_stats, list_trophies};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

fn spawn_daemon(bin: &PathBuf, dir: &PathBuf, sock: &PathBuf, pack: &PathBuf) -> Child {
    let child = Command::new(bin)
        .args([
            "--socket",
            &sock.display().to_string(),
            "--config-dir",
            &dir.join("cfg").display().to_string(),
            "--data-dir",
            &dir.join("data").display().to_string(),
            "--pack",
            &pack.display().to_string(),
            "--health-addr",
            "none",
            "--no-notify",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn onehundod");

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if sock.exists() && UnixStream::connect(sock).is_ok() {
            return child;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("onehundod failed to listen on {}", sock.display());
}

#[test]
fn sigkill_crash_recovery_commit_before_ack_and_replay_idempotency() {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_onehundod"));
    let dir = common::temp_test_dir("sigkill");
    let sock = dir.join("onehundo.sock");
    let pack = common::protocol_root().join("conformance/fixtures/packs/bbport_sample.onehundo");

    let mut child = spawn_daemon(&bin, &dir, &sock, &pack);
    let session_id = "550e8400-e29b-41d4-a716-446655440099";

    {
        let mut stream = UnixStream::connect(&sock).expect("connect to daemon");
        let (h_ack, _) = common::send_and_recv(
            &mut stream,
            MSG_HELLO,
            0,
            &format!(
                r#"{{"canonical_game_id":"sony.bloodborne","producer_id":"bbport_pc_x64","session_id":"{}","plane":2,"progression_mode":"STANDARD","trust_tier":"VERIFIED_INTEGRATION"}}"#,
                session_id
            ),
        );
        assert_eq!(h_ack.msg_type, MSG_HELLO_ACK);

        // Emit seq=1 STAT_INC and seq=2 UNLOCK_REQUEST and wait for EVENT_ACK
        let (ack1, _) = common::send_and_recv(
            &mut stream,
            MSG_EVENT_PUBLISH,
            1,
            &format!(
                r#"{{"event_type":"STAT_INC","canonical_game_id":"sony.bloodborne","producer_id":"bbport_pc_x64","session_id":"{}","sequence_number":1,"key":"bosses_defeated","int_value":1}}"#,
                session_id
            ),
        );
        assert_eq!(ack1.msg_type, MSG_EVENT_ACK);
        assert_eq!(ack1.sequence_number, 1);

        let (ack2, ack2_body) = common::send_and_recv(
            &mut stream,
            MSG_EVENT_PUBLISH,
            2,
            &format!(
                r#"{{"event_type":"UNLOCK_REQUEST","canonical_game_id":"sony.bloodborne","producer_id":"bbport_pc_x64","session_id":"{}","sequence_number":2,"key":"ach_cleric_beast","int_value":1}}"#,
                session_id
            ),
        );
        assert_eq!(ack2.msg_type, MSG_EVENT_ACK);
        assert_eq!(ack2.sequence_number, 2);
        assert!(ack2_body.contains("ach_cleric_beast"));
    }

    // Hard SIGKILL (kill -9) immediately after receiving EVENT_ACK
    let _ = child.kill();
    let _ = child.wait();

    // Restart daemon against same SQLite WAL data dir
    let mut child2 = spawn_daemon(&bin, &dir, &sock, &pack);
    {
        let mut stream = UnixStream::connect(&sock).expect("reconnect after SIGKILL");
        let (h_ack, _) = common::send_and_recv(
            &mut stream,
            MSG_HELLO,
            0,
            &format!(
                r#"{{"canonical_game_id":"sony.bloodborne","producer_id":"bbport_pc_x64","session_id":"{}","plane":2,"progression_mode":"STANDARD","trust_tier":"VERIFIED_INTEGRATION"}}"#,
                session_id
            ),
        );
        assert_eq!(h_ack.msg_type, MSG_HELLO_ACK);

        // Replay seq=1 (must deduplicate cleanly without double-incrementing bosses_defeated!)
        let (r_ack1, r_body1) = common::send_and_recv(
            &mut stream,
            MSG_EVENT_PUBLISH,
            1,
            &format!(
                r#"{{"event_type":"STAT_INC","canonical_game_id":"sony.bloodborne","producer_id":"bbport_pc_x64","session_id":"{}","sequence_number":1,"key":"bosses_defeated","int_value":1}}"#,
                session_id
            ),
        );
        assert_eq!(r_ack1.msg_type, MSG_EVENT_ACK);
        assert!(r_body1.contains("\"deduplicated\":true"), "body={}", r_body1);

        // Send new seq=3
        let (ack3, _) = common::send_and_recv(
            &mut stream,
            MSG_EVENT_PUBLISH,
            3,
            &format!(
                r#"{{"event_type":"STAT_INC","canonical_game_id":"sony.bloodborne","producer_id":"bbport_pc_x64","session_id":"{}","sequence_number":3,"key":"bosses_defeated","int_value":1}}"#,
                session_id
            ),
        );
        assert_eq!(ack3.msg_type, MSG_EVENT_ACK);
        assert_eq!(ack3.sequence_number, 3);

        let _ = onehundo_client::ipc::wire::write_wire_frame(&mut stream, MSG_GOODBYE, 0, &[]);
    }

    let _ = child2.kill();
    let _ = child2.wait();

    // Verify SQLite WAL state directly: 3 journal rows total, bosses_defeated == 2 (not 3!), ach_cleric_beast unlocked
    let db = open_and_init_db(&dir.join("data/onehundo_state.db")).expect("open db");
    let stats = query_journal_stats(&db).expect("journal stats");
    assert_eq!(stats.total_events, 3);

    let stat_rows = list_stats(&db, Some("sony.bloodborne")).expect("list stats");
    let bosses = stat_rows.iter().find(|s| s.stat_key == "bosses_defeated").unwrap();
    assert_eq!(bosses.int_value, Some(2));

    let trophies = list_trophies(&db, Some("sony.bloodborne")).expect("list trophies");
    assert_eq!(trophies.len(), 1);
    assert_eq!(trophies[0].canonical_achievement_id, "ach_cleric_beast");
    assert_eq!(trophies[0].verification_state, "VERIFIED");

    let _ = std::fs::remove_dir_all(&dir);
}
