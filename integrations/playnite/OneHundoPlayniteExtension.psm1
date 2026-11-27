<#
.SYNOPSIS
    Playnite script extension bridging Playnite game lifecycle & library paths to onehundod.
.DESCRIPTION
    Hooks Playnite's OnGameStarting / OnGameStopped / OnLibraryUpdated events:
    - Reports active game executable, app/plugin ID, installDir, and optional saveDir to
      POST http://127.0.0.1:19850/v1/launchers/report (fails open cleanly if onehundod is offline).
    - Queries GET http://127.0.0.1:19850/v1/launchers/status to surface OneHundo completion %.
#>

$Script:OneHundoBaseUrl = if ($env:ONEHUNDO_HEALTH_URL) { $env:ONEHUNDO_HEALTH_URL.TrimEnd('/') } else { "http://127.0.0.1:19850" }

function Send-OneHundoLauncherReport {
    param(
        [Parameter(Mandatory=$true)]$Game,
        [Parameter(Mandatory=$true)][bool]$Active,
        [string]$SaveDir = $null
    )

    $bodyObj = @{
        launcher = "playnite"
        app_id = if ($Game.GameId) { [string]$Game.GameId } else { [string]$Game.Id }
        executable = if ($Game.GameActions -and $Game.GameActions.Count -gt 0) { [string]$Game.GameActions[0].Path } else { $null }
        install_dir = if ($Game.InstallDirectory) { [string]$Game.InstallDirectory } else { $null }
        save_dir = if ($SaveDir) { $SaveDir } elseif ($Game.InstallDirectory) { [string]$Game.InstallDirectory } else { $null }
        os = "windows"
        active = $Active
    }

    $json = $bodyObj | ConvertTo-Json -Depth 4
    try {
        return Invoke-RestMethod -Uri "$Script:OneHundoBaseUrl/v1/launchers/report" -Method Post -ContentType "application/json" -Body $json -TimeoutSec 2
    } catch {
        # Fail-open: never block Playnite game launch if onehundod is offline
        return $null
    }
}

function Get-OneHundoCompletionStatus {
    param([string]$AppId = $null, [string]$CanonicalGameId = $null)

    $qs = @()
    if ($CanonicalGameId) { $qs += "canonical_game_id=$([uri]::EscapeDataString($CanonicalGameId))" }
    if ($AppId) { $qs += "app_id=$([uri]::EscapeDataString($AppId))" }
    $suffix = if ($qs.Count -gt 0) { "?" + ($qs -join "&") } else { "" }

    try {
        return Invoke-RestMethod -Uri "$Script:OneHundoBaseUrl/v1/launchers/status$suffix" -Method Get -TimeoutSec 2
    } catch {
        return $null
    }
}

function OnGameStarting {
    param($eventArgs)
    if ($eventArgs -and $eventArgs.Game) {
        $null = Send-OneHundoLauncherReport -Game $eventArgs.Game -Active $true
    }
}

function OnGameStopped {
    param($eventArgs)
    if ($eventArgs -and $eventArgs.Game) {
        $null = Send-OneHundoLauncherReport -Game $eventArgs.Game -Active $false
    }
}

Export-ModuleMember -Function Send-OneHundoLauncherReport, Get-OneHundoCompletionStatus, OnGameStarting, OnGameStopped
