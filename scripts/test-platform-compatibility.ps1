[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidateSet(
        'windows-10-22h2-x64',
        'windows-11-23h2-x64',
        'windows-11-24h2-x64',
        'windows-11-25h2-x64',
        'windows-11-26h2-x64',
        'windows-11-26h1-arm64-x64-emulation'
    )]
    [string] $Profile,
    [string] $EvidencePath,
    [switch] $EnvironmentOnly
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = Split-Path -Parent $PSScriptRoot
if (-not $EvidencePath) {
    $EvidencePath = Join-Path $repositoryRoot "test-results\platform-$Profile.json"
}
$EvidencePath = [IO.Path]::GetFullPath($EvidencePath)
$startedAt = [DateTimeOffset]::UtcNow
$status = 'failed'
$failure = $null

$profiles = @{
    'windows-10-22h2-x64' = [ordered]@{
        product_name = 'Windows 10'
        release = '22H2'
        build = 19045
        os_architecture = 'X64'
        emulation = $false
    }
    'windows-11-23h2-x64' = [ordered]@{
        product_name = 'Windows 11'
        release = '23H2'
        build = 22631
        os_architecture = 'X64'
        emulation = $false
    }
    'windows-11-24h2-x64' = [ordered]@{
        product_name = 'Windows 11'
        release = '24H2'
        build = 26100
        os_architecture = 'X64'
        emulation = $false
    }
    'windows-11-25h2-x64' = [ordered]@{
        product_name = 'Windows 11'
        release = '25H2'
        build = 26200
        os_architecture = 'X64'
        emulation = $false
    }
    'windows-11-26h2-x64' = [ordered]@{
        product_name = 'Windows 11'
        release = '26H2'
        build = 26300
        os_architecture = 'X64'
        emulation = $false
    }
    'windows-11-26h1-arm64-x64-emulation' = [ordered]@{
        product_name = 'Windows 11'
        release = '26H1'
        build = 28000
        os_architecture = 'Arm64'
        emulation = $true
    }
}

$expected = $profiles[$Profile]
$actual = $null
try {
    if (-not $IsWindows) { throw 'The compatibility matrix can run only on Windows.' }
    $operatingSystem = Get-CimInstance -ClassName Win32_OperatingSystem
    $actualBuild = [int] $operatingSystem.BuildNumber
    $actualArchitecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
    $actual = [ordered]@{
        caption = [string] $operatingSystem.Caption
        version = [string] $operatingSystem.Version
        build = $actualBuild
        os_architecture = $actualArchitecture
        process_architecture = [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture.ToString()
    }
    if ($actualBuild -ne $expected.build) {
        throw "Profile '$Profile' requires OS build $($expected.build); found $actualBuild."
    }
    if ($actualArchitecture -cne $expected.os_architecture) {
        throw "Profile '$Profile' requires $($expected.os_architecture); found $actualArchitecture."
    }

    if (-not $EnvironmentOnly) {
        & (Join-Path $PSScriptRoot 'test-windows-installer.ps1')
        if ($LASTEXITCODE -ne 0) {
            throw "The installed x64 product smoke failed with exit code $LASTEXITCODE."
        }
    }
    $status = if ($EnvironmentOnly) { 'environment-only' } else { 'passed' }
} catch {
    $failure = [ordered]@{ message = $_.Exception.Message }
    throw
} finally {
    $finishedAt = [DateTimeOffset]::UtcNow
    $evidence = [ordered]@{
        schema_version = 1
        profile = $Profile
        status = $status
        started_at_utc = $startedAt.ToString('o')
        finished_at_utc = $finishedAt.ToString('o')
        expected = $expected
        actual = $actual
        package_architecture = 'x86_64'
        x64_emulation_required = $expected.emulation
        installed_product_smoke = (-not $EnvironmentOnly -and $status -eq 'passed')
        failure = $failure
    }
    $parent = Split-Path -Parent $EvidencePath
    [IO.Directory]::CreateDirectory($parent) | Out-Null
    [IO.File]::WriteAllText(
        $EvidencePath,
        (($evidence | ConvertTo-Json -Depth 6) + [Environment]::NewLine),
        [Text.UTF8Encoding]::new($false)
    )
}
