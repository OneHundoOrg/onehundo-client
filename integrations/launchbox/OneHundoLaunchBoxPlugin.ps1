<#
.SYNOPSIS
    Windows LaunchBox / Big Box bridge & Plane C observation exporter for OneHundo.
.DESCRIPTION
    1) Reports active/installed game metadata (app_id, executable, install_dir, save_dir) to
       POST http://127.0.0.1:19850/v1/launchers/report so onehundod resolves the matching pack
       and auto-watches save_dir.
    2) Optionally emits a schema-valid Plane C observation JSON (`observation-record.schema.json`)
       using LOCALLY_OBSERVED provenance when `-Key` is supplied.
    3) Fails open if onehundod is offline.
#>
param(
    [string]$CanonicalGameId = $null,
    [string]$AppId = $null,
    [string]$Executable = $null,
    [string]$InstallDir = $null,
    [string]$SaveDir = $null,
    [bool]$Active = $true,
    [string]$Key = $null,
    [string]$BaseUrl = "http://127.0.0.1:19850"
)

$base = $BaseUrl.TrimEnd('/')

$report = @{
    launcher = "launchbox"
    canonical_game_id = $CanonicalGameId
    app_id = $AppId
    executable = $Executable
    install_dir = $InstallDir
    save_dir = $SaveDir
    os = "windows"
    active = $Active
} | ConvertTo-Json -Depth 4

try {
    $bridgeResp = Invoke-RestMethod -Uri "$base/v1/launchers/report" -Method Post -ContentType "application/json" -Body $report -TimeoutSec 2
    Write-Output $bridgeResp
} catch {
    Write-Warning "onehundod launcher bridge unavailable (fail-open): $_"
}

if ($Key -and $CanonicalGameId) {
    $obs = @{
        schema_version = "0.1.0"
        observation_id = [guid]::NewGuid().ToString()
        canonical_game_id = $CanonicalGameId
        observer_id = "launchbox_windows_plugin_v1"
        source_type = "log_parser"
        observed_at = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
        record_kind = "PLAYTIME_EVIDENCE"
        key = $Key
        int_value = 1
        progression_mode = "STANDARD"
        trust_tier = "LOCAL"
        evidence_class = "LOCALLY_OBSERVED"
        verification_state = "UNVERIFIED"
        provenance_notes = "Exported from LaunchBox Windows launcher metadata"
    } | ConvertTo-Json -Depth 4

    try {
        Invoke-RestMethod -Uri "$base/v1/observation/import" -Method Post -ContentType "application/json" -Body $obs -TimeoutSec 2
    } catch {
        Write-Warning "onehundod observation import unavailable (fail-open): $_"
    }
}
