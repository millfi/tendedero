$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'msix-registration.ps1')

# No real processes or packages are touched. These command doubles exercise the
# same helpers that the production script uses, without requiring Pester.
$script:actions = [Collections.Generic.List[string]]::new()
$script:packages = @()
$script:processes = @()
function Get-AppxPackage {
    [CmdletBinding()]param([string]$Name)
    $script:packages
}
function Get-Process {
    [CmdletBinding()]param([string]$Name)
    $script:processes
}
function Stop-Process {
    [CmdletBinding()]param($InputObject)
    $script:actions.Add("stop:$($InputObject.Id)")
}
function Remove-AppxPackage {
    [CmdletBinding()]param([string]$Package)
    $script:actions.Add("remove:$Package")
}
function New-TestProcess([int]$ProcessId, [string]$Path, [bool]$Exits = $true) {
    $process = [pscustomobject]@{ Id = $ProcessId; Path = $Path; Exits = $Exits }
    $process | Add-Member -MemberType ScriptMethod -Name WaitForExit -Value {
        param([int]$Timeout)
        $script:actions.Add("wait:$($this.Id)")
        return $this.Exits
    }
    return $process
}
function Assert-Equal($Actual, $Expected, [string]$Case) {
    if ($Actual -ne $Expected) { throw "$Case : expected '$Expected', got '$Actual'" }
}

$testBase = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../target/registration-test-fixtures'))
$layout = Join-Path $testBase 'msix-layout'
$registered = Join-Path $testBase 'Snapline-msix-layout'
$oldInstall = Join-Path $testBase 'old-registration'
$script:packages = @(
    [pscustomobject]@{ Name = 'Snapline.Shelf'; PackageFullName = 'Snapline.Shelf_old'; InstallLocation = $oldInstall },
    [pscustomobject]@{ Name = 'Other.App'; PackageFullName = 'Other.App_old'; InstallLocation = (Join-Path $testBase 'unrelated') }
)
$script:processes = @(
    (New-TestProcess 101 (Join-Path $registered 'snapline.exe')),
    (New-TestProcess 102 (Join-Path $testBase 'other-checkout/snapline.exe')),
    (New-TestProcess 103 (Join-Path $oldInstall 'snapline.exe'))
)
Initialize-DevelopmentRegistration -PackageName 'Snapline.Shelf' -LayoutPaths @($layout, $registered)
Assert-Equal ($script:actions -join '|') 'stop:101|wait:101|stop:103|wait:103|remove:Snapline.Shelf_old' 'stop and wait before unregister; leave other checkouts alone'

$script:actions.Clear()
$script:packages = @()
Initialize-DevelopmentRegistration -PackageName 'Snapline.Shelf' -LayoutPaths @($layout, $registered)
Assert-Equal ($script:actions -join '|') 'stop:101|wait:101' 'retry after a partial registration'

$script:actions.Clear()
$script:packages = @([pscustomobject]@{ Name = 'Snapline.Shelf'; PackageFullName = 'Snapline.Shelf_old'; InstallLocation = $oldInstall })
$script:processes = @((New-TestProcess 104 (Join-Path $registered 'snapline.exe') $false))
$failed = $false
try { Initialize-DevelopmentRegistration -PackageName 'Snapline.Shelf' -LayoutPaths @($layout, $registered) }
catch { $failed = $true }
Assert-Equal $failed $true 'stop if the process does not exit'
Assert-Equal ($script:actions -join '|') 'stop:104|wait:104' 'do not unregister on shutdown failure'

foreach ($wrongPath in @($testBase, (Join-Path $testBase '../outside'), (Join-Path $testBase 'other-layout'))) {
    $failed = $false
    try { Remove-StagingLayout -Path $wrongPath -ParentPath $testBase -DirectoryName 'msix-layout' }
    catch { $failed = $true }
    Assert-Equal $failed $true 'refuse paths outside the intended staging directory'
}
Write-Output 'MSIX registration checks passed (4 cases).'
