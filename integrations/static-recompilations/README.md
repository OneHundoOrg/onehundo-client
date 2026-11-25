# Static Recompilation Integration Adapter

Static recompilations (N64, PS1, PS2, etc.) can bind via:
- **Plane A (`ONEHUNDO_PLANE_MEMORY_ARENA`)** when exposing deterministic flat guest RAM regions via `onehundo_arena_register_region()` and `onehundo_arena_publish_slot()`. See [plane_a_recomp_harness.c](plane_a_recomp_harness.c).
- **Plane B (`ONEHUNDO_PLANE_SEMANTIC_EVENT`)** when emitting direct recompilation hook events (`onehundo_stat_inc`, `onehundo_unlock_request`).

Always set `matchers` with `recomp_game_id` (and `build_id` where applicable) in the pack `manifest.json` so `onehundod` resolves the canonical game ID deterministically.

## Compiling & Running the Plane A Static Recomp Harness

```bash
gcc -std=c99 -pedantic -Wall -Wextra -Werror \
  -I../../onehundo-protocol/include \
  -I../../onehundo-protocol/reference/c \
  integrations/static-recompilations/plane_a_recomp_harness.c \
  ../../onehundo-protocol/reference/c/transport_posix.c \
  -lrt -o /tmp/plane_a_recomp_harness
```
