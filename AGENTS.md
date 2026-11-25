# Agent Instructions for `onehundo-client`

> **Repository:** [`onehundo-client`](https://github.com/OneHundoOrg/onehundo-client) (Public, dual `MIT OR Apache-2.0`)  
> **Organization:** `OneHundoOrg` (`onehundo.org`)  
> **Role:** Local background daemon (`onehundod`), SQLite append-only journal & materialized state, Plane A (`rcheevos`) & Plane B (semantic) evaluators, Plane C external importers, baseline desktop notifications, `onehundo-desktop` CLI & `Game Health` diagnostics, and cloud sync worker (`onehundo-platform` `/v1/*` client).  
> **Current Phase:** Post-Phase 1.5 baseline verified; entering **Phase 2 — Plane A Shared-Memory Cross-Runtime Proof**.  
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
- **DO NOT** gate core tracking on graphics-API overlays (`ui/overlays/` remains optional for later phases; baseline desktop toast notifications + `Game Health` diagnostics are required first).
- **DO NOT** commit copyrighted ROMs, game dumps, decryption keys, or proprietary binaries into `tests/`.

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
   - Persistent user daemon unit lives at `deploy/systemd/onehundod.service` (installed to `~/.config/systemd/user/onehundod.service`, binaries in `~/.local/bin`, targeting `ONEHUNDO_PLATFORM_URL=https://app-3bb4-3000.prg1.zerops.app`).
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

- `apps/desktop/`: `onehundo-desktop` CLI entrypoint (`start`, `stop`, `status`, `health`, `packs`, `import-obs`, `set-api-key`, `sync-now`).
- `deploy/systemd/`: Checked-in `onehundod.service` `systemd --user` unit.
- `daemon/core/`: Core daemon state machine, configuration (`~/.config/onehundo/`), `flock` single-instance lockfile (`lockfile.rs`), and session orchestration.
- `daemon/ipc/`: Cross-platform IPC listeners (Unix domain sockets on Linux/macOS, named pipes on Windows) and SHM arena readers (`shm_arena.rs`).
- `daemon/storage/journal/` & `daemon/storage/state/`: SQLite WAL append-only `event_journal`, sync outbox tracking, and materialized `trophy_state` / `stat_state`.
- `daemon/packs/`: `.onehundo` (provisional) pack discovery, archive/schema validation (enforcing zip-slip/symlink/size safety), and deterministic matcher precedence.
- `daemon/evaluation/memory-arena/`: Plane A memory-arena / `rcheevos` evaluator, save-state progress serialization, reset hooks, and `FRAME_GAP` tracking.
- `daemon/evaluation/semantic-events/`: Plane B stat accumulators, thresholds, and direct unlock evaluation.
- `daemon/observation/external-importers/`: Plane C passive save/file/platform observers (strictly isolated from `evaluation/`).
- `daemon/sync/`: Background HTTP batch sync client targeting `onehundo-platform` (`/v1/devices/register`, `/v1/packs/resolve`, `/v1/sync/events`).
- `daemon/diagnostics/`: `Game Health` status API (`http://127.0.0.1:19850/health`, `/packs`, `/sync-now`, `/shutdown`).
- `integrations/`: Opt-in integration adapters (`emulators/` including `plane_a_emulator_harness.c`, `static-recompilations/`, `native-ports/` including `bbport_live_demo.c` and `onehundo_quick.h`, `pc-games/`, and Windows `launchbox/`).
- `packs/`: Local validated packs (including `sony.bloodborne.bbport.onehundo`, SHA-256 `6f35c670...`).
- `ui/`: Baseline `notifications/` (OS toast notifications), `game-health/`, `pack-browser/`, and optional future `overlays/`.
- `tests/`: Unit, crash-recovery (`SIGKILL` commit-before-ack), replay-idempotency, mode-isolation (`STANDARD`/`ENCORE`/`SPECTATOR`), daemon lock/auto-sync, Plane A evaluator/gaps, Plane C isolation, and end-to-end C SDK + platform tests.

---

## 4. Phase 2 Entry State & Verification Commands

- **Phase 2 Goal:** Wire live Plane A shared-memory ring consumption (`daemon/ipc/shm_arena.rs`) + `rcheevos` evaluation (`daemon/evaluation/memory-arena/`) to prove a single `.onehundo` memory-condition pack runs identically across both a console emulator harness (`plane_a_emulator_harness.c` / libretro bridge) and a flat-memory static recompilation runtime, surfacing `FRAME_GAP` counters and ring latency in `/health` with zero game-thread stalls.
- **Verify:**
  - `cargo test --offline` (or `cargo test`)
  - `cargo clippy --all-targets`
  - Compile C99 integration examples (`bbport_live_demo.c`, `plane_a_emulator_harness.c`, `onehundo_quick.h`) cleanly under `-std=c99 -pedantic -Wall -Wextra -Werror`.
