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
    [string]$Version = "0.1.0",
    [switch]$SkipInstaller,
    [switch]$RequireInstaller
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$crateDir = Join-Path $repoRoot "asus-fan-control-rs"

# Locate the release binaries. The crate pins its own target-dir in
# .cargo/config.toml, and that path is not necessarily under $env:USERPROFILE -
# on CI it lands elsewhere while cargo still uses it, so ask the config rather
# than guessing, then fall back to the usual locations.
function Find-ReleaseDir {
    $candidates = @()

    # CARGO_TARGET_DIR beats .cargo/config.toml, so it has to be checked first.
    if ($env:CARGO_TARGET_DIR) {
        $candidates += Join-Path $env:CARGO_TARGET_DIR "release"
    }

    $cfg = Join-Path $crateDir ".cargo\config.toml"
    if (Test-Path $cfg) {
        $m = Select-String -Path $cfg -Pattern '^\s*target-dir\s*=\s*"(?<p>[^"]+)"' |
            Select-Object -First 1
        if ($m) {
            $td = $m.Matches[0].Groups["p"].Value -replace '/', [IO.Path]::DirectorySeparatorChar
            if (-not [IO.Path]::IsPathRooted($td)) { $td = Join-Path $crateDir $td }
            $candidates += Join-Path $td "release"
        }
    }

    $candidates += Join-Path $crateDir "target\release"
    $candidates += Join-Path $env:USERPROFILE ".cargo_target\asus_fan_control\release"

    foreach ($dir in $candidates) {
        if (Test-Path (Join-Path $dir "asus-fan-app.exe")) { return $dir }
    }
    return $null
}

$targetDir = Find-ReleaseDir

if (-not $targetDir) {
    Write-Error "Release binaries not found - run 'cargo build --release' in asus-fan-control-rs/ first."
    exit 1
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

# A package without its binaries is worthless - fail loudly instead of
# publishing a ZIP that only contains the DLL.
$missing = @($binaries | Where-Object { -not (Test-Path (Join-Path $binDir $_)) })
if ($missing.Count -gt 0) {
    Write-Error "  ✗ Staging incomplete, missing: $($missing -join ', ')"
    exit 1
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

# Build the Inno Setup installer. The ZIP is the portable option; the setup
# EXE is what registers Asus Fan Control in the Start Menu and in
# Settings > Apps, and it is what creates the optional logon task.
$setupPath = $null
if (-not $SkipInstaller) {
    Write-Host "Building installer..." -ForegroundColor Green

    $iscc = $null
    $isccCmd = Get-Command "ISCC.exe" -ErrorAction SilentlyContinue
    if ($isccCmd) { $iscc = $isccCmd.Source }
    if (-not $iscc) {
        $candidates = @(
            "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
            "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
            "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
        )
        foreach ($c in $candidates) {
            if ($c -and (Test-Path $c)) { $iscc = $c; break }
        }
    }

    if (-not $iscc) {
        $msg = "ISCC.exe (Inno Setup 6) not found - install with 'winget install JRSoftware.InnoSetup' or 'choco install innosetup'."
        if ($RequireInstaller) { Write-Error $msg; exit 1 }
        Write-Warning "  $msg"
    } else {
        $setupPath = Join-Path $repoRoot "dist\AsusFanControl-rs-v$Version-Setup.exe"
        if (Test-Path $setupPath) { Remove-Item $setupPath -Force }

        $iss = Join-Path $repoRoot "packaging\AsusFanControl.iss"
        # Capture ISCC's output: its diagnostics are the only clue when this
        # fails on CI, and they must not be swallowed by the pipeline.
        $issOut = & $iscc "/DAppVersion=$Version" $iss 2>&1
        $issCode = $LASTEXITCODE
        foreach ($line in $issOut) { Write-Host "    $line" }
        if ($issCode -ne 0) {
            Write-Error "  Installer build failed (ISCC exit $issCode)."
            exit 1
        }
        if (-not (Test-Path $setupPath)) {
            Write-Error "  ISCC reported success but $setupPath is missing."
            exit 1
        }
        Write-Host "  ✓ $setupPath ($([math]::Round((Get-Item $setupPath).Length / 1MB, 2)) MB)"
    }
}

$zipSizeMB = [math]::Round((Get-Item $zipPath).Length / 1MB, 2)
Write-Host ""
Write-Host "=== DONE ===" -ForegroundColor Cyan
Write-Host "Package: $zipPath"
Write-Host "Size:    $zipSizeMB MB"
if ($setupPath) { Write-Host "Setup:   $setupPath" }
Write-Host ""
Write-Host "Contents:"
Get-ChildItem $stageDir -Recurse | ForEach-Object {
    $rel = $_.FullName.Substring($stageDir.Length + 1)
    $size = if (-not $_.PSIsContainer) { " ($([math]::Round($_.Length/1KB,1)) KB)" } else { "" }
    Write-Host "  $rel$size"
}