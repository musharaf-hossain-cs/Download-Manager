[CmdletBinding()]
param(
    [string] $EvidencePath,
    [switch] $SkipBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = Split-Path -Parent $PSScriptRoot
if (-not $EvidencePath) {
    $EvidencePath = Join-Path $repositoryRoot 'test-results\s182-release-candidate-journey.json'
}
$EvidencePath = [IO.Path]::GetFullPath($EvidencePath)
$results = [Collections.Generic.List[object]]::new()
$currentStage = 'install'
$startedAt = [DateTimeOffset]::UtcNow

function Add-StageResult([string] $Name, [string] $Command, [string] $Coverage) {
    $results.Add([ordered]@{
        name = $Name
        status = 'passed'
        command = $Command
        coverage = $Coverage
    })
}

function Invoke-Checked([string] $Name, [string] $Command, [scriptblock] $Action, [string] $Coverage) {
    $script:currentStage = $Name
    & $Action
    if ($LASTEXITCODE -ne 0) {
        throw "Stage '$Name' failed with exit code $LASTEXITCODE."
    }
    Add-StageResult $Name $Command $Coverage
}

$journey = {
    Add-StageResult 'install' `
        'powershell -File scripts/test-windows-installer.ps1 [-SkipBuild] -BeforeUninstall <journey>' `
        'Built and installed the current-user package in an isolated profile; launched both packaged executables; verified and repaired native-host registration.'

    Invoke-Checked 'capture' `
        'pnpm --filter @download-manager/browser-extension exec vitest run test/link-handoff.test.ts' `
        { pnpm --filter '@download-manager/browser-extension' exec vitest run test/link-handoff.test.ts } `
        'Adapter contract only: acknowledged handoff, cancel-after-accept, rejection/offline fallback, timeout, and duplicate suppression. No installed browser is launched.'

    Invoke-Checked 'resume' `
        'cargo test --locked -p download-manager-engine same_object_resumes_only_the_missing_range' `
        { cargo test --locked -p download-manager-engine same_object_resumes_only_the_missing_range } `
        'Production resume path validates remote identity and requests only the missing suffix from a local HTTP fixture.'

    Invoke-Checked 'licensed_media' `
        'cargo test --locked -p download-manager-engine --test media_corpus_s173 --test media_corpus_s174' `
        { cargo test --locked -p download-manager-engine --test media_corpus_s173 --test media_corpus_s174 } `
        'CC0 progressive, HLS, DASH, subtitle, live-window, hash, duration, and synchronization fixtures.'

    Invoke-Checked 'update' `
        'cargo test --locked -p download-manager-engine update_activation::tests' `
        { cargo test --locked -p download-manager-engine 'update_activation::tests' } `
        'In-process signed staging/activation contracts cover successful activation and forced rollback while preserving paused jobs, queues, offsets, and partials.'

    $script:currentStage = 'uninstall'
}

$status = 'passed'
$failure = $null
Push-Location $repositoryRoot
try {
    $installerArguments = @{ BeforeUninstall = $journey }
    if ($SkipBuild) { $installerArguments.SkipBuild = $true }
    & (Join-Path $PSScriptRoot 'test-windows-installer.ps1') @installerArguments
    Add-StageResult 'uninstall' `
        'packaging/windows/uninstall.ps1 -UserData Keep; reinstall; -UserData Remove' `
        'Existing package smoke verifies both explicit data choices remove app/browser integration while preserving completed downloads.'
} catch {
    $status = 'failed'
    $failure = [ordered]@{ stage = $currentStage; message = $_.Exception.Message }
    throw
} finally {
    Pop-Location
    $finishedAt = [DateTimeOffset]::UtcNow
    $document = [ordered]@{
        schema_version = 1
        slice = 'S182'
        status = $status
        started_at_utc = $startedAt.ToString('o')
        finished_at_utc = $finishedAt.ToString('o')
        duration_seconds = [Math]::Round(($finishedAt - $startedAt).TotalSeconds, 3)
        environment = [ordered]@{
            execution = 'isolated current-runner composed contract gate'
            hosted_windows_workflow = '.github/workflows/release-candidate.yml'
            supported_windows_vm_matrix_claimed = $false
        }
        evidence_boundaries = @(
            'No installed browser capture is exercised.',
            'The production native host AddDownload path uses authenticated current-user backend IPC; installed-browser execution remains outside this composed gate.',
            'No packaged Windows update activation adapter is exercised.',
            'This is not a Windows 10/11 supported-OS clean-VM matrix claim.',
            'S180 requirement blockers and v1 release-readiness gates remain unchanged.'
        )
        stages = @($results)
        failure = $failure
    }
    [IO.Directory]::CreateDirectory((Split-Path -Parent $EvidencePath)) | Out-Null
    [IO.File]::WriteAllText(
        $EvidencePath,
        (($document | ConvertTo-Json -Depth 6) + [Environment]::NewLine),
        [Text.UTF8Encoding]::new($false)
    )
    Write-Host "S182 release-candidate journey evidence: $EvidencePath"
}
