[CmdletBinding()]
param(
    [string] $OutputDirectory,
    [string] $TargetTriple = 'x86_64-pc-windows-msvc',
    [switch] $SkipBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$desktopRoot = Join-Path $repositoryRoot 'apps\desktop'
$tauriRoot = Join-Path $desktopRoot 'src-tauri'
$configurationPath = Join-Path $tauriRoot 'tauri.conf.json'
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $repositoryRoot 'dist\private-use' }
if ($TargetTriple -cne 'x86_64-pc-windows-msvc') {
    throw "PU08 supports only target triple 'x86_64-pc-windows-msvc'."
}
if (-not $IsWindows) { throw 'The private-use NSIS installer must be built on Windows.' }
$hostLine = rustc -vV | Where-Object { $_ -like 'host: *' } | Select-Object -First 1
if (-not $hostLine -or $hostLine.Substring('host: '.Length).Trim() -cne $TargetTriple) {
    throw "PU08 must run natively on the '$TargetTriple' laptop."
}

$configuration = Get-Content -LiteralPath $configurationPath -Raw | ConvertFrom-Json
$version = [string] $configuration.version
if ($version -cnotmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$') {
    throw 'The Tauri version is not valid for an installer artifact.'
}
$workspaceManifest = Get-Content -LiteralPath (Join-Path $repositoryRoot 'Cargo.toml') -Raw
if ($workspaceManifest -notmatch '(?m)^version = "([^\"]+)"$' -or $Matches[1] -cne $version) {
    throw 'The Tauri and Rust workspace versions must match before packaging.'
}

$targetBase = if ($env:CARGO_TARGET_DIR) {
    if ([IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) {
        [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)
    } else {
        [IO.Path]::GetFullPath((Join-Path $repositoryRoot $env:CARGO_TARGET_DIR))
    }
} else {
    Join-Path $repositoryRoot 'target'
}
$releaseRoot = Join-Path $targetBase 'release'
$bundleRoot = Join-Path $releaseRoot 'bundle\nsis'
$sidecarRoot = Join-Path $tauriRoot 'binaries'

if (-not $SkipBuild) {
    [IO.Directory]::CreateDirectory($sidecarRoot) | Out-Null
    Push-Location $repositoryRoot
    try {
        & cargo build --locked --release -p download-manager-cli --bins |
            ForEach-Object { Write-Host $_ }
        if ($LASTEXITCODE -ne 0) { throw 'The release backend/CLI build failed.' }

        foreach ($name in @('download-manager-backend', 'download-manager-cli')) {
            $source = Join-Path $releaseRoot "$name.exe"
            if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
                throw "The required release sidecar is missing: '$source'."
            }
            Copy-Item -LiteralPath $source -Destination `
                (Join-Path $sidecarRoot "$name-$TargetTriple.exe") -Force
        }

        & pnpm --filter '@download-manager/desktop' exec tauri build --ci `
            --bundles nsis --no-sign |
            ForEach-Object { Write-Host $_ }
        if ($LASTEXITCODE -ne 0) { throw 'The Tauri NSIS build failed.' }
    } finally {
        Pop-Location
    }
}

$expectedBundleName = "Download Manager_${version}_x64-setup.exe"
$bundlePath = Join-Path $bundleRoot $expectedBundleName
if (-not (Test-Path -LiteralPath $bundlePath -PathType Leaf)) {
    throw "The expected Tauri NSIS bundle is missing: '$bundlePath'."
}

$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
[IO.Directory]::CreateDirectory($OutputDirectory) | Out-Null
$artifactName = "download-manager-$version-windows-x64-setup.exe"
$artifactPath = Join-Path $OutputDirectory $artifactName
$hashPath = "$artifactPath.sha256"
if ((Test-Path -LiteralPath $artifactPath) -or (Test-Path -LiteralPath $hashPath)) {
    throw "Refusing to overwrite an existing private-use artifact: '$artifactPath'."
}
Copy-Item -LiteralPath $bundlePath -Destination $artifactPath
$hash = (Get-FileHash -LiteralPath $artifactPath -Algorithm SHA256).Hash.ToLowerInvariant()
[IO.File]::WriteAllText(
    $hashPath,
    "$hash  $artifactName$([Environment]::NewLine)",
    [Text.UTF8Encoding]::new($false)
)

Write-Output $artifactPath
