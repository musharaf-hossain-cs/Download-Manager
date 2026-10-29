[CmdletBinding()]
param([switch] $SkipBuild)

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
        $buildArguments.BinaryDirectory = Join-Path $repositoryRoot 'target\release'
    }
    $archivePath = & $buildScript @buildArguments
    if (@($archivePath).Count -ne 1 -or -not (Test-Path -LiteralPath $archivePath -PathType Leaf)) {
        throw 'The installer build did not produce exactly one archive.'
    }

    Expand-Archive -LiteralPath $archivePath -DestinationPath $expandedRoot
    $manifest = Get-Content -LiteralPath (Join-Path $expandedRoot 'package.json') -Raw |
        ConvertFrom-Json
    if ($manifest.install_scope -ne 'current_user' -or @($manifest.files).Count -ne 3) {
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
    $chromiumExtensionId = 'abcdefghijklmnopabcdefghijklmnop'
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
    Write-Host 'Per-user installer and three-browser native-host smoke test passed.'
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
