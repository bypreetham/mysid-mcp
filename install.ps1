<#
.SYNOPSIS
    Installs mysid into %LOCALAPPDATA%\mysid\bin and registers it globally in the Windows User PATH.
#>
[CmdletBinding()]
param(
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

Write-Host "=========================================" -ForegroundColor Cyan
Write-Host "       mysid Global Installer" -ForegroundColor Cyan
Write-Host "=========================================" -ForegroundColor Cyan

$ScriptDir = $PSScriptRoot

# 1. Determine Rust project and binary paths
$ProjectDir = $null
if (Test-Path (Join-Path $ScriptDir "rust-mcp\Cargo.toml")) {
    $ProjectDir = Join-Path $ScriptDir "rust-mcp"
} elseif (Test-Path (Join-Path $ScriptDir "mysid-mcp\rust-mcp\Cargo.toml")) {
    $ProjectDir = Join-Path $ScriptDir "mysid-mcp\rust-mcp"
} elseif (Test-Path (Join-Path $ScriptDir "Cargo.toml")) {
    $ProjectDir = $ScriptDir
}

$SourceExe = $null
if ($ProjectDir) {
    $Candidate = Join-Path $ProjectDir "target\release\mysid.exe"
    if (Test-Path $Candidate) {
        $SourceExe = $Candidate
    }
}

# Check for adjacent precompiled binary
if (-not $SourceExe) {
    $AdjacentExe = Join-Path $ScriptDir "mysid.exe"
    if (Test-Path $AdjacentExe) {
        $SourceExe = $AdjacentExe
    }
}

# Compile if needed
if (-not $SourceExe -and -not $SkipBuild) {
    if (-not $ProjectDir) {
        Write-Error "Could not locate rust-mcp project directory or precompiled mysid.exe."
        exit 1
    }

    Write-Host "[1/4] Compiling latest mysid release binary via Cargo..." -ForegroundColor Yellow
    $Cargo = "C:\Users\harip\.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin\cargo.exe"
    if (-not (Test-Path $Cargo)) {
        $Cargo = (Get-Command "cargo" -ErrorAction SilentlyContinue).Source
    }
    if (-not $Cargo) {
        Write-Error "Cargo not found. Please install Rust or provide a precompiled binary."
        exit 1
    }

    Push-Location $ProjectDir
    try {
        & $Cargo build --release
    } finally {
        Pop-Location
    }
    $SourceExe = Join-Path $ProjectDir "target\release\mysid.exe"
}

if (-not (Test-Path $SourceExe)) {
    Write-Error "Binary not found at $SourceExe"
    exit 1
}

# 2. Destination directory
$InstallDir = Join-Path $env:LOCALAPPDATA "mysid\bin"
if (-not (Test-Path $InstallDir)) {
    Write-Host "[2/4] Creating installation directory: $InstallDir" -ForegroundColor Yellow
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
} else {
    Write-Host "[2/4] Installation directory exists: $InstallDir" -ForegroundColor Yellow
}

$DestExe = Join-Path $InstallDir "mysid.exe"

# 3. Copy binary
Write-Host "[3/4] Installing binary to $DestExe..." -ForegroundColor Yellow
Copy-Item -Path $SourceExe -Destination $DestExe -Force

# Also mirror to ~/.local/bin if it exists
$LocalBin = Join-Path $env:USERPROFILE ".local\bin"
if (Test-Path $LocalBin) {
    Copy-Item -Path $SourceExe -Destination (Join-Path $LocalBin "mysid.exe") -Force
    Write-Host "  -> Mirrored to: $LocalBin\mysid.exe" -ForegroundColor Green
}

# 4. Configure User PATH
Write-Host "[4/4] Configuring Environment PATH..." -ForegroundColor Yellow
$UserPath = [Environment]::GetEnvironmentVariable("PATH", "User")
$Paths = $UserPath -split ";" | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }

if ($Paths -notcontains $InstallDir) {
    $NewUserPath = ($Paths + $InstallDir) -join ";"
    [Environment]::SetEnvironmentVariable("PATH", $NewUserPath, "User")
    Write-Host "  -> Added '$InstallDir' to User PATH (Registry)." -ForegroundColor Green
} else {
    Write-Host "  -> '$InstallDir' is already in User PATH." -ForegroundColor Green
}

# Update current session PATH
if (($env:PATH -split ";") -notcontains $InstallDir) {
    $env:PATH = "$InstallDir;$env:PATH"
}

# 5. MCP Auto-Registration
$GeminiConfigDir = Join-Path $env:USERPROFILE ".gemini\config"
if (Test-Path $GeminiConfigDir) {
    $GeminiMcp = Join-Path $GeminiConfigDir "mcp_config.json"
    try {
        $obj = $null
        if (Test-Path $GeminiMcp) {
            $raw = Get-Content $GeminiMcp -Raw
            if (-not [string]::IsNullOrWhiteSpace($raw)) {
                $obj = $raw | ConvertFrom-Json
            }
        }
        if (-not $obj) {
            $obj = [PSCustomObject]@{ mcpServers = [PSCustomObject]@{} }
        } elseif (-not $obj.mcpServers) {
            $obj | Add-Member -MemberType NoteProperty -Name "mcpServers" -Value ([PSCustomObject]@{}) -Force
        }
        $mysidServer = [PSCustomObject]@{ command = "mysid"; args = @() }
        if ($obj.mcpServers.PSObject.Properties['mysid']) {
            $obj.mcpServers.mysid = $mysidServer
        } else {
            $obj.mcpServers | Add-Member -MemberType NoteProperty -Name "mysid" -Value $mysidServer -Force
        }
        $obj | ConvertTo-Json -Depth 5 | Set-Content $GeminiMcp -Encoding utf8
        Write-Host "  -> Registered in Antigravity/Gemini: $GeminiMcp" -ForegroundColor Green
    } catch {
        Write-Warning "Failed updating Gemini config: $_"
    }
}

Write-Host "`n-----------------------------------------" -ForegroundColor Green
Write-Host "  SUCCESS: mysid is installed globally!" -ForegroundColor Green
Write-Host "-----------------------------------------" -ForegroundColor Green
Write-Host "Run anywhere: mysid --help`n" -ForegroundColor Cyan

# Test invocation
& $DestExe --help
