# Agent Instructions for `onehundo-client`

> **Repository:** [`onehundo-client`](https://github.com/OneHundoOrg/onehundo-client) (Public, dual `MIT OR Apache-2.0`)  
> **Organization:** `OneHundoOrg` (`onehundo.org`)  
> **Role:** Unified `onehundo-desktop` native Grid desktop app + system tray (`eframe`/`egui` + `ksni`), embedded/standalone `onehundod` daemon, SQLite append-only journal & materialized state, Plane A (`rcheevos`) & Plane B (semantic) evaluators, Plane C external importers, instant game-detection & unlock notifications, and cloud sync worker (`onehundo-platform` `/v1/*` client).  
> **Current Phase:** **Phase 4 Stage 1 (Mass Launcher, Decomp/Recomp & Emulator Adoption)** verified; remaining **Phase 4** items (**Authoring Studio, Rarity, Ed25519 Signing & `COMPETITIVE` Lane**) next.  
> **Target Audience:** Autonomous AI agents and human contributors working on the OneHundo local runtime, evaluators, and integrations.

---

## 1. Primary Mandate and Scope

`onehundo-client` implements the local runtime, evaluation engine, and presentation layer defined by `onehundo-protocol` (`v0.1.0`), syncing durable local progress to `onehundo-platform`.

### Strict Separation of Responsibilities

| Responsibility | Repository | Notes |
| :--- | :--- | :--- |
| **Protocol Definitions, C99 SDK & Wire/Sync Schemas** | [`onehundo-protocol`](https://github.com/OneHundoOrg/onehundo-protocol) *(Public, `MIT OR Apache-2.0`)* | Consumed via pinned releases/headers/schemas (`v0.1.0`)—never redefine or fork contracts locally. |
| **Local Daemon (`onehundod`), Evaluators & Desktop Runtime** | [`onehundo-client`](https://github.com/OneHundoOrg/onehundo-client) *(this repo, Public, `MIT OR Apache-2.0`)* | IPC/SHM server, SQLite WAL journal & materialized state, `rcheevos`, semantic evaluation, Plane C importers, notifications, `Game Health`, sync outbox. |
| **Web App, OAP Catalog, Auth, Pack Signing & Sync Backend** | [`onehundo-platform`](https://github.com/OneHundoOrg/onehundo-platform) *(Private, Proprietary)* | Canonical user/game/edition/achievement IDs, `api_keys` / `devices` auth, `/v1/*` sync ingestion, Open Achievement Protocol (OAP) projections, async rarity workers. |

**Invariants for Agents:**
- **DO NOT** modify or fork `onehundo.h`, `manifest.schema.json`, or `openapi.yaml` inside this repository; consume pinned `onehundo-protocol` artifacts.
- **DO NOT** route Plane C external observations through gameplay evaluators—passive importers flow strictly through `daemon/observation/external-importers/` with distinct provenance (`PLATFORM_CONFIRMED`, `LOCALLY_OBSERVED`, `INFERRED`).
- **DO NOT** gate core tracking on graphics-API overlays (`ui/overlays/` remains optional for later phases; the native `onehundo-desktop` app + system tray + desktop toast notifications cover core desktop UX).
- **DO NOT** commit copyrighted ROMs, game dumps, decryption keys, or proprietary binaries into `tests/`.
- **Canonical Pack/Edition Titles & Provenance:** Keep public game titles clean (e.g., `"The Legend of Zelda: Ocarina of Time"`, matching `onehundo-platform` `spec/identifiers.md` §2.6) and keep runtime/port identifiers (`longshot`, `bbport`) strictly in internal/provenance fields (`pack_id`, `matchers`, `producer_ids`, and observation provenance).

---

## 2. Non-Negotiable Technical Invariants

1. **Zero Game-Thread Blocking (Fail-Open):**
   - Game-side calls (`onehundo.h`) must return immediately and never stall frame pacing or crash a game when `onehundod` is offline, busy, or restarting.
   - Offload IPC/HTTP I/O to non-blocking background workers with bounded memory rings and optional persistent client-side spooling.
2. **Commit-Before-Ack Durability & Idempotency:**
   - `onehundod` must commit incoming durable events (`STAT_SET`, `STAT_INC`, `UNLOCK_REQUEST`) to SQLite (`PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;`) **before** sending an `EVENT_ACK` over IPC.
   - Deduplicate strictly on `(producer_id, session_id, sequence_number)` so replays after crashes or timeouts are safe no-ops.
3. **Daemon Lifecycle & Socket Hygiene:**
   - Default Unix socket path is `$XDG_RUNTIME_DIR/onehundo.sock` (falling back to `/tmp/onehundo.sock`, or overridden by `ONEHUNDO_SOCKET` / `--socket`).
   - Enforce single-instance exclusivity via `flock` (`$XDG_RUNTIME_DIR/onehundod.lock`), graceful shutdown on `SIGINT`/`SIGTERM` or `POST /shutdown` (`onehundo-desktop stop`), and concise probing via `onehundo-desktop status`.
   - Persistent user daemon unit lives at `deploy/systemd/onehundod.service` (installed to `~/.config/systemd/user/onehundod.service`, binaries in `~/.local/bin`, targeting `ONEHUNDO_PLATFORM_URL=https://onehundo.org`).
4. **Four Orthogonal State Dimensions (Never Collapse):**
   - `progression_mode`: `STANDARD` | `ENCORE` | `SPECTATOR`
     - `SPECTATOR` must **never** mutate persistent player progression (local materialized state or synced progression).
     - `ENCORE` preserves historical `first_unlocked_at` timestamps while logging replay runs separately.
   - `trust_tier`: `LOCAL` | `VERIFIED_INTEGRATION` | `COMPETITIVE`
     - **Dual-lane trust:** Support frictionless `LOCAL` mode for quick/vibe-coded ports (`onehundo_quick_*`), mods, save states, and custom FPS patches alongside strict `VERIFIED_INTEGRATION` checks.
     - If a `VERIFIED_INTEGRATION` session triggers disallowed fair-play actions (e.g., save-state load, cheat/memory-poke toggle, debug console override), automatically downgrade the session/unlock to `LOCAL` (`UNVERIFIED`) rather than deleting player progress or crashing the game.
   - `evidence_class`: `DIRECT_INTEGRATION_EVENT` | `RUNTIME_OBSERVED` | `PLATFORM_CONFIRMED` | `LOCALLY_OBSERVED` | `INFERRED`
   - `verification_state`: `PENDING` | `VERIFIED` | `UNVERIFIED` | `REJECTED`
5. **Plane A Frame Sequencing & Explicit Gap Detection:**
   - Enforce `FREE -> WRITING -> READY -> READING -> FREE` shared-memory slot ownership with acquire/release atomics (`daemon/ipc/shm_arena.rs`) and per-frame stepping (`daemon/evaluation/memory-arena/` / `rcheevos`).
   - On ring overflow or sequence gaps, increment `FRAME_GAP` counters, surface explicit diagnostics in `Game Health` (`/health`), and mark deterministic evaluation incomplete—never silently skip frames.
6. **Cloud Sync Outbox Discipline (`onehundo-platform` `/v1/*` Contract):**
   - Authenticate via user-configured API key (`oh_live_...`, `0600` credentials file in `~/.config/onehundo/`) / registered device bearer token (`POST /v1/devices/register`), resolve packs (`GET /v1/packs/resolve`), and batch unsynced journal events (`POST /v1/sync/events`, 2.5s default auto-sync loop) strictly adhering to `schemas/sync/openapi.yaml` (`additionalProperties: false`—no unknown fields).
   - Mark local journal rows synced **only after** receiving HTTP `200` with durable `receipt_status` (`RECEIVED` | `DUPLICATE`), keeping `receipt_status` orthogonal to `acceptance_status` (`ACCEPTED` | `PENDING_REVIEW` | `REJECTED`).
   - Never log raw API keys or device tokens, and never echo secrets into diagnostics or raw observation records.

---

## 3. Directory Layout Guidelines

- `apps/desktop/`: Unified `onehundo-desktop` native Grid desktop app + system tray (`--minimized` autostart support) and CLI subcommands (`start`, `stop`, `status`, `health`, `packs`, `mode`, `import-obs`, `watches`, `watch-add`, `watch-remove`, `launcher-status`, `notify-test`, `set-api-key`, `sync-now`).
- `deploy/systemd/`: Checked-in `onehundod.service` `systemd --user` unit.
- `daemon/core/`: Core daemon state machine, configuration (`~/.config/onehundo/`, watched observation dirs), `flock` single-instance lockfile (`lockfile.rs`), and session orchestration.
- `daemon/ipc/`: Cross-platform IPC listeners (Unix domain sockets on Linux/macOS, named pipes on Windows) and SHM arena readers (`shm_arena.rs`).
- `daemon/storage/journal/` & `daemon/storage/state/`: SQLite WAL append-only `event_journal` / `observation_journal`, sync outbox tracking, and materialized `trophy_state` / `stat_state`.
- `daemon/packs/`: `.onehundo` (provisional) pack discovery, archive/schema validation (enforcing zip-slip/symlink/size safety), SHA-256 + OAP edition summaries, and deterministic matcher precedence.
- `daemon/evaluation/memory-arena/`: Plane A memory-arena / `rcheevos` evaluator, save-state progress serialization, reset hooks, and `FRAME_GAP` tracking.
- `daemon/evaluation/semantic-events/`: Plane B stat accumulators, thresholds, and direct unlock evaluation.
- `daemon/observation/external-importers/`: Plane C passive Goldberg/GSE, RPCS3/shadPS4/`bbport` (`trophies.jsonl`) trophy dir, Longshot binary save (`longshot.rs`, including Launcher 2 `~/.local/share/longshot/launcher/profiles/<profile>/saves/{save,save-work}/<uid>/slot_XX/progress.sav`), and `ObservationRecord`/save-export importers + directory watcher (`MAX_SCAN_DEPTH = 8`, live-process dir auto-discovery, strictly isolated from `evaluation/`).
- `daemon/sync/`: Background HTTP batch sync client targeting `onehundo-platform` (`/v1/devices/register`, `/v1/packs/resolve`, `/v1/sync/events`).
- `daemon/diagnostics/`: `Game Health` control center & status API (`http://127.0.0.1:19850/`, `/ui/packs`, `/health`, `/packs`, `/notify-test`, `/sync-now`, `/session/mode`, `/v1/observation/import`, `/shutdown`).
- `integrations/`: Opt-in integration adapters (`emulators/` including `plane_a_emulator_harness.c`, `static-recompilations/`, `native-ports/` including `bbport_live_demo.c` and `onehundo_quick.h`, `pc-games/`, and Windows `launchbox/`).
- `packs/`: Local validated packs (including `sony.bloodborne.bbport.onehundo`, SHA-256 `8f11e393...` with `*bb-probe*`/`*bbport*` executable matchers, and `nintendo.oot.longshot.pc.onehundo`, `v0.2.0`, SHA-256 `146242f8...`, 13 milestones `a01`–`a13`).
- `ui/`: `desktop/` (minimalist Grid `eframe`/`egui` desktop window, `ksni` system tray, `127.0.0.1` OAuth loopback client, unified backend), `notifications/` (`OneHundo Active` game-detection & unlock OS toasts via `notify-rust`), `game-health/` (control center HTML), `pack-browser/` (installed `.onehundo` pack browser HTML), and optional future `overlays/`.
- `tests/`: Unit, crash-recovery (`SIGKILL` commit-before-ack), replay-idempotency, mode-isolation (`STANDARD`/`ENCORE`/`SPECTATOR`), daemon lock/auto-sync, Plane A evaluator/gaps, Plane C isolation & importers/watcher/UI, `notify-test` non-mutation, and end-to-end C SDK + platform tests.

---

## 4. Phase 3 Entry State & Verification Commands

- **Phase 3 & Stage 1 Verified -> Remaining Phase 4 Next:** Plane C external importers (`goldberg.rs`, `trophy_dir.rs`, `longshot.rs`, `generic_export.rs`, `watcher.rs` strictly isolated from `evaluation/`), `packs/nintendo.oot.longshot.pc.onehundo` (`v0.2.0`, 13 milestones `a01`–`a13` starting with `"It works, it works!"`, audited MurmurHash3/placed `Bool64bitKey` save rules), `--mode <STANDARD|ENCORE|SPECTATOR>` daemon flag, SQL `NULL` timestamp preservation, `SPECTATOR` non-mutation, `POST /v1/observation/import`, `POST /session/mode`, `POST /notify-test`, `POST /sync-now`, upgraded dark Swiss/Grid Game Health & Pack Browser UI (`http://127.0.0.1:19850` with per-achievement/stat lists on `/ui/packs`), and `onehundo-desktop` (`packs`, `mode`, `import-obs`, `notify-test`) are verified (`tests/plane_c_isolation.rs`, `tests/notify_test.rs`); Stage 1 declarative save watchers (`declarative_save.rs`), debounced polling, manual watch overrides (`/watches*`), RetroArch UDP bridge (`daemon/ipc/retroarch.rs`), launcher bridge (`/v1/launchers/*`, `integrations/{playnite,launchbox,recomp-launchers,retroarch,mobile}/`), and 28 validated `.onehundo` packs are verified (`tests/stage1_watchers_and_bridge.rs`); remaining Phase 4 work adds community `.onehundo` authoring helpers and Ed25519 pack signature verification.
- **Verify:**
  - `cargo build --offline --bins && cargo test --offline` (or `cargo test`)
  - `cargo check --offline --all-targets`
  - Compile C99 integration examples (`bbport_live_demo.c`, `plane_a_emulator_harness.c`, `plane_a_recomp_harness.c`, `onehundo_quick.h`) cleanly under `-std=c99 -pedantic -Wall -Wextra -Werror`.
