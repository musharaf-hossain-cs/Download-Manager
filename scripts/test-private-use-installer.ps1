[CmdletBinding()]
param(
    [string] $InstallerPath,
    [switch] $SkipBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$buildScript = Join-Path $PSScriptRoot 'build-private-use-installer.ps1'
$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) `
    "download-manager-pu08-$([Guid]::NewGuid().ToString('N'))"
$installRoot = Join-Path $env:LOCALAPPDATA 'Programs\Download Manager'
$shortcutPath = Join-Path $env:APPDATA `
    'Microsoft\Windows\Start Menu\Programs\Download Manager.lnk'
$dataRoot = Join-Path $env:LOCALAPPDATA 'DownloadManager'
$sentinelPath = Join-Path $dataRoot "pu08-retain-$([Guid]::NewGuid().ToString('N')).sentinel"
$dataRootExisted = Test-Path -LiteralPath $dataRoot -PathType Container
$installed = $false
$guiProcess = $null
$backendProcess = $null

function Invoke-CapturedProcess {
    param(
        [Parameter(Mandatory)] [string] $FilePath,
        [string[]] $Arguments = @(),
        [hashtable] $Environment = @{},
        [int] $TimeoutMilliseconds = 30000
    )

    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $FilePath
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($argument in $Arguments) { $startInfo.ArgumentList.Add($argument) }
    foreach ($entry in $Environment.GetEnumerator()) {
        $startInfo.Environment[[string] $entry.Key] = [string] $entry.Value
    }
    $process = [Diagnostics.Process]::Start($startInfo)
    try {
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit($TimeoutMilliseconds)) {
            $process.Kill($true)
            throw "Process timed out: '$FilePath'."
        }
        [pscustomobject]@{
            ExitCode = $process.ExitCode
            Stdout = $stdoutTask.GetAwaiter().GetResult()
            Stderr = $stderrTask.GetAwaiter().GetResult()
        }
    } finally {
        $process.Dispose()
    }
}

function Invoke-SilentInstaller([string] $Path) {
    $process = Start-Process -FilePath $Path -ArgumentList '/S' -Wait -PassThru
    try { return $process.ExitCode } finally { $process.Dispose() }
}

