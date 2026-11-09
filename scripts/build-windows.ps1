$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath (Join-Path $PSScriptRoot '..')
$researchRoot = (Get-Location).Path
$researchFlags = @('--remap-path-prefix', "$researchRoot=linkdesk", '--remap-path-prefix', "$env:USERPROFILE=local-user")
if ($env:CARGO_ENCODED_RUSTFLAGS) {
    $researchFlags = @($env:CARGO_ENCODED_RUSTFLAGS -split [char]31) + $researchFlags
} elseif ($env:RUSTFLAGS) {
    $researchFlags = @($env:RUSTFLAGS -split '\s+') + $researchFlags
}
$env:CARGO_ENCODED_RUSTFLAGS = $researchFlags -join [char]31
cargo build --release --locked --target-dir target
if ($LASTEXITCODE -ne 0) { throw 'Rust build failed' }
New-Item -ItemType Directory -Path '.\dist\windows' -Force | Out-Null
Copy-Item -LiteralPath '.\target\release\linkdesk.exe' -Destination '.\dist\windows\linkdesk.exe'
# GNU development toolchains may need their own runtime DLLs next to the EXE.
# MSVC release builds in CI use the static CRT.
$researchCompiler = Get-Command g++ -ErrorAction SilentlyContinue
if ($null -ne $researchCompiler) {
    foreach ($researchRuntime in @('libstdc++-6.dll', 'libgcc_s_seh-1.dll', 'libwinpthread-1.dll')) {
        $researchRuntimePath = & $researchCompiler.Source "-print-file-name=$researchRuntime"
        if (Test-Path -LiteralPath $researchRuntimePath -PathType Leaf) {
            Copy-Item -LiteralPath $researchRuntimePath -Destination '.\dist\windows'
        }
    }
}
Write-Output 'Built dist/windows/linkdesk.exe'
