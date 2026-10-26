[CmdletBinding(SupportsShouldProcess, ConfirmImpact = 'Medium')]
param(
    [Parameter(Mandatory)]
    [ValidateSet('Register', 'Unregister')]
    [string] $Action,

    [string] $HostPath,
    [string[]] $ExtensionId,
    [string] $ManifestPath,

    [ValidateSet('Chrome', 'Edge', 'Firefox', 'All')]
    [string] $Browser = 'All',

    [string] $RegistrationRoot = 'Registry::HKEY_CURRENT_USER\Software'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$hostName = 'io.github.musharaf_hossain_cs.download_manager'
$registryRoot = 'Registry::HKEY_CURRENT_USER\Software'

if ($RegistrationRoot.StartsWith('Registry::', [StringComparison]::OrdinalIgnoreCase) -and
    -not $RegistrationRoot.Equals($registryRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Registry registration is restricted to $registryRoot."
}

if (-not $PSBoundParameters.ContainsKey('ExtensionId')) {
    $ExtensionId = if ($Browser -eq 'Firefox') {
        @('download-manager@musharaf-hossain-cs.github.io')
    } else {
        @('abcdefghijklmnopabcdefghijklmnop')
    }
}

if (-not $ManifestPath) {
    if (-not $env:LOCALAPPDATA) { throw 'LOCALAPPDATA is required when ManifestPath is omitted.' }
    $manifestFileName = if ($Browser -eq 'Firefox') { "$hostName.firefox.json" } else { "$hostName.json" }
    $ManifestPath = Join-Path $env:LOCALAPPDATA "DownloadManager\NativeMessagingHosts\$manifestFileName"
}
$ManifestPath = [IO.Path]::GetFullPath($ManifestPath)

$ids = @($ExtensionId | Sort-Object -Unique)
$invalidIds = @(if ($Browser -eq 'Firefox') {
        $ids | Where-Object { $_ -cnotmatch '^[A-Za-z0-9._@{}-]{1,256}$' }
    } else {
        $ids | Where-Object { $_ -cnotmatch '^[a-p]{32}$' }
    })
if ($ids.Count -eq 0 -or $ids.Count -ne @($ExtensionId).Count -or $invalidIds.Count -ne 0) {
    throw 'Each extension ID must be unique and valid for the selected browser.'
}

$browserKeys = switch ($Browser) {
    'Chrome' { 'Google\Chrome\NativeMessagingHosts' }
    'Edge' { 'Microsoft\Edge\NativeMessagingHosts' }
    'Firefox' { 'Mozilla\NativeMessagingHosts' }
    'All' {
        'Google\Chrome\NativeMessagingHosts'
        'Microsoft\Edge\NativeMessagingHosts'
    }
}

function Get-KeyPath([string] $Suffix) {
    if ($RegistrationRoot.StartsWith('Registry::', [StringComparison]::OrdinalIgnoreCase)) {
        return "$RegistrationRoot\$Suffix\$hostName"
    }
    return Join-Path ([IO.Path]::GetFullPath($RegistrationRoot)) "$Suffix\$hostName"
}

function Assert-FilesystemChild([string] $Path) {
    $root = [IO.Path]::GetFullPath($RegistrationRoot).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    $target = [IO.Path]::GetFullPath($Path)
    if (-not $target.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to modify a path outside the registration root: $target"
    }
}

if ($Action -eq 'Register') {
    if (-not $HostPath -or -not (Test-Path -LiteralPath $HostPath -PathType Leaf)) {
        throw 'Register requires HostPath to an existing native-host executable.'
    }
    $resolvedHost = (Resolve-Path -LiteralPath $HostPath).Path
    $manifest = [ordered]@{
        name = $hostName
        description = 'Download Manager native messaging host'
        path = $resolvedHost
        type = 'stdio'
    }
    if ($Browser -eq 'Firefox') {
        $manifest.allowed_extensions = @($ids)
    } else {
        $manifest.allowed_origins = @($ids | ForEach-Object { "chrome-extension://$_/" })
    }

    if ($PSCmdlet.ShouldProcess($ManifestPath, 'Write native-host manifest')) {
        $manifestDirectory = Split-Path -Parent $ManifestPath
        [IO.Directory]::CreateDirectory($manifestDirectory) | Out-Null
        $temporaryManifest = Join-Path $manifestDirectory ".$hostName.$([Guid]::NewGuid().ToString('N')).tmp"
        try {
            [IO.File]::WriteAllText(
                $temporaryManifest,
                (($manifest | ConvertTo-Json -Depth 3) + [Environment]::NewLine),
                [Text.UTF8Encoding]::new($false)
            )
            if (Test-Path -LiteralPath $ManifestPath -PathType Leaf) {
                [IO.File]::Replace($temporaryManifest, $ManifestPath, $null)
            } else {
                [IO.File]::Move($temporaryManifest, $ManifestPath)
            }
        } finally {
            if (Test-Path -LiteralPath $temporaryManifest) {
                Remove-Item -LiteralPath $temporaryManifest -Force
            }
        }
    }

    foreach ($suffix in $browserKeys) {
        $keyPath = Get-KeyPath $suffix
        if ($PSCmdlet.ShouldProcess($keyPath, 'Register current-user native host')) {
            if ($RegistrationRoot.StartsWith('Registry::', [StringComparison]::OrdinalIgnoreCase)) {
                New-Item -Path $keyPath -Force | Out-Null
                Set-Item -Path $keyPath -Value $ManifestPath
            } else {
                Assert-FilesystemChild $keyPath
                [IO.Directory]::CreateDirectory($keyPath) | Out-Null
                [IO.File]::WriteAllText((Join-Path $keyPath '.default-value'), $ManifestPath)
            }
        }
    }
    return
}

foreach ($suffix in $browserKeys) {
    $keyPath = Get-KeyPath $suffix
    if ($PSCmdlet.ShouldProcess($keyPath, 'Unregister current-user native host') -and
        (Test-Path -LiteralPath $keyPath)) {
        if (-not $RegistrationRoot.StartsWith('Registry::', [StringComparison]::OrdinalIgnoreCase)) {
            Assert-FilesystemChild $keyPath
        }
        Remove-Item -LiteralPath $keyPath -Recurse -Force
    }
}

if (Test-Path -LiteralPath $ManifestPath -PathType Leaf) {
    $existing = Get-Content -LiteralPath $ManifestPath -Raw | ConvertFrom-Json
    if ($existing.name -ne $hostName) {
        throw "Refusing to remove a manifest owned by '$($existing.name)'."
    }
    if ($PSCmdlet.ShouldProcess($ManifestPath, 'Remove native-host manifest')) {
        Remove-Item -LiteralPath $ManifestPath -Force
    }
}
