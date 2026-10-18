# installed by flockr
# managed by flockr; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# FLOCKR_INTEGRATION_ID=codex
# FLOCKR_INTEGRATION_VERSION=10

param([string]$Action = "")

if ($Action -ne "session") { exit 0 }
if ($env:FLOCKR_ENV -ne "1") { exit 0 }
if ([string]::IsNullOrWhiteSpace($env:FLOCKR_PANE_ID)) { exit 0 }

# Read hook input as raw bytes so UTF-8 payloads decode correctly even when the
# console input code page is not UTF-8 (for example CP949 on Korean Windows).
$stdinReader = New-Object System.IO.StreamReader([Console]::OpenStandardInput(), [System.Text.Encoding]::UTF8)
$inputText = $stdinReader.ReadToEnd()
try {
    $payload = if ([string]::IsNullOrWhiteSpace($inputText)) { $null } else { $inputText | ConvertFrom-Json }
} catch {
    exit 0
}

if ($payload.hook_event_name -and $payload.hook_event_name -ne "SessionStart") { exit 0 }

$sessionId = $payload.session_id
if ([string]::IsNullOrWhiteSpace($sessionId)) { exit 0 }
if ([string]::IsNullOrWhiteSpace($payload.transcript_path)) { exit 0 }
if (-not [string]::IsNullOrWhiteSpace($env:CODEX_THREAD_ID) -and $env:CODEX_THREAD_ID -ne $sessionId) { exit 0 }

$seq = [DateTimeOffset]::UtcNow.Ticks
$flockr = if ([string]::IsNullOrWhiteSpace($env:FLOCKR_BIN_PATH)) { "flockr" } else { $env:FLOCKR_BIN_PATH }
try {
    $args = @(
        "pane",
        "report-agent-session",
        $env:FLOCKR_PANE_ID,
        "--source",
        "flockr:codex",
        "--agent",
        "codex",
        "--seq",
        "$seq",
        "--agent-session-id",
        "$sessionId"
    )
    if ($payload.hook_event_name -eq "SessionStart" -and $payload.source -is [string] -and -not [string]::IsNullOrWhiteSpace($payload.source)) {
        $args += @("--session-start-source", "$($payload.source)")
    }
    & $flockr @args 2>$null | Out-Null
} catch {
}
