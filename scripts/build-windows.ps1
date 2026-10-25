<#
.SYNOPSIS
Release ビルドを MSIX パッケージにまとめる。

.DESCRIPTION
`--features msix` でビルドした実行ファイルを、Windows App Runtime 2 の
フレームワークパッケージを依存先とする MSIX にパッケージングする。
WinUI のランタイムはパッケージに同梱せず、インストール済みの
Microsoft.WindowsAppRuntime.2 を共有する。

.PARAMETER TargetDir
Cargo のビルド先。既定はリポジトリの target。

.PARAMETER OutputDir
MSIX の出力先。既定はリポジトリの build。

.PARAMETER Publisher
AppxManifest の Identity/Publisher。証明書を渡した場合は証明書の Subject を使う。

.PARAMETER PublisherDisplayName
AppxManifest の PublisherDisplayName。既定は Publisher の CN。

.PARAMETER CertificatePath
署名に使う PFX。指定すると Publisher はこの証明書の Subject になる。

.PARAMETER CertificatePassword
PFX のパスワード。

.PARAMETER SelfSigned
この端末で試すための自己署名証明書を作成（または再利用）して署名する。
公開証明書を <OutputDir>/Snapline.cer に書き出す。

.PARAMETER Register
署名せずに展開済みレイアウトを開発者モードで登録し、実行確認できるようにする。

.PARAMETER PackageName
AppxManifest の Identity/Name。既定は Snapline.Shelf。

.EXAMPLE
.\scripts\build-windows.ps1 -SelfSigned
#>
param(
    [string]$TargetDir,
    [string]$OutputDir,
    [string]$PackageName = 'Snapline.Shelf',
    [string]$Publisher,
    [string]$PublisherDisplayName,
    [string]$CertificatePath,
    [string]$CertificatePassword,
    [switch]$SelfSigned,
    [switch]$Register
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$target = if ($TargetDir) { $TargetDir } elseif ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
$output = if ($OutputDir) { $OutputDir } else { Join-Path $repo 'build' }
$release = Join-Path $target 'release'
$layout = Join-Path $target 'msix-layout'
$msix = Join-Path $output 'Snapline.msix'

# The Windows SDK tools are not on PATH by default; take the newest installed version.
function Find-SdkTool([string]$Name) {
    $roots = @("${env:ProgramFiles(x86)}\Windows Kits\10\bin", "$env:ProgramFiles\Windows Kits\10\bin")
    foreach ($root in $roots) {
        if (-not $root -or -not (Test-Path -LiteralPath $root)) { continue }
        $found = Get-ChildItem -LiteralPath $root -Directory -ErrorAction SilentlyContinue |
            Sort-Object { try { [version]$_.Name } catch { [version]'0.0' } } -Descending |
            ForEach-Object { Join-Path $_.FullName "x64\$Name" } |
            Where-Object { Test-Path -LiteralPath $_ } |
            Select-Object -First 1
        if ($found) { return $found }
    }
    $command = Get-Command $Name -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    throw "Windows SDK の $Name が見つかりません。Windows SDK をインストールしてください。"
}

function ConvertTo-XmlText([string]$Text) {
    # Manifest values come from a certificate subject, so escape before substitution.
    return $Text -replace '&', '&amp;' -replace '<', '&lt;' -replace '>', '&gt;' -replace '"', '&quot;'
}

$cargoToml = Get-Content -LiteralPath (Join-Path $repo 'Cargo.toml') -Raw
$version = [regex]::Match($cargoToml, '(?m)^\s*version\s*=\s*"(\d+)\.(\d+)\.(\d+)"')
if (-not $version.Success) { throw 'Cargo.toml の [package] version を読み取れません。' }
# MSIX needs four numeric parts.
$packageVersion = '{0}.{1}.{2}.0' -f $version.Groups[1].Value, $version.Groups[2].Value, $version.Groups[3].Value

$hostTriple = (& rustc -vV | Where-Object { $_ -like 'host:*' }) -replace '^host:\s*', ''
$hostTriple = $hostTriple.Trim()
$arch = switch -Regex ($hostTriple) {
    '^x86_64' { 'x64' }
    '^aarch64' { 'arm64' }
    '^i686' { 'x86' }
    default { throw "MSIX のアーキテクチャを判定できません: $hostTriple" }
}

# Resolve the signing certificate first: Identity/Publisher has to match it exactly.
$certificate = $null
$pfxPath = $null
if ($CertificatePath) {
    $pfxPath = (Resolve-Path -LiteralPath $CertificatePath).Path
    $certificate = if ($CertificatePassword) {
        [System.Security.Cryptography.X509Certificates.X509Certificate2]::new(
            $pfxPath,
            $CertificatePassword,
            [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]::EphemeralKeySet)
    } else {
        [System.Security.Cryptography.X509Certificates.X509Certificate2]::new($pfxPath)
    }
} elseif ($SelfSigned) {
    $subject = if ($Publisher) { $Publisher } else { 'CN=Snapline' }
    $certificate = Get-ChildItem Cert:\CurrentUser\My |
        Where-Object { $_.Subject -eq $subject -and $_.HasPrivateKey -and $_.NotAfter -gt (Get-Date).AddDays(1) } |
        Sort-Object NotAfter -Descending |
        Select-Object -First 1
    if (-not $certificate) {
        $certificate = New-SelfSignedCertificate -Type Custom -Subject $subject -KeyUsage DigitalSignature `
            -FriendlyName 'Snapline MSIX' -CertStoreLocation 'Cert:\CurrentUser\My' `
            -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}') `
            -NotAfter (Get-Date).AddYears(5)
    }
}
if ($certificate) { $Publisher = $certificate.Subject }
if (-not $Publisher) { $Publisher = 'CN=Snapline' }
if (-not $PublisherDisplayName) {
    $PublisherDisplayName = if ($Publisher -match 'CN=([^,]+)') { $Matches[1] } else { 'Snapline' }
}

