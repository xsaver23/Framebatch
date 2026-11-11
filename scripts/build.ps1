param([string]$ToolchainRoot)
$ErrorActionPreference = 'Stop'
if ($ToolchainRoot) {
    $ToolchainRoot = (Resolve-Path -LiteralPath $ToolchainRoot).Path
    $env:CARGO_HOME = Join-Path $ToolchainRoot 'cargo'
    $env:RUSTUP_HOME = Join-Path $ToolchainRoot 'rustup'
    $env:PATH = "$env:CARGO_HOME/bin;$env:PATH"
}
Push-Location (Split-Path $PSScriptRoot)
try {
    npm.cmd ci
    if ($LASTEXITCODE) { throw 'npm ci failed' }
    cargo test --locked --release --manifest-path src-tauri/Cargo.toml
    if ($LASTEXITCODE) { throw 'Rust tests failed' }
    cargo build --locked --release --bins --manifest-path src-tauri/Cargo.toml
    if ($LASTEXITCODE) { throw 'Rust build failed' }
    npm.cmd run build -- --locked
    if ($LASTEXITCODE) { throw 'NSIS build failed' }
} finally { Pop-Location }
