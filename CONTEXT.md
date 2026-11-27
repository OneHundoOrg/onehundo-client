# `onehundo-client` — Repository Context & Phase State

> **Repository:** [`onehundo-client`](https://github.com/OneHundoOrg/onehundo-client) (Public, dual `MIT OR Apache-2.0`)  
> **Organization:** `OneHundoOrg` (`onehundo.org`)  
> **Upstream Contracts:** [`onehundo-protocol`](https://github.com/OneHundoOrg/onehundo-protocol) (`v0.1.0`, Public, dual `MIT OR Apache-2.0`)  
> **Cloud Target:** [`onehundo-platform`](https://github.com/OneHundoOrg/onehundo-platform) (Private, Proprietary; canonical domain `https://onehundo.org`, Zerops origin `https://app-3bb4-3000.prg1.zerops.app`)  
> **Current Phase:** Phase 3 Complete — Ready for **Phase 4 (Community Authoring, Launch Integrations, Rarity & Competitive Lane)**

---

## 1. Completed Baseline (Phase 1 · Phase 1.5 · Phase 2 · Phase 3)

### [x] Phase 1 — Plane B End-to-End Runtime & Cloud Sync
- `onehundod` (`src/main.rs`, `daemon/core/engine.rs`): Unix socket IPC server, Commit-Before-Ack SQLite WAL journal (`PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;`), `event_journal` deduplication on `(producer_id, session_id, sequence_number)`, and materialized `trophy_state` / `stat_state`.
- Mode & trust isolation: `STANDARD`, `ENCORE`, and `SPECTATOR` progression modes (`SPECTATOR` never mutates persistent state; `ENCORE` preserves `first_unlocked_at`), plus fair-play downgrades from `VERIFIED_INTEGRATION` to `LOCAL` (`UNVERIFIED`).
- Cloud sync client (`daemon/sync/client.rs`, `daemon/sync/http.rs`): `POST /v1/devices/register`, `GET /v1/packs/resolve`, and `POST /v1/sync/events` keeping `receipt_status` (`RECEIVED` | `DUPLICATE`) orthogonal to `acceptance_status` (`ACCEPTED` | `PENDING_REVIEW` | `REJECTED`), with `0600` credential storage in `~/.config/onehundo/`.
- Plane B evaluation (`daemon/evaluation/semantic-events/mod.rs`) and `bbport` integration (`integrations/native-ports/bbport/bbport_live_demo.c`, `integrations/native-ports/quick/onehundo_quick.h`) verified end-to-end.

### [x] Phase 1.5 — Service Persistence, Daemon Hygiene & Bloodborne Pack Parity
- **Socket discovery & lockfile:** `$ONEHUNDO_SOCKET` / `$XDG_RUNTIME_DIR/onehundo.sock` discovery (before `/tmp/onehundo.sock` fallback) paired with `flock`-based single-instance `InstanceLock` (`daemon/core/lockfile.rs`, `$XDG_RUNTIME_DIR/onehundod.lock`) preventing port `19850` `EADDRINUSE` conflicts.
- **Graceful shutdown & desktop CLI:** `SIGINT`/`SIGTERM` signal handlers + `POST /shutdown` endpoint in `daemon/diagnostics/server.rs`; `onehundo-desktop` (`apps/desktop/main.rs`) subcommands `start`, `stop`, `status`, `health`, `packs`, `import-obs`, `notify-test`, `set-api-key`, and `sync-now`.
- **Persistent service & binaries:** Checked-in `deploy/systemd/onehundod.service` (`~/.config/systemd/user/onehundod.service`) targeting canonical `ONEHUNDO_PLATFORM_URL=https://onehundo.org`, with release binaries installed to `~/.local/bin` and 2.5s background auto-sync verified live with `bbport` gameplay (`Cleric Beast` unlock -> OS toast -> cloud sync -> platform profile).
- **Bloodborne pack parity:** `packs/sony.bloodborne.bbport.onehundo` (`pack_id`: `sony.bloodborne.bbport.pc`, `v0.7.0`, SHA-256 `6f35c670aef7f6101f571785e9c6e4d7334d17c02d418ee421d6a00c45301603`) aligned to Bloodborne's 34 base PSN trophies (`"0"`..`"33"`, with `a35`–`a40` / `d01`–`d06` provisional pending Old Hunters DLC in OAP).

### [x] Phase 2 — Plane A Shared-Memory Cross-Runtime Proof
- **Lock-free SHM Ring & Diagnostics (`daemon/ipc/shm_arena.rs`, `daemon/ipc/unix_socket.rs`, `daemon/diagnostics/mod.rs`, `daemon/core/engine.rs`, `ui/game-health/mod.rs`):** POSIX `shm_open` + `mmap` ring consumer enforcing `FREE -> WRITING -> READY -> READING -> FREE` with Acquire/Release atomics, IEEE CRC32 payload validation, zero game-thread blocking, `SlotOccupancy`/`ShmRingStats` polling telemetry, and merged SHM + IPC `FRAME_GAP` counters surfaced in `GET /health` and Game Health HTML.
- **`rcheevos`-Compatible Evaluator (`daemon/evaluation/memory-arena/mod.rs`, `daemon/packs/manifest.rs`):** Full RetroAchievements `memaddr` parser and evaluator (`Mem`, `Delta`, `Prior`, `Once`, `ResetIf`, `PauseIf`, `AddSource`, `SubSource`, `AddHits`, `SubHits`, `AndNext`, `OrNext`, `ResetNextIf`, `Measured`, `i8`/`i16`/`i32`/`u24`/`u32_be`/nibbles/`bitcount`/`bit0..7`, memory-vs-memory RHS) plus `memory_stats` (`STAT_SET` / `STAT_INC`), transactional SQLite WAL (`synchronous = FULL`) commits with `RUNTIME_OBSERVED` provenance, `SPECTATOR` non-mutation, and `VERIFIED_INTEGRATION` -> `LOCAL` downgrade on `SaveStateLoad`, cheats, and `FairPlayViolation::ExcessiveFrameGap` (`plane_a_max_frame_gaps_before_downgrade`, default `10`).
- **Dual-Runtime Portability Proof (`packs/onehundo.plane_a.cross_runtime.onehundo`, `integrations/emulators/plane_a_emulator_harness.c`, `integrations/static-recompilations/plane_a_recomp_harness.c`, `tests/plane_a_cross_runtime_proof.rs`):** Single `onehundo-validate`-verified `.onehundo` pack produces identical `RUNTIME_OBSERVED` unlocks and stats across both emulator (`rom_hash`) and static recompilation (`recomp_game_id` / `build_id`) harnesses, verified with ring-contention `FRAME_GAP` reporting, `SPECTATOR` non-mutation, `SAVE_STATE_LOAD` trust downgrade, and end-to-end `/v1/sync/events` cloud sync validation.

### [x] Phase 3 — Plane C External Observation & Desktop Experience
- **Isolated External Importers & Directory Watchers (`daemon/observation/external-importers/{mod,goldberg,trophy_dir,longshot,generic_export,watcher}.rs`):** Strictly isolated from `daemon/evaluation/` (`tests/plane_c_isolation.rs`), supporting Goldberg/GSE (`achievements.json`, `achievements.ini`), emulator trophy directories (`RPCS3` `TROPUSR.DAT` + `TROP.SFM`, `shadPS4` `TROP.XML` / `trophies.json`), **Longshot (`Ocarina of Time` Switch 2 -> PC) binary `progress.sav` (`0x01020304`) save/slot watcher** (supporting both `~/.local/share/longshot/save/` and Launcher 2 `~/.local/share/longshot/launcher/profiles/<profile>/saves/{save,save-work}/<uid>/slot_XX/progress.sav` at `MAX_SCAN_DEPTH = 8`) paired with `packs/nintendo.oot.longshot.pc.onehundo` (`pack_id`: `nintendo.oot.longshot.pc`, `v0.2.0`, SHA-256 `146242f8130275823b069b7b98d64de9a329501c83ffc50c6f64116cbff3ec08`, 13 milestones `a01`–`a13` starting with `"It works, it works!"` via `HavePlayedEvent.Dm_Kokiri_0030`, audited MurmurHash3/placed `Bool64bitKey` save rules, plus 3 stats `skulltulas_collected`, `heart_pieces`, `bosses_defeated`, mapped onto `nintendo-provisional-oot-switch2`), and generic `ObservationRecord` / save-export JSON; preserves missing/`0` unlock timestamps as SQL `NULL` (never invented), deduplicates idempotently in SQLite WAL (`synchronous = FULL`), tags records `LOCALLY_OBSERVED` / `LOCAL` / `UNVERIFIED`, enforces `SPECTATOR` non-mutation, and supports background watched directories (`watched_observation_dirs`, `watch_poll_interval_ms`, `--watch-dir`, `--watch-interval-ms`, `--mode <STANDARD|ENCORE|SPECTATOR>`).
- **Game Health Control Center & Pack Management (`daemon/diagnostics/{mod,server}.rs`, `ui/game-health/mod.rs`, `ui/pack-browser/mod.rs`, `ui/notifications/mod.rs`, `apps/desktop/main.rs`):** Upgraded `http://127.0.0.1:19850` (`GET /`, `GET /ui/packs`, `GET /health`, `GET /packs`, `POST /notify-test`, `POST /sync-now`, `POST /session/mode`, `POST /v1/observation/import`) using `onehundo-platform/DESIGN.md` dark Swiss/Grid tokens and WCAG 2.2 AAA semantics (zero secrets exposed), surfacing active mode/trust/downgrade reasons, `shm_frame_gaps_reported` / `ipc_frame_gaps_reported`, installed `.onehundo` packs with SHA-256, mapped OAP edition, per-achievement status (`Unlocked`, `Observed · SPECTATOR`, `Locked`) and stats (`render_pack_browser_with_state_html`), Plane C import history (`null (not invented)` badges), non-zombie `notify-send` dispatch, and interactive `onehundo-desktop` commands (`packs`, `mode`, `import-obs --game-id --mode --dry-run --url`, `notify-test [--game-id --game --title --body --url --json]`).

---

## 2. Phase 4 Scope & File Ownership (Next)

**Objective:** Enable low-friction community `.onehundo` pack authoring, ship launch integrations (RetroArch, LaunchBox, Playnite, flagship recompilations), and add Ed25519 signature verification + `COMPETITIVE` trust-lane enforcement.

### Target Modules
- **Pack Security & Verification (`daemon/packs/`):** Ed25519 signature verification (`signature_key_id` + `pack_sha256`), remote `GET /v1/packs/resolve` auto-download/cache, and `COMPETITIVE` reproducible-build/frame-gap checks.
- **Creator & Frontend Integrations (`integrations/`, `daemon/diagnostics/server.rs`, `apps/desktop/main.rs`):** Save-diff & named-state authoring helpers, libretro/RetroArch Plane A bridge, and LaunchBox / Playnite read-only shelf polling against `127.0.0.1:19850`.

### Hard Boundaries
- Never fork `onehundo-protocol` headers or schemas locally; consume `v0.1.0+` contracts from `onehundo-protocol`.
- Keep `daemon/observation/external-importers/` strictly isolated from `daemon/evaluation/` (`tests/plane_c_isolation.rs`).
- Never block game threads on IPC/HTTP I/O; never commit copyrighted ROMs, BIOSes, or game assets.

---

## 3. Verification Commands

```bash
# Build updated binaries (including onehundo-desktop) and run full Rust test suite (offline)
cargo build --offline --bins && cargo test --offline

# Compile / lint check (all targets)
cargo check --offline --all-targets

# Strict ISO C99 compile checks for reference/integration examples
gcc -std=c99 -pedantic -Wall -Wextra -Werror -I../onehundo-protocol/include \
  -fsyntax-only integrations/emulators/plane_a_emulator_harness.c \
  integrations/static-recompilations/plane_a_recomp_harness.c \
  integrations/native-ports/bbport/bbport_live_demo.c
gcc -std=c99 -pedantic -Wall -Wextra -Werror -I../onehundo-protocol/include \
  -x c -fsyntax-only integrations/native-ports/quick/onehundo_quick.h
```
