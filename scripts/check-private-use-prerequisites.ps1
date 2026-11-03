[CmdletBinding()]
param(
    [switch] $SkipBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$failures = [Collections.Generic.List[object]]::new()
$checks = [Collections.Generic.List[object]]::new()

function Add-Check {
    param([string] $Name, [bool] $Passed, [string] $Detail, [string] $Guidance)
    $checks.Add([pscustomobject]@{ Name = $Name; Passed = $Passed; Detail = $Detail })
    if (-not $Passed) {
        $failures.Add([pscustomobject]@{ Name = $Name; Guidance = $Guidance })
    }
}

function Read-CommandOutput {
    param([string] $Command, [string[]] $Arguments)
    if (-not (Get-Command $Command -ErrorAction SilentlyContinue)) { return $null }
    $output = & $Command @Arguments 2>$null
    if ($LASTEXITCODE -ne 0) { return $null }
    return (($output | Out-String).Trim())
}

function Invoke-BuildCheck {
    param([string] $Name, [string] $Command, [string[]] $Arguments)
    Write-Host "`n==> $Name"
    & $Command @Arguments
    Add-Check $Name ($LASTEXITCODE -eq 0) "exit code $LASTEXITCODE" `
        "Resolve the preceding build error, then rerun this command."
}

$rustVersion = (Get-Content (Join-Path $repositoryRoot 'rust-toolchain.toml') | `
    Select-String 'channel\s*=\s*"([^"]+)"').Matches.Groups[1].Value
$nodeVersion = (Get-Content (Join-Path $repositoryRoot '.node-version') -Raw).Trim()
$package = Get-Content (Join-Path $repositoryRoot 'package.json') -Raw | ConvertFrom-Json
$pnpmVersion = [string] $package.engines.pnpm

$os = if ($IsWindows) { Get-CimInstance -ClassName Win32_OperatingSystem } else { $null }
$osBuild = if ($os) { [int] $os.BuildNumber } else { 0 }
$osArchitecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
Add-Check 'Windows 10 22H2 build 19045 x64' `
    ($IsWindows -and $osBuild -eq 19045 -and $osArchitecture -ceq 'X64') `
    ($(if ($os) { "$($os.Caption), build $osBuild, $osArchitecture" } else { 'not Windows' })) `
    'Run the private-use build on the target laptop: Windows 10 Pro 22H2, build 19045, x64.'

$rustDetails = Read-CommandOutput 'rustc' @('-Vv')
$rustMatches = $rustDetails -and $rustDetails -match "release:\s+$([regex]::Escape($rustVersion))(\s|$)" -and `
    $rustDetails -match 'host:\s+x86_64-pc-windows-msvc(\s|$)'
Add-Check "Rust $rustVersion (MSVC x64)" ([bool] $rustMatches) `
    ($(if ($rustDetails) { ($rustDetails -replace "`r?`n", '; ') } else { 'rustc not found' })) `
    "Run: rustup toolchain install $rustVersion --profile minimal --component clippy,rustfmt; then select the repository toolchain."

$cargoDetails = Read-CommandOutput 'cargo' @('--version')
Add-Check "Cargo $rustVersion" ([bool] ($cargoDetails -match "^cargo $([regex]::Escape($rustVersion))\b")) `
    ($(if ($cargoDetails) { $cargoDetails } else { 'cargo not found' })) `
    "Install Cargo with: rustup toolchain install $rustVersion --profile minimal --component clippy,rustfmt."

$actualNode = Read-CommandOutput 'node' @('--version')
$actualNode = if ($actualNode) { $actualNode.TrimStart('v') } else { $null }
Add-Check "Node.js $nodeVersion" ($actualNode -ceq $nodeVersion) `
    ($(if ($actualNode) { $actualNode } else { 'node not found' })) `
    "Install/select Node.js $nodeVersion with your user-scoped version manager or the official Node.js distribution: https://nodejs.org/en/download."

$actualPnpm = Read-CommandOutput 'pnpm' @('--version')
Add-Check "pnpm $pnpmVersion" ($actualPnpm -ceq $pnpmVersion) `
    ($(if ($actualPnpm) { $actualPnpm } else { 'pnpm not found' })) `
    "Run: corepack enable; corepack prepare pnpm@$pnpmVersion --activate."

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vsInstall = if (Test-Path -LiteralPath $vswhere) {
    (& $vswhere -latest -products * `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath 2>$null | Select-Object -First 1)
} else { $null }
$link = if ($vsInstall) {
    Get-ChildItem -LiteralPath (Join-Path $vsInstall 'VC\Tools\MSVC') -Filter link.exe -Recurse `
        -ErrorAction SilentlyContinue | Where-Object FullName -Like '*\bin\Hostx64\x64\link.exe' | `
        Sort-Object FullName -Descending | Select-Object -First 1
} else { $null }
Add-Check 'MSVC C++ x64 build tools' ([bool] ($vsInstall -and $link)) `
    ($(if ($link) { $link.FullName } else { 'required x64 linker not found' })) `
    'Install Visual Studio Build Tools 2022 with Desktop development with C++, including MSVC x64/x86 build tools: https://visualstudio.microsoft.com/downloads/.'

$kitsRoot = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10'
$sdkVersion = if (Test-Path -LiteralPath (Join-Path $kitsRoot 'Include')) {
    Get-ChildItem -LiteralPath (Join-Path $kitsRoot 'Include') -Directory | Sort-Object Name -Descending | `
        Where-Object {
            (Test-Path -LiteralPath (Join-Path $_.FullName 'um\Windows.h')) -and
            (Test-Path -LiteralPath (Join-Path $kitsRoot "Lib\$($_.Name)\um\x64\kernel32.lib"))
        } | Select-Object -First 1
} else { $null }
Add-Check 'Windows 10/11 SDK (x64)' ([bool] $sdkVersion) `
    ($(if ($sdkVersion) { $sdkVersion.Name } else { 'usable Windows SDK not found' })) `
    'Add a Windows 10 or Windows 11 SDK through Visual Studio Installer under Desktop development with C++.'

$webViewClient = '{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
$webViewPaths = @(
    "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\$webViewClient",
    "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$webViewClient",
    "HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$webViewClient"
)
$webViewVersion = $webViewPaths | ForEach-Object {
    if (Test-Path -LiteralPath $_) { (Get-ItemProperty -LiteralPath $_ -ErrorAction SilentlyContinue).pv }
} | Where-Object { $_ -and $_ -ne '0.0.0.0' } | Select-Object -First 1
Add-Check 'Microsoft Edge WebView2 Evergreen Runtime' ([bool] $webViewVersion) `
    ($(if ($webViewVersion) { [string] $webViewVersion } else { 'runtime not found' })) `
    'Install the Evergreen Standalone WebView2 Runtime for x64: https://developer.microsoft.com/microsoft-edge/webview2/.'

$tauriReady = [bool] ($rustMatches -and $actualNode -and $actualPnpm -and $link -and `
    $sdkVersion -and $webViewVersion)
Add-Check 'Tauri 2 Windows prerequisites' $tauriReady `
    'Rust MSVC, Node.js, pnpm, the MSVC linker, Windows SDK, and WebView2 are required; a global Tauri CLI is not required.' `
    'Install the failed prerequisite(s) above. PU05 will pin the Tauri CLI in the repository instead of requiring a global install.'

Add-Check 'LOCALAPPDATA runtime root' ([bool] $env:LOCALAPPDATA) `
    ($(if ($env:LOCALAPPDATA) { $env:LOCALAPPDATA } else { 'LOCALAPPDATA missing' })) `
    'Run as a normal interactive Windows user whose LOCALAPPDATA environment variable is available.'
Add-Check 'USERPROFILE test-download root' ([bool] $env:USERPROFILE) `
    ($(if ($env:USERPROFILE) { $env:USERPROFILE } else { 'USERPROFILE missing' })) `
    'Run as a normal interactive Windows user whose USERPROFILE environment variable is available.'
$testDownloadPath = if ($env:USERPROFILE) {
    [IO.Path]::GetFullPath((Join-Path $env:USERPROFILE 'Downloads\DownloadManager-Test'))
} else { $null }
$repositoryPrefix = [IO.Path]::GetFullPath($repositoryRoot).TrimEnd('\') + '\'
Add-Check 'Test-download folder is outside the repository' `
    ([bool] ($testDownloadPath -and -not $testDownloadPath.StartsWith(
        $repositoryPrefix,
        [StringComparison]::OrdinalIgnoreCase
    ))) `
    ($(if ($testDownloadPath) { $testDownloadPath } else { 'test-download path unavailable' })) `
    'Choose a test-download folder outside the repository working tree.'

Write-Host 'Private-use prerequisite report'
foreach ($check in $checks) {
    $label = if ($check.Passed) { 'PASS' } else { 'FAIL' }
    Write-Host ("[{0}] {1}: {2}" -f $label, $check.Name, $check.Detail)
}

Write-Host "`nRuntime locations (reported only; not created by this check):"
Write-Host "  State:    %LOCALAPPDATA%\DownloadManager\State"
Write-Host "  Logs:     %LOCALAPPDATA%\DownloadManager\Logs"
Write-Host "  Partials: %LOCALAPPDATA%\DownloadManager\State\partials"
Write-Host "  IPC:      %LOCALAPPDATA%\DownloadManager\ipc"
Write-Host "  Test:     %USERPROFILE%\Downloads\DownloadManager-Test"

if ($failures.Count -eq 0 -and -not $SkipBuild) {
    Push-Location $repositoryRoot
    try {
        Invoke-BuildCheck 'Cargo workspace build' 'cargo' @('build', '--workspace', '--locked')
        Invoke-BuildCheck 'TypeScript workspace build' 'pnpm' @('build')
        Invoke-BuildCheck 'TypeScript workspace typecheck' 'pnpm' @('typecheck')
    } finally {
        Pop-Location
    }
}

if ($failures.Count -gt 0) {
    Write-Host "`nRequired actions:"
    foreach ($failure in $failures) {
        Write-Host ("- {0}: {1}" -f $failure.Name, $failure.Guidance)
    }
    exit 1
}

if ($SkipBuild) {
    Write-Host "`nPrivate-use prerequisites passed; repository build checks were skipped by request."
} else {
    Write-Host "`nPrivate-use build prerequisites and build checks passed."
}