try {
    if ((Test-Path -LiteralPath $installRoot) -or (Test-Path -LiteralPath $shortcutPath)) {
        throw 'PU08 smoke refuses to replace an existing installation or Start Menu shortcut.'
    }

    [IO.Directory]::CreateDirectory($temporaryRoot) | Out-Null
    if (-not $InstallerPath) {
        $artifactRoot = Join-Path $temporaryRoot 'artifacts'
        $buildArguments = @{ OutputDirectory = $artifactRoot }
        if ($SkipBuild) { $buildArguments.SkipBuild = $true }
        $InstallerPath = & $buildScript @buildArguments
    }
    $InstallerPath = [IO.Path]::GetFullPath($InstallerPath)
    if (-not (Test-Path -LiteralPath $InstallerPath -PathType Leaf)) {
        throw "The installer artifact is missing: '$InstallerPath'."
    }
    if ((Split-Path -Leaf $InstallerPath) -cnotmatch `
        '^download-manager-\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?-windows-x64-setup\.exe$') {
        throw 'The private-use artifact name must include its version and Windows x64 target.'
    }

    $hashPath = "$InstallerPath.sha256"
    $hashLine = (Get-Content -LiteralPath $hashPath -Raw).Trim()
    $actualHash = (Get-FileHash -LiteralPath $InstallerPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($hashLine -cne "$actualHash  $(Split-Path -Leaf $InstallerPath)") {
        throw 'The published SHA-256 record does not match the installer artifact.'
    }

    [IO.Directory]::CreateDirectory($dataRoot) | Out-Null
    [IO.File]::WriteAllText($sentinelPath, 'retained-app-state')
    $completedPath = Join-Path $temporaryRoot 'Downloads\completed.bin'
    [IO.Directory]::CreateDirectory((Split-Path -Parent $completedPath)) | Out-Null
    [IO.File]::WriteAllText($completedPath, 'completed-download')

    if ((Invoke-SilentInstaller $InstallerPath) -ne 0) {
        throw 'The clean current-user NSIS installation failed.'
    }
    $installed = $true

    $desktopPath = Join-Path $installRoot 'download-manager-desktop.exe'
    $backendPath = Join-Path $installRoot 'download-manager-backend.exe'
    $cliPath = Join-Path $installRoot 'download-manager-cli.exe'
    foreach ($path in @($desktopPath, $backendPath, $cliPath, (Join-Path $installRoot 'uninstall.exe'))) {
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            throw "The installed payload is incomplete: '$path'."
        }
    }
    if (@(Get-ChildItem -LiteralPath $installRoot -File -Recurse | Where-Object {
        $_.Name -match 'native-host|ffmpeg'
    }).Count -ne 0) {
        throw 'PU08 unexpectedly packaged a browser native host or FFmpeg.'
    }
    if (-not (Test-Path -LiteralPath $shortcutPath -PathType Leaf)) {
        throw 'The GUI Start Menu shortcut was not created.'
    }
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($shortcutPath)
    if (-not [IO.Path]::GetFullPath($shortcut.TargetPath).Equals(
        $desktopPath,
        [StringComparison]::OrdinalIgnoreCase
    )) {
        throw "The Start Menu shortcut does not target the installed GUI: '$($shortcut.TargetPath)'."
    }

    $profileRoot = Join-Path $temporaryRoot 'profile'
    $profileLocalAppData = Join-Path $profileRoot 'AppData\Local'
    $profileAppData = Join-Path $profileRoot 'AppData\Roaming'
    [IO.Directory]::CreateDirectory($profileLocalAppData) | Out-Null
    [IO.Directory]::CreateDirectory($profileAppData) | Out-Null
    $isolatedEnvironment = @{
        LOCALAPPDATA = $profileLocalAppData
        APPDATA = $profileAppData
        USERPROFILE = $profileRoot
    }

    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $desktopPath
    $startInfo.UseShellExecute = $false
    foreach ($entry in $isolatedEnvironment.GetEnumerator()) {
        $startInfo.Environment[[string] $entry.Key] = [string] $entry.Value
    }
    $guiProcess = [Diagnostics.Process]::Start($startInfo)

    $health = $null
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    do {
        Start-Sleep -Milliseconds 200
        $candidate = Invoke-CapturedProcess -FilePath $cliPath -Arguments @('health', '--json') `
            -Environment $isolatedEnvironment -TimeoutMilliseconds 5000
        if ($candidate.ExitCode -eq 0) { $health = $candidate }
    } while (-not $health -and [DateTime]::UtcNow -lt $deadline)
    if (-not $health) { throw 'The installed GUI did not reach authenticated backend health.' }
    $healthDocument = $health.Stdout | ConvertFrom-Json
    if (-not $healthDocument.ok -or -not $healthDocument.data.version) {
        throw 'The installed CLI returned an invalid backend health document.'
    }

    $backendProcess = Get-Process -Name 'download-manager-backend' -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and [IO.Path]::GetFullPath($_.Path) -ceq $backendPath } |
        Select-Object -First 1
    if (-not $backendProcess) {
        throw 'The healthy backend is not running from the final installation directory.'
    }
    if (-not [IO.Path]::GetFullPath($guiProcess.MainModule.FileName).Equals(
        $desktopPath,
        [StringComparison]::OrdinalIgnoreCase
    )) {
        throw 'The GUI is not running from the final installation directory.'
    }

    [void] $guiProcess.CloseMainWindow()
    if (-not $guiProcess.WaitForExit(10000)) {
        $guiProcess.Kill($true)
        $guiProcess.WaitForExit()
    }
    $guiProcess.Dispose()
    $guiProcess = $null

    $shutdown = Invoke-CapturedProcess -FilePath $cliPath -Arguments @('shutdown', '--json') `
        -Environment $isolatedEnvironment -TimeoutMilliseconds 10000
    if ($shutdown.ExitCode -ne 0 -or -not (($shutdown.Stdout | ConvertFrom-Json).data.stopped)) {
        throw "The installed backend did not shut down cleanly: $($shutdown.Stderr)"
    }
    if (-not $backendProcess.WaitForExit(10000)) {
        throw 'The installed backend remained alive after authenticated shutdown.'
    }
    $backendProcess.Dispose()
    $backendProcess = $null

    $uninstallerPath = Join-Path $installRoot 'uninstall.exe'
    if ((Invoke-SilentInstaller $uninstallerPath) -ne 0) {
        throw 'The silent current-user uninstall failed.'
    }
    if ((Test-Path -LiteralPath $installRoot) -or (Test-Path -LiteralPath $shortcutPath)) {
        throw 'Uninstall left application files or the Start Menu shortcut.'
    }
    $installed = $false
    if ((Get-Content -LiteralPath $sentinelPath -Raw) -cne 'retained-app-state') {
        throw 'Conservative uninstall did not retain application state.'
    }
    if ((Get-Content -LiteralPath $completedPath -Raw) -cne 'completed-download') {
        throw 'Uninstall changed a completed download.'
    }

    $corruptPath = Join-Path $temporaryRoot 'corrupt-installer.exe'
    Copy-Item -LiteralPath $InstallerPath -Destination $corruptPath
    $stream = [IO.File]::Open($corruptPath, [IO.FileMode]::Open, [IO.FileAccess]::Write)
    try { $stream.SetLength([Math]::Max(4096, [int64] ($stream.Length / 2))) } finally { $stream.Dispose() }
    $corruptRejected = $false
    $installed = $true
    try {
        $corruptRejected = (Invoke-SilentInstaller $corruptPath) -ne 0
    } catch {
        $corruptRejected = $true
    }
    if (-not (Test-Path -LiteralPath $installRoot)) { $installed = $false }
    if (-not $corruptRejected -or $installed) {
        throw 'A truncated NSIS payload was not rejected before installation.'
    }

    Write-Host 'PU08 per-user Tauri/NSIS build, install, health, integrity, and uninstall smoke passed.'
} finally {
    if ($guiProcess) {
        if (-not $guiProcess.HasExited) { $guiProcess.Kill($true) }
        $guiProcess.Dispose()
    }
    if ($backendProcess) {
        if (-not $backendProcess.HasExited) { $backendProcess.Kill($true) }
        $backendProcess.Dispose()
    }
    if ($installed -and (Test-Path -LiteralPath (Join-Path $installRoot 'uninstall.exe'))) {
        try { [void] (Invoke-SilentInstaller (Join-Path $installRoot 'uninstall.exe')) } catch {}
    }
    if (Test-Path -LiteralPath $sentinelPath) {
        Remove-Item -LiteralPath $sentinelPath -Force
    }
    if (-not $dataRootExisted -and (Test-Path -LiteralPath $dataRoot -PathType Container) -and
        @(Get-ChildItem -LiteralPath $dataRoot -Force).Count -eq 0) {
        Remove-Item -LiteralPath $dataRoot -Force
    }
    $resolvedTemporaryRoot = [IO.Path]::GetFullPath($temporaryRoot)
    $systemTemporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    if ($resolvedTemporaryRoot.StartsWith($systemTemporaryRoot, [StringComparison]::OrdinalIgnoreCase) -and
        (Split-Path -Leaf $resolvedTemporaryRoot).StartsWith('download-manager-pu08-')) {
        Remove-Item -LiteralPath $resolvedTemporaryRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
