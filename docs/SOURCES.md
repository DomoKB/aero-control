# Where the command facts came from

aero-control does not copy AeroCtl. That GPLv3 project is now a functional replacement target: its README, UI labels and public configuration declarations were consulted for feature coverage. Command behaviour is established from the primary sources below. No AeroCtl source is copied or vendored here; see AEROCTL_PARITY.md.

## Windows method names and parameter names

The Windows backend opens the firmware WMI data blocks by GUID using Windows' data-block API. It does not load Gigabyte's `acpimof.dll` or require Gigabyte MOF/CIM class registration. The MOF listing below supplies method IDs and parameter layouts as reference material; it is not a runtime dependency. Availability of the corresponding firmware WMI blocks is still required.

Gigabyte Aero 16 SKU P86YE5 MOF listing posted in [lm-sensors issue 400](https://github.com/lm-sensors/lm-sensors/issues/400). That dump is the source for:

- `GB_WMIACPI_Get` / `GB_WMIACPI_Set` in `root\WMI`
- `GetFanIndexValue(Index) -> Temperture, Value` (the temperature parameter is spelled that way)
- `SetFanIndexValue(Index, Temperture, Value)`
- Simple setters take an in-parameter named `Data`, not the MOF `Description(...)` text
- `getCpuTemp`, `getGpuTemp1`, `getGpuTemp2`, `getRpm1`, `getRpm2` return `uint16 Data`
- Fan-mode setters used by the scripts: `SetAutoFanStatus`, `SetFixedFanStatus`, `SetStepFanStatus`, `SetFixedFanSpeed`, `SetGPUFanDuty`, `SetCurrentFanStep`, `SetSuperQuiet`, `SetFanAdjustStatus`
- The same comment's performance-mode traces (Creator, Turbo, Gaming, Silence), which are for that Aero 16, not for the AERO 17 YB

`SetFanCustomStatus` and `GetSuperQuiet` are **not** in that MOF. The scope names the custom-cap setter, so the upload tries it and skips a missing method.

## 15 slots, RPM byte order, packed curve word

[tangalbert919/gigabyte-laptop-wmi `aorus-laptop.c`](https://github.com/tangalbert919/gigabyte-laptop-wmi/blob/master/aorus-laptop.c):

- `FAN_CURVE_POINTS` is 15 and the driver reads index `0..14`
- `convert_fan_rpm` is `rol16` (byte swap). The driver applies it unless the DMI family is `GIGABYTE GAMING`
- The GET path stores `temperature = low byte`, `speed = high byte`

That packing is the Linux WMBC selector (`0x68`), which is a different entry point from `GB_WMIACPI_Get`. The Windows MOF returns two bytes, and that is what we send and prefer on readback. The packed decoder is only a fallback. Issue 22 on that repo shows a 2025/2026 AERO already returning native RPM; the probe therefore prints raw, swapped, and the value it chose.

## Duty scale 0–229

- The scope, from Control Center's decompiled DeepFan path: max duty is 229, and Control Center truncates with `duty * 229 / 100`
- [Aero 15 EC notes](https://github.com/tangalbert919/p37-ec-aero-15/blob/master/Aero%2015%20Fan%20Control%20Registers.md): `0xE5` (229) is 100%, `0x50` (80) is 35%, and the rest of that table matches truncating percent. aero-control preserves those exact raw bytes for Factory; other percent-based custom points round half up

[ejempty/aero-keyboard-fan](https://github.com/ejempty/aero-keyboard-fan) (MIT) documents `SetCurrentFanStep`, the class GUID, and a max-fan sequence that sends duty **100**. This project follows the scope and the Aero 16 Turbo trace (`229`), not that 100.

## Mode names

The Control Center quick-start manual names Normal, Gaming, Quiet, Customize (fixed or max), and Deep Control (15 points). The "46°C / 44%" sentence there is an example of a rule violation, not a default curve.

The factory custom curve for model names containing `yb` (the same branch as `xa` / `va` / `ya` and the rest of that family) is the `setDefault_Click` table in DeepFan.dll. On this AERO 17 YB (P77YB, BIOS FB10), `GetFanIndexValue` 0..14 returned the same temperatures with duty bytes equal to `floor(percent * 229 / 100)`:

| °C | % | Chip raw | Factory raw |
|---|---|---|---|
| 0 | 25 | 57 | 57 |
| 50 | 30 | 68 | 68 |
| 53 | 35 | 80 | 80 |
| 56 | 40 | 91 | 91 |
| 59 | 45 | 103 | 103 |
| 62 | 50 | 114 | 114 |
| 65 | 55 | 125 | 125 |
| 68 | 60 | 137 | 137 |
| 71 | 65 | 148 | 148 |
| 74 | 70 | 160 | 160 |
| 77 | 75 | 171 | 171 |
| 80 | 80 | 183 | 183 |
| 83 | 85 | 194 | 194 |
| 86 | 90 | 206 | 206 |
| 89 | 100 | 229 | 229 |

Quiet, Normal, Gaming, and Max are firmware switches, not other curves. Control Center ships this one default custom table per model family. The Factory preset and shipped profile use the chip's exact raw bytes, and readback compares the bytes actually sent.

## Firmware verification (AERO 17 YB, BIOS FB10)

Startup/resume persistence was traced in the local DeepFan reference on 2026-11-08. `gaming_Click` writes `SMARTFAN = 2` and `PreviousFanMode = 2` to the 64-bit `HKLM\SOFTWARE\Gigabyte\SmartManager` key (DeepFan.cs lines 6915–6917). The constructor reads `SMARTFAN` at line 1245 and actively reapplies Gaming at lines 1332–1350, including the YB NVIDIA request. `ResumeProcedure` rereads the preference at line 8766 and reapplies Gaming at lines 8854–8866. MainWindow routes suspend-resume notifications to this procedure. This establishes application-level restoration, not nonvolatile firmware storage. Aero-control independently stores fan-only preferences in its per-user TOML; it does not write the Gigabyte registry.

The official `ControlCenter.exe/SoftwareCenter/MainWindow.cs` checks the `FirstTime` registry value and hides its window on subsequent starts (lines 2340–2355). Its close button hides the window rather than exiting (lines 3052–3061). `App.cs` uses a named mutex and activates the already-running window on a second launch (lines 90–102). Together with DeepFan's constructor/resume paths, this confirms that restoration can happen without manually opening the Control Center window while its process runs. The reference tree does not establish the installer's exact Windows startup registration mechanism. Aero-control uses an explicit scheduled logon task and a background process; it does not claim restoration before sign-in.

The reference tree contains the decompiled DSDT and Control Center DeepFan code. Normal and Quiet use auto flag 0; Gaming uses 1. Recovery uses the Normal sequence. The manual Normal, Quiet and Gaming button handlers do not call SetCurrentFanStep or SetSuperQuiet on this YB; aero-control follows those paths. Deep Control resets the step, while Customize fixed and the fan hotkey use their respective flag and duty sequences. With AI disabled FanAndOcTheme sets NvPowerConfig only for Gaming. Calls in aero-control remain paced and verified, and registry and AI integration are not reproduced. These field values alone do not establish physical fan behaviour.

On 2026-11-08, the registered aero-control logon task was started on demand on the reference laptop. The release stayed running with no main control window, restored Gaming through the normal verified sequence, and persisted the successful fan-only preference. The actual task's ownership/action validation and the native tray path were also checked. This was a task-launch check, not a physical reboot or sleep/wake test.

The DSDT shows SetFanAdjustStatus(0) writes zero to both fan registers and sets GFAN. It is not a clear and is no longer included in the fan scripts. The suggestion that GFAN enables OS fan control is unverified. The fixed sequence follows Control Center and clears GFAN through SetAutoFanStatus; register readback does not prove the fans respond.

WMI transport errors have uncertain side effects. The operation stops and attempts Normal recovery, and reports a failure if recovery fails. Unknown firmware method IDs may return success without acting. Hardware validation, including RPM and thermal observation, remains outstanding.

## Additional replacement features (2026-11-01)

- `gigabyte.mof` getter 87 is `GetNvThermalTarget`; DSDT WMBC case `0x57` returns CRAF and WMBD sets it. The earlier statement that this target has no thermal getter was incorrect; the new path verifies readback.
- Camera197, sleep USB charging122 and hibernation USB charging123 have both MOF declarations and actual YB DSDT handlers (CDON, S3UC, S4UC). Their presence alone does not prove physical port/camera behaviour.
- `MainWindow.cs` vendor-HID reports map `04 00 00 84` to fan, `04 00 00 7C` to Wi-Fi, and `04 00 00 80` to Fn+F6. Fn+F6's action is panel dependent in the official app; aero-control requires explicit display-off selection. DSDT display-off ID196 delegates to SMM `0x95`, so acceptance is not panel-state measurement.
- Display brightness keys notify Windows through DSDT BRTN. Lighting events synchronize an already-applied firmware change. Neither is blindly toggled a second time. The YB model gate does not enable the older PS/2 touchpad path.
- `GetFanAdjustStatus`112 is not a GFAN-ownership getter on this YB: WMBC case `0x70` returns FAN1 raw duty, whereas the setter changes GFAN and both fan registers. No GFAN getter is claimed.
- Startup mode recognition compares durable fan flags and the thermal profile. The NVIDIA power-request flag is transient and is not part of the mode match. It does not guess a custom mode from the step flag or guess which app owns a matching configuration.
- AeroCtl's separate Smart Charge flag and cycle-byte interpretation have not been established for this target. `GetBatteryCount`72 is declared in the MOF but has no target getter handler; the adjacent cycle-byte getters are not combined into an invented total.

Percent-based custom conversion rounds half up; Factory uses the truncated values above. The physical firmware still needs to be checked for exact readback.

## CPU/GPU power controls (2026-11-01)

`SmartDashboard.dll/SmartDashboard/Dashboard.cs` maps CPU buttons 1–5 through `cpuprocess` to CPUOC stages `one`–`five`. `CPUOC.dll/CPUOC/CPU.cs` selects `CPUOC.Resource.P77VB.xml` for AERO 17 YB. Its i7 entries use XTU control 48 for PL1 and 47 for PL2: 38/107, 45/107, 52/107, 58/107 and 62/107 W; `zero` is 52/107 W and `dc` is 45/56.25 W. The i9-only sixth level is not enabled for the verified i7 target. The independent Rust preset table preserves the battery fraction in milliwatts. Arbitrary power sliders and undervolting are outside this implementation.

The same Dashboard's `gpustage0`/`gpustage1` use `SetNvPowerConfig(false/true)` behind the YB `bNvPowerCfg` gate. Since `bGpuBoost` is also true for YB, both then clear `SetAIBoostStatus(0)` (the helper's misleading name is `SetAIBoost208`, but this MOF's actual method ID is 129 / 0x81). WMBD/WMBC map it to the EC FESC bit. The direct GPU controls reproduce this sequence and check the FESC clear. The fan buttons' `FanAndOcTheme` path sends the NVIDIA request without this extra Dashboard step; those sequences remain distinct.

The NVIDIA flag is a transient request, not a verified active power mode. On this laptop with NVIDIA driver 621.01, a bounded native one-byte `SetNvPowerConfig(1)` request read 1 immediately and 0 after 50 ms and 500 ms. A padded four-byte request likewise did not remain at 1; changing the input width was not adopted as a fix. The DSDT sets the external `PEGP.TEMQ` object, sets NLIM, then notifies the NVIDIA device. A driver/firmware consumer is consistent with the observed clearing, but this does not prove the requested power budget was applied. The original GPU request and fan flags were restored and verified after the probe.

Requiring `GetNvPowerConfig == 1` after a 500 ms pause produced false GPU failures and caused Gaming mode to retry and restore Normal despite its fan flags matching. That equality check was removed from fan-mode verification and standalone GPU request handling. Transport errors, malformed request readback, fan/thermal mismatches, and failed FESC clears still fail. The GUI highlights only the last GPU request sent in its current session; it never captures a cleared request as a confirmed Level 1. Resulting GPU wattage and load performance remain unverified.

A second bounded hardware check exercised the corrected core paths on AC power from Normal: manual Level 2 returned success while the transient flag read 0, and Gaming applied in one attempt with fan/thermal flags `(auto=1, fixed=0, step=0, thermal=0)` and the request flag 0. Normal restoration succeeded and all original flags, the request flag and FESC were confirmed as 0 afterward. This verifies command completion and mode flags, not load power or full thermal behaviour.

CPU writes require the installed Intel SDK/service, independently of the direct Gigabyte WMI path. The SDK is loaded from its standard Intel installation directory only after signature verification. The bridge uses the SDK's exposed control metadata and active hardware refresh, rejects locked, incompatible or reboot-required settings, and attempts verified rollback if either limit fails. Process timeout means the final CPU limits are uncertain. Profiles with a CPU preset fail before WMI writes when CPU tuning is unavailable; later WMI failure leaves an already-applied CPU preset in place and says so explicitly.

Read-only inspection found Intel XTU 7.14.2.14 and a running XTU3SERVICE, but its SDK reported `SYSTEM_UNCHECKED` and exposed no usable tuning connection. Its service log also records VBS/hypervisor and BIOS undervolt-protection incompatibilities. Actual CPU power writes and thermal/load behaviour were not tested. CPU controls remain unavailable in this state; the app does not disable security features or replace drivers. Mock SDK tests cover exact limits, locked controls, rejected/no-op writes, rollback and failed recovery.
