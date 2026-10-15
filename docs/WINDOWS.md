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
- The SCM launches the binary directly, so the registered command line must end in `service run <name>`; `service install` adds this automatically. If you installed with an older build (where the service appeared to start but did nothing), uninstall and reinstall to refresh the registered command line:
  ```powershell
  .\target\release\flex-server.exe service uninstall --force
  .\target\release\flex-server.exe service install --config "C:\ProgramData\Flex\flex.toml" --database "C:\ProgramData\Flex\flex.db" --cache-dir "C:\ProgramData\Flex\cache" --start
  ```

## FFmpeg

Download FFmpeg from [ffmpeg.org](https://ffmpeg.org/download.html) and add its `bin\` directory to the system `PATH`. Both `ffmpeg.exe` and `ffprobe.exe` must be available.

## Logging

Logs are written to **both** stdout and a file:

- Console mode → `./data/logs/flex-server.log`
- Windows Service → `%ProgramData%\Flex\logs\flex-server.log`

Set `FLEX_LOG_DIR` to override the directory. This file is the only reliable
place to see why a service fails to start, since a service has no console and the
SCM does **not** capture stdout/stderr into the Windows Event Log (it only logs
its own lifecycle events, e.g. errors `1053`/`1063`).

The log level follows `RUST_LOG` (default `flex_server=info,tower_http=info`),
e.g. `RUST_LOG=debug` for verbose output.

If a service does not start, check the log file first, then the Service Control
Manager events (Event Viewer → Windows Logs → System). The command line the SCM
uses is visible with `sc.exe qc flex-server` and should end in
`service run flex-server`.

## Firewall

If binding to `0.0.0.0` or a non-loopback address, ensure Windows Firewall allows the chosen TCP port.

## Uninstall

```powershell
.\target\release\flex-server.exe service uninstall --force
```

Or via Services.msc and delete data under `%ProgramData%\Flex\` if desired.
