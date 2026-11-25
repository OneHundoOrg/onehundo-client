<#
.SYNOPSIS
    Windows LaunchBox Plane C observation exporter for OneHundo.
.DESCRIPTION
    Emits a schema-valid Plane C observation JSON (`observation-record.schema.json`)
    using LOCALLY_OBSERVED or INFERRED provenance, and optionally submits it to
    the local onehundod Game Health /observations/import endpoint.
#>
param(
    [Parameter(Mandatory=$true)][string]$CanonicalGameId,
    [Parameter(Mandatory=$true)][string]$Key,
    [string]$HealthUrl = "http://127.0.0.1:19850/observations/import"
)

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

Invoke-RestMethod -Uri $HealthUrl -Method Post -ContentType "application/json" -Body $obs
