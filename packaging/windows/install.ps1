[CmdletBinding()]
param(
    [switch] $NoShortcut,
    [switch] $PassThru,
    [string] $RegistrationRoot = 'Registry::HKEY_CURRENT_USER\Software'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$packageRoot = $PSScriptRoot
$manifestPath = Join-Path $packageRoot 'package.json'
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw 'The installer package manifest is missing.'
}
if (-not $env:LOCALAPPDATA) { throw 'LOCALAPPDATA is required for a current-user install.' }
if (-not $NoShortcut -and -not $env:APPDATA) {
    throw 'APPDATA is required to create the current-user Start Menu shortcut.'
}

$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ($manifest.schema_version -ne 1 -or
    $manifest.product -ne 'Download Manager' -or
    $manifest.install_scope -ne 'current_user' -or
    $manifest.architecture -ne 'x86_64' -or
    -not $manifest.version -or
    @($manifest.files).Count -ne 3) {
    throw 'The installer package manifest is unsupported or incomplete.'
}

$localAppData = [IO.Path]::GetFullPath($env:LOCALAPPDATA).TrimEnd('\', '/')
$programsRoot = Join-Path $localAppData 'Programs'
$installRoot = Join-Path $programsRoot 'Download Manager'
$installPrefix = $localAppData + [IO.Path]::DirectorySeparatorChar
if (-not $installRoot.StartsWith($installPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'The install destination escaped the current-user application data root.'
}
if (Test-Path -LiteralPath $installRoot) {
    throw "Download Manager is already installed at '$installRoot'."
}

$expectedPayloads = @{
    'payload/download-manager.exe' = 'download-manager.exe'
    'payload/download-manager-native-host.exe' = 'download-manager-native-host.exe'
    'payload/native-host-registration.ps1' = 'native-host-registration.ps1'
}
$seenDestinations = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
foreach ($file in @($manifest.files)) {
    if ($file.source -cnotmatch '^payload/[a-z0-9.-]+\.(?:exe|ps1)$' -or
        $expectedPayloads[[string] $file.source] -cne $file.destination -or
        -not $seenDestinations.Add([string] $file.destination) -or
        $file.sha256 -cnotmatch '^[A-Fa-f0-9]{64}$') {
        throw 'The installer package contains an invalid file entry.'
    }
    $source = Join-Path $packageRoot ($file.source -replace '/', '\')
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
        throw "The installer payload is missing '$($file.source)'."
    }
    $actualHash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash
    if (-not $actualHash.Equals([string] $file.sha256, [StringComparison]::OrdinalIgnoreCase)) {
        throw "The installer payload failed integrity verification: '$($file.source)'."
    }
}
if ($seenDestinations.Count -ne $expectedPayloads.Count) {
    throw 'The installer package does not contain every required payload file.'
}

[IO.Directory]::CreateDirectory($programsRoot) | Out-Null
$stagingRoot = Join-Path $programsRoot ".download-manager-install-$([Guid]::NewGuid().ToString('N'))"
$stagingPrefix = $programsRoot.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
if (-not $stagingRoot.StartsWith($stagingPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'The staging destination escaped the current-user Programs root.'
}

try {
    [IO.Directory]::CreateDirectory($stagingRoot) | Out-Null
    foreach ($file in @($manifest.files)) {
        $source = Join-Path $packageRoot ($file.source -replace '/', '\')
        Copy-Item -LiteralPath $source -Destination (Join-Path $stagingRoot $file.destination)
    }
    [IO.File]::WriteAllText(
        (Join-Path $stagingRoot 'install.json'),
        (($manifest | ConvertTo-Json -Depth 5) + [Environment]::NewLine),
        [Text.UTF8Encoding]::new($false)
    )

    $versionOutput = & (Join-Path $stagingRoot 'download-manager.exe') --version
    if ($LASTEXITCODE -ne 0 -or $versionOutput -ne "download-manager-cli $($manifest.version)") {
        throw 'The installed application launch check failed.'
    }

    [IO.Directory]::Move($stagingRoot, $installRoot)
    & (Join-Path $installRoot 'native-host-registration.ps1') `
        -HostPath (Join-Path $installRoot 'download-manager-native-host.exe') `
        -OnboardingPath (Join-Path $installRoot 'browser-onboarding.json') `
        -RegistrationRoot $RegistrationRoot
    if (-not $NoShortcut) {
        $startMenuRoot = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
        [IO.Directory]::CreateDirectory($startMenuRoot) | Out-Null
        $shell = New-Object -ComObject WScript.Shell
        $shortcut = $shell.CreateShortcut((Join-Path $startMenuRoot 'Download Manager.lnk'))
        $shortcut.TargetPath = Join-Path $installRoot 'download-manager.exe'
        $shortcut.WorkingDirectory = $installRoot
        $shortcut.Description = 'Download Manager'
        $shortcut.Save()
    }
} finally {
    if (Test-Path -LiteralPath $stagingRoot) {
        Remove-Item -LiteralPath $stagingRoot -Recurse -Force
    }
}

if ($PassThru) { Write-Output $installRoot }
