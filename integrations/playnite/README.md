# Playnite OneHundo Bridge Extension

Bridges **Playnite** desktop/fullscreen launcher events with `onehundod`'s loopback launcher API (`http://127.0.0.1:19850`).

## Features
- **Fail-open launch reporting (`OnGameStarting` / `OnGameStopped`):** Reports `launcher: "playnite"`, `app_id`, `executable`, `install_dir`, and `save_dir` to `POST /v1/launchers/report`.
- **Automatic save folder binding:** When `save_dir` exists on disk, `onehundod` automatically registers it as a debounced Plane C watch override for the matched `.onehundo` pack so forked save locations work out of the box.
- **Completion queries:** `Get-OneHundoCompletionStatus` queries `GET /v1/launchers/status` to display local unlocked counts and exact integer completion percentages.

## Installation
1. Copy `extension.yaml` and `OneHundoPlayniteExtension.psm1` into `%APPDATA%\Playnite\Extensions\OneHundoBridge\`.
2. Ensure `onehundod` is running locally (`onehundo-desktop status`).
3. For custom save locations outside the game's `InstallDirectory`, bind directly via `onehundo-desktop watch-add <canonical_game_id> <save_dir>`.
