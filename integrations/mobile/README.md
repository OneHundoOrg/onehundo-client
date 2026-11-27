# Mobile & Android / iOS Emulator Bridge Workflows

`onehundod` and `.onehundo` packs support multi-OS matchers (`android`, `ios`) and save-path globs for mobile emulators without requiring root access or forked emulator APKs.

## Supported Workflows

### 1. Android (RetroArch / DuckStation / NetherSX2 / Dolphin / Azahar / Eden / Vita3K)
1. Enable **Network Commands** (`network_cmd_enable = "true"`, `network_cmd_port = "55355"`) in RetroArch Android, **OR** export `.srm` / memory card saves / `.oh-obs.json` snapshots to a shared folder.
2. Run `onehundod` on Android via Termux (or sync the save folder via Syncthing to a desktop running `onehundod`) with `platform_os = "android"` device registration (`POST /v1/devices/register`).
3. Bind the mobile emulator save folder directly:
   ```bash
   onehundo-desktop watch-add sony.gravity_rush /storage/emulated/0/Android/data/org.vita3k.emulator/files/vita/ux0/user/00/savedata
   ```
4. For RetroAchievements users, pair existing RetroAchievements / platform exports into `.oh-obs.json` (`source_type: "log_parser"`, `evidence_class: "LOCALLY_OBSERVED"`, `source_timestamp: null` when missing).

### 2. iOS (Delta / Provenance / RetroArch iOS)
- Point Syncthing / iCloud Drive / Files sync to a local folder synced with desktop `onehundod` and bind it with `onehundo-desktop watch-add <canonical_game_id> <synced_folder>`.
- `.onehundo` `watch_paths` support `"os": "ios"` entries alongside `android`, `steamos`, `linux`, `windows`, and `macos`.

### Provenance & Trust Rules
- Save/SRAM and RetroArch UDP network memory observations always record under `LOCAL` trust (`LOCALLY_OBSERVED` or `RUNTIME_OBSERVED` + `UNVERIFIED`) and preserve missing timestamps as `null`.
- Unverified local saves can reach `Completed` on `onehundo-platform`, never `Mastered` or leaderboard competitive tier.
