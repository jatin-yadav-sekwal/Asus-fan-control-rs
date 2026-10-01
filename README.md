<p align="center">
  <img src="asus-fan-control-rs/assets/logo-horizontal.svg" alt="Asus Fan Control" width="420">
</p>

# AsusFanControl (Rust)

A standalone Windows fan-control application for ASUS laptops, rewritten from
the original **AsusFanControl** C# app in Rust.

It reads live fan RPM and CPU temperature and lets you drive the fans either
automatically (temperature curve) or manually (fixed 0–100 % duty).

> **Credits** — this project is a rewrite of
> [**Karmel0x/AsusFanControl**](https://github.com/Karmel0x/AsusFanControl),
> the original legacy application that discovered this control path.
> It is MIT licensed, © 2023 **Karmel0x**. Without that work none of this
> would exist. `AsusWinIO64.dll` is © ASUSTek COMPUTER INC.; see
> [`THIRD_PARTY.md`](THIRD_PARTY.md).

---

## Download

Grab the latest ZIP from the [Releases](../../releases) page, extract it
anywhere, and double-click `bin\asus-fan-app.exe`.

The ZIP contains everything needed at runtime — `AsusWinIO64.dll` plus the
`AsusSAIO.sys` kernel driver it installs on first run. No PsExec, no manual
setup, no console windows.

## What it does

- **Live telemetry** — CPU package temperature and per-fan RPM, refreshed at 1 Hz.
- **Fan profiles** — `Silent`, `Balanced`, `Turbo`, `BIOS Default`, and `Manual`.
- **Manual control** — pick a preset (0 / 25 / 40 / 50 / 60 / 70 / 80 / 90 / 100 %)
  or click anywhere on the 0–100 % track.
- **Temperature curve** — for the automatic profiles, the active curve points are
  shown next to the slider so you can see what the fan is doing and why.
- **Requested vs. hardware duty** — the UI shows what was requested *and* what the
  EC actually acknowledged, so a silent write failure is never mistaken for success.
- **Safety watchdog** — forces 100 % fan if the CPU passes the emergency threshold.
- **Auto-start** — one click to register the app for logon.
- **Self-contained** — one folder. Double-click, accept the UAC prompt, done.

## Why it needs `SYSTEM`

ASUS exposes the fans through the `\\.\AsusSAIO` device (installed by the
*ASUS System Control Interface* / `MyASUS` package). **That device only accepts
`NT AUTHORITY\SYSTEM`** — a normal user, and even a fully elevated
administrator, gets `-1` back from every query.

So the hardware is owned by a small **SYSTEM helper** that this application
starts and talks to over a named pipe:

1. The GUI launches elevated (`ShellExecuteW` + `runas`, one UAC prompt).
2. It registers a one-shot scheduled task (`AsusFanControlSystemHelper`) that
   runs `asus-fan-app.exe --sys-helper --pipe=...` as `NT AUTHORITY\SYSTEM`.
3. The helper opens `\\.\AsusSAIO`, waits on a GUID-named pipe whose DACL
   grants access to SYSTEM and the launching user only, and serves the GUI.
4. When the GUI closes — or crashes — the pipe breaks, the helper hands the
   fans back to the BIOS curve and deletes its own task.

Nothing is left latched: the last commanded duty never outlives the GUI, and a
killed process cannot leave a resident SYSTEM worker behind.

`PsExec.exe` is **not used anywhere** in this project. Sysinternals does not
grant redistribution rights for PsTools, and the scheduled-task helper removes
the need for it entirely.

> Run **one** fan-control app at a time. If the legacy C# `AsusFanControlGUI.exe`
> is already holding `\\.\AsusSAIO`, this app shows a hardware-access banner
> instead of silently reading `0 RPM`.

---

## Requirements

- Windows 10/11 **x64**
- An ASUS laptop with the [ASUS System Control Interface](https://www.asus.com/support/faq/1047338/)
  installed (the `ASUS System Analysis` service must be running — it comes with
  `MyASUS`)
- Rust toolchain, only if you want to build from source

## How to use it

### Running a build

```
bin\asus-fan-app.exe
```

Accept the UAC prompt. The header reads `SYSTEM helper attached` once the pipe
is up — that is your confirmation that `\\.\AsusSAIO` answered.

`run-gui.bat` is a thin double-click wrapper for the same thing, and
`run-cli.bat` opens an elevated command-line status check.

### Building from source

```powershell
.\build.bat
```

`build.bat` runs `cargo build --release` and copies `asus-fan-app.exe`,
`asus-driver-cli.exe` and `hardware-probe.exe` into `bin\`. Close the running
app first — Windows will refuse to replace a running EXE.

### Command line

`bin\asus-driver-cli.exe` is a hardware test harness. It elevates itself, starts
the same SYSTEM helper, runs the command, and tears everything down:

```
asus-driver-cli status            # temperature + fan RPM
asus-driver-cli set-all 60        # all fans to 60% (0 restores BIOS control)
asus-driver-cli set 0 40          # fan #0 to 40%
asus-driver-cli reset             # hand control back to the BIOS curve
asus-driver-cli monitor -i 1000   # 1 Hz telemetry loop
```

`bin\hardware-probe.exe` exercises the DLL directly, including a 100 % duty
write test. It bypasses the helper and therefore needs a SYSTEM shell of its
own — for day-to-day checks use `asus-driver-cli` instead.

## Repository layout

```
asus-fan-control-rs/
├── crates/
│   ├── asus-driver/     # WinIO/ACPI FFI, driver lifecycle, pipe IPC,
│   │   └── src/bin/     #   scheduled-task bootstrap, asus-driver-cli, hardware-probe
│   ├── fan-engine/      # thermal loop: curves, hysteresis, watchdog, telemetry
│   ├── win-os/          # autostart / scheduler helpers
│   └── asus-fan-app/    # gpui desktop UI
├── assets/              # app icon, logos, bundled AsusWinIO64.dll
├── build.bat            # release build + install into bin\
.github/workflows/       # tagged release builds the ZIP automatically
bin/                     # built binaries (not tracked)
run-gui.bat              # double-click launcher
run-cli.bat              # CLI status (self-elevates)
AsusFanControl/          # the original C# app, its own git repository
```

### Logs

`bin\app.log` records every launch (token SID, helper task start, pipe name,
PID) and a telemetry line every 5 s. The SYSTEM helper appends to the same
file, so a failed handshake is visible without hunting through
`systemprofile\AppData`. `bin\panic.log` is written if the UI panics.

---

## Credits

- **[Karmel0x/AsusFanControl](https://github.com/Karmel0x/AsusFanControl)** —
  the original legacy application this rewrite is based on. MIT © 2023 Karmel0x.
  Everything here is a derivative of that work.
- **ASUSTek COMPUTER INC.** — `AsusWinIO64.dll` and the ASUS System Control
  Interface driver package.
- The **gpui** project for the UI toolkit.

## License

MIT — see [`LICENSE`](LICENSE). Copyright (c) 2023 Karmel0x, retained from the
original [AsusFanControl](https://github.com/Karmel0x/AsusFanControl) this
project is derived from.
