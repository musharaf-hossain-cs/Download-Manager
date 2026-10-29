[CmdletBinding()]
param([switch] $SkipBuild)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$buildScript = Join-Path $PSScriptRoot 'build-windows-installer.ps1'
$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) "download-manager-installer-smoke-$([Guid]::NewGuid().ToString('N'))"
$previousLocalAppData = $env:LOCALAPPDATA
$previousAppData = $env:APPDATA

try {
    $artifactRoot = Join-Path $temporaryRoot 'artifacts'
    $expandedRoot = Join-Path $temporaryRoot 'expanded'
    $env:LOCALAPPDATA = Join-Path $temporaryRoot 'profile\AppData\Local'
    $env:APPDATA = Join-Path $temporaryRoot 'profile\AppData\Roaming'

    $buildArguments = @{ OutputDirectory = $artifactRoot }
    if ($SkipBuild) {
        $buildArguments.SkipBuild = $true
        $buildArguments.BinaryDirectory = Join-Path $repositoryRoot 'target\release'
    }
    $archivePath = & $buildScript @buildArguments
    if (@($archivePath).Count -ne 1 -or -not (Test-Path -LiteralPath $archivePath -PathType Leaf)) {
        throw 'The installer build did not produce exactly one archive.'
    }

    Expand-Archive -LiteralPath $archivePath -DestinationPath $expandedRoot
    $manifest = Get-Content -LiteralPath (Join-Path $expandedRoot 'package.json') -Raw |
        ConvertFrom-Json
    if ($manifest.install_scope -ne 'current_user' -or @($manifest.files).Count -ne 2) {
        throw 'The packaged current-user manifest is invalid.'
    }

    $tamperedRoot = Join-Path $temporaryRoot 'tampered'
    Copy-Item -LiteralPath $expandedRoot -Destination $tamperedRoot -Recurse
    [IO.File]::AppendAllText(
        (Join-Path $tamperedRoot 'payload\download-manager.exe'),
        'tampered',
        [Text.Encoding]::ASCII
    )
    $cleanLocalAppData = $env:LOCALAPPDATA
    $env:LOCALAPPDATA = Join-Path $temporaryRoot 'tampered-profile\AppData\Local'
    $tamperRejected = $false
    try {
        & (Join-Path $tamperedRoot 'install.ps1') -NoShortcut
    } catch {
        $tamperRejected = $_.Exception.Message -like '*integrity verification*'
    }
    if (-not $tamperRejected -or
        (Test-Path -LiteralPath (Join-Path $env:LOCALAPPDATA 'Programs\Download Manager'))) {
        throw 'A tampered installer payload was not rejected before installation.'
    }
    $env:LOCALAPPDATA = $cleanLocalAppData

    $installRoot = & (Join-Path $expandedRoot 'install.ps1') -NoShortcut -PassThru
    $expectedRoot = Join-Path $env:LOCALAPPDATA 'Programs\Download Manager'
    if ($installRoot -ne $expectedRoot -or -not (Test-Path -LiteralPath $installRoot -PathType Container)) {
        throw 'The package did not install beneath the current-user profile.'
    }
    foreach ($name in @('download-manager.exe', 'download-manager-native-host.exe', 'install.json')) {
        if (-not (Test-Path -LiteralPath (Join-Path $installRoot $name) -PathType Leaf)) {
            throw "The installed package is missing '$name'."
        }
    }

    $appVersion = & (Join-Path $installRoot 'download-manager.exe') --version
    if ($LASTEXITCODE -ne 0 -or $appVersion -ne "download-manager-cli $($manifest.version)") {
        throw 'The installed application did not launch successfully.'
    }
    $hostVersion = & (Join-Path $installRoot 'download-manager-native-host.exe') --version
    if ($LASTEXITCODE -ne 0 -or
        $hostVersion -ne "download-manager-native-host $($manifest.version)") {
        throw 'The installed native host did not launch successfully.'
    }

    if (Test-Path -LiteralPath (Join-Path $env:LOCALAPPDATA 'DownloadManager\NativeMessagingHosts')) {
        throw 'S152 must not create S153 native-host registrations or manifests.'
    }
    Write-Host 'Per-user installer package/install/launch smoke test passed.'
} finally {
    $env:LOCALAPPDATA = $previousLocalAppData
    $env:APPDATA = $previousAppData
    $resolvedTemporaryRoot = [IO.Path]::GetFullPath($temporaryRoot)
    $systemTemporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    if ($resolvedTemporaryRoot.StartsWith($systemTemporaryRoot, [StringComparison]::OrdinalIgnoreCase) -and
        (Split-Path -Leaf $resolvedTemporaryRoot).StartsWith('download-manager-installer-smoke-')) {
        Remove-Item -LiteralPath $resolvedTemporaryRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
