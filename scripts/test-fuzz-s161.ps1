[CmdletBinding()]
param(
    [ValidateRange(1, 86400)]
    [int] $DurationSeconds = 1200,

    [ValidateRange(1, 1000000)]
    [int] $CasesPerPass = 256
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'cargo is required. Install the toolchain pinned in rust-toolchain.toml with rustup.'
}

$regressions = Join-Path $PSScriptRoot '..\crates\download-manager-engine\tests\fuzz_s161.proptest-regressions'
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$passes = 0
$previousCases = $env:PROPTEST_CASES

try {
    $env:PROPTEST_CASES = $CasesPerPass.ToString([Globalization.CultureInfo]::InvariantCulture)
    do {
        $ErrorActionPreference = 'Continue'
        $passOutput = & cargo test --locked -p download-manager-engine --test fuzz_s161 -- --test-threads=1 2>&1
        $passExitCode = $LASTEXITCODE
        $ErrorActionPreference = 'Stop'
        if ($passExitCode -ne 0) {
            $passOutput | Write-Host
            throw "S161 fuzz pass failed; minimized regressions are saved in $regressions"
        }
        $passes++
    } while ($stopwatch.Elapsed.TotalSeconds -lt $DurationSeconds)
}
finally {
    $env:PROPTEST_CASES = $previousCases
}

Write-Host "S161 fuzz smoke passed $passes randomized pass(es) in $([Math]::Round($stopwatch.Elapsed.TotalSeconds, 1)) seconds."
Write-Host "Persisted regression corpus: $regressions"