Push-Location $repo
try {
    # Static CRT: a packaged app does not get the VC++ redistributable, and the
    # Windows App Runtime framework package does not ship vcruntime140.dll.
    $rustflags = '["-C","target-feature=+crt-static"]'
    cargo build --locked --release --features msix --target-dir $target --config "target.$hostTriple.rustflags=$rustflags"
    if ($LASTEXITCODE -ne 0) { throw 'Release ビルドに失敗しました。' }
} finally { Pop-Location }

$assets = Join-Path $release 'msix-assets'
if (-not (Test-Path -LiteralPath $assets)) {
    throw "ロゴ素材がありません: $assets。--features msix でのビルドが必要です。"
}
if (Test-Path -LiteralPath $layout) { Remove-Item -LiteralPath $layout -Recurse -Force }
New-Item -ItemType Directory -Force -Path (Join-Path $layout 'Assets') | Out-Null
# Only the application binary and its assets: the runtime comes from the framework package.
Copy-Item -LiteralPath (Join-Path $release 'snapline.exe') -Destination $layout
Copy-Item -LiteralPath (Join-Path $release 'snapline.ico') -Destination $layout
Get-ChildItem -LiteralPath $assets -Filter '*.png' | Copy-Item -Destination (Join-Path $layout 'Assets')
Copy-Item -LiteralPath (Join-Path $repo 'LICENSE') -Destination $layout
Copy-Item -LiteralPath (Join-Path $repo 'README.ja.md') -Destination (Join-Path $layout 'README.md')
$manifest = Get-Content -LiteralPath (Join-Path $repo 'packaging\AppxManifest.xml') -Raw
$manifest = $manifest.
    Replace('{{NAME}}', (ConvertTo-XmlText $PackageName)).
    Replace('{{PUBLISHER}}', (ConvertTo-XmlText $Publisher)).
    Replace('{{PUBLISHER_NAME}}', (ConvertTo-XmlText $PublisherDisplayName)).
    Replace('{{VERSION}}', $packageVersion).
    Replace('{{ARCH}}', $arch)
Set-Content -LiteralPath (Join-Path $layout 'AppxManifest.xml') -Value $manifest -Encoding utf8 -NoNewline

New-Item -ItemType Directory -Force -Path $output | Out-Null
$makeappx = Find-SdkTool 'makeappx.exe'
& $makeappx pack /d $layout /p $msix /o
if ($LASTEXITCODE -ne 0) { throw 'MSIX の作成に失敗しました。' }

