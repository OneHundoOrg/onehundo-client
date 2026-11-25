# `onehundo-client` — Repository Context & Phase State

> **Repository:** [`onehundo-client`](https://github.com/OneHundoOrg/onehundo-client) (Public, dual `MIT OR Apache-2.0`)  
> **Organization:** `OneHundoOrg` (`onehundo.org`)  
> **Upstream Contracts:** [`onehundo-protocol`](https://github.com/OneHundoOrg/onehundo-protocol) (`v0.1.0`, Public, dual `MIT OR Apache-2.0`)  
> **Cloud Target:** [`onehundo-platform`](https://github.com/OneHundoOrg/onehundo-platform) (Private, Proprietary; Zerops staging at `https://app-3bb4-3000.prg1.zerops.app`)  
> **Current Phase:** Post-Phase 1.5 Baseline Complete — Ready for **Phase 2 (Plane A Shared-Memory Cross-Runtime Proof)**

---

## 1. Completed Baseline (Phase 1 & Phase 1.5)

### [x] Phase 1 — Plane B End-to-End Runtime & Cloud Sync
- `onehundod` (`src/main.rs`, `daemon/core/engine.rs`): Unix socket IPC server, Commit-Before-Ack SQLite WAL journal (`PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;`), `event_journal` deduplication on `(producer_id, session_id, sequence_number)`, and materialized `trophy_state` / `stat_state`.
- Mode & trust isolation: `STANDARD`, `ENCORE`, and `SPECTATOR` progression modes (`SPECTATOR` never mutates persistent state; `ENCORE` preserves `first_unlocked_at`), plus fair-play downgrades from `VERIFIED_INTEGRATION` to `LOCAL` (`UNVERIFIED`).
- Cloud sync client (`daemon/sync/client.rs`, `daemon/sync/http.rs`): `POST /v1/devices/register`, `GET /v1/packs/resolve`, and `POST /v1/sync/events` keeping `receipt_status` (`RECEIVED` | `DUPLICATE`) orthogonal to `acceptance_status` (`ACCEPTED` | `PENDING_REVIEW` | `REJECTED`), with `0600` credential storage in `~/.config/onehundo/`.
- Plane B evaluation (`daemon/evaluation/semantic-events/mod.rs`) and `bbport` integration (`integrations/native-ports/bbport/bbport_live_demo.c`, `integrations/native-ports/quick/onehundo_quick.h`) verified end-to-end.

### [x] Phase 1.5 — Service Persistence, Daemon Hygiene & Bloodborne Pack Parity
- **Socket discovery & lockfile:** `$ONEHUNDO_SOCKET` / `$XDG_RUNTIME_DIR/onehundo.sock` discovery (before `/tmp/onehundo.sock` fallback) paired with `flock`-based single-instance `InstanceLock` (`daemon/core/lockfile.rs`, `$XDG_RUNTIME_DIR/onehundod.lock`) preventing port `19850` `EADDRINUSE` conflicts.
- **Graceful shutdown & desktop CLI:** `SIGINT`/`SIGTERM` signal handlers + `POST /shutdown` endpoint in `daemon/diagnostics/server.rs`; `onehundo-desktop` (`apps/desktop/main.rs`) subcommands `start`, `stop`, `status`, `health`, `packs`, `import-obs`, `set-api-key`, and `sync-now`.
- **Persistent service & binaries:** Checked-in `deploy/systemd/onehundod.service` (`~/.config/systemd/user/onehundod.service`) targeting `ONEHUNDO_PLATFORM_URL=https://app-3bb4-3000.prg1.zerops.app`, with release binaries installed to `~/.local/bin` and 2.5s background auto-sync verified live with `bbport` gameplay (`Cleric Beast` unlock -> OS toast -> cloud sync -> Zerops profile).
- **Bloodborne pack parity:** `packs/sony.bloodborne.bbport.onehundo` (`pack_id`: `sony.bloodborne.bbport.pc`, `v0.7.0`, SHA-256 `6f35c670aef7f6101f571785e9c6e4d7334d17c02d418ee421d6a00c45301603`) aligned to Bloodborne's 34 base PSN trophies (`"0"`..`"33"`, with `a35`–`a40` / `d01`–`d06` provisional pending Old Hunters DLC in OAP).
- **Plane A scaffolding:** `daemon/ipc/shm_arena.rs` (POSIX `shm_open` + `mmap` ring consumer enforcing `FREE -> WRITING -> READY -> READING -> FREE`), `daemon/evaluation/memory-arena/mod.rs`, `integrations/emulators/plane_a_emulator_harness.c`, and `tests/plane_a_evaluator_and_gaps.rs`.

---

## 2. Phase 2 Scope & File Ownership (Next)

**Objective:** Prove that a single `.onehundo` memory-condition pack runs identically across both a classic console emulator and a flat-memory static recompilation runtime with zero game-thread blocking.

### Target Modules
- **SHM Ring & Daemon Loop (`daemon/ipc/shm_arena.rs`, `daemon/core/engine.rs`, `src/main.rs`):** Connect live `ARENA_REGISTER` sessions to continuous `shm_open` ring slot consumption (`FREE -> WRITING -> READY -> READING -> FREE` via acquire/release atomics).
- **Evaluator (`daemon/evaluation/memory-arena/mod.rs`):** Integrate real `rcheevos` rule evaluation over normalized guest memory snapshots, handle `SAVE_STATE_LOAD` / `SOFT_RESET` / `HARD_RESET` lifecycle signals cleanly, and emit `RUNTIME_OBSERVED` `UNLOCK_REQUEST` events to the SQLite journal & sync outbox.
- **Diagnostics (`daemon/diagnostics/server.rs`, `ui/game-health/mod.rs`):** Surface `FRAME_GAP` counters, ring latency, and `evaluation_incomplete` status in `http://127.0.0.1:19850/health`.
- **Dual-Runtime Harness (`integrations/emulators/plane_a_emulator_harness.c`, `integrations/static-recompilations/`):** Execute the exact same `.onehundo` memory pack against both an emulator bridge (`RETRO_MEMORY_SYSTEM_RAM`) and a flat-memory static recompilation target, verifying identical unlocks and zero frame stalls.

### Hard Boundaries
- Never fork `onehundo-protocol` headers or schemas locally; consume `v0.1.0` contracts from `onehundo-protocol`.
- Keep `daemon/observation/external-importers/` strictly isolated from `daemon/evaluation/` (`tests/plane_c_isolation.rs`).
- Never block game threads on IPC/HTTP I/O; never commit copyrighted ROMs, BIOSes, or game assets.

---

## 3. Verification Commands

```bash
# Full Rust test suite (offline)
cargo test --offline

# Clippy lint check
cargo clippy --all-targets

# Strict ISO C99 compile checks for reference/integration examples
gcc -std=c99 -pedantic -Wall -Wextra -Werror -I../onehundo-protocol/include \
  -fsyntax-only integrations/native-ports/quick/onehundo_quick.h
```
