# Agent Instructions for `onehundo-client`

> **Repository:** `onehundo-client` (Public / Source-Available)  
> **Role:** Local background daemon (`onehundod`), SQLite append-only journal & materialized state, Plane A (`rcheevos`) & Plane B (semantic) evaluators, Plane C external importers, baseline desktop notifications & `Game Health` diagnostics, and cloud sync worker (`onehundo-platform` `/v1/*` client).  
> **Target Audience:** Autonomous AI agents and human contributors working on the OneHundo local runtime, evaluators, and integrations.

---

## 1. Primary Mandate and Scope

`onehundo-client` implements the local runtime, evaluation engine, and presentation layer defined by `onehundo-protocol` (`v0.1.0`), syncing durable local progress to `onehundo-platform`.

### Strict Separation of Responsibilities

| Responsibility | Repository | Notes |
| :--- | :--- | :--- |
| **Protocol Definitions, C99 SDK & Wire/Sync Schemas** | `onehundo-protocol` | Consumed via pinned releases/headers/schemas (`v0.1.0`)—never redefine or fork contracts locally. |
| **Local Daemon (`onehundod`), Evaluators & Desktop Runtime** | `onehundo-client` *(this repo)* | IPC/SHM server, SQLite WAL journal & materialized state, `rcheevos`, semantic evaluation, Plane C importers, notifications, `Game Health`, sync outbox. |
| **Web App, OAP Catalog, Auth, Pack Signing & Sync Backend** | `onehundo-platform` | Canonical user/game/edition/achievement IDs, `api_keys` / `devices` auth, `/v1/*` sync ingestion, OAP projections, async rarity workers. |

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
   - `onehundod` must commit incoming durable events (`STAT_SET`, `STAT_INC`, `UNLOCK_REQUEST`) to SQLite (`PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;`) **before** sending an `ACK` over IPC.
   - Deduplicate strictly on `(producer_id, session_id, sequence_number)` so replays after crashes or timeouts are safe no-ops.
3. **Four Orthogonal State Dimensions (Never Collapse):**
   - `progression_mode`: `STANDARD` | `ENCORE` | `SPECTATOR`
     - `SPECTATOR` must **never** mutate persistent player progression (local materialized state or synced progression).
     - `ENCORE` preserves historical `first_unlocked_at` timestamps while logging replay runs separately.
   - `trust_tier`: `LOCAL` | `VERIFIED_INTEGRATION` | `COMPETITIVE`
     - **Dual-lane trust:** Support frictionless `LOCAL` mode for quick/vibe-coded ports (`onehundo_quick_*`), mods, save states, and custom FPS patches alongside strict `VERIFIED_INTEGRATION` checks.
     - If a `VERIFIED_INTEGRATION` session triggers disallowed fair-play actions (e.g., save-state load, cheat/memory-poke toggle, debug console override), automatically downgrade the session/unlock to `LOCAL` (`UNVERIFIED`) rather than deleting player progress or crashing the game.
   - `evidence_class`: `DIRECT_INTEGRATION_EVENT` | `RUNTIME_OBSERVED` | `PLATFORM_CONFIRMED` | `LOCALLY_OBSERVED` | `INFERRED`
   - `verification_state`: `PENDING` | `VERIFIED` | `UNVERIFIED` | `REJECTED`
4. **Plane A Frame Sequencing & Explicit Gap Detection:**
   - Enforce `FREE -> WRITING -> READY -> READING -> FREE` shared-memory slot ownership with acquire/release atomics and per-frame stepping (`rc_client_do_frame`).
   - On ring overflow or sequence gaps, increment `FRAME_GAP` counters, surface explicit diagnostics in `Game Health`, and mark deterministic evaluation incomplete—never silently skip frames.
5. **Cloud Sync Outbox Discipline (`onehundo-platform` `/v1/*` Contract):**
   - Authenticate via user-configured API key (`api_keys`) / registered device bearer token (`POST /v1/devices/register`), resolve packs (`GET /v1/packs/resolve`), and batch unsynced journal events (`POST /v1/sync/events`) strictly adhering to `schemas/sync/openapi.yaml` (`additionalProperties: false`—no unknown fields).
   - Mark local journal rows synced **only after** receiving HTTP `200` with durable `receipt_status` (`RECEIVED` | `DUPLICATE`), keeping `receipt_status` orthogonal to `acceptance_status` (`ACCEPTED` | `PENDING_REVIEW` | `REJECTED`).
   - Never log raw API keys or device tokens, and never echo secrets into diagnostics or raw observation records.

---

## 3. Directory Layout Guidelines

- `apps/desktop/`: Desktop service entrypoint, system tray, and lifecycle management.
- `daemon/core/`: Core daemon state machine, configuration (`~/.config/onehundo/`), and session orchestration.
- `daemon/ipc/`: Cross-platform IPC listeners (Unix domain sockets on Linux/macOS, named pipes on Windows) and SHM arena readers.
- `daemon/storage/journal/` & `daemon/storage/state/`: SQLite WAL append-only `event_journal`, sync outbox tracking, and materialized `trophy_state` / `stat_state`.
- `daemon/packs/`: `.onehundo` (provisional) pack discovery, archive/schema validation (enforcing zip-slip/symlink/size safety), and deterministic matcher precedence.
- `daemon/evaluation/memory-arena/`: Plane A `rcheevos` integration, save-state progress serialization, reset hooks, and `FRAME_GAP` tracking.
- `daemon/evaluation/semantic-events/`: Plane B stat accumulators, thresholds, and direct unlock evaluation.
- `daemon/observation/external-importers/`: Plane C passive save/file/platform observers (strictly isolated from `evaluation/`).
- `daemon/sync/`: Background HTTP batch sync client targeting `onehundo-platform` (`/v1/devices/register`, `/v1/packs/resolve`, `/v1/sync/events`).
- `daemon/diagnostics/`: `Game Health` status API (pack match, active trust lane, `FRAME_GAP`/overflow counters, sync queue depth, last receipt/acceptance statuses).
- `integrations/`: Opt-in integration adapters (`emulators/`, `static-recompilations/`, `native-ports/` including `bbport`, `pc-games/`, and Windows `launchbox/`).
- `ui/`: Baseline `notifications/` (OS toast notifications), `game-health/`, `pack-browser/`, and optional future `overlays/`.
- `tests/`: Unit, crash-recovery (`SIGKILL` commit-before-ack), replay-idempotency, mode-isolation (`STANDARD`/`ENCORE`/`SPECTATOR`), and `onehundo-protocol` conformance fixture runner tests.
