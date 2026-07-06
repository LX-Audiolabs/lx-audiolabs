# build-local-zip.ps1
# Builds all plugins via cargo-truce, packages each as Plugin-vX.Y.Z-win.zip
# Usage: .\build-local-zip.ps1 [plugin1,plugin2,...]
#   No args = all plugins
#   .\build-local-zip.ps1 aether,meridian

param(
    [string[]]$Plugins = @("aether", "aurum", "equilibrium", "lucent", "meridian")
)

$ErrorActionPreference = "Stop"
$distDir = "dist"
$bundlesDir = "target\bundles"

# Map plugin crate names to their CLAP bundle names (Cargo.toml name != CLAP name for some)
$clapNames = @{
    "aether" = "Aether"
    "aurum" = "Aurum"
    "equilibrium" = "Equilibrium"
    "meridian" = "Meridian"
    "lucent" = "Lucent"
    "lucent-relay" = "Lucent Relay"
}

Write-Host "=== Building plugins: $($Plugins -join ', ') ===" -ForegroundColor Cyan

$pkgArgs = $Plugins | ForEach-Object { "-p"; $_ }
cargo truce build --clap @pkgArgs
if ($LASTEXITCODE -ne 0) { throw "Build failed" }

New-Item -ItemType Directory -Force -Path $distDir | Out-Null

Write-Host "=== Packaging ZIPs ===" -ForegroundColor Cyan

foreach ($plugin in $Plugins) {
    $cargoToml = "plugins/$plugin/Cargo.toml"
    if (-not (Test-Path $cargoToml)) { Write-Warning "Skipping $plugin — no Cargo.toml"; continue }

    $ver = (Select-String '^version\s*=\s*"' $cargoToml | Select-Object -First 1).Line -replace '.*"(.+)".*', '$1'
    $clapName = $clapNames[$plugin]
    $clapPath = "$bundlesDir/$clapName.clap"
    $zipName = "$clapName-v$ver-win.zip"
    $zipPath = "$distDir/$zipName"

    if (-not (Test-Path $clapPath)) { Write-Error "Bundle not found: $clapPath"; continue }

    Compress-Archive -Path $clapPath -DestinationPath $zipPath -Force
    Write-Host "  $zipName" -ForegroundColor Green
}

# Lucent bundle special: Lucent + Lucent-Relay together
if ($Plugins -contains "lucent") {
    $lucentVer = (Select-String '^version\s*=\s*"' "plugins/lucent/Cargo.toml" | Select-Object -First 1).Line -replace '.*"(.+)".*', '$1'
    $bundleZip = "$distDir/Lucent-Bundle-v$lucentVer-win.zip"
    $lucentClap = "$bundlesDir/Lucent.clap"
    $relayClap = "$bundlesDir/Lucent Relay.clap"
    if ((Test-Path $lucentClap) -and (Test-Path $relayClap)) {
        Compress-Archive -Path $lucentClap, $relayClap -DestinationPath $bundleZip -Force
        Write-Host "  Lucent-Bundle-v$lucentVer-win.zip" -ForegroundColor Green
    }
}

Write-Host "=== Done: $distDir ===" -ForegroundColor Cyan
