# Where the command facts came from

aero-control does not copy AeroCtl. That project is GPLv3 and was not needed: the questions the scope left open were settled from the sources below. No AeroCtl source is vendored here.

## Windows method names and parameter names

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
- [Aero 15 EC notes](https://github.com/tangalbert919/p37-ec-aero-15/blob/master/Aero%2015%20Fan%20Control%20Registers.md): `0xE5` (229) is 100%, `0x50` (80) is 35%, and the rest of that table matches truncating percent. aero-control rounds half up instead, on purpose

[ejempty/aero-keyboard-fan](https://github.com/ejempty/aero-keyboard-fan) (MIT) documents `SetCurrentFanStep`, the class GUID, and a max-fan sequence that sends duty **100**. This project follows the scope and the Aero 16 Turbo trace (`229`), not that 100.

## Mode names

The Control Center quick-start manual names Normal, Gaming, Quiet, Customize (fixed or max), and Deep Control (15 points). The "46°C / 44%" sentence there is an example of a rule violation, not a default curve.

The factory custom curve for model names containing `yb` (the same branch as `xa` / `va` / `ya` and the rest of that family) is the `setDefault_Click` table in DeepFan.dll. On this AERO 17 YB (P77YB, BIOS FB10), `GetFanIndexValue` 0..14 returned the same temperatures with duty bytes equal to `floor(percent * 229 / 100)`:

| °C | % | Chip raw | aero-control raw |
|---|---|---|---|
| 0 | 25 | 57 | 57 |
| 50 | 30 | 68 | 69 |
| 53 | 35 | 80 | 80 |
| 56 | 40 | 91 | 92 |
| 59 | 45 | 103 | 103 |
| 62 | 50 | 114 | 115 |
| 65 | 55 | 125 | 126 |
| 68 | 60 | 137 | 137 |
| 71 | 65 | 148 | 149 |
| 74 | 70 | 160 | 160 |
| 77 | 75 | 171 | 172 |
| 80 | 80 | 183 | 183 |
| 83 | 85 | 194 | 195 |
| 86 | 90 | 206 | 206 |
| 89 | 100 | 229 | 229 |

Quiet, Normal, Gaming, and Max are firmware switches, not other curves. Control Center ships this one default custom table per model family. aero-control keeps half-up rounding, so an upload of the percent table will not bit-match the chip's truncated bytes (30% is raw 69 here and raw 68 on the chip). Readback compares the raw byte, so that one-count difference restores firmware auto.

## What is provisional

| Item | Why |
|---|---|
| Quiet's decision to leave auto **on** while setting `SetSuperQuiet(1)` | The Aero 16 Silence trace turns auto off, but only together with whisper mode and a thermal target we do not send. Leaving auto on fails safe. |
| Gaming's fan script matching Normal | The Aero 16 Gaming trace uses the same fan flags as Creator/Normal. The gaming change there is Dynamic Boost / `NvPowerConfig`, which is not applied yet. AERO 17 YB DeepFan may differ. |
| `SetFanAdjustStatus(0)` and `SetSuperQuiet(0)` as extra clears | Defensive. Not part of the scope's three-call restore tail. |
| `SetFanCustomStatus` | Named in the scope, absent from the Aero 16 MOF. |
| `CustomFanTable` byte layouts | The registry path is from the scope. The binary layouts are inferred and labelled in the parser warnings. |
| Stair-step drawing | The editor draws the duty holding until the next temperature. Whether the EC interpolates is not separately confirmed. |

## Confirmed enough to send

- Normal / restore tail: `SetFixedFanStatus 0`, `SetStepFanStatus 0`, `SetAutoFanStatus 1` (scope), plus `SetCurrentFanStep 0` (scope §5.1 and the Aero 16 traces).
- Max / fixed shape: auto off, fixed off then on, step on, `SetFixedFanSpeed` and `SetGPUFanDuty`. Max uses 229.
- Custom upload: clear auto and fixed, `SetCurrentFanStep 0`, `SetStepFanStatus 1`, then all 15 `SetFanIndexValue` calls, then `GetFanIndexValue` for every slot.
- Percent conversion rounds half up. 50% is raw 115, not the truncated 114. 30% is raw 69, not the chip's 68.
- Curve index base `0..14` on this P77YB: `GetFanIndexValue` returned the 15 factory points above.
- The YB factory percent table is the DeepFan `setDefault_Click` default, not a guessed curve.
- A `(0, 0)` point is rejected anywhere in a user curve because it terminates the table.
