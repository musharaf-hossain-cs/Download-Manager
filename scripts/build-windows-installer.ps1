[CmdletBinding()]
param(
    [string] $OutputDirectory,
    [string] $BinaryDirectory,
    [string] $TargetTriple = 'x86_64-pc-windows-msvc',
    [switch] $SkipBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = Split-Path -Parent $PSScriptRoot
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $repositoryRoot 'dist' }
if ($TargetTriple -cne 'x86_64-pc-windows-msvc') {
    throw "The Windows x64 package requires target triple 'x86_64-pc-windows-msvc'."
}
$hostLine = rustc -vV | Where-Object { $_ -like 'host: *' } | Select-Object -First 1
if (-not $hostLine) {
    throw 'The Rust host target could not be detected.'
}
$hostTriple = $hostLine.Substring('host: '.Length).Trim()
$crossCompiling = $hostTriple -cne $TargetTriple
if (-not $BinaryDirectory) {
    $BinaryDirectory = if ($crossCompiling) {
        Join-Path $repositoryRoot "target\$TargetTriple\release"
    } else {
        Join-Path $repositoryRoot 'target\release'
    }
}
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
$BinaryDirectory = [IO.Path]::GetFullPath($BinaryDirectory)

if (-not $SkipBuild) {
    Push-Location $repositoryRoot
    try {
        $buildArguments = @('build', '--locked', '--release')
        if ($crossCompiling) { $buildArguments += @('--target', $TargetTriple) }
        $buildArguments += @(
            '-p', 'download-manager-cli',
            '-p', 'download-manager-native-host'
        )
        cargo @buildArguments
        if ($LASTEXITCODE -ne 0) { throw 'The release executable build failed.' }
    } finally {
        Pop-Location
    }
}

$rootPackage = Get-Content -LiteralPath (Join-Path $repositoryRoot 'package.json') -Raw |
    ConvertFrom-Json
$version = [string] $rootPackage.version
if ($version -cnotmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$') {
    throw 'The workspace package version is not valid for an installer artifact.'
}

$inputs = @(
    [pscustomobject]@{
        Source = Join-Path $BinaryDirectory 'download-manager-cli.exe'
        PackagePath = 'payload/download-manager.exe'
        Destination = 'download-manager.exe'
    },
    [pscustomobject]@{
        Source = Join-Path $BinaryDirectory 'download-manager-native-host.exe'
        PackagePath = 'payload/download-manager-native-host.exe'
        Destination = 'download-manager-native-host.exe'
    },
    [pscustomobject]@{
        Source = Join-Path $repositoryRoot 'packaging\windows\native-host-registration.ps1'
        PackagePath = 'payload/native-host-registration.ps1'
        Destination = 'native-host-registration.ps1'
    },
    [pscustomobject]@{
        Source = Join-Path $repositoryRoot 'packaging\windows\uninstall.ps1'
        PackagePath = 'payload/uninstall.ps1'
        Destination = 'uninstall.ps1'
    }
)
foreach ($input in $inputs) {
    if (-not (Test-Path -LiteralPath $input.Source -PathType Leaf)) {
        throw "A required release executable is missing: '$($input.Source)'."
    }
}

$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) "download-manager-package-$([Guid]::NewGuid().ToString('N'))"
$archiveName = "download-manager-$version-windows-x64.zip"
$archivePath = Join-Path $OutputDirectory $archiveName
try {
    $payloadRoot = Join-Path $temporaryRoot 'payload'
    [IO.Directory]::CreateDirectory($payloadRoot) | Out-Null
    foreach ($input in $inputs) {
        Copy-Item -LiteralPath $input.Source -Destination `
            (Join-Path $temporaryRoot ($input.PackagePath -replace '/', '\'))
    }
    Copy-Item -LiteralPath (Join-Path $repositoryRoot 'packaging\windows\install.ps1') `
        -Destination (Join-Path $temporaryRoot 'install.ps1')

    $files = @($inputs | ForEach-Object {
        $packagedPath = Join-Path $temporaryRoot ($_.PackagePath -replace '/', '\')
        [ordered]@{
            source = $_.PackagePath
            destination = $_.Destination
            sha256 = (Get-FileHash -LiteralPath $packagedPath -Algorithm SHA256).Hash.ToLowerInvariant()
        }
    })
    $manifest = [ordered]@{
        schema_version = 1
        product = 'Download Manager'
        version = $version
        architecture = 'x86_64'
        install_scope = 'current_user'
        files = $files
    }
    [IO.File]::WriteAllText(
        (Join-Path $temporaryRoot 'package.json'),
        (($manifest | ConvertTo-Json -Depth 5) + [Environment]::NewLine),
        [Text.UTF8Encoding]::new($false)
    )

    [IO.Directory]::CreateDirectory($OutputDirectory) | Out-Null
    if (Test-Path -LiteralPath $archivePath) {
        throw "Refusing to overwrite an existing installer artifact: '$archivePath'."
    }
    Compress-Archive -LiteralPath @(
        (Join-Path $temporaryRoot 'install.ps1'),
        (Join-Path $temporaryRoot 'package.json'),
        $payloadRoot
    ) -DestinationPath $archivePath -CompressionLevel Optimal
    Write-Output $archivePath
} finally {
    $resolvedTemporaryRoot = [IO.Path]::GetFullPath($temporaryRoot)
    $systemTemporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    if ($resolvedTemporaryRoot.StartsWith($systemTemporaryRoot, [StringComparison]::OrdinalIgnoreCase) -and
        (Split-Path -Leaf $resolvedTemporaryRoot).StartsWith('download-manager-package-')) {
        Remove-Item -LiteralPath $resolvedTemporaryRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
