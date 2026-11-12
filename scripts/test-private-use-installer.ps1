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
$hostName = 'io.github.musharaf_hossain_cs.download_manager'
$extensionId = 'ackbonlbpniilcfmmnfdohnhbdbajhii'
$manifestRoot = Join-Path $dataRoot 'NativeMessagingHosts'
$nativeManifestPath = Join-Path $manifestRoot "$hostName.json"
$chromeRegistration = "Registry::HKEY_CURRENT_USER\Software\Google\Chrome\NativeMessagingHosts\$hostName"
$edgeRegistration = "Registry::HKEY_CURRENT_USER\Software\Microsoft\Edge\NativeMessagingHosts\$hostName"

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

function Read-Exact([IO.Stream] $Stream, [int] $Length) {
    [byte[]] $buffer = [byte[]]::new($Length)
    $offset = 0
    while ($offset -lt $Length) {
        $count = $Stream.Read($buffer, $offset, $Length - $offset)
        if ($count -eq 0) { throw "Stream ended after $offset of $Length bytes." }
        $offset += $count
    }
    return ,$buffer
}

function Test-NativeConnection([string] $HostPath) {
    $requestId = [Guid]::NewGuid().ToString()
    $request = [ordered]@{
        version = 1
        request_id = $requestId
        nonce = 'abcdefghijklmnop'
        extension_id = $extensionId
        payload = [ordered]@{ type = 'health' }
    } | ConvertTo-Json -Depth 3 -Compress
    [byte[]] $body = [Text.Encoding]::UTF8.GetBytes($request)
    [byte[]] $header = [BitConverter]::GetBytes([uint32] $body.Length)
    if (-not [BitConverter]::IsLittleEndian) { [Array]::Reverse($header) }

    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $HostPath
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardInput = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.ArgumentList.Add("chrome-extension://$extensionId/")
    $process = [Diagnostics.Process]::Start($startInfo)
    try {
        $process.StandardInput.BaseStream.Write($header, 0, $header.Length)
        $process.StandardInput.BaseStream.Write($body, 0, $body.Length)
        $process.StandardInput.BaseStream.Flush()
        [byte[]] $responseHeader = Read-Exact $process.StandardOutput.BaseStream 4
        if (-not [BitConverter]::IsLittleEndian) { [Array]::Reverse($responseHeader) }
        $responseLength = [BitConverter]::ToUInt32($responseHeader, 0)
        [byte[]] $responseBody = Read-Exact $process.StandardOutput.BaseStream $responseLength
        $response = [Text.Encoding]::UTF8.GetString($responseBody) | ConvertFrom-Json
        if ($response.version -ne 1 -or $response.request_id -ne $requestId -or
            $response.payload.type -ne 'healthy' -or -not $response.payload.host_version) {
            throw 'The installed native host did not return a correlated health response.'
        }
        $process.StandardInput.Close()
        if (-not $process.WaitForExit(5000) -or $process.ExitCode -ne 0) {
            throw "The installed native host did not exit cleanly: $($process.StandardError.ReadToEnd())"
        }
    } finally {
        if (-not $process.HasExited) { $process.Kill($true) }
        $process.Dispose()
    }
}

function Assert-BrowserRegistration([string] $HostPath) {
    if (-not (Test-Path -LiteralPath $nativeManifestPath -PathType Leaf)) {
        throw 'The installed Chromium native-host manifest is missing.'
    }
    $nativeManifest = Get-Content -LiteralPath $nativeManifestPath -Raw | ConvertFrom-Json
    if ($nativeManifest.name -ne $hostName -or $nativeManifest.path -ne $HostPath -or
        @($nativeManifest.allowed_origins).Count -ne 1 -or
        $nativeManifest.allowed_origins[0] -ne "chrome-extension://$extensionId/") {
        throw 'The installed Chromium native-host manifest is invalid.'
    }
    foreach ($key in @($chromeRegistration, $edgeRegistration)) {
        if (-not (Test-Path -LiteralPath $key)) {
            throw "The current-user browser registration is missing: '$key'."
        }
        $registeredManifest = (Get-Item -LiteralPath $key).GetValue('')
        if ($registeredManifest -ne $nativeManifestPath) {
            throw "The current-user browser registration targets '$registeredManifest'."
        }
    }
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
    $hostPath = Join-Path $installRoot 'download-manager-native-host.exe'
    $registrationScript = Join-Path $installRoot 'native-host-registration.ps1'
    $onboardingPath = Join-Path $installRoot 'browser-onboarding.json'
    $extensionRoot = Join-Path $installRoot 'browser-extension'
    foreach ($path in @(
        $desktopPath,
        $backendPath,
        $cliPath,
        $hostPath,
        $registrationScript,
        $onboardingPath,
        (Join-Path $installRoot 'BROWSER_SETUP.txt'),
        (Join-Path $extensionRoot 'manifest.json'),
        (Join-Path $extensionRoot 'src\service-worker.js'),
        (Join-Path $installRoot 'uninstall.exe')
    )) {
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            throw "The installed payload is incomplete: '$path'."
        }
    }
    if (@(Get-ChildItem -LiteralPath $installRoot -File -Recurse | Where-Object {
        $_.Name -match 'ffmpeg'
    }).Count -ne 0) {
        throw 'The private-use installer unexpectedly packaged FFmpeg.'
    }
    $extensionManifest = Get-Content -LiteralPath (Join-Path $extensionRoot 'manifest.json') -Raw |
        ConvertFrom-Json
    if ($extensionManifest.manifest_version -ne 3 -or
        $extensionManifest.background.service_worker -ne 'src/service-worker.js' -or
        -not $extensionManifest.key) {
        throw 'The installed unpacked Chromium extension is invalid.'
    }
    Assert-BrowserRegistration $hostPath
    $onboarding = Get-Content -LiteralPath $onboardingPath -Raw | ConvertFrom-Json
    if ($onboarding.schema_version -ne 1 -or
        (@($onboarding.browsers.browser) -join ',') -ne 'chrome,edge' -or
        @($onboarding.browsers | Where-Object {
            $_.extension_id -ne $extensionId -or $_.next_step -ne 'load_packaged_extension'
        }).Count -ne 0) {
        throw 'The installer did not create exact Chrome/Edge onboarding state.'
    }
    Test-NativeConnection $hostPath

    # Exercise the installed repair entry point after both corrupt and missing state.
    [IO.File]::WriteAllText($nativeManifestPath, '{"name":"wrong.owner"}')
    Remove-Item -LiteralPath $chromeRegistration -Recurse -Force
    Set-Item -LiteralPath $edgeRegistration -Value 'C:\stale\native-host.json'
    [IO.File]::WriteAllText($onboardingPath, '{"schema_version":0}')
    & $registrationScript -Action Repair -Browser Chromium -HostPath $hostPath `
        -OnboardingPath $onboardingPath
    Assert-BrowserRegistration $hostPath
    $repairedOnboarding = Get-Content -LiteralPath $onboardingPath -Raw | ConvertFrom-Json
    if ((@($repairedOnboarding.browsers.browser) -join ',') -ne 'chrome,edge') {
        throw 'Repair did not restore exact Chrome/Edge onboarding state.'
    }
    Test-NativeConnection $hostPath
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
    if ((Test-Path -LiteralPath $chromeRegistration) -or
        (Test-Path -LiteralPath $edgeRegistration) -or
        (Test-Path -LiteralPath $nativeManifestPath)) {
        throw 'Uninstall left a Chrome/Edge native-host registration or manifest.'
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

    Write-Host 'PU12 private NSIS browser packaging, install, repair, health, and cleanup smoke passed.'
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
