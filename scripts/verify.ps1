[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

node scripts/verify-markdown.mjs
if ($LASTEXITCODE -ne 0) { throw 'Documentation verification failed.' }

node scripts/verify-status.mjs
if ($LASTEXITCODE -ne 0) { throw 'Status verification failed.' }

& "$PSScriptRoot/rust-quality.ps1"
& "$PSScriptRoot/typescript-quality.ps1"

Write-Host 'All fast verification gates passed.'

