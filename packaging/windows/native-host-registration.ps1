[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $HostPath,

    [string] $ManifestRoot,
    [string] $OnboardingPath,
    [string] $RegistrationRoot = 'Registry::HKEY_CURRENT_USER\Software',
    [switch] $PassThru
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$hostName = 'io.github.musharaf_hossain_cs.download_manager'
$chromiumExtensionId = 'abcdefghijklmnopabcdefghijklmnop'
$firefoxExtensionId = 'download-manager@musharaf-hossain-cs.github.io'
$registryRoot = 'Registry::HKEY_CURRENT_USER\Software'

if ($RegistrationRoot.StartsWith('Registry::', [StringComparison]::OrdinalIgnoreCase) -and
    -not $RegistrationRoot.Equals($registryRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Native-host registration is restricted to $registryRoot."
}
if (-not (Test-Path -LiteralPath $HostPath -PathType Leaf)) {
    throw 'Native-host registration requires an installed host executable.'
}
$resolvedHost = (Resolve-Path -LiteralPath $HostPath).Path
if (-not $ManifestRoot) {
    if (-not $env:LOCALAPPDATA) { throw 'LOCALAPPDATA is required when ManifestRoot is omitted.' }
    $ManifestRoot = Join-Path $env:LOCALAPPDATA 'DownloadManager\NativeMessagingHosts'
}
$ManifestRoot = [IO.Path]::GetFullPath($ManifestRoot)
if (-not $OnboardingPath) {
    $OnboardingPath = Join-Path (Split-Path -Parent $resolvedHost) 'browser-onboarding.json'
}
$OnboardingPath = [IO.Path]::GetFullPath($OnboardingPath)
$chromiumManifestPath = Join-Path $ManifestRoot "$hostName.json"
$firefoxManifestPath = Join-Path $ManifestRoot "$hostName.firefox.json"

function Write-JsonAtomic([string] $Path, [object] $Value) {
    $directory = Split-Path -Parent $Path
    [IO.Directory]::CreateDirectory($directory) | Out-Null
    $temporaryPath = Join-Path $directory ".$hostName.$([Guid]::NewGuid().ToString('N')).tmp"
    try {
        [IO.File]::WriteAllText(
            $temporaryPath,
            (($Value | ConvertTo-Json -Depth 6) + [Environment]::NewLine),
            [Text.UTF8Encoding]::new($false)
        )
        if (Test-Path -LiteralPath $Path -PathType Leaf) {
            [IO.File]::Replace($temporaryPath, $Path, $null)
        } else {
            [IO.File]::Move($temporaryPath, $Path)
        }
    } finally {
        if (Test-Path -LiteralPath $temporaryPath) {
            Remove-Item -LiteralPath $temporaryPath -Force
        }
    }
}

function Get-KeyPath([string] $Suffix) {
    if ($RegistrationRoot.StartsWith('Registry::', [StringComparison]::OrdinalIgnoreCase)) {
        return "$RegistrationRoot\$Suffix\$hostName"
    }
    return Join-Path ([IO.Path]::GetFullPath($RegistrationRoot)) "$Suffix\$hostName"
}

function Set-Registration([string] $Suffix, [string] $ManifestPath) {
    $keyPath = Get-KeyPath $Suffix
    if ($RegistrationRoot.StartsWith('Registry::', [StringComparison]::OrdinalIgnoreCase)) {
        New-Item -Path $keyPath -Force | Out-Null
        Set-Item -Path $keyPath -Value $ManifestPath
        return
    }
    $root = [IO.Path]::GetFullPath($RegistrationRoot).TrimEnd('\', '/') +
        [IO.Path]::DirectorySeparatorChar
    $target = [IO.Path]::GetFullPath($keyPath)
    if (-not $target.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to register outside the isolated registration root: $target"
    }
    [IO.Directory]::CreateDirectory($target) | Out-Null
    [IO.File]::WriteAllText(
        (Join-Path $target '.default-value'),
        $ManifestPath,
        [Text.UTF8Encoding]::new($false)
    )
}

$chromiumManifest = [ordered]@{
    name = $hostName
    description = 'Download Manager native messaging host'
    path = $resolvedHost
    type = 'stdio'
    allowed_origins = @("chrome-extension://$chromiumExtensionId/")
}
$firefoxManifest = [ordered]@{
    name = $hostName
    description = 'Download Manager native messaging host'
    path = $resolvedHost
    type = 'stdio'
    allowed_extensions = @($firefoxExtensionId)
}

Write-JsonAtomic $chromiumManifestPath $chromiumManifest
Write-JsonAtomic $firefoxManifestPath $firefoxManifest
Set-Registration 'Google\Chrome\NativeMessagingHosts' $chromiumManifestPath
Set-Registration 'Microsoft\Edge\NativeMessagingHosts' $chromiumManifestPath
Set-Registration 'Mozilla\NativeMessagingHosts' $firefoxManifestPath

$onboarding = [ordered]@{
    schema_version = 1
    host_name = $hostName
    browsers = @(
        [ordered]@{
            browser = 'chrome'
            extension_id = $chromiumExtensionId
            manifest = $chromiumManifestPath
            state = 'native_host_registered'
            next_step = 'install_or_enable_extension'
        },
        [ordered]@{
            browser = 'edge'
            extension_id = $chromiumExtensionId
            manifest = $chromiumManifestPath
            state = 'native_host_registered'
            next_step = 'install_or_enable_extension'
        },
        [ordered]@{
            browser = 'firefox'
            extension_id = $firefoxExtensionId
            manifest = $firefoxManifestPath
            state = 'native_host_registered'
            next_step = 'install_or_enable_extension'
        }
    )
}
Write-JsonAtomic $OnboardingPath $onboarding
if ($PassThru) { Write-Output $onboarding }
