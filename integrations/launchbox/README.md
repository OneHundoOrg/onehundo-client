# LaunchBox / Big Box OneHundo Integration

Supports both PowerShell pre/post-launch hooks (`OneHundoLaunchBoxPlugin.ps1`) and native C# plugin builds (`OneHundoLaunchBoxBridge.cs`).

## Usage (PowerShell Additional App / Emulator Hook)
```powershell
pwsh -File OneHundoLaunchBoxPlugin.ps1 -AppId $GameId -Executable $ExePath -SaveDir $SaveFolder -Active $true
```

## Endpoints Used
- `POST http://127.0.0.1:19850/v1/launchers/report` — registers active/idle launcher state, resolves multi-OS/multi-fork matchers, and auto-binds `save_dir` for declarative Plane C watching.
- `GET http://127.0.0.1:19850/v1/launchers/status` — fetches pack resolution & live completion counts.
- `POST http://127.0.0.1:19850/v1/observation/import` — optional direct observation submission (`LOCALLY_OBSERVED` / `LOCAL` / `UNVERIFIED`).
