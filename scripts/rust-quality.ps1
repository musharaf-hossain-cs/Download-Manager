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

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'cargo is required. Install the toolchain pinned in rust-toolchain.toml with rustup.'
}

$actualRust = (& rustc --version).Split(' ')[1]
if ($actualRust -ne '1.99.1') {
    throw "Rust 1.99.1 is required; found $actualRust. Run: rustup toolchain install 1.99.1 --component clippy,rustfmt"
}

Invoke-Checked 'cargo fmt' { cargo fmt --all -- --check }
Invoke-Checked 'cargo clippy' { cargo clippy --workspace --all-targets -- -D warnings }
Invoke-Checked 'cargo test' { cargo test --workspace --locked }

Write-Host 'Rust quality gates passed.'

