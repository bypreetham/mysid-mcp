<#
.SYNOPSIS
    Packages mysid into a standalone, zero-dependency portable bundle and ZIP archive.
#>
[CmdletBinding()]
param(
    [string]$OutputDir = "",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

$ScriptDir = $PSScriptRoot
if (-not $ScriptDir) {
    $ScriptDir = (Get-Location).Path
}
if (-not $OutputDir) {
    $OutputDir = Join-Path $ScriptDir "dist\mysid-portable"
}

Write-Host "=========================================" -ForegroundColor Cyan
Write-Host "    mysid Portability Packaging Tool" -ForegroundColor Cyan
Write-Host "=========================================" -ForegroundColor Cyan

# 1. Determine Rust project directory
$RustProjectDir = $null
if (Test-Path (Join-Path $ScriptDir "rust-mcp\Cargo.toml")) {
    $RustProjectDir = Join-Path $ScriptDir "rust-mcp"
} elseif (Test-Path (Join-Path $ScriptDir "mysid-mcp\rust-mcp\Cargo.toml")) {
    $RustProjectDir = Join-Path $ScriptDir "mysid-mcp\rust-mcp"
} else {
    Write-Error "Cannot locate rust-mcp directory."
    exit 1
}

$SourceExe = Join-Path $RustProjectDir "target\release\mysid.exe"

# 2. Build release binary if missing or requested
if (-not (Test-Path $SourceExe) -and -not $SkipBuild) {
    Write-Host "[1/4] Compiling release binary via Cargo..." -ForegroundColor Yellow
    $Cargo = "C:\Users\harip\.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin\cargo.exe"
    if (-not (Test-Path $Cargo)) {
        $Cargo = (Get-Command "cargo" -ErrorAction SilentlyContinue).Source
    }
    if (-not $Cargo) {
        Write-Error "Cargo not found. Please compile mysid first or supply a release binary."
        exit 1
    }
    Push-Location $RustProjectDir
    try {
        & $Cargo build --release
    } finally {
        Pop-Location
    }
}

if (-not (Test-Path $SourceExe)) {
    Write-Error "Binary not found at $SourceExe"
    exit 1
}

# 3. Prepare distribution directory
Write-Host "[2/4] Preparing portable bundle at $OutputDir..." -ForegroundColor Yellow
if (Test-Path $OutputDir) {
    Remove-Item -Path $OutputDir -Recurse -Force
}
New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null

# 4. Copy binary and assets
Copy-Item -Path $SourceExe -Destination (Join-Path $OutputDir "mysid.exe") -Force
$ExeSizeMB = [math]::Round(((Get-Item (Join-Path $OutputDir "mysid.exe")).Length / 1MB), 2)
Write-Host "  -> Bundled mysid.exe ($ExeSizeMB MB)" -ForegroundColor Green

$SvgLogo = Join-Path $ScriptDir "mysid-mcp.svg"
if (Test-Path $SvgLogo) {
    Copy-Item -Path $SvgLogo -Destination (Join-Path $OutputDir "mysid-mcp.svg") -Force
    Write-Host "  -> Bundled mysid-mcp.svg logo" -ForegroundColor Green
}

# 5. Generate standalone installer for the portable bundle
$StandaloneInstaller = @'
<#
.SYNOPSIS
    Zero-dependency installer for mysid. Runs on ANY Windows PC without Rust/Cargo.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

Write-Host "=========================================" -ForegroundColor Cyan
Write-Host "       mysid Standalone Installer" -ForegroundColor Cyan
Write-Host "=========================================" -ForegroundColor Cyan

$ScriptDir = $PSScriptRoot
$SourceExe = Join-Path $ScriptDir "mysid.exe"

if (-not (Test-Path $SourceExe)) {
    Write-Error "mysid.exe not found in installer directory."
    exit 1
}

# 1. Create target directory
$InstallDir = Join-Path $env:LOCALAPPDATA "mysid\bin"
if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

$DestExe = Join-Path $InstallDir "mysid.exe"
Copy-Item -Path $SourceExe -Destination $DestExe -Force
Write-Host "[1/3] Installed binary to: $DestExe" -ForegroundColor Green

$LocalBin = Join-Path $env:USERPROFILE ".local\bin"
if (Test-Path $LocalBin) {
    Copy-Item -Path $SourceExe -Destination (Join-Path $LocalBin "mysid.exe") -Force
    Write-Host "  -> Also mirrored to: $LocalBin\mysid.exe" -ForegroundColor Green
}

# 2. Add to User PATH
Write-Host "[2/3] Registering in User PATH..." -ForegroundColor Yellow
$UserPath = [Environment]::GetEnvironmentVariable("PATH", "User")
$Paths = $UserPath -split ";" | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }

