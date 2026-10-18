# installed by flockr
# managed by flockr; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# FLOCKR_INTEGRATION_ID=qwen
# FLOCKR_INTEGRATION_VERSION=2

param([string]$Action = "")

if ($Action -ne "session") { exit 0 }
if ($env:FLOCKR_ENV -ne "1") { exit 0 }
if ([string]::IsNullOrWhiteSpace($env:FLOCKR_PANE_ID)) { exit 0 }
if ([string]::IsNullOrWhiteSpace($env:FLOCKR_SOCKET_PATH)) { exit 0 }

# Read hook input as raw bytes so UTF-8 payloads decode correctly even when the
# console input code page is not UTF-8 (for example CP949 on Korean Windows).
$stdinReader = New-Object System.IO.StreamReader([Console]::OpenStandardInput(), [System.Text.Encoding]::UTF8)
$inputText = $stdinReader.ReadToEnd()
try {
    $payload = if ([string]::IsNullOrWhiteSpace($inputText)) { $null } else { $inputText | ConvertFrom-Json }
} catch {
    $payload = $null
}

if ($null -eq $payload -or [string]::IsNullOrWhiteSpace($payload.session_id)) { exit 0 }

$seq = [DateTime]::UtcNow.Ticks
$flockr = if ([string]::IsNullOrWhiteSpace($env:FLOCKR_BIN_PATH)) { "flockr" } else { $env:FLOCKR_BIN_PATH }
$commandArgs = @(
    "pane", "report-agent-session", $env:FLOCKR_PANE_ID,
    "--source", "flockr:qwen", "--agent", "qwen",
    "--agent-session-id", [string]$payload.session_id,
    "--seq", [string]$seq
)
if ($payload.source -in @("startup", "resume", "clear", "compact", "branch")) {
    $commandArgs += @("--session-start-source", [string]$payload.source)
}
try {
    & $flockr @commandArgs 2>$null | Out-Null
} catch {
}
