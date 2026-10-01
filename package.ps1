<#>
.SYNOPSIS
    Creates a distributable release package for AsusFanControl (Rust rewrite)

.DESCRIPTION
    Packages the release binaries, the ASUS driver DLL, and the launch scripts
    into a versioned ZIP suitable for GitHub Releases.

    PsExec is deliberately NOT packaged: Sysinternals does not grant
    redistribution rights for PsTools, and the application no longer needs it —
    hardware access is provided by an in-app Task Scheduler helper running as
    NT AUTHORITY\SYSTEM.

.NOTES
    Run from the repository root (where this script lives).
    Requires a successful `cargo build --release` in asus-fan-control-rs/
#>

param(
    [string]$Version = "0.1.0"
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$crateDir = Join-Path $repoRoot "asus-fan-control-rs"
$targetDir = "$env:USERPROFILE\.cargo_target\asus_fan_control\release"

# Fallback to standard target dir if custom one doesn't exist
if (-not (Test-Path (Join-Path $targetDir "asus-fan-app.exe"))) {
    $targetDir = Join-Path $crateDir "target\release"
}

$stageDir = Join-Path $repoRoot "dist\AsusFanControl-rs-v$Version"
$binDir = Join-Path $stageDir "bin"

Write-Host "=== AsusFanControl Release Packager ===" -ForegroundColor Cyan
Write-Host "Version: $Version"
Write-Host "Source:  $targetDir"
Write-Host "Stage:   $stageDir"
Write-Host ""

# Clean previous staging
if (Test-Path $stageDir) {
    Write-Host "Cleaning previous stage..." -ForegroundColor Yellow
    Remove-Item $stageDir -Recurse -Force
}

# Create directory structure
New-Item -ItemType Directory -Path $binDir -Force | Out-Null

# Copy release binaries
$binaries = @(
    "asus-fan-app.exe",
    "asus-driver-cli.exe",
    "hardware-probe.exe"
)

Write-Host "Copying release binaries..." -ForegroundColor Green
foreach ($bin in $binaries) {
    $src = Join-Path $targetDir $bin
    $dst = Join-Path $binDir $bin
    if (Test-Path $src) {
        Copy-Item $src $dst -Force
        Write-Host "  ✓ $bin ($([math]::Round((Get-Item $src).Length/1KB,1)) KB)"
    } else {
        Write-Warning "  ✗ Missing: $src"
    }
}

# Copy runtime dependencies.
#
# AsusWinIO64.dll ships with the application (precedent: Karmel0x/AsusFanControl
# does the same). The DriverStore copy is intentionally not used - it is a newer,
# incompatible ASUS System Control Interface build that answers our
# HealthyTable_* calls with -1.
#
# AsusSAIO.sys must ship beside it: the DLL's own error strings
# ("Attempt to install driver [%s]") show it looks for .\AsusSAIO.sys relative
# to itself and installs the kernel service from that path.
Write-Host "Copying runtime dependencies..." -ForegroundColor Green

function Copy-Dependency {
    param(
        [Parameter(Mandatory)] [string] $Destination,
        [Parameter(Mandatory)] [string[]] $Sources,
        [Parameter(Mandatory)] [string] $Label,
        [switch] $Required
    )
    foreach ($src in $Sources) {
        if (Test-Path $src) {
            Copy-Item $src $Destination -Force
            Write-Host "  ✓ $Label <- $src"
            return
        }
    }
    if ($Required) {
        Write-Host "  ✗ REQUIRED missing: $Label" -ForegroundColor Red
    } else {
        Write-Host "  ⊘ Optional missing: $Label" -ForegroundColor Gray
    }
}

Copy-Dependency -Destination (Join-Path $binDir "AsusWinIO64.dll") `
    -Sources @((Join-Path $crateDir "assets\AsusWinIO64.dll"), (Join-Path $repoRoot "bin\AsusWinIO64.dll")) `
    -Label "AsusWinIO64.dll" -Required

Copy-Dependency -Destination (Join-Path $binDir "AsusSAIO.sys") `
    -Sources @((Join-Path $crateDir "assets\AsusSAIO.sys"), (Join-Path $repoRoot "bin\AsusSAIO.sys")) `
    -Label "AsusSAIO.sys" -Required

foreach ($required in @("AsusWinIO64.dll", "AsusSAIO.sys")) {
    if (-not (Test-Path (Join-Path $binDir $required))) {
        Write-Error "  ✗ REQUIRED missing: $required"
        exit 1
    }
}

# Copy launch scripts (for end-user convenience)
Write-Host "Copying launch scripts..." -ForegroundColor Green
$scripts = @("run-gui.bat", "run-cli.bat")
foreach ($script in $scripts) {
    $src = Join-Path $repoRoot $script
    $dst = Join-Path $stageDir $script
    if (Test-Path $src) {
        Copy-Item $src $dst -Force
        Write-Host "  ✓ $script"
    }
}

# Copy documentation
Write-Host "Copying documentation..." -ForegroundColor Green
$docs = @("README.md", "LICENSE", "THIRD_PARTY.md")
foreach ($doc in $docs) {
    $src = Join-Path $repoRoot $doc
    $dst = Join-Path $stageDir $doc
    if (Test-Path $src) {
        Copy-Item $src $dst -Force
        Write-Host "  ✓ $doc"
    }
}

# Create ZIP
Write-Host "Creating ZIP archive..." -ForegroundColor Green
$zipPath = Join-Path $repoRoot "dist\AsusFanControl-rs-v$Version.zip"
if (Test-Path $zipPath) { Remove-Item $zipPath -Force }

Compress-Archive -Path $stageDir -DestinationPath $zipPath -CompressionLevel Optimal

$zipSizeMB = [math]::Round((Get-Item $zipPath).Length / 1MB, 2)
Write-Host ""
Write-Host "=== DONE ===" -ForegroundColor Cyan
Write-Host "Package: $zipPath"
Write-Host "Size:    $zipSizeMB MB"
Write-Host ""
Write-Host "Contents:"
Get-ChildItem $stageDir -Recurse | ForEach-Object {
    $rel = $_.FullName.Substring($stageDir.Length + 1)
    $size = if (-not $_.PSIsContainer) { " ($([math]::Round($_.Length/1KB,1)) KB)" } else { "" }
    Write-Host "  $rel$size"
}