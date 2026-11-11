[CmdletBinding()]
param(
    [switch] $SkipBuild,
    [scriptblock] $BeforeUninstall
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$buildScript = Join-Path $PSScriptRoot 'build-windows-installer.ps1'
$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) "download-manager-installer-smoke-$([Guid]::NewGuid().ToString('N'))"
$previousLocalAppData = $env:LOCALAPPDATA
$previousAppData = $env:APPDATA

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

function Test-NativeConnection(
    [string] $HostPath,
    [string[]] $Arguments,
    [string] $ExtensionId,
    [string] $Browser
) {
    $requestId = [Guid]::NewGuid().ToString()
    $request = [ordered]@{
        version = 1
        request_id = $requestId
        nonce = 'abcdefghijklmnop'
        extension_id = $ExtensionId
        payload = [ordered]@{ type = 'health' }
    } | ConvertTo-Json -Depth 3 -Compress
    [byte[]] $body = [Text.Encoding]::UTF8.GetBytes($request)
    [byte[]] $header = [BitConverter]::GetBytes([uint32]$body.Length)
    if (-not [BitConverter]::IsLittleEndian) { [Array]::Reverse($header) }

    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $HostPath
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardInput = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($argument in $Arguments) { $startInfo.ArgumentList.Add($argument) }
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
            throw "$Browser did not receive a correlated native-host health response."
        }
        $process.StandardInput.Close()
        if (-not $process.WaitForExit(5000) -or $process.ExitCode -ne 0) {
            throw "$Browser native host did not exit cleanly: $($process.StandardError.ReadToEnd())"
        }
    } finally {
        if (-not $process.HasExited) { $process.Kill($true) }
        $process.Dispose()
    }
}

