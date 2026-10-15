# Install and configure Flex Server as a Windows Service
#
# Requires Administrator privileges.

param(
    [string]$ServiceName = "flex-server",
    [string]$DisplayName = "Flex Server",
    [string]$Description = "Flex headless media server",
    [string]$ConfigPath,
    [string]$DataDir = "$env:ProgramData\Flex",
    [string]$CacheDir = "$env:ProgramData\Flex\cache",
    [string]$DbPath = "$env:ProgramData\Flex\flex.db",
    [string]$Start = $true
)

$ErrorActionPreference = "Stop"

if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Error "This script must be run as Administrator."
    exit 1
}

# Locate flex-server.exe
$flexExe = Join-Path (Split-Path -Parent $MyInvocation.MyCommand.Path | Split-Path -Parent) "target\release\flex-server.exe"
if (-not (Test-Path $flexExe)) {
    # Try x86_64-pc-windows-gnu release
    $flexExe = Join-Path (Split-Path -Parent $MyInvocation.MyCommand.Path | Split-Path -Parent) "target\x86_64-pc-windows-gnu\release\flex-server.exe"
}

if (-not (Test-Path $flexExe)) {
    Write-Host "flex-server.exe not found. Build it first:" -ForegroundColor Yellow
    Write-Host "  cargo build -p flex-server --release (on Windows)" -ForegroundColor Yellow
    Write-Host "  or cross-compile to x86_64-pc-windows-gnu" -ForegroundColor Yellow
    exit 1
}

# Create data directories
New-Item -ItemType Directory -Force -Path $DataDir | Out-Null
New-Item -ItemType Directory -Force -Path $CacheDir | Out-Null

# Default config if not provided
if (-not $ConfigPath -or $ConfigPath -eq "") {
    $ConfigPath = Join-Path $DataDir "flex.toml"
}

if (-not (Test-Path $ConfigPath)) {
    $example = Join-Path (Split-Path -Parent $MyInvocation.MyCommand.Path | Split-Path -Parent) "config\flex.example.toml"
    if (Test-Path $example) {
        Copy-Item $example $ConfigPath
    } else {
        @"
host = "0.0.0.0"
port = 8080
database = "$DbPath"
cache_dir = "$CacheDir"
libraries = []
"@ | Out-File -FilePath $ConfigPath -Encoding utf8
    }
    Write-Host "Created default config at $ConfigPath" -ForegroundColor Green
}

# Build service install args
$binArgs = @(
    "service", "install",
    "--name", $ServiceName,
    "--display", $DisplayName,
    "--description", $Description,
    "--config", $ConfigPath,
    "--database", $DbPath,
    "--cache-dir", $CacheDir
)
if ($Start) {
    $binArgs += "--start"
}

# Run install
& $flexExe @binArgs

Write-Host "Flex Server Windows service installed." -ForegroundColor Green
Write-Host "Config: $ConfigPath" -ForegroundColor Cyan
Write-Host "Database: $DbPath" -ForegroundColor Cyan
Write-Host "Cache: $CacheDir" -ForegroundColor Cyan
Write-Host "Manage with: sc.exe start/stop/query $ServiceName or `"$flexExe`" service start/stop/status" -ForegroundColor Cyan
