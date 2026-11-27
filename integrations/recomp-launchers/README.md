# Community Recompilation Launcher Bridge

Allows community recompilation launchers (Longshot Launcher, Project ReLink, Triforce, ScarlettPort, bbport, UnleashedRecomp, Zelda64Recomp, Ship of Harkinian / 2Ship, OpenGOAL, Starship, etc.) to integrate cleanly with `onehundod` using **zero custom binary SDK dependency**.

## Protocol
1. Before launching the game process:
   `POST http://127.0.0.1:19850/v1/launchers/report`
   ```json
   {
     "launcher": "longshot_launcher",
     "executable": "/opt/games/longshot/oot_recomp",
     "save_dir": "/home/user/.local/share/longshot/saves",
     "os": "linux",
     "active": true
   }
   ```
   `onehundod` resolves matching `.onehundo` packs by `executable`, `app_id`, `save_path_glob`, or `canonical_game_id` and automatically binds `save_dir` as a watched directory.
2. When querying progress tiles inside the launcher UI:
   `GET http://127.0.0.1:19850/v1/launchers/status`
3. After process exit:
   Repeat `POST /v1/launchers/report` with `"active": false` and optionally trigger `POST /watches/poll`.
