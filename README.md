# aero-control

Open-source fan, profile, and sensor UI for a Gigabyte **AERO 17 YB** (Windows 10/11, i7-10875H, RTX 2080 Super Max-Q). It talks to the laptop's embedded controller the same way Gigabyte Control Center does: `GB_WMIACPI_Get` / `GB_WMIACPI_Set` in `root\WMI`.

The firmware runs the fans. This program does **not** poll temperatures and write fan speeds in a loop. Sensor reads are for display, the probe, and a getter-only drift check. If an upload fails or the apply path panics, it sends the firmware back to automatic mode.

Thermal protection is still the CPU, the GPU, and the embedded controller (PROCHOT and GPU slowdown). Validation here reduces mistakes. It does not replace those limits.

AeroCtl is GPLv3 and is not a code source. Facts and the ones we could not confirm are listed in [docs/SOURCES.md](docs/SOURCES.md). The design notes are in [docs/SCOPE.md](docs/SCOPE.md).

## Layout

```
crates/aero-wmi/     WMI transport, mock EC, RPM and curve decoding
crates/aero-core/    Modes, curve rules, TOML profiles, upload and restore
crates/aero-proto/   Versioned JSON messages for a future pipe
crates/aero-svc/     Dispatch for that pipe. The Windows service host is not wired.
crates/aero-probe/   Getter-only capability dump. The writer feature is off.
crates/aero-gui/     egui status view and curve editor (real WMI on Windows, mock otherwise)
profiles/            Example TOML profiles
```

Rust 1.85 or newer.

## Linux (no laptop required)

```bash
cargo test --workspace
cargo run -p aero-probe -- --mock
cargo run -p aero-gui
```

`--mock` prints a JSON dump of a fake AERO 17 YB. `setters_invoked` is 0. RPM values in the fixture are byte-swapped on the wire; the report shows raw, swapped, and the decoded RPM.

The GUI is a mock. Status refresh only calls getters. **Upload curve** and the firmware-mode buttons run the real command sequences against that mock, including the 15-slot upload and the readback.

`cargo check --workspace --target x86_64-pc-windows-gnu` and the same command for `x86_64-pc-windows-msvc` both typecheck, including the Windows WMI backend. That backend uses `wmi` 0.14's `exec_method_native_wrapper` (this crate's name for `IWbemServices::ExecMethod`: class object, method signature, spawned in-params, `VT_UI1`, then exec). Newer `wmi` releases rename it to `exec_method`; 0.18 does not typecheck against the `windows` crates this workspace resolves.

## Windows

Build from a normal terminal. Run the probe from an **elevated** PowerShell. Admin is required for `root\WMI` on these machines. The probe still never calls a setter.

```powershell
rustup target add x86_64-pc-windows-msvc
cargo build --release -p aero-probe
.\target\release\aero-probe.exe
```

`cargo build --release -p aero-gui` embeds an administrator manifest. Launch the exe from an elevated shell (or accept the UAC prompt). The window shows **REAL** and talks to `GB_WMIACPI` on the instance path. `--mock` keeps the fake controller. If the process is not elevated, the window stays read-only and says so.

Every WMI call is appended to `%ProgramData%\AeroCtl\logs\aero-control.log` (or `logs` next to the exe). Closing the window restores firmware auto when the last command was Fixed or Max.

The first supervised write is **Test: Max for 10 s then restore Auto**.

```powershell
cargo build --release -p aero-probe
.\target\release\aero-probe.exe --diag
cargo build --release -p aero-gui
.\target\release\aero-gui.exe
```

`--diag` prints each getter and setter signature (names and CIM types) and the instance path, then still calls only getters.

Useful probe flags:

```powershell
.\aero-probe.exe --help
.\aero-probe.exe --mock
.\aero-probe.exe --no-pace
.\aero-probe.exe --rpm-order auto-swap
```

On real hardware each WMI call waits 500 ms, so a full dump takes about a minute. `--no-pace` skips that. `--rpm-order` is `auto-swap` (default, right for an older AERO), `auto-native`, `swap`, or `native`. The JSON always contains both byte orders.

