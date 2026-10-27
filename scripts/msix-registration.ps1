# Helpers shared by build-windows.ps1 and its registration regression checks.
# Importing this file does not stop processes or change package registrations.
function Initialize-DevelopmentRegistration {
    param(
        [Parameter(Mandatory)][string]$PackageName,
        [Parameter(Mandatory)][string[]]$LayoutPaths
    )
    $packages = @(Get-AppxPackage -Name $PackageName -ErrorAction Stop |
        Where-Object { $_.Name -eq $PackageName })
    $directories = @($LayoutPaths) + @($packages | ForEach-Object { $_.InstallLocation })
    $executables = @($directories | Where-Object { $_ } | ForEach-Object {
        [System.IO.Path]::GetFullPath((Join-Path $_ 'snapline.exe'))
    })
    foreach ($snaplineProcess in @(Get-Process -Name snapline -ErrorAction SilentlyContinue)) {
        # A process whose executable cannot be identified must not be killed
        # based on its name alone. Unregistration will report any remaining use.
        try { $processPath = $snaplineProcess.Path } catch { continue }
        if (-not $processPath) { continue }
        $processPath = [System.IO.Path]::GetFullPath($processPath)
        if ($executables -notcontains $processPath) { continue }
        Write-Host "登録対象の Snapline を停止します (PID: $($snaplineProcess.Id))"
        Stop-Process -InputObject $snaplineProcess -ErrorAction Stop
        if (-not $snaplineProcess.WaitForExit(10000)) {
            throw "Snapline の終了を確認できませんでした (PID: $($snaplineProcess.Id))。配置ファイルは削除していません。"
        }
    }
    # Release deployment locks before removing or replacing any registered file.
    foreach ($package in $packages) {
        Remove-AppxPackage -Package $package.PackageFullName -ErrorAction Stop
    }
}

function Remove-StagingLayout {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$ParentPath,
        [Parameter(Mandatory)][string]$DirectoryName
    )
    $parent = [System.IO.Path]::GetFullPath($ParentPath).TrimEnd('\', '/')
    $resolved = [System.IO.Path]::GetFullPath($Path).TrimEnd('\', '/')
    $expected = [System.IO.Path]::GetFullPath((Join-Path $parent $DirectoryName)).TrimEnd('\', '/')
    if (-not $resolved.Equals($expected, [StringComparison]::OrdinalIgnoreCase) -or
        -not $resolved.StartsWith($parent + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "想定した配置フォルダー以外は削除できません: $resolved"
    }
    if (Test-Path -LiteralPath $resolved) {
        $entry = Get-Item -LiteralPath $resolved -Force
        if (-not $entry.PSIsContainer -or ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "配置フォルダーが通常のディレクトリではありません: $resolved"
        }
        Remove-Item -LiteralPath $resolved -Recurse -Force -ErrorAction Stop
    }
}
