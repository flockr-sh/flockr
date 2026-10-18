Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$architecture = [System.Runtime.InteropServices.RuntimeInformation, mscorlib]::OSArchitecture.ToString()
Write-Host "OS architecture: $architecture"
Write-Host "Process architecture: $([System.Runtime.InteropServices.RuntimeInformation, mscorlib]::ProcessArchitecture)"
Write-Host "PowerShell: $($PSVersionTable.PSVersion) $($PSVersionTable.PSEdition)"
Write-Host "Windows: $([Environment]::OSVersion.VersionString)"
if ($architecture -ne "Arm64") {
    throw "This test requires Windows ARM64, found $architecture."
}

$root = Join-Path $env:RUNNER_TEMP "flockr-windows-arm64-installer-test"
$env:FLOCKR_HOME = Join-Path $root "home"
$env:FLOCKR_INSTALL_DIR = Join-Path $root "bin"
$env:FLOCKR_CHANNEL = "preview"
Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $root | Out-Null
$installer = (Resolve-Path (Join-Path $PSScriptRoot "..\distribution\install.ps1")).Path

$previousErrorActionPreference = $ErrorActionPreference
$ErrorActionPreference = "Continue"
try {
    $installerOutput = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $installer 2>&1
    $installerExitCode = $LASTEXITCODE
} finally {
    $ErrorActionPreference = $previousErrorActionPreference
}
$installerOutput | ForEach-Object { Write-Host $_ }
if ($installerExitCode -ne 0) {
    throw "The installer failed on Windows ARM64 with exit code $installerExitCode."
}

$installedFlockr = Join-Path $env:FLOCKR_INSTALL_DIR "flockr.exe"
if (-not (Test-Path -LiteralPath $installedFlockr -PathType Leaf)) {
    throw "The installer exited successfully without activating flockr.exe."
}
& $installedFlockr --version
if ($LASTEXITCODE -ne 0) {
    throw "The installed x86_64 Flockr executable failed with exit code $LASTEXITCODE."
}

$releases = Join-Path $env:FLOCKR_HOME "packages\standalone\releases"
if (@(Get-ChildItem -LiteralPath $releases -Force -Directory -Filter ".staging.*" -ErrorAction SilentlyContinue).Count -ne 0) {
    throw "The installer succeeded but left a staging directory behind."
}

Write-Host "Windows ARM64 installer test passed."
