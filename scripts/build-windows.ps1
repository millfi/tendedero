param([string]$TargetDir, [string]$OutputDir)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$target = if ($TargetDir) { $TargetDir } elseif ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
$output = if ($OutputDir) { $OutputDir } else { Join-Path $repo 'build/Snapline' }
Push-Location $repo
try {
    cargo build --locked --release --target-dir $target
    if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
    New-Item -ItemType Directory -Force $output | Out-Null
    # Stage only the portable executable and its runtime, never Cargo's build/deps directories.
    Get-ChildItem -LiteralPath (Join-Path $target 'release') | Where-Object {
        $_.Name -eq 'snapline.exe' -or $_.Extension -in '.dll','.pri','.ico' -or
        ($_.PSIsContainer -and ($_.Name -match '^[a-z]{2,3}-' -or $_.Name -eq 'Microsoft.UI.Xaml'))
    } | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $output -Recurse -Force }
    Copy-Item -LiteralPath (Join-Path $repo 'LICENSE') -Destination $output
    Copy-Item -LiteralPath (Join-Path $repo 'docs/windows.md') -Destination (Join-Path $output 'README.md')
    Write-Output (Join-Path $output 'snapline.exe')
} finally { Pop-Location }
