# Static Recompilation Integration Adapter

Static recompilations (N64, PS1, PS2, etc.) can bind via:
- **Plane A (`ONEHUNDO_PLANE_MEMORY_ARENA`)** when exposing deterministic guest RAM regions via `onehundo_arena_register_region()` and `onehundo_arena_publish_slot()`.
- **Plane B (`ONEHUNDO_PLANE_SEMANTIC_EVENT`)** when emitting direct recompilation hook events (`onehundo_stat_inc`, `onehundo_unlock_request`).

Always set `matchers` with `recomp_game_id` in the pack `manifest.json` so `onehundod` resolves the canonical game ID deterministically.
