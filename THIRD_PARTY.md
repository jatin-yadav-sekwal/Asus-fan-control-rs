# Third-party components

## Karmel0x / AsusFanControl

This project is a rewrite of
[`github.com/Karmel0x/AsusFanControl`](https://github.com/Karmel0x/AsusFanControl),
which first described the `\\.\AsusSAIO` control path used here.

```
MIT License

Copyright (c) 2023 Karmel0x

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

The full text is also in [`LICENSE`](LICENSE).

## AsusWinIO64.dll

`AsusWinIO64.dll` is © ASUSTek COMPUTER INC. It is redistributed alongside this
application because the copy that ships in the Windows DriverStore is a newer,
incompatible *ASUS System Control Interface* build that does not answer this
application's `HealthyTable_*` calls.

Redistribution is not licensed; it is included because the upstream project this
rewrite derives from does the same, and because the application cannot function
without it. If you would rather not redistribute it, delete
`asus-fan-control-rs/assets/AsusWinIO64.dll` — the application then resolves the
DLL from `ASUS_WINIO_PATH`, from next to its own executable, or from the current
working directory.

## AsusSAIO.sys

`AsusSAIO.sys` is the kernel half of the same ASUS component (© ASUSTek
COMPUTER INC.) and is byte-identical to the copy Windows keeps in the
DriverStore (`asussci2.inf_amd64_*\ASUSSystemAnalysis\`). It ships beside
`AsusWinIO64.dll` because that DLL looks for `.\AsusSAIO.sys` relative to its
own location and installs the service from there — the two files always travel
together. Delete both if you would rather not redistribute them; the DLL will
then fall back to the DriverStore copy.

## Not included: PsExec.exe / PsTools

Earlier revisions of this project launched a SYSTEM process with `PsExec.exe`.
It is **not shipped** and is no longer used. The Sysinternals licensing terms
forbid redistribution of PsTools, and the functionality has been replaced by a
Task Scheduler helper (`AsusFanControlSystemHelper`) plus a named pipe.

## gpui

The UI is built with [gpui](https://github.com/zed-industries/gpui), MIT
licensed, © Zed Industries.
