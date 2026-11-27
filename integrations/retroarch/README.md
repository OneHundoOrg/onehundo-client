# RetroArch Network Command Memory Bridge

`onehundod` includes a built-in UDP Network Command bridge (`daemon/ipc/retroarch.rs`) that lets RetroArch cores run declarative `.onehundo` **Plane A memory rules** (`memory_rules` + `rcheevos` `memaddr`) **without patching RetroArch**.

## Setup in RetroArch
Enable network commands in `retroarch.cfg`:
```ini
network_cmd_enable = "true"
network_cmd_port = "55355"
```
By default `onehundod` polls `127.0.0.1:55355` (configurable via `--retroarch-addr <host:port>` or `none` to disable) and fails open silently when RetroArch is closed.

## How It Works
1. `onehundod` queries `GET_STATUS` (`PLAYING <system>,<content>,<crc32>`).
2. Matches installed `.onehundo` packs by `rom_hash` (`content_crc`) or `profiles.memory.retroarch` (`core_name` / `content_crc`).
3. Fetches declared `profiles.memory.regions` via chunked `READ_CORE_MEMORY <addr> <len>` and feeds frames directly into `DaemonEngine::handle_plane_a_frame` in `LOCAL` trust tier (`RUNTIME_OBSERVED` / `LOCAL` / `UNVERIFIED` unless verified via native SHM integration).
4. **SRAM / Save Fallback:** Any `.onehundo` pack can simultaneously declare `rules/save.json` watchers for `.srm` / `.sav` files so portable RetroArch setups work even when `network_cmd_enable` is off.