try {
    $artifactRoot = Join-Path $temporaryRoot 'artifacts'
    $expandedRoot = Join-Path $temporaryRoot 'expanded'
    $registrationRoot = Join-Path $temporaryRoot 'registry'
    $env:LOCALAPPDATA = Join-Path $temporaryRoot 'profile\AppData\Local'
    $env:APPDATA = Join-Path $temporaryRoot 'profile\AppData\Roaming'

    $buildArguments = @{ OutputDirectory = $artifactRoot }
    if ($SkipBuild) {
        $buildArguments.SkipBuild = $true
    }
    $archivePath = & $buildScript @buildArguments
    if (@($archivePath).Count -ne 1 -or -not (Test-Path -LiteralPath $archivePath -PathType Leaf)) {
        throw 'The installer build did not produce exactly one archive.'
    }

    Expand-Archive -LiteralPath $archivePath -DestinationPath $expandedRoot
    $manifest = Get-Content -LiteralPath (Join-Path $expandedRoot 'package.json') -Raw |
        ConvertFrom-Json
    if ($manifest.install_scope -ne 'current_user' -or @($manifest.files).Count -ne 4) {
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

    $installRoot = & (Join-Path $expandedRoot 'install.ps1') -NoShortcut -PassThru `
        -RegistrationRoot $registrationRoot
    $expectedRoot = Join-Path $env:LOCALAPPDATA 'Programs\Download Manager'
    if ($installRoot -ne $expectedRoot -or -not (Test-Path -LiteralPath $installRoot -PathType Container)) {
        throw 'The package did not install beneath the current-user profile.'
    }
    foreach ($name in @(
        'download-manager.exe',
        'download-manager-native-host.exe',
        'native-host-registration.ps1',
        'uninstall.ps1',
        'browser-onboarding.json',
        'install.json'
    )) {
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

    $manifestRoot = Join-Path $env:LOCALAPPDATA 'DownloadManager\NativeMessagingHosts'
    $chromiumManifestPath = Join-Path $manifestRoot `
        'io.github.musharaf_hossain_cs.download_manager.json'
    $firefoxManifestPath = Join-Path $manifestRoot `
        'io.github.musharaf_hossain_cs.download_manager.firefox.json'
    $chromiumManifest = Get-Content -LiteralPath $chromiumManifestPath -Raw | ConvertFrom-Json
    $firefoxManifest = Get-Content -LiteralPath $firefoxManifestPath -Raw | ConvertFrom-Json
    $hostPath = Join-Path $installRoot 'download-manager-native-host.exe'
    $chromiumExtensionId = 'ackbonlbpniilcfmmnfdohnhbdbajhii'
    $firefoxExtensionId = 'download-manager@musharaf-hossain-cs.github.io'
    if ($chromiumManifest.path -ne $hostPath -or
        @($chromiumManifest.allowed_origins).Count -ne 1 -or
        $chromiumManifest.allowed_origins[0] -ne "chrome-extension://$chromiumExtensionId/" -or
        $firefoxManifest.path -ne $hostPath -or
        @($firefoxManifest.allowed_extensions).Count -ne 1 -or
        $firefoxManifest.allowed_extensions[0] -ne $firefoxExtensionId) {
        throw 'The installer produced invalid packaged native-host manifests.'
    }

    $registrationExpectations = @{
        'Google\Chrome\NativeMessagingHosts' = $chromiumManifestPath
        'Microsoft\Edge\NativeMessagingHosts' = $chromiumManifestPath
        'Mozilla\NativeMessagingHosts' = $firefoxManifestPath
    }
    foreach ($entry in $registrationExpectations.GetEnumerator()) {
        $marker = Join-Path $registrationRoot `
            "$($entry.Key)\io.github.musharaf_hossain_cs.download_manager\.default-value"
        if ((Get-Content -LiteralPath $marker -Raw) -ne $entry.Value) {
            throw "The packaged browser registration is invalid: $($entry.Key)"
        }
    }

    $onboarding = Get-Content -LiteralPath `
        (Join-Path $installRoot 'browser-onboarding.json') -Raw | ConvertFrom-Json
    if ($onboarding.schema_version -ne 1 -or @($onboarding.browsers).Count -ne 3 -or
        (@($onboarding.browsers.browser) -join ',') -ne 'chrome,edge,firefox' -or
        @($onboarding.browsers | Where-Object {
            $_.state -ne 'native_host_registered' -or
            $_.next_step -ne 'install_or_enable_extension'
        }).Count -ne 0) {
        throw 'The installer did not produce complete three-browser onboarding state.'
    }

    $origin = "chrome-extension://$chromiumExtensionId/"
    Test-NativeConnection $hostPath @($origin) $chromiumExtensionId 'Chrome'
    Test-NativeConnection $hostPath @($origin) $chromiumExtensionId 'Edge'
    Test-NativeConnection $hostPath @($firefoxManifestPath, $firefoxExtensionId) `
        $firefoxExtensionId 'Firefox'

    # Seed a mixture of missing and corrupt state, then exercise the installed
    # repair entry point rather than rerunning the package installer.
    [IO.File]::WriteAllText($chromiumManifestPath, '{"name":"wrong.owner"}')
    Remove-Item -LiteralPath $firefoxManifestPath -Force
    $chromeMarker = Join-Path $registrationRoot `
        'Google\Chrome\NativeMessagingHosts\io.github.musharaf_hossain_cs.download_manager\.default-value'
    [IO.File]::WriteAllText($chromeMarker, 'C:\stale\native-host.json')
    $edgeKey = Join-Path $registrationRoot `
        'Microsoft\Edge\NativeMessagingHosts\io.github.musharaf_hossain_cs.download_manager'
    $firefoxKey = Join-Path $registrationRoot `
        'Mozilla\NativeMessagingHosts\io.github.musharaf_hossain_cs.download_manager'
    Remove-Item -LiteralPath $edgeKey -Recurse -Force
    Remove-Item -LiteralPath $firefoxKey -Recurse -Force
    [IO.File]::WriteAllText(
        (Join-Path $installRoot 'browser-onboarding.json'),
        '{"schema_version":0}'
    )

    & (Join-Path $installRoot 'native-host-registration.ps1') -Action Repair `
        -HostPath $hostPath -OnboardingPath (Join-Path $installRoot 'browser-onboarding.json') `
        -RegistrationRoot $registrationRoot

    $repairedChromium = Get-Content -LiteralPath $chromiumManifestPath -Raw | ConvertFrom-Json
    $repairedFirefox = Get-Content -LiteralPath $firefoxManifestPath -Raw | ConvertFrom-Json
    if ($repairedChromium.name -ne 'io.github.musharaf_hossain_cs.download_manager' -or
        $repairedChromium.path -ne $hostPath -or
        @($repairedChromium.allowed_origins).Count -ne 1 -or
        $repairedChromium.allowed_origins[0] -ne "chrome-extension://$chromiumExtensionId/" -or
        $repairedFirefox.name -ne 'io.github.musharaf_hossain_cs.download_manager' -or
        $repairedFirefox.path -ne $hostPath -or
        @($repairedFirefox.allowed_extensions).Count -ne 1 -or
        $repairedFirefox.allowed_extensions[0] -ne $firefoxExtensionId) {
        throw 'Repair did not restore exact packaged native-host manifests.'
    }
    foreach ($entry in $registrationExpectations.GetEnumerator()) {
        $marker = Join-Path $registrationRoot `
            "$($entry.Key)\io.github.musharaf_hossain_cs.download_manager\.default-value"
        if ((Get-Content -LiteralPath $marker -Raw) -ne $entry.Value) {
            throw "Repair did not restore the packaged browser registration: $($entry.Key)"
        }
    }
    $repairedOnboarding = Get-Content -LiteralPath `
        (Join-Path $installRoot 'browser-onboarding.json') -Raw | ConvertFrom-Json
    if ($repairedOnboarding.schema_version -ne 1 -or
        (@($repairedOnboarding.browsers.browser) -join ',') -ne 'chrome,edge,firefox') {
        throw 'Repair did not restore three-browser onboarding state.'
    }
    Test-NativeConnection $hostPath @($origin) $chromiumExtensionId 'repaired Chrome'
    Test-NativeConnection $hostPath @($origin) $chromiumExtensionId 'repaired Edge'
    Test-NativeConnection $hostPath @($firefoxManifestPath, $firefoxExtensionId) `
        $firefoxExtensionId 'repaired Firefox'

    if ($BeforeUninstall) {
        & $BeforeUninstall
    }

    $uninstallPath = Join-Path $installRoot 'uninstall.ps1'
    $userDataParameter = (Get-Command $uninstallPath).Parameters['UserData']
    if (@($userDataParameter.Attributes | Where-Object {
        $_ -is [Management.Automation.ParameterAttribute] -and $_.Mandatory
    }).Count -ne 1) {
        throw 'Uninstall must require an explicit user-data choice.'
    }

    $stateRoot = Join-Path $env:LOCALAPPDATA 'DownloadManager\State'
    [IO.Directory]::CreateDirectory((Join-Path $stateRoot 'partials')) | Out-Null
    [IO.File]::WriteAllText((Join-Path $stateRoot 'settings.json'), 'settings')
    [IO.File]::WriteAllText((Join-Path $stateRoot 'history.sqlite'), 'history')
    [IO.File]::WriteAllText((Join-Path $stateRoot 'partials\job.part'), 'partial')
    $completedPath = Join-Path $temporaryRoot 'Downloads\completed.bin'
    [IO.Directory]::CreateDirectory((Split-Path -Parent $completedPath)) | Out-Null
    [IO.File]::WriteAllText($completedPath, 'completed-download')
    $nestedCompletedPath = Join-Path $env:LOCALAPPDATA `
        'DownloadManager\user-downloads\completed-inside-data-root.bin'
    [IO.Directory]::CreateDirectory((Split-Path -Parent $nestedCompletedPath)) | Out-Null
    [IO.File]::WriteAllText($nestedCompletedPath, 'nested-completed-download')
    $shortcutPath = Join-Path $env:APPDATA `
        'Microsoft\Windows\Start Menu\Programs\Download Manager.lnk'
    [IO.Directory]::CreateDirectory((Split-Path -Parent $shortcutPath)) | Out-Null
    [IO.File]::WriteAllText($shortcutPath, 'shortcut')
    $registrationSentinel = Join-Path $registrationRoot 'unrelated-registration.txt'
    [IO.File]::WriteAllText($registrationSentinel, 'keep')

    & $uninstallPath -UserData Keep -RegistrationRoot $registrationRoot
    if (Test-Path -LiteralPath $installRoot) { throw 'Keep uninstall left application files.' }
    if (Test-Path -LiteralPath $shortcutPath) { throw 'Keep uninstall left the Start Menu shortcut.' }
    if ((Test-Path -LiteralPath $chromiumManifestPath) -or
        (Test-Path -LiteralPath $firefoxManifestPath)) {
        throw 'Keep uninstall left a native-host manifest.'
    }
    foreach ($entry in $registrationExpectations.GetEnumerator()) {
        $key = Join-Path $registrationRoot `
            "$($entry.Key)\io.github.musharaf_hossain_cs.download_manager"
        if (Test-Path -LiteralPath $key) {
            throw "Keep uninstall left a browser registration: $($entry.Key)"
        }
    }
    foreach ($name in @('settings.json', 'history.sqlite', 'partials\job.part')) {
        if (-not (Test-Path -LiteralPath (Join-Path $stateRoot $name) -PathType Leaf)) {
            throw "Keep uninstall removed retained user data: $name"
        }
    }
    if ((Get-Content -LiteralPath $completedPath -Raw) -ne 'completed-download') {
        throw 'Keep uninstall changed a completed download.'
    }
    if ((Get-Content -LiteralPath $nestedCompletedPath -Raw) -ne 'nested-completed-download') {
        throw 'Keep uninstall changed a completed download inside the application data root.'
    }
    if ((Get-Content -LiteralPath $registrationSentinel -Raw) -ne 'keep') {
        throw 'Keep uninstall changed an unrelated registration.'
    }

    # Reinstall over the retained data, then exercise the explicit removal
    # choice. Completed downloads remain outside every owned cleanup root.
    $installRoot = & (Join-Path $expandedRoot 'install.ps1') -NoShortcut -PassThru `
        -RegistrationRoot $registrationRoot
    $uninstallPath = Join-Path $installRoot 'uninstall.ps1'
    [IO.File]::WriteAllText($shortcutPath, 'shortcut')
    & $uninstallPath -UserData Remove -RegistrationRoot $registrationRoot
    if (Test-Path -LiteralPath $installRoot) { throw 'Remove uninstall left application files.' }
    if (Test-Path -LiteralPath $stateRoot) {
        throw 'Remove uninstall retained owned history, settings, or partial data.'
    }
    if (Test-Path -LiteralPath $shortcutPath) { throw 'Remove uninstall left the Start Menu shortcut.' }
    foreach ($entry in $registrationExpectations.GetEnumerator()) {
        $key = Join-Path $registrationRoot `
            "$($entry.Key)\io.github.musharaf_hossain_cs.download_manager"
        if (Test-Path -LiteralPath $key) {
            throw "Remove uninstall left a browser registration: $($entry.Key)"
        }
    }
    if ((Get-Content -LiteralPath $completedPath -Raw) -ne 'completed-download') {
        throw 'Remove uninstall changed a completed download.'
    }
    if ((Get-Content -LiteralPath $nestedCompletedPath -Raw) -ne 'nested-completed-download') {
        throw 'Remove uninstall changed a completed download inside the application data root.'
    }
    if ((Get-Content -LiteralPath $registrationSentinel -Raw) -ne 'keep') {
        throw 'Remove uninstall changed an unrelated registration.'
    }
    Write-Host 'Per-user install, repair, and explicit-choice uninstall smoke test passed.'
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
