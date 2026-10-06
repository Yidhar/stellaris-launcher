# Packs a Release build into package/stellaris-launcher-<version>.zip (plus its SHA-256 file). Used by CI; run by hand after `cargo build --release`:
#
#     pwsh scripts/package_release.ps1 -Version dev-local
param(
    [string]$Version = "dev-local",
    [string]$Out = "package"
)
$ErrorActionPreference = "Stop"
$name = "stellaris-launcher-$Version"
$dir = Join-Path $Out $name
if (Test-Path $dir) { Remove-Item -Recurse -Force $dir }
New-Item -ItemType Directory -Force -Path "$dir/docs", "$dir/examples" | Out-Null
foreach ($exe in "stellaris-launcher.exe", "stl.exe") {
    $p = "target/release/$exe"
    if (-not (Test-Path $p)) { throw "missing build output: $p" }
    Copy-Item $p $dir
}
Copy-Item README.md, LICENSE $dir
Copy-Item docs/DESIGN.md, docs/PLUGINS.md, docs/FINDINGS.md, docs/PROBLEMS.md "$dir/docs/"
Copy-Item -Recurse examples/plugins "$dir/examples/plugins"
$zip = Join-Path $Out "$name.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path $dir -DestinationPath $zip
$hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
"$hash  $name.zip" | Set-Content -Encoding ascii "$zip.sha256"
Write-Host "packed $zip ($([math]::Round((Get-Item $zip).Length / 1MB, 2)) MB)"
if ($env:GITHUB_OUTPUT) { "version=$Version" >> $env:GITHUB_OUTPUT }