Exit codes: `0` report written, `2` bad arguments or no Windows backend, `4` `GB_WMIACPI_Get` is not registered. Individual getters that this SKU does not implement are failures inside the JSON, not a failed process.

If the class is missing, keep Gigabyte's `acpimof.dll` registration, or compile the MIT clean-room subset from [aero-keyboard-fan](https://github.com/ejempty/aero-keyboard-fan) once:

```powershell
mofcomp aero_fan.mof
```

That MOF only declares a few setters. The probe wants the full getter class, which ships with Control Center's `acpimof.dll`.

## Fan modes and curves

| Control | What is sent | Confirmed? |
|---|---|---|
| Normal, Restore auto | Fixed off, step off, auto on, current step 0, quiet off | Tail is the Control Center restore sequence |
| Quiet (Super Quiet) | Same clears, auto left **on**, then `SetSuperQuiet 1` | Provisional. Quiet and Super Quiet are one switch |
| Gaming | Same fan writes as Normal | Provisional. See SOURCES.md |
| Max | Both fans raw **229** | Duty 229 and the fixed-mode shape |
| Fixed | Same shape as Max, with your duty | Floor is 35% (raw 80) unless the profile sets `expert_mode` |
| Upload curve | Full 15-slot write, then readback | Sequence is the §5.1 bugfix |

Editor curves (Silent, Balanced, Performance, Max-ish, Deep Control) are **provisional stand-ins**. Control Center's manual does not publish the factory Deep Control points. They are not loaded into the EC until you press **Upload curve**.

What the upload does, and Control Center does not:

- Sends all 15 slots in order. Unused slots are `(0, 0)`, which is the terminator, so an old point cannot linger after it.
- Reads every slot back with `GetFanIndexValue`. One mismatch retries the whole upload once, then restores firmware auto.
- Clears auto, fixed, the current step, and (if the method exists) the custom cap before the points. Control Center's apply path skipped `SetCurrentFanStep(0)` and never cleared that cap.
- Rounds percent ↔ raw half up. 50% is raw 115. Control Center truncates 50% to 114 (`50 * 229 / 100`).
- Rejects a `(0, 0)` point anywhere in the curve, including the middle, because the firmware treats it as the end of the table.

Pasting `HKLM\SOFTWARE\Gigabyte\SmartManager\CustomFanTable` into the curve tab parses it. On a Windows build, **Read CustomFanTable from the registry** opens that key read-only. Nothing in this repo writes the Gigabyte registry.

Profiles are TOML. `profiles/config.toml` binds Gaming to AC and Quiet to DC. `curve_pairs` are percent. A `duty_raw` field, when present, is the exact byte that will be sent. CPU power limits and the GPU/battery fields are stored and **not** sent; applying a profile says so.

## What is still stubbed

- The GUI and the service are not connected to a live EC. `WindowsReader` / `WindowsWriter` in `aero-wmi` are compiled only for Windows and are what the probe (reader) and a later apply path use.
- No Windows service, no named pipe, no SDDL, no resume listener. `aero-svc` dispatches the same apply engine in-process and exits if you run the binary. Power-event re-apply is implemented and tested; nothing yet subscribes to `WM_POWERBROADCAST`.
- No tray icon, global hotkey, XTU/PL1/PL2 bridge, charge-limit or GPU WMI writes, MOF installer, or GCC coexistence wizard.
- `SetDeepFan` (the old 5-point call) and `TurnOffFan` are known and never called. `TurnOffFan` is refused if something asks for it.

## Suggested next step

Run `aero-probe.exe` from an elevated shell on the AERO 17 YB and keep the JSON. That shows which getters exist, whether RPM bytes are actually swapped, and what `GetFanIndexValue` returns for indexes 0..14. Then one supervised write: Max, read the flags back, restore auto. After that, point the GUI at `WindowsWriter` instead of the mock.
