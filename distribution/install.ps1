[CmdletBinding()]
param(
    [string]$Channel = $env:FLOCKR_CHANNEL,
    [string]$ManifestUrl = $env:FLOCKR_MANIFEST_URL,
    [string]$InstallDir = $env:FLOCKR_INSTALL_DIR,
    [string]$ExpectedBuildId = $env:FLOCKR_EXPECTED_BUILD_ID,
    [int]$Retain = 3,
    [string]$LocalPackagePath,
    [string]$LocalPackageFormat,
    [string]$LocalPackageIdentity,
    [string]$LocalPackageSha256
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$channelWasExplicit = -not [string]::IsNullOrWhiteSpace($Channel)
if ($channelWasExplicit -and $Channel -notin @("stable", "preview")) {
    Write-Error "Invalid Flockr channel '$Channel'. Use 'stable' or 'preview'."
    exit 1
}

$localPackageValueCount = @(
    $LocalPackagePath,
    $LocalPackageFormat,
    $LocalPackageIdentity,
    $LocalPackageSha256 |
        Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
).Count
if ($localPackageValueCount -notin @(0, 4)) {
    throw "Local package mode requires path, format, identity, and SHA-256."
}
$useLocalPackage = $localPackageValueCount -eq 4
if ($useLocalPackage -and $LocalPackageFormat -notin @("zip", "exe")) {
    throw "Local Flockr package has unsupported format '$LocalPackageFormat'."
}

function Write-Step {
    param([string]$Message)
    Write-Host "==> $Message"
}

function Write-WarningStep {
    param([string]$Message)
    Write-Warning $Message
}

function Get-FlockrCommandSource {
    $existing = Get-Command flockr -ErrorAction SilentlyContinue
    if ($null -eq $existing) {
        return $null
    }

    return $existing.Source
}

function Get-FlockrMigrationFallback {
    param([string]$CurrentDir)

    if (Test-IsJunction -Path $CurrentDir) {
        $target = [string](Get-Item -LiteralPath $CurrentDir -Force).Target
        $candidate = Join-Path $target "flockr.exe"
        if (Test-RegularFile -Path $candidate) {
            return $candidate
        }
    }

    return $null
}

function Get-FlockrExecutableKind {
    param(
        [string]$Path,
        [string]$ReleasesDir,
        [string]$CurrentDir,
        [string]$VisibleBinDir
    )

    if ([string]::IsNullOrWhiteSpace($Path) -or
        -not [System.IO.Path]::GetFileName($Path).Equals("flockr.exe", [System.StringComparison]::OrdinalIgnoreCase)) {
        return $null
    }

    try {
        $fullPath = [System.IO.Path]::GetFullPath($Path)
        foreach ($alias in @($CurrentDir, $VisibleBinDir)) {
            $aliasFlockr = [System.IO.Path]::GetFullPath((Join-Path $alias "flockr.exe"))
            if ($fullPath.Equals($aliasFlockr, [System.StringComparison]::OrdinalIgnoreCase)) {
                return "alias"
            }
        }

        $parent = Split-Path -Parent $fullPath
        if ([System.IO.Path]::GetFullPath((Split-Path -Parent $parent)).TrimEnd("\").Equals(
            [System.IO.Path]::GetFullPath($ReleasesDir).TrimEnd("\"),
            [System.StringComparison]::OrdinalIgnoreCase
        )) {
            return "release"
        }
    } catch {
        return $null
    }

    return $null
}

function Prepend-PathEntry {
    param(
        [string]$PathValue,
        [string]$Entry,
        [string[]]$OwnedEntriesToRemove = @(),
        [string]$OwnedEntryParentToRemove
    )

    $normalize = {
        param([string]$Value)
        $comparison = [Environment]::ExpandEnvironmentVariables($Value.Trim().Trim('"')).TrimEnd("\")
        try { [System.IO.Path]::GetFullPath($comparison).TrimEnd("\") } catch { $comparison }
    }
    $needle = & $normalize $Entry
    $owned = @($OwnedEntriesToRemove | ForEach-Object { & $normalize $_ })
    $ownedParent = if ([string]::IsNullOrWhiteSpace($OwnedEntryParentToRemove)) {
        $null
    } else {
        & $normalize $OwnedEntryParentToRemove
    }
    $segments = @($Entry)
    if (-not [string]::IsNullOrWhiteSpace($PathValue)) {
        $segments += $PathValue.Split(";", [System.StringSplitOptions]::RemoveEmptyEntries) |
            Where-Object {
                $segment = & $normalize $_
                try { $parent = [System.IO.Path]::GetDirectoryName($segment) } catch { $parent = $null }
                $segment -ine $needle -and
                    -not ($owned -icontains $segment) -and
                    ($null -eq $ownedParent -or $parent -ine $ownedParent)
            }
    }

    return ($segments -join ";")
}

function Update-PathRegistryEntry {
    param(
        [Microsoft.Win32.RegistryKey]$EnvironmentKey,
        [string]$Entry,
        [string[]]$OwnedEntriesToRemove = @(),
        [string]$OwnedEntryParentToRemove
    )

    $options = [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames
    $value = $EnvironmentKey.GetValue("Path", $null, $options)
    $kind = if ($null -eq $value) {
        [Microsoft.Win32.RegistryValueKind]::String
    } else {
        $EnvironmentKey.GetValueKind("Path")
    }
    $newValue = Prepend-PathEntry `
        -PathValue $value `
        -Entry $Entry `
        -OwnedEntriesToRemove $OwnedEntriesToRemove `
        -OwnedEntryParentToRemove $OwnedEntryParentToRemove
    if ($newValue -ceq $value) {
        return $false
    }

    $EnvironmentKey.SetValue("Path", $newValue, $kind)
    return $true
}

function Publish-EnvironmentChange {
    if (-not ("FlockrInstaller.EnvironmentNativeMethods" -as [type])) {
        Add-Type -Namespace FlockrInstaller -Name EnvironmentNativeMethods -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true, CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
public static extern System.IntPtr SendMessageTimeout(
    System.IntPtr hWnd,
    uint message,
    System.UIntPtr wParam,
    string lParam,
    uint flags,
    uint timeout,
    out System.UIntPtr result);
'@
    }

    $result = [UIntPtr]::Zero
    [FlockrInstaller.EnvironmentNativeMethods]::SendMessageTimeout(
        [IntPtr]0xffff,
        0x1a,
        [UIntPtr]::Zero,
        "Environment",
        0x0002,
        1000,
        [ref]$result
    ) | Out-Null
}

function Get-ManifestAsset {
    param(
        [object]$Manifest,
        [string]$Target
    )

    $property = $Manifest.assets.PSObject.Properties[$Target]
    if ($null -eq $property) {
        throw "Release manifest does not include a binary for $Target."
    }

    $sha256 = $null
    $shaMapProperty = $Manifest.PSObject.Properties["sha256"]
    if ($null -ne $shaMapProperty -and $null -ne $shaMapProperty.Value) {
        $targetShaProperty = $shaMapProperty.Value.PSObject.Properties[$Target]
        if ($null -ne $targetShaProperty -and -not [string]::IsNullOrWhiteSpace([string]$targetShaProperty.Value)) {
            $sha256 = [string]$targetShaProperty.Value
        }
    }

    $asset = $property.Value
    if ($asset -is [string]) {
        $url = [string]$asset
        return [PSCustomObject]@{
            Url = $url
            Sha256 = $sha256
            Format = if ($url.EndsWith(".zip", [System.StringComparison]::OrdinalIgnoreCase)) { "zip" } else { "exe" }
        }
    }

    $urlProperty = $asset.PSObject.Properties["url"]
    if ($null -eq $urlProperty -or [string]::IsNullOrWhiteSpace([string]$urlProperty.Value)) {
        throw "Release manifest asset $Target is missing a URL."
    }

    $url = [string]$urlProperty.Value
    $formatProperty = $asset.PSObject.Properties["format"]
    $format = if ($null -eq $formatProperty -or [string]::IsNullOrWhiteSpace([string]$formatProperty.Value)) {
        if ($url.EndsWith(".zip", [System.StringComparison]::OrdinalIgnoreCase)) { "zip" } else { "exe" }
    } else {
        [string]$formatProperty.Value
    }
    if ($format -notin @("zip", "exe")) {
        throw "Release manifest asset $Target has unsupported format '$format'."
    }
    $shaProperty = $asset.PSObject.Properties["sha256"]
    if ($null -ne $shaProperty -and -not [string]::IsNullOrWhiteSpace([string]$shaProperty.Value)) {
        $sha256 = [string]$shaProperty.Value
    }

    return [PSCustomObject]@{
        Url = $url
        Sha256 = $sha256
        Format = $format
    }
}

function Invoke-CurlDownload {
    param(
        [string]$Uri,
        [string]$Destination
    )

    $parsedUri = $null
    if (-not [System.Uri]::TryCreate($Uri, [System.UriKind]::Absolute, [ref]$parsedUri) -or
        $parsedUri.Scheme -notin @("http", "https")) {
        throw "Flockr download URL must use HTTP or HTTPS: $Uri"
    }

    $curl = Get-Command curl.exe -CommandType Application -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($null -eq $curl) {
        throw "Flockr installation requires curl.exe, which is included with supported Windows versions."
    }

    $arguments = @(
        "--fail",
        "--silent",
        "--show-error",
        "--location",
        "--connect-timeout", "30",
        "--speed-limit", "1024",
        "--speed-time", "30"
    )
    if ($parsedUri.Scheme -eq "https") {
        $arguments += @("--proto", "=https", "--tlsv1.2")
    }
    $arguments += @("--output", $Destination, "--", $Uri)

    $curlOutput = & $curl.Source @arguments 2>&1
    $curlExitCode = $LASTEXITCODE
    if ($curlExitCode -ne 0) {
        $detail = ($curlOutput | ForEach-Object { $_.ToString() }) -join [Environment]::NewLine
        $message = "Failed to download $Uri (curl exit code $curlExitCode)."
        if (-not [string]::IsNullOrWhiteSpace($detail)) {
            $message += " $detail"
        }
        throw $message
    }
}

function ConvertTo-ManifestObject {
    param([object]$Manifest)

    if ($Manifest -isnot [string]) {
        return $Manifest
    }

    $json = $Manifest.TrimStart([char]0xFEFF)
    $utf8BomDecodedAsLatin1 = [string]::Concat([char]0x00EF, [char]0x00BB, [char]0x00BF)
    if ($json.StartsWith($utf8BomDecodedAsLatin1)) {
        $json = $json.Substring(3)
    }

    return $json | ConvertFrom-Json
}

function Get-RemoteManifest {
    param([string]$Uri)

    $manifestPath = Join-Path ([System.IO.Path]::GetTempPath()) ("flockr-manifest-" + [System.Guid]::NewGuid().ToString("N") + ".json")
    try {
        Invoke-CurlDownload -Uri $Uri -Destination $manifestPath
        return ConvertTo-ManifestObject -Manifest ([System.IO.File]::ReadAllText($manifestPath))
    } finally {
        Remove-Item -LiteralPath $manifestPath -Force -ErrorAction SilentlyContinue
    }
}

function Test-FileDigest {
    param(
        [string]$Path,
        [string]$ExpectedDigest
    )

    if ([string]::IsNullOrWhiteSpace($ExpectedDigest)) {
        throw "A SHA-256 checksum is required for $Path."
    }
    if ($ExpectedDigest -notmatch '^[0-9a-fA-F]{64}$') {
        throw "Invalid SHA-256 checksum for $Path."
    }

    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
        $bytes = [System.IO.File]::ReadAllBytes($Path)
        $actual = [System.BitConverter]::ToString($sha256.ComputeHash($bytes)).Replace("-", "").ToLowerInvariant()
    } finally {
        $sha256.Dispose()
    }
    if ($actual -ne $ExpectedDigest.ToLowerInvariant()) {
        throw "Downloaded Flockr checksum did not match. Expected $ExpectedDigest but got $actual."
    }
}

function Test-RegularFile {
    param([string]$Path)

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        return $false
    }
    $item = Get-Item -LiteralPath $Path -Force
    return -not ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)
}

function Test-RegularDirectory {
    param([string]$Path)

    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        return $false
    }
    $item = Get-Item -LiteralPath $Path -Force
    return -not ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)
}

function Test-FlockrReleaseComplete {
    param(
        [string]$ReleaseDir,
        [string]$Format
    )

    if (-not (Test-RegularDirectory -Path $ReleaseDir)) {
        return $false
    }
    $flockrExe = Join-Path $ReleaseDir "flockr.exe"
    if (-not (Test-RegularFile -Path $flockrExe)) {
        return $false
    }
    if ($Format -eq "exe") {
        return $true
    }

    $conptyRoot = Join-Path $ReleaseDir "conpty"
    if (-not (Test-RegularDirectory -Path $conptyRoot) -or
        -not (Test-RegularDirectory -Path (Join-Path $conptyRoot "x64")) -or
        -not (Test-RegularDirectory -Path (Join-Path $conptyRoot "arm64"))) {
        return $false
    }
    $markerPath = Join-Path $conptyRoot "flockr-conpty.json"
    $required = @(
        "conpty/conpty.dll",
        "conpty/x64/OpenConsole.exe",
        "conpty/arm64/OpenConsole.exe",
        "THIRD-PARTY-NOTICES/Microsoft.Windows.Console.ConPTY-LICENSE.txt",
        "THIRD-PARTY-NOTICES/Microsoft.Windows.Console.ConPTY-NOTICE.md"
    )
    foreach ($relative in $required) {
        if (-not (Test-RegularFile -Path (Join-Path $ReleaseDir ($relative -replace '/', '\')))) {
            return $false
        }
    }
    if (-not (Test-RegularFile -Path $markerPath)) {
        return $false
    }

    try {
        $marker = ConvertTo-ManifestObject -Manifest (Get-Content -LiteralPath $markerPath -Raw)
        $schemaProperty = $marker.PSObject.Properties["schema_version"]
        $packageProperty = $marker.PSObject.Properties["package"]
        $versionProperty = $marker.PSObject.Properties["version"]
        $architectureProperty = $marker.PSObject.Properties["architecture"]
        $filesProperty = $marker.PSObject.Properties["files"]
        if ($null -eq $schemaProperty -or [int]$schemaProperty.Value -ne 1 -or
            $null -eq $packageProperty -or [string]$packageProperty.Value -ne "Microsoft.Windows.Console.ConPTY" -or
            $null -eq $versionProperty -or [string]::IsNullOrWhiteSpace([string]$versionProperty.Value) -or
            $null -eq $architectureProperty -or [string]$architectureProperty.Value -ne "x86_64" -or
            $null -eq $filesProperty) {
            return $false
        }

        $expectedConptyFiles = @(
            "conpty/conpty.dll",
            "conpty/x64/OpenConsole.exe",
            "conpty/arm64/OpenConsole.exe"
        )
        $markerFileNames = @($filesProperty.Value.PSObject.Properties | ForEach-Object { $_.Name })
        if (@(Compare-Object $expectedConptyFiles $markerFileNames).Count -ne 0) {
            return $false
        }

        $bundleEntries = @(Get-ChildItem -LiteralPath $conptyRoot -Force -Recurse)
        if (@($bundleEntries | Where-Object {
            $_.Attributes -band [IO.FileAttributes]::ReparsePoint
        }).Count -ne 0) {
            return $false
        }
        $releaseRoot = [System.IO.Path]::GetFullPath($ReleaseDir).TrimEnd('\')
        $actualBundleFiles = @($bundleEntries | Where-Object { -not $_.PSIsContainer } | ForEach-Object {
            $_.FullName.Substring($releaseRoot.Length + 1).Replace('\', '/')
        })
        $expectedBundleFiles = @($expectedConptyFiles) + "conpty/flockr-conpty.json"
        if (@(Compare-Object $expectedBundleFiles $actualBundleFiles).Count -ne 0) {
            return $false
        }
        foreach ($relative in $expectedConptyFiles) {
            $digestProperty = $filesProperty.Value.PSObject.Properties[$relative]
            if ($null -eq $digestProperty) {
                return $false
            }
            Test-FileDigest -Path (Join-Path $ReleaseDir ($relative -replace '/', '\')) -ExpectedDigest ([string]$digestProperty.Value)
        }
    } catch {
        return $false
    }
    return $true
}

function Move-DirectoryWithRetry {
    param(
        [string]$Source,
        [string]$Destination,
        [int]$TimeoutMilliseconds = 5000
    )

    $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMilliseconds)
    while ($true) {
        try {
            [System.IO.Directory]::Move($Source, $Destination)
            return
        } catch {
            $retryable = $false
            $exception = $_.Exception
            while ($null -ne $exception) {
                if ($exception -is [System.IO.IOException] -or
                    $exception -is [System.UnauthorizedAccessException]) {
                    $retryable = $true
                    break
                }
                $exception = $exception.InnerException
            }
            if (-not $retryable -or
                [DateTime]::UtcNow -ge $deadline -or
                -not (Test-Path -LiteralPath $Source -PathType Container) -or
                (Test-Path -LiteralPath $Destination)) {
                throw
            }
            Start-Sleep -Milliseconds 100
        }
    }
}

function Remove-DirectoryWithRetry {
    param(
        [string]$Path,
        [int]$TimeoutMilliseconds = 5000
    )

    $fullPath = [System.IO.Path]::GetFullPath($Path)
    $extendedPath = if ($fullPath.StartsWith("\\")) {
        "\\?\UNC\" + $fullPath.TrimStart([char]'\')
    } else {
        "\\?\" + $fullPath
    }
    $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMilliseconds)
    while (Test-Path -LiteralPath $Path) {
        try {
            [System.IO.Directory]::Delete($extendedPath, $true)
            return
        } catch {
            if ([DateTime]::UtcNow -ge $deadline) {
                Write-WarningStep "Flockr installed successfully but could not remove a temporary release backup at $Path."
                return
            }
            Start-Sleep -Milliseconds 100
        }
    }
}

function Invoke-WithInstallLock {
    param(
        [string]$LockPath,
        [scriptblock]$Script
    )

    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $LockPath) | Out-Null
    $lock = $null
    while ($null -eq $lock) {
        try {
            $lock = [System.IO.File]::Open(
                $LockPath,
                [System.IO.FileMode]::OpenOrCreate,
                [System.IO.FileAccess]::ReadWrite,
                [System.IO.FileShare]::None
            )
        } catch [System.IO.IOException] {
            Start-Sleep -Milliseconds 250
        }
    }

    try {
        & $Script
    } finally {
        $lock.Dispose()
    }
}

function Test-IsJunction {
    param([string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) {
        return $false
    }

    $item = Get-Item -LiteralPath $Path -Force
    return ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -and $item.LinkType -eq "Junction"
}

function Set-ManagedJunction {
    param(
        [string]$LinkPath,
        [string]$TargetPath,
        [string]$ManagedTargetPrefix,
        [bool]$AllowLegacyFlockrBinMigration = $false
    )

    if (Test-Path -LiteralPath $LinkPath) {
        $item = Get-Item -LiteralPath $LinkPath -Force
        if (Test-IsJunction -Path $LinkPath) {
            $existingTarget = [string]$item.Target
            if (-not [string]::IsNullOrWhiteSpace($ManagedTargetPrefix)) {
                $ownedPrefix = $ManagedTargetPrefix.TrimEnd("\")
                if (-not $existingTarget.StartsWith($ownedPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
                    throw "Refusing to retarget junction at $LinkPath because it is not managed by this installer."
                }
            }
            if ($existingTarget.Equals($TargetPath, [System.StringComparison]::OrdinalIgnoreCase)) {
                return
            }
            Remove-Item -LiteralPath $LinkPath -Recurse -Force
        } elseif ($item.PSIsContainer) {
            if ((Get-ChildItem -LiteralPath $LinkPath -Force | Select-Object -First 1) -ne $null) {
                if (-not (Move-LegacyFlockrBinDirectory -Path $LinkPath -AllowMigration $AllowLegacyFlockrBinMigration)) {
                    throw "Refusing to replace non-empty directory at $LinkPath with a junction."
                }
            } else {
                Remove-Item -LiteralPath $LinkPath -Recurse -Force
            }
        } else {
            throw "Refusing to replace file at $LinkPath with a junction."
        }
    }

    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $LinkPath) | Out-Null
    New-Item -ItemType Junction -Path $LinkPath -Target $TargetPath | Out-Null
}

function Move-LegacyFlockrBinDirectory {
    param(
        [string]$Path,
        [bool]$AllowMigration
    )

    if (-not $AllowMigration) {
        return $false
    }

    $entries = @(Get-ChildItem -LiteralPath $Path -Force)
    if (($entries | Where-Object { $_.PSIsContainer } | Select-Object -First 1) -ne $null) {
        return $false
    }

    if (($entries | Where-Object { $_.Name -ieq "flockr.exe" } | Select-Object -First 1) -eq $null) {
        return $false
    }

    $legacyPath = "$Path.legacy.$([System.Guid]::NewGuid().ToString("N"))"
    Move-Item -LiteralPath $Path -Destination $legacyPath
    Write-Step "Moved legacy Flockr bin directory to $legacyPath."
    return $true
}

function Remove-StaleInstallArtifacts {
    param([string]$ReleasesDir)

    if (-not (Test-Path -LiteralPath $ReleasesDir -PathType Container)) {
        return
    }

    Get-ChildItem -LiteralPath $ReleasesDir -Force -Directory -Filter ".staging.*" -ErrorAction SilentlyContinue |
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
}

function Remove-OldReleases {
    param(
        [string]$ReleasesDir,
        [string]$CurrentReleaseDir,
        [int]$Keep
    )

    if ($Keep -lt 1 -or -not (Test-Path -LiteralPath $ReleasesDir -PathType Container)) {
        return
    }

    $currentFullPath = [System.IO.Path]::GetFullPath($CurrentReleaseDir)
    $releaseDirs = Get-ChildItem -LiteralPath $ReleasesDir -Force -Directory -ErrorAction SilentlyContinue |
        Where-Object { -not $_.Name.StartsWith(".staging.") -and -not $_.Name.StartsWith(".backup.") } |
        Sort-Object LastWriteTimeUtc -Descending
    $kept = 0
    foreach ($dir in $releaseDirs) {
        $dirFullPath = [System.IO.Path]::GetFullPath($dir.FullName)
        if ($dirFullPath.Equals($currentFullPath, [System.StringComparison]::OrdinalIgnoreCase)) {
            $kept += 1
            continue
        }
        if ($kept -lt $Keep) {
            $kept += 1
            continue
        }
        Remove-Item -LiteralPath $dir.FullName -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Resolve-FlockrVersion {
    param(
        [object]$Manifest,
        [string]$SelectedChannel
    )

    if ($SelectedChannel -eq "preview") {
        if ([string]::IsNullOrWhiteSpace([string]$Manifest.base_version) -or [string]::IsNullOrWhiteSpace([string]$Manifest.build_id)) {
            throw "Preview manifest is missing base_version or build_id."
        }
        return "$($Manifest.base_version)-preview.$($Manifest.build_id)"
    }

    if ([string]::IsNullOrWhiteSpace([string]$Manifest.version)) {
        throw "Stable manifest is missing version."
    }
    return [string]$Manifest.version
}

if ($env:OS -ne "Windows_NT") {
    Write-Error "install.ps1 supports Windows only. Use install.sh on Linux or macOS."
    exit 1
}

if (-not [Environment]::Is64BitOperatingSystem) {
    Write-Error "Flockr requires 64-bit Windows."
    exit 1
}

$architecture = [System.Runtime.InteropServices.RuntimeInformation,mscorlib]::OSArchitecture.ToString()
switch ($architecture) {
    "X64" {
        $target = "windows-x86_64"
        $targetTriple = "x86_64-pc-windows-msvc"
    }
    "Arm64" {
        $target = "windows-x86_64"
        $targetTriple = "x86_64-pc-windows-msvc"
        Write-Step "Windows ARM64 detected; installing the x86_64 build under Windows emulation."
    }
    default {
        Write-Error "Unsupported Windows architecture: $architecture"
        exit 1
    }
}

$flockrHome = if ([string]::IsNullOrWhiteSpace($env:FLOCKR_HOME)) {
    Join-Path $env:USERPROFILE ".flockr"
} else {
    $env:FLOCKR_HOME
}
$flockrHome = [System.IO.Path]::GetFullPath($flockrHome)
$standaloneRoot = Join-Path $flockrHome "packages\standalone"
$releasesDir = Join-Path $standaloneRoot "releases"
$currentDir = Join-Path $standaloneRoot "current"
$lockPath = Join-Path $standaloneRoot "install.lock"

$defaultVisibleBinDir = Join-Path $env:LOCALAPPDATA "Programs\Flockr\bin"
$visibleBinDir = if ([string]::IsNullOrWhiteSpace($InstallDir)) {
    $defaultVisibleBinDir
} else {
    $InstallDir
}
$allowLegacyVisibleBinMigration = $false
try {
    $allowLegacyVisibleBinMigration = [System.IO.Path]::GetFullPath($visibleBinDir).TrimEnd("\").Equals(
        [System.IO.Path]::GetFullPath($defaultVisibleBinDir).TrimEnd("\"),
        [System.StringComparison]::OrdinalIgnoreCase
    )
} catch {
    $allowLegacyVisibleBinMigration = $false
}

$commandFlockr = Get-FlockrCommandSource
$channelFlockr = $commandFlockr
if ([string]::IsNullOrWhiteSpace($channelFlockr)) { $channelFlockr = Get-FlockrMigrationFallback -CurrentDir $currentDir }
$existingFlockrKind = Get-FlockrExecutableKind `
    -Path $channelFlockr `
    -ReleasesDir $releasesDir `
    -CurrentDir $currentDir `
    -VisibleBinDir $visibleBinDir
if (-not [string]::IsNullOrWhiteSpace($commandFlockr) -and $null -eq $existingFlockrKind) {
    Write-Step "Detected existing Flockr command at $commandFlockr"
    Write-WarningStep "PATH order decides which Flockr runs. This installer will put the active versioned release first for future and current PowerShell sessions."
}

if ($useLocalPackage) {
    $versionIdentity = $LocalPackageIdentity
    $asset = [PSCustomObject]@{
        Sha256 = $LocalPackageSha256
        Format = $LocalPackageFormat
    }
} else {
    if (-not $channelWasExplicit) {
        if (-not [string]::IsNullOrWhiteSpace($channelFlockr) -and $null -eq $existingFlockrKind) {
            throw "Refusing to run unrecognized Flockr command at $channelFlockr to detect its update channel. Rerun with -Channel stable or -Channel preview."
        }
        if (-not [string]::IsNullOrWhiteSpace($channelFlockr)) {
            $detectedChannel = [string](& $channelFlockr channel show 2>$null | Select-Object -Last 1)
            $detectedChannel = $detectedChannel.Trim()
            if ($LASTEXITCODE -ne 0 -or $detectedChannel -notin @("stable", "preview")) {
                throw "Could not determine the existing Flockr update channel. Rerun with -Channel stable or -Channel preview."
            }
            $Channel = $detectedChannel
            Write-Step "Preserving existing Flockr $Channel channel"
        } elseif (-not [string]::IsNullOrWhiteSpace($ManifestUrl) -and $ManifestUrl -match "/preview\.json$") {
            $Channel = "preview"
        } else {
            $Channel = "stable"
        }
    }

    if ([string]::IsNullOrWhiteSpace($ManifestUrl)) {
        $ManifestUrl = if ($Channel -eq "preview") {
            "https://raw.githubusercontent.com/flockr-sh/flockr/master/distribution/preview.json"
        } else {
            "https://raw.githubusercontent.com/flockr-sh/flockr/master/distribution/latest.json"
        }
    }

    Write-Step "Fetching Flockr $Channel manifest"
    $manifest = Get-RemoteManifest -Uri $ManifestUrl
    $manifestChannelProperty = $manifest.PSObject.Properties["channel"]
    if (-not $channelWasExplicit -and $null -ne $manifestChannelProperty -and [string]$manifestChannelProperty.Value -eq "preview") {
        $Channel = "preview"
    }
    $assetsProperty = $manifest.PSObject.Properties["assets"]
    $assetProperty = if ($null -eq $assetsProperty) {
        $null
    } else {
        $assetsProperty.Value.PSObject.Properties[$target]
    }
    if ($null -eq $assetProperty -and
        -not $channelWasExplicit -and
        $Channel -eq "stable" -and
        $ManifestUrl -match "/latest\.json$") {
        Write-WarningStep "The stable manifest does not include Windows yet; using preview during the stable-channel rollout."
        $Channel = "preview"
        $ManifestUrl = $ManifestUrl.Substring(0, $ManifestUrl.Length - "latest.json".Length) + "preview.json"
        Write-Step "Fetching Flockr preview manifest"
        $manifest = Get-RemoteManifest -Uri $ManifestUrl
    }
    $asset = Get-ManifestAsset -Manifest $manifest -Target $target
    if (-not [string]::IsNullOrWhiteSpace($ExpectedBuildId) -and [string]$manifest.build_id -ne $ExpectedBuildId) {
        throw "Preview manifest changed while updating. Expected build $ExpectedBuildId but found $($manifest.build_id). Run flockr update again."
    }
    $versionIdentity = Resolve-FlockrVersion -Manifest $manifest -SelectedChannel $Channel
}
$safeVersionIdentity = $versionIdentity -replace '[^0-9A-Za-z._-]', '-'
$releaseName = "$safeVersionIdentity-$targetTriple"
$releaseDir = Join-Path $releasesDir $releaseName

Write-Step "Installing Flockr $versionIdentity for $targetTriple"
$tempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("flockr-install-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $tempDir | Out-Null

$userPathChanged = $false
try {
    $userPathChanged = Invoke-WithInstallLock -LockPath $lockPath -Script {
        Remove-StaleInstallArtifacts -ReleasesDir $releasesDir

        if (-not (Test-FlockrReleaseComplete -ReleaseDir $releaseDir -Format $asset.Format)) {
            $downloadPath = if ($useLocalPackage) {
                $LocalPackagePath
            } else {
                Join-Path $tempDir "flockr-download.$($asset.Format)"
            }
            $stagingDir = Join-Path $releasesDir ".staging.$releaseName.$PID"
            if (-not $useLocalPackage) {
                Write-Step "Downloading Flockr"
                Invoke-CurlDownload -Uri $asset.Url -Destination $downloadPath
            }
            Test-FileDigest -Path $downloadPath -ExpectedDigest $asset.Sha256

            if ($asset.Format -eq "zip") {
                Expand-Archive -LiteralPath $downloadPath -DestinationPath $stagingDir
            } else {
                New-Item -ItemType Directory -Force -Path $stagingDir | Out-Null
                Copy-Item -LiteralPath $downloadPath -Destination (Join-Path $stagingDir "flockr.exe")
            }
            if (-not (Test-FlockrReleaseComplete -ReleaseDir $stagingDir -Format $asset.Format)) {
                throw "Downloaded Flockr package is incomplete or failed ConPTY verification."
            }
            $stagedFlockr = Join-Path $stagingDir "flockr.exe"
            & $stagedFlockr --version *> $null
            if ($LASTEXITCODE -ne 0) {
                throw "Downloaded Flockr command failed verification: $stagedFlockr --version"
            }
            $backupDir = $null
            if (Test-Path -LiteralPath $releaseDir) {
                $backupDir = Join-Path $releasesDir ".backup.$releaseName.$([System.Guid]::NewGuid().ToString('N'))"
                [System.IO.Directory]::Move($releaseDir, $backupDir)
            }
            try {
                Move-DirectoryWithRetry -Source $stagingDir -Destination $releaseDir
            } catch {
                if ($null -ne $backupDir -and -not (Test-Path -LiteralPath $releaseDir)) {
                    [System.IO.Directory]::Move($backupDir, $releaseDir)
                }
                Write-WarningStep "Windows could not activate the downloaded release. Another process may have a package file open, such as antivirus or indexing. No incomplete release was activated. Run flockr update again."
                throw
            }
        }

        $releaseFlockr = Join-Path $releaseDir "flockr.exe"
        & $releaseFlockr --version *> $null
        if ($LASTEXITCODE -ne 0) {
            throw "Installed Flockr command failed verification: $releaseFlockr --version"
        }
        Get-ChildItem -LiteralPath $releasesDir -Force -Directory -Filter ".backup.$releaseName.*" -ErrorAction SilentlyContinue |
            ForEach-Object { Remove-DirectoryWithRetry -Path $_.FullName }

        Set-ManagedJunction -LinkPath $currentDir -TargetPath $releaseDir -ManagedTargetPrefix $releasesDir
        Set-ManagedJunction -LinkPath $visibleBinDir -TargetPath $releaseDir -ManagedTargetPrefix $standaloneRoot -AllowLegacyFlockrBinMigration $allowLegacyVisibleBinMigration

        $ownedPathEntries = @($visibleBinDir, $currentDir)
        $userEnvironmentKey = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey("Environment")
        if ($null -eq $userEnvironmentKey) {
            throw "Unable to open the current user's environment registry key."
        }
        try {
            $pathChanged = Update-PathRegistryEntry `
                -EnvironmentKey $userEnvironmentKey `
                -Entry $releaseDir `
                -OwnedEntriesToRemove $ownedPathEntries `
                -OwnedEntryParentToRemove $releasesDir
        } finally {
            $userEnvironmentKey.Dispose()
        }

        $env:Path = Prepend-PathEntry `
            -PathValue $env:Path `
            -Entry $releaseDir `
            -OwnedEntriesToRemove $ownedPathEntries `
            -OwnedEntryParentToRemove $releasesDir
        Remove-OldReleases -ReleasesDir $releasesDir -CurrentReleaseDir $releaseDir -Keep $Retain
        return $pathChanged
    }
} finally {
    Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
}

if ($userPathChanged) {
    Publish-EnvironmentChange
    Write-Step "PATH updated for future PowerShell sessions."
} else {
    Write-Step "$releaseDir is already first on PATH."
}

$resolvedFlockr = Get-FlockrCommandSource
$resolvedFlockrKind = Get-FlockrExecutableKind `
    -Path $resolvedFlockr `
    -ReleasesDir $releasesDir `
    -CurrentDir $currentDir `
    -VisibleBinDir $visibleBinDir
$releaseFlockr = Join-Path $releaseDir "flockr.exe"
if ($resolvedFlockrKind -ne "release" -or
    -not [System.IO.Path]::GetFullPath($resolvedFlockr).Equals(
        [System.IO.Path]::GetFullPath($releaseFlockr),
        [System.StringComparison]::OrdinalIgnoreCase
    )) {
    Write-WarningStep "PowerShell still resolves flockr to $resolvedFlockr. Open a new PowerShell window or inspect PATH order manually."
}

Write-Step "Current PowerShell session: flockr"
Write-Step "Future PowerShell windows: open a new PowerShell window and run: flockr"
Write-Host "Flockr $versionIdentity installed successfully."
