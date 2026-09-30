# AsusFanControl (Rust)

A standalone Windows fan-control application for ASUS laptops, rewritten from
the original **AsusFanControl** C# app in Rust.

It reads live fan RPM and CPU temperature and lets you drive the fans either
automatically (temperature curve) or manually (fixed 0–100 % duty).

> **Credits** — this project is a rewrite of
> [**Karmel0x/AsusFanControl**](https://github.com/Karmel0x/AsusFanControl),
> the original legacy application that discovered this control path.
> It is MIT licensed, © 2023 **Karmel0x**. Without that work none of this
> would exist. `AsusWinIO64.dll` is © ASUSTek COMPUTER INC. and is taken from
> the ASUS System Control Interface driver package; `PsExec.exe` is ©
> Sysinternals (Microsoft).

---

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
- **Self-contained** — one EXE. Double-click, accept the UAC prompt, done. No
  `.bat`, no console windows.

## Why it needs `SYSTEM`

ASUS exposes the fans through the `\\.\AsusSAIO` device (installed by the
*ASUS System Control Interface* / `MyASUS` package). **That device only accepts
`NT AUTHORITY\SYSTEM`** — a normal user, and even a fully elevated
administrator, gets `-1` back from every query.

So the app bootstraps itself in two steps:

1. If it is not elevated, it re-launches itself with a UAC prompt
   (`ShellExecuteW` + `runas`).
2. If it is elevated but not `SYSTEM`, it re-launches itself through
   `PsExec.exe -w <app dir> -i -s -d`, with `CREATE_NO_WINDOW` so nothing flashes.

The relaunched instance passes `--as-system-child` and `--parent-pid=<n>` back to
itself so the bootstrap never loops and never mistakes its own parent for a
second, competing fan-control process.

> Run **one** fan-control app at a time. If the legacy C# `AsusFanControlGUI.exe`
> (or a second copy of this app) is already holding `\\.\AsusSAIO`, this app shows
> a hardware-access banner instead of silently reading `0 RPM`.

---

## Requirements

- Windows 10/11 **x64**
- An ASUS laptop with the [ASUS System Control Interface](https://www.asus.com/support/faq/1047338/)
  installed (the `ASUS System Analysis` service must be running — it comes with
  `MyASUS`)
- `PsExec.exe` in the same folder as the application (already present in this repo)
- Rust toolchain, only if you want to build from source

## How to use it

### Running a build

```
bin\asus-fan-app.exe
```

Accept the UAC prompt — the window opens as `SYSTEM` and the header shows
`SYSTEM session`. That's it.

`run-gui.bat` still works if you prefer launching through PsExec directly, and
`run-cli.bat` opens an elevated command-line status check.

### Building from source

```powershell
.\build.bat
```

`build.bat` runs `cargo build --release` and copies `asus-fan-app.exe`,
`asus-driver-cli.exe` and `hardware-probe.exe` into `bin\`. Close the running
app first — Windows will refuse to replace a running EXE.

### Command line

`bin\asus-driver-cli.exe` is a hardware test harness (run it as `SYSTEM`,
e.g. via `run-cli.bat`):

```
asus-driver-cli status            # temperature + fan RPM
asus-driver-cli set-all 60        # all fans to 60% (0 restores BIOS control)
asus-driver-cli set 0 40          # fan #0 to 40%
asus-driver-cli reset             # hand control back to the BIOS curve
asus-driver-cli monitor -i 1000   # 1 Hz telemetry loop
```

`bin\hardware-probe.exe` exercises the DLL directly, including a 100 % duty
write test, and is useful for confirming the driver is reachable.

## Repository layout

```
asus-fan-control-rs/
├── crates/
│   ├── asus-driver/     # WinIO/ACPI FFI, driver lifecycle, SYSTEM bootstrap
│   │   └── src/bin/     #   asus-driver-cli, hardware-probe
│   ├── fan-engine/      # thermal loop: curves, hysteresis, watchdog, telemetry
│   ├── win-os/          # autostart / scheduler helpers
│   └── asus-fan-app/    # gpui desktop UI
├── build.bat            # release build + install into bin\
bin/                     # built binaries (not tracked)
run-gui.bat              # PsExec launcher
run-cli.bat              # CLI status as SYSTEM
AsusFanControl/          # the original C# app, its own git repository
```

### Logs

`bin\app.log` records every launch (token SID, driver probe, PID) and a
telemetry line every 5 s. `bin\panic.log` is written if the UI panics.

---

## Credits

- **[Karmel0x/AsusFanControl](https://github.com/Karmel0x/AsusFanControl)** —
  the original legacy application this rewrite is based on. MIT © 2023 Karmel0x.
  Everything here is a derivative of that work.
- **ASUSTek COMPUTER INC.** — `AsusWinIO64.dll` and the ASUS System Control
  Interface driver package.
- **Sysinternals / Microsoft** — `PsExec.exe`, used to reach `NT AUTHORITY\SYSTEM`.
- The **gpui** project for the UI toolkit.

## License

MIT — see [`LICENSE`](LICENSE). Copyright (c) 2023 Karmel0x, retained from the
original [AsusFanControl](https://github.com/Karmel0x/AsusFanControl) this
project is derived from.