if ($Paths -notcontains $InstallDir) {
    $NewUserPath = ($Paths + $InstallDir) -join ";"
    [Environment]::SetEnvironmentVariable("PATH", $NewUserPath, "User")
    Write-Host "  -> Added '$InstallDir' to User PATH." -ForegroundColor Green
} else {
    Write-Host "  -> '$InstallDir' already in User PATH." -ForegroundColor Green
}

# 3. Optional MCP IDE auto-configuration
Write-Host "[3/3] Scanning for MCP client configurations..." -ForegroundColor Yellow

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

$ClaudeConfigDir = Join-Path $env:APPDATA "Claude"
if (Test-Path $ClaudeConfigDir) {
    $ClaudeMcp = Join-Path $ClaudeConfigDir "claude_desktop_config.json"
    try {
        $obj = $null
        if (Test-Path $ClaudeMcp) {
            $raw = Get-Content $ClaudeMcp -Raw
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
        $obj | ConvertTo-Json -Depth 5 | Set-Content $ClaudeMcp -Encoding utf8
        Write-Host "  -> Registered in Claude Desktop: $ClaudeMcp" -ForegroundColor Green
    } catch {
        Write-Warning "Failed updating Claude config: $_"
    }
}

Write-Host "`n-----------------------------------------" -ForegroundColor Green
Write-Host "  SUCCESS: mysid is ready to use!" -ForegroundColor Green
Write-Host "-----------------------------------------" -ForegroundColor Green
Write-Host "Open any terminal and run: mysid --help`n" -ForegroundColor Cyan
'@
Set-Content -Path (Join-Path $OutputDir "install.ps1") -Value $StandaloneInstaller -Encoding utf8

# 6. Generate Standalone Uninstaller
$StandaloneUninstaller = @'
<#
.SYNOPSIS
    Uninstalls mysid and removes it from the User PATH.
#>
$InstallDir = Join-Path $env:LOCALAPPDATA "mysid\bin"
if (Test-Path $InstallDir) {
    Remove-Item -Path (Split-Path $InstallDir -Parent) -Recurse -Force
    Write-Host "Removed $InstallDir" -ForegroundColor Green
}

$UserPath = [Environment]::GetEnvironmentVariable("PATH", "User")
$NewPath = ($UserPath -split ";" | Where-Object { $_ -ne $InstallDir }) -join ";"
[Environment]::SetEnvironmentVariable("PATH", $NewPath, "User")
Write-Host "Removed from User PATH." -ForegroundColor Green
'@
Set-Content -Path (Join-Path $OutputDir "uninstall.ps1") -Value $StandaloneUninstaller -Encoding utf8

# 7. Generate README.md
$ReadmeContent = @'
# mysid — High-Performance Codebase Intelligence & MCP Server

`mysid` is a standalone, ultra-fast native tool engine for AI coding agents and developers.

## Quick Installation

Run in PowerShell:
```powershell
.\install.ps1
```

## Features
- **Zero dependencies**: Single standalone binary (~3MB). No Python, Node.js, or runtime libraries needed.
- **Sub-20ms speed**: 100% compiled native Rust.
- **Dual mode**: Works as a direct command-line utility for human engineers and as an MCP stdio JSON-RPC server for AI agents.
'@
Set-Content -Path (Join-Path $OutputDir "README.md") -Value $ReadmeContent -Encoding utf8

# 8. Create ZIP archive
Write-Host "[3/4] Creating portable ZIP archive..." -ForegroundColor Yellow
$ZipPath = Join-Path (Split-Path $OutputDir -Parent) "mysid-windows-x64.zip"
if (Test-Path $ZipPath) {
    Remove-Item -Path $ZipPath -Force
}
Compress-Archive -Path "$OutputDir\*" -DestinationPath $ZipPath -Force
$ZipSizeMB = [math]::Round(((Get-Item $ZipPath).Length / 1MB), 2)
Write-Host "  -> Created archive: $ZipPath ($ZipSizeMB MB)" -ForegroundColor Green

Write-Host "`n[4/4] Portable Bundle Created Successfully!" -ForegroundColor Green
Write-Host "  Folder:  $OutputDir" -ForegroundColor Cyan
Write-Host "  Archive: $ZipPath" -ForegroundColor Cyan
