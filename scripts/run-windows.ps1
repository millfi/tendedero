param([switch]$Test, [switch]$SlowFlight, [string]$TargetDir)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$target = if ($TargetDir) { $TargetDir } elseif ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
$exe = [System.IO.Path]::GetFullPath((Join-Path $target 'debug/snapline.exe'))
# Only stop instances of this checkout's executable before relinking it.
Get-Process snapline -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe -or $_.Path -eq (Join-Path $repo 'target/debug/snapline.exe') } | Stop-Process
Push-Location $repo
try {
    cargo build --offline --target-dir $target
    if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
    if ($Test) {
        $launchArgs = @('--ui-test', '--data-dir', (Join-Path $repo 'test-data'))
        if ($SlowFlight) { $launchArgs += '--slow-flight' }
        Start-Process -FilePath $exe -ArgumentList $launchArgs -WindowStyle Hidden
    } else {
        Start-Process -FilePath $exe -WindowStyle Hidden
    }
} finally { Pop-Location }
