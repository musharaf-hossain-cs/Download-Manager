[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-Checked {
    param(
        [Parameter(Mandatory)] [string] $Name,
        [Parameter(Mandatory)] [scriptblock] $Command
    )

    Write-Host "==> $Name"
    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$Name failed with exit code $LASTEXITCODE"
    }
}

foreach ($tool in 'cargo-audit', 'cargo-deny') {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        throw "$tool is required for the security gate. See .github/workflows/security.yml for pinned installation commands."
    }
}

Invoke-Checked 'Rust advisories' { cargo audit --deny warnings }
Invoke-Checked 'Rust dependency policy' { cargo deny check bans licenses sources }
Invoke-Checked 'Node advisories' { pnpm audit:dependencies }
Invoke-Checked 'Node licenses' { pnpm audit:licenses }

Write-Host 'Dependency and license gates passed.'

