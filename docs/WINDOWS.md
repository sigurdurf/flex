# Windows Installation and Usage

Flex Server supports running natively on Windows as a console application or as a Windows Service.

## Prerequisites

- [Rust toolchain](https://rustup.rs) (stable) if building from source.
- [ffmpeg](https://ffmpeg.org/download.html) in your `PATH` (required for remux/transcode and subtitle processing).
- Administrator privileges for service installation.

## Building

From the repository root:

```powershell
# Build release
cargo build -p flex-server --release
# Binaries in target\release\flex-server.exe
```

## Running as a console app

```powershell
.\target\release\flex-server.exe --config .\flex.toml
```

Or create a configuration at `%ProgramData%\Flex\flex.toml` and run:

```powershell
.\target\release\flex-server.exe --config "%ProgramData%\Flex\flex.toml"
```

User management (on Windows, stdin prompts work in console):

```powershell
.\target\release\flex-server.exe user add admin --admin
```

## Running as a Windows Service

### Install using the PowerShell helper script

From the repository root (as Administrator):

```powershell
.\scripts\install-windows.ps1
```

This script will:
- Create data directories under `%ProgramData%\Flex\`
- Create a default `flex.toml` if none exists
- Register `flex-server` as an automatic-start Windows Service
- Optionally start it immediately

Advanced usage:

```powershell
.\scripts\install-windows.ps1 -ServiceName flex-server -DataDir "C:\ProgramData\Flex" -Start $true
```

### Install/uninstall using the binary directly

```powershell
# Install (as Administrator)
.\target\release\flex-server.exe service install --config "C:\ProgramData\Flex\flex.toml" --start

# Start/stop/status
.\target\release\flex-server.exe service start
.\target\release\flex-server.exe service stop
.\target\release\flex-server.exe service status

# Uninstall
.\target\release\flex-server.exe service uninstall --force
```

### Notes on paths

- When installed as a service, the Service Control Manager sets the working directory to `System32` by default. The service registration script passes absolute paths via CLI flags (config, database, cache). This ensures paths resolve correctly regardless of working directory.
- For best results, use absolute paths in `flex.toml` when running as a service.
- The database defaults to `./data/flex.db` in console mode; for service usage prefer an absolute path (the install script sets `--database`).

## FFmpeg

Download FFmpeg from [ffmpeg.org](https://ffmpeg.org/download.html) and add its `bin\` directory to the system `PATH`. Both `ffmpeg.exe` and `ffprobe.exe` must be available.

## Logging

Logs go to stdout/stderr. When running as a console app, they appear in the terminal. When running as a Windows Service, they appear in the Windows Event Log (via stdout/stderr captured by SCM in many cases) or can be redirected. For troubleshooting, run the binary in console mode first.

## Firewall

If binding to `0.0.0.0` or a non-loopback address, ensure Windows Firewall allows the chosen TCP port.

## Uninstall

```powershell
.\target\release\flex-server.exe service uninstall --force
```

Or via Services.msc and delete data under `%ProgramData%\Flex\` if desired.