$publicCertificate = $null
if ($certificate) {
    $signTool = Find-SdkTool 'signtool.exe'
    if ($pfxPath) {
        $signArgs = @('sign', '/fd', 'SHA256', '/f', $pfxPath, '/d', 'Snapline')
        if ($CertificatePassword) { $signArgs += @('/p', $CertificatePassword) }
    } else {
        $signArgs = @('sign', '/fd', 'SHA256', '/sha1', $certificate.Thumbprint, '/d', 'Snapline')
        $publicCertificate = Join-Path $output 'Snapline.cer'
        Export-Certificate -Cert $certificate -FilePath $publicCertificate -Type CERT | Out-Null
    }
    $signArgs += $msix
    & $signTool @signArgs
    if ($LASTEXITCODE -ne 0) { throw 'MSIX の署名に失敗しました。' }
}

if ($Register) {
    # Developer mode only: register the unpacked layout under its own identity, so the
    # packaged build can be exercised without a signing certificate. Windows refuses the
    # notification area icon for packages registered from a non-system drive, so stage a
    # copy on the system drive first; ordinary installs land in WindowsApps anyway.
    $registeredLayout = $layout
    if ([System.IO.Path]::GetPathRoot($layout).TrimEnd('\') -ne [System.IO.Path]::GetPathRoot($env:SystemDrive + '\').TrimEnd('\')) {
        $registeredLayout = Join-Path $env:LOCALAPPDATA 'Snapline-msix-layout'
        if (Test-Path -LiteralPath $registeredLayout) { Remove-Item -LiteralPath $registeredLayout -Recurse -Force }
        Copy-Item -LiteralPath $layout -Destination $registeredLayout -Recurse
    }
    # Re-registering the same version is blocked, so drop the previous registration.
    Get-AppxPackage -Name $PackageName -ErrorAction SilentlyContinue | Remove-AppxPackage
    Add-AppxPackage -Register (Join-Path $registeredLayout 'AppxManifest.xml')
}

# Best effort: name the framework package when it is missing or too old for the manifest.
try {
    $framework = Get-AppxPackage -Name 'Microsoft.WindowsAppRuntime.2' -ErrorAction Stop |
        Sort-Object Version -Descending | Select-Object -First 1
    if (-not $framework) {
        Write-Warning 'Microsoft.WindowsAppRuntime.2 が未インストールです。Windows App Runtime 2 の再頒布可能パッケージを先に入れてください。'
    } elseif ([version]$framework.Version -lt [version]'2.4.0.0') {
        Write-Warning "Microsoft.WindowsAppRuntime.2 が古いため、このパッケージはインストールできません: $($framework.Version)"
    }
} catch {
    Write-Verbose "フレームワークパッケージを確認できませんでした: $($_.Exception.Message)"
}

$size = [math]::Round((Get-Item -LiteralPath $msix).Length / 1MB, 1)
Write-Output $msix
Write-Output "  バージョン: $packageVersion ($arch)"
Write-Output "  サイズ: $size MB"
Write-Output '  依存: Microsoft.WindowsAppRuntime.2 (>= 2.4.0.0)'
Write-Output "  署名: $(if ($certificate) { $certificate.Subject } else { 'なし' })"
if ($publicCertificate) {
    Write-Output ''
    Write-Output '配布物としてインストールするには、証明書を端末の信頼済みストアへ追加してから登録します（どちらも管理者権限）:'
    Write-Output "  Import-Certificate -FilePath '$publicCertificate' -CertStoreLocation Cert:\LocalMachine\TrustedPeople"
    Write-Output "  Add-AppxPackage '$msix'"
}
if ($Register) {
    Write-Output ''
    Write-Output "開発者モードで登録しました: $registeredLayout"
    Write-Output "  $env:LOCALAPPDATA\Microsoft\WindowsApps\snapline.exe --ui-test"
} elseif (-not $certificate) {
    Write-Output ''
    Write-Output '署名なしのまま入れて動作確認する場合（開発者モード）:'
    Write-Output "  Add-AppxPackage '$msix' -AllowUnsigned"
    Write-Output '署名せずに展開済みレイアウトを登録する場合:'
    Write-Output '  .\scripts\build-windows.ps1 -Register'
}
