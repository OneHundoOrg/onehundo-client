<#
.SYNOPSIS
    Fail-open Windows bridge wrapper for community recompilation launchers.
.DESCRIPTION
    Notifies onehundod when starting/stopping a community fork or portable recomp build,
    auto-binds the fork's custom save folder, and polls watches immediately upon exit.
#>
param(
    [Parameter(Mandatory=$true)][string]$Executable,
    [string[]]$Arguments = @(),
    [string]$Launcher = "recomp_launcher",
    [string]$CanonicalGameId = $null,
    [string]$AppId = $null,
    [string]$SaveDir = $null,
    [string]$HealthUrl = "http://127.0.0.1:19850"
)

$base = $HealthUrl.TrimEnd('/')

function Send-Report([bool]$active) {
    $payload = @{
        launcher = $Launcher
        canonical_game_id = $CanonicalGameId
        app_id = $AppId
        executable = $Executable
        save_dir = $SaveDir
        os = "windows"
        active = $active
    } | ConvertTo-Json -Depth 4
    try {
        $null = Invoke-RestMethod -Uri "$base/v1/launchers/report" -Method Post -ContentType "application/json" -Body $payload -TimeoutSec 2
    } catch {
        # Fail-open if onehundod is offline
    }
}

Send-Report $true
$proc = Start-Process -FilePath $Executable -ArgumentList $Arguments -PassThru -Wait
Send-Report $false
try {
    $null = Invoke-RestMethod -Uri "$base/watches/poll" -Method Post -ContentType "application/json" -Body "{}" -TimeoutSec 2
} catch {}
exit $proc.ExitCode
