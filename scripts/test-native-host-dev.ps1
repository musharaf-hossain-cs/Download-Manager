[CmdletBinding()]
param([string] $HostPath)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$extensionId = 'abcdefghijklmnopabcdefghijklmnop'
$origin = "chrome-extension://$extensionId/"
$firefoxExtensionId = 'download-manager@musharaf-hossain-cs.github.io'
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$registrationScript = Join-Path $PSScriptRoot 'native-host-dev.ps1'

if (-not $HostPath) {
    $candidates = @(
        (Join-Path $repositoryRoot 'target\debug\download-manager-native-host.exe'),
        (Join-Path $repositoryRoot 'target\x86_64-pc-windows-gnu\debug\download-manager-native-host.exe')
    )
    $HostPath = $candidates |
        Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
        ForEach-Object { Get-Item -LiteralPath $_ } |
        Sort-Object LastWriteTimeUtc -Descending |
        Select-Object -First 1 -ExpandProperty FullName
}
if (-not $HostPath) {
    throw 'Build the native host first or pass -HostPath.'
}
$HostPath = (Resolve-Path -LiteralPath $HostPath).Path

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

$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) "download-manager-native-host-smoke-$([Guid]::NewGuid().ToString('N'))"
$registrationRoot = Join-Path $temporaryRoot 'registry'
$manifestPath = Join-Path $temporaryRoot 'manifest\native-host.json'
$sentinelPath = Join-Path $registrationRoot 'keep.txt'
$process = $null

try {
    [IO.Directory]::CreateDirectory($registrationRoot) | Out-Null
    [IO.File]::WriteAllText($sentinelPath, 'preserve root')
    & $registrationScript -Action Register -HostPath $HostPath -ExtensionId $extensionId `
        -ManifestPath $manifestPath -Browser All -RegistrationRoot $registrationRoot

    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    if ($manifest.name -ne 'io.github.musharaf_hossain_cs.download_manager' -or
        $manifest.path -ne $HostPath -or $manifest.type -ne 'stdio' -or
        @($manifest.allowed_origins).Count -ne 1 -or $manifest.allowed_origins[0] -ne $origin) {
        throw 'Generated native-host manifest is invalid.'
    }
    foreach ($relativeKey in @(
        'Google\Chrome\NativeMessagingHosts\io.github.musharaf_hossain_cs.download_manager',
        'Microsoft\Edge\NativeMessagingHosts\io.github.musharaf_hossain_cs.download_manager'
    )) {
        $marker = Join-Path (Join-Path $registrationRoot $relativeKey) '.default-value'
        if ((Get-Content -LiteralPath $marker -Raw) -ne $manifestPath) {
            throw "Registration marker is invalid: $marker"
        }
    }

    $requestId = [Guid]::NewGuid().ToString()
    $request = [ordered]@{
        version = 1
        request_id = $requestId
        nonce = 'abcdefghijklmnop'
        extension_id = $extensionId
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
    $startInfo.Arguments = $origin
    $process = [Diagnostics.Process]::Start($startInfo)
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
        throw 'Native-host health response is invalid or uncorrelated.'
    }
    $process.StandardInput.Close()
    if (-not $process.WaitForExit(5000) -or $process.ExitCode -ne 0) {
        throw "Native host did not exit cleanly: $($process.StandardError.ReadToEnd())"
    }

    & $registrationScript -Action Unregister -ExtensionId $extensionId `
        -ManifestPath $manifestPath -Browser All -RegistrationRoot $registrationRoot
    if (Test-Path -LiteralPath $manifestPath) { throw 'Manifest survived unregister.' }
    if (-not (Test-Path -LiteralPath $sentinelPath -PathType Leaf)) {
        throw 'Unregister removed data outside its exact targets.'
    }
    if (@(Get-ChildItem -LiteralPath $registrationRoot -Recurse -Filter '.default-value').Count -ne 0) {
        throw 'A registration marker survived unregister.'
    }

    $firefoxManifestPath = Join-Path $temporaryRoot 'manifest\native-host.firefox.json'
    & $registrationScript -Action Register -HostPath $HostPath -ExtensionId $firefoxExtensionId `
        -ManifestPath $firefoxManifestPath -Browser Firefox -RegistrationRoot $registrationRoot
    $firefoxManifest = Get-Content -LiteralPath $firefoxManifestPath -Raw | ConvertFrom-Json
    if ($firefoxManifest.name -ne 'io.github.musharaf_hossain_cs.download_manager' -or
        $firefoxManifest.path -ne $HostPath -or $firefoxManifest.type -ne 'stdio' -or
        @($firefoxManifest.allowed_extensions).Count -ne 1 -or
        $firefoxManifest.allowed_extensions[0] -ne $firefoxExtensionId -or
        $firefoxManifest.PSObject.Properties.Name -contains 'allowed_origins') {
        throw 'Generated Firefox native-host manifest is invalid.'
    }
    $firefoxMarker = Join-Path $registrationRoot `
        'Mozilla\NativeMessagingHosts\io.github.musharaf_hossain_cs.download_manager\.default-value'
    if ((Get-Content -LiteralPath $firefoxMarker -Raw) -ne $firefoxManifestPath) {
        throw "Firefox registration marker is invalid: $firefoxMarker"
    }
    & $registrationScript -Action Unregister -ExtensionId $firefoxExtensionId `
        -ManifestPath $firefoxManifestPath -Browser Firefox -RegistrationRoot $registrationRoot
    if (Test-Path -LiteralPath $firefoxManifestPath) { throw 'Firefox manifest survived unregister.' }
    Write-Host 'Native-host register/connect/unregister smoke test passed.'
} finally {
    if ($process -and -not $process.HasExited) { $process.Kill($true) }
    $resolvedTemporaryRoot = [IO.Path]::GetFullPath($temporaryRoot)
    $systemTemporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    if ($resolvedTemporaryRoot.StartsWith($systemTemporaryRoot, [StringComparison]::OrdinalIgnoreCase) -and
        (Split-Path -Leaf $resolvedTemporaryRoot).StartsWith('download-manager-native-host-smoke-')) {
        Remove-Item -LiteralPath $resolvedTemporaryRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
