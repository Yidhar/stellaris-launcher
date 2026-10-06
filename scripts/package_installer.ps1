# Builds the Windows setup program from a Release build: package/stellaris-launcher-setup-v<version>.exe (plus its SHA-256 file).
# Needs Inno Setup 7 (ISCC.exe): set $env:ISCC, or install it (CI downloads the pinned release from github.com/jrsoftware/issrc).
#
#     cargo build --release
#     pwsh scripts/package_installer.ps1 -Version v0.1.0
param(
    [string]$Version = "v0.0.0",
    [string]$Out = "package"
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$num = $Version.TrimStart("v")
if ($num -notmatch '^\d+\.\d+\.\d+$') { $num = "0.0.0" }   # dev builds: Inno needs a numeric version

$iscc = $env:ISCC
if (-not $iscc) {
    $iscc = @(
        "$env:LOCALAPPDATA\Programs\Inno Setup 7\ISCC.exe",
        "${env:ProgramFiles(x86)}\Inno Setup 7\ISCC.exe",
        "$env:ProgramFiles\Inno Setup 7\ISCC.exe"
    ) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
}
if (-not $iscc) { throw "Inno Setup 7 (ISCC.exe) not found; set `$env:ISCC" }

$src = Join-Path $root "target\release"
foreach ($exe in "stellaris-launcher.exe", "stl.exe") {
    if (-not (Test-Path (Join-Path $src $exe))) { throw "missing build output: $src\$exe" }
}
& $iscc "/Q" "/DAppVersion=$num" "/DSourceDir=$src" (Join-Path $root "installer\stellaris-launcher.iss")
if ($LASTEXITCODE -ne 0) { throw "ISCC failed ($LASTEXITCODE)" }

$setup = Join-Path $root "package\stellaris-launcher-setup-v$num.exe"
$final = Join-Path $Out "stellaris-launcher-setup-$Version.exe"
if ((Resolve-Path -LiteralPath $setup).Path -ne [IO.Path]::GetFullPath($final)) { Move-Item -Force $setup $final }
$hash = (Get-FileHash $final -Algorithm SHA256).Hash.ToLower()
"$hash  $(Split-Path -Leaf $final)" | Set-Content -Encoding ascii "$final.sha256"
Write-Host "built $final ($([math]::Round((Get-Item $final).Length / 1MB, 2)) MB)"
