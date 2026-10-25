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

if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
    throw 'Node.js is required. Install the version pinned in .node-version.'
}
if (-not (Get-Command pnpm -ErrorAction SilentlyContinue)) {
    throw 'pnpm is required. Run: corepack enable'
}

$actualNode = (& node --version).TrimStart('v')
$actualPnpm = (& pnpm --version)
if ($actualNode -ne '24.22.1') {
    throw "Node.js 24.22.1 is required; found $actualNode."
}
if ($actualPnpm -ne '12.15.0') {
    throw "pnpm 12.15.0 is required; found $actualPnpm."
}

Invoke-Checked 'Prettier' { pnpm format:check }
Invoke-Checked 'ESLint' { pnpm lint }
Invoke-Checked 'TypeScript build' { pnpm build }
Invoke-Checked 'TypeScript typecheck' { pnpm typecheck }
Invoke-Checked 'Vitest' { pnpm test }

Write-Host 'TypeScript quality gates passed.'

