<#
.SYNOPSIS
    Installs mysid into %LOCALAPPDATA%\mysid\bin and registers it globally in the Windows User PATH.
#>
[CmdletBinding()]
param(
    [switch]$SkipBuild,
    # Download the latest prebuilt mysid.exe from GitHub Releases (no Rust, no source needed)
    [switch]$Download
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

# Prebuilt binary: the GitHub release that the repository's workflow publishes on every push to main
$ReleaseUrl = "https://github.com/bypreetham/mysid-mcp/releases/download/latest/mysid.exe"
function Get-ReleaseExe {
    Write-Host "[1/4] Downloading the latest mysid.exe from GitHub Releases..." -ForegroundColor Yellow
    $TmpDir = Join-Path ([System.IO.Path]::GetTempPath()) ("mysid-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $TmpDir | Out-Null
    $Tmp = Join-Path $TmpDir "mysid.exe"
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -Uri $ReleaseUrl -OutFile $Tmp -UseBasicParsing
    if ((Get-Item $Tmp).Length -lt 1MB) {
        Write-Error "The download from $ReleaseUrl is too small to be mysid.exe."
        exit 1
    }
    # It must run and say what it is before it replaces anything.
    $Probe = & $Tmp help 2>&1 | Out-String
    if ($Probe -notmatch "code intelligence") {
        Write-Error "The downloaded file did not behave like mysid (mysid help printed something else)."
        exit 1
    }
    return $Tmp
}

if ($Download) {
    $SourceExe = Get-ReleaseExe
} elseif (-not $SourceExe -and -not $SkipBuild) {
    $Cargo = (Get-Command "cargo" -ErrorAction SilentlyContinue).Source
    if (-not $Cargo) {
        # rustup keeps cargo in the toolchain folder even when ~/.cargo/bin is not on PATH
        $Cargo = Get-ChildItem (Join-Path $env:USERPROFILE ".rustup\toolchains\*\bin\cargo.exe") -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
    }
    if ($ProjectDir -and $Cargo) {
        Write-Host "[1/4] Compiling latest mysid release binary via Cargo..." -ForegroundColor Yellow
        Push-Location $ProjectDir
        try {
            & $Cargo build --release
        } finally {
            Pop-Location
        }
        $SourceExe = Join-Path $ProjectDir "target\release\mysid.exe"
    } else {
        Write-Host "No Rust toolchain or source here: using the prebuilt release instead." -ForegroundColor Yellow
        $SourceExe = Get-ReleaseExe
    }
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

function Safe-Install-Binary ($Source, $Target) {
    try {
        Copy-Item -Path $Source -Destination $Target -Force -ErrorAction Stop
        return $true
    } catch {
        $OldFile = "$Target.old"
        Remove-Item -Path $OldFile -Force -ErrorAction SilentlyContinue
        try {
            Move-Item -Path $Target -Destination $OldFile -Force -ErrorAction Stop
            Copy-Item -Path $Source -Destination $Target -Force -ErrorAction Stop
            return $true
        } catch {
            return $false
        }
    }
}

# 3. Copy binary
Write-Host "[3/4] Installing binary to $DestExe..." -ForegroundColor Yellow
if (Safe-Install-Binary $SourceExe $DestExe) {
    Write-Host "  -> Installed to: $DestExe" -ForegroundColor Green
} else {
    Write-Warning "Could not update $DestExe (file locked)."
}

# Also mirror to ~/.local/bin if it exists
$LocalBin = Join-Path $env:USERPROFILE ".local\bin"
if (Test-Path $LocalBin) {
    $LocalBinExe = Join-Path $LocalBin "mysid.exe"
    if (Safe-Install-Binary $SourceExe $LocalBinExe) {
        Write-Host "  -> Mirrored to: $LocalBinExe" -ForegroundColor Green
    } else {
        Write-Warning "Could not update $LocalBinExe (file locked)."
    }
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

# 5. MCP auto-registration: every client whose config folder exists gets { "mysid": { "command": "mysid", "args": [] } }
function Register-McpJson ($ConfigFile, $Label) {
    try {
        $obj = $null
        if (Test-Path $ConfigFile) {
            $raw = Get-Content $ConfigFile -Raw
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
        $obj | ConvertTo-Json -Depth 8 | Set-Content $ConfigFile -Encoding utf8
        Write-Host "  -> Registered in ${Label}: $ConfigFile" -ForegroundColor Green
    } catch {
        Write-Warning "Failed updating ${Label} config: $_"
    }
}

# Antigravity / Gemini
$GeminiConfigDir = Join-Path $env:USERPROFILE ".gemini\config"
if (Test-Path $GeminiConfigDir) {
    Register-McpJson (Join-Path $GeminiConfigDir "mcp_config.json") "Antigravity/Gemini"
}

# Claude Desktop
$ClaudeDesktopDir = Join-Path $env:APPDATA "Claude"
if (Test-Path $ClaudeDesktopDir) {
    Register-McpJson (Join-Path $ClaudeDesktopDir "claude_desktop_config.json") "Claude Desktop (restart it to load mysid)"
}

# Claude Code (the `claude` command): user scope, so every project has mysid
$ClaudeCli = Get-Command "claude" -ErrorAction SilentlyContinue
if ($ClaudeCli) {
    & claude mcp get mysid *> $null
    if ($LASTEXITCODE -eq 0) {
        Write-Host "  -> Claude Code: mysid is already registered." -ForegroundColor Green
    } else {
        & claude mcp add --scope user mysid -- mysid
        if ($LASTEXITCODE -eq 0) {
            Write-Host "  -> Registered in Claude Code (user scope). Start a new Claude Code session to load it." -ForegroundColor Green
        } else {
            Write-Warning "Could not register in Claude Code. Run by hand:  claude mcp add --scope user mysid -- mysid"
        }
    }
} else {
    Write-Host "  -> Claude Code CLI not found; if you use it, run:  claude mcp add --scope user mysid -- mysid" -ForegroundColor Yellow
}

Write-Host "`n-----------------------------------------" -ForegroundColor Green
Write-Host "  SUCCESS: mysid is installed globally!" -ForegroundColor Green
Write-Host "-----------------------------------------" -ForegroundColor Green
Write-Host "Run anywhere: mysid --help`n" -ForegroundColor Cyan

# Test invocation
& $DestExe --help
