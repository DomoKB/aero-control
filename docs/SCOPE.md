# aero-control — Scope Document

**Project location:** `/workspace/Development/aero-control/` (this file: `SCOPE.md`). This is a scope document only. No code has been written and no `cargo init` has been run.

**Target:** Gigabyte AERO 17 YB, Windows 10/11, i7-10875H, RTX 2080 Super Max-Q.
**Goal:** an open-source Rust + egui app that replaces Gigabyte Control Center (GCC) for fans, power, GPU, battery and profiles.

---

## 1. Summary (plain English)

Gigabyte's Control Center doesn't control the fans itself. It sends short commands to the laptop's firmware (BIOS → embedded controller) through a Windows interface called WMI. The fan chip then runs on its own. aero-control sends the same commands from a small, clean, open-source app: pick a fan mode, draw your own fan curve, set GPU boost and a battery charge limit, and switch profiles automatically on charger or battery. It never runs its own fan loop. If anything goes wrong, it hands the fans back to the firmware's automatic mode. CPU power limits are the exception. Control Center doesn't set those through the firmware. It uses Intel's XTU SDK (see §3.6), so that feature is optional and comes later.

---

## 2. Features

| Area | Feature | Mechanism (as GCC does it) |
|---|---|---|
| Fans | Modes: Normal/Auto, Quiet (SuperQuiet), Gaming, Max/Fixed, Custom curve | `GB_WMIACPI_Set` switches: `SetAutoFanStatus`, `SetFixedFanStatus`, `SetStepFanStatus`, `SetSuperQuiet`, `SetFanAdjustStatus`, `SetCurrentFanStep` (byte 0/1) |
| Fans | Fixed duty (CPU + GPU fan) | `SetFixedFanSpeed`, `SetGPUFanDuty`, scale 0–229 (229 = max) |
| Fans | Custom curve, up to 15 points | `SetStepFanStatus 1` → `SetFanIndexValue(Index, Temperture, Value)` ×N → `(0,0)` terminator. Read back with `GetFanIndexValue` |
| Fans | Fn fan hotkey (cycle modes) | Global hotkey in the GUI, or the hotkey event GCC listens for (to be confirmed) → service command |
| Sensors (display only) | CPU/GPU temperature, fan RPM | `GB_WMIACPI_Get`: `getCpuTemp`, `getGpuTemp1/2`, `getRpm1/2` (the RPM bytes may be swapped) |
| GPU | NVIDIA Dynamic Boost on/off | `SetDynamicBoostStatus` / `GetDynamicBoostStatus` (WMI) |
| GPU | GPU power config (TGP mode) | `SetNvPowerConfig` / `GetNvPowerConfig` (WMI; GCC writes Data 0/1) |
| GPU | GPU thermal target | `SetNvThermalTarget(byte)` / `GetNvThermalTarget` (WMI) |
| GPU | dGPU/iGPU mode (MUX / Optimus) | `SetPEGorSG` / `GetPEGorSG` (needs a reboot; the UI must warn) |
| Power | AI Boost / power mode presets | `SetAIBoostStatus`, plus CPU PL presets (see §3.6) |
| Power | **CPU power limits PL1/PL2** | **Not GB_WMIACPI.** GCC uses the Intel XTU SDK (see §3.6). Optional, behind a feature flag |
| Power | **GPU power limit** | `SetNvPowerConfig` + `SetDynamicBoostStatus` + `SetNvThermalTarget` (WMI). GCC's older-model fallback is NVAPI clock offsets via `NvFunc.dll` (out of scope) |
| Battery | Charge limit 60–100% | `SetChargePolicy` (custom mode) + `SetChargeStop(percent)`, read back with the getters |
| Profiles | Named TOML profiles, auto-switching on AC/DC | Service applies on AC/DC change |
| Lifecycle | Re-apply at startup, on resume and on AC/DC change | Power-broadcast notifications in the service |
| Safety | Restore firmware auto on exit or error, curve validation, readback | See §4.5 |

**Non-goals:** RGB keyboard and LED control (possible later: `SetWiFiLED` etc. exist), GPU/memory overclocking, a software fan loop, Linux support.

---

## 3. Command details and sources

### 3.1 Reference priority
1. **Primary:**
   - GCC decompiled code in `/workspace/gcc/out/`: DeepFan.dll (`Helpers.cs`, `DeepFan.cs` mode logic around lines 1295–1460), SmartManager.dll (`CWMI.cs`, `UserControl1.cs`), AiBoost.dll (`Helper.cs`, `Boost.cs`) and SmartDashboard.dll (`Helpers.cs`: NvPowerConfig and NvThermalTarget).
   - `gigabyte-laptop-wmi`: `USAGE.md`, `aorus-laptop.c`, DSDT dumps.
   - `aero-keyboard-fan` (MIT): clean-room `aero_fan.mof` and the scheduled-task elevation pattern.
   - `alfc` (MIT): for command details only (it has a software loop, which we do NOT copy). Its `server/native/windows/CPUOC.cs` documents the XTU PL calls.
2. **Fallback only:** AeroCtl (gitlab.com/wtwrp/aeroctl, **GPLv3**, C#). `P7FanController.cs` confirms the 15-point curve, the max of 229 and model selection by SKU prefix (P7/P8/P4). Consult it only when the primary sources can't settle a question. **Copying its code would make aero-control GPLv3.** Use it for facts only and record any such use.

### 3.2 WMI channel
- Namespace `root\WMI`. Classes `GB_WMIACPI_Get` (getters return the value in an out-param) and `GB_WMIACPI_Set` (setters take a `Data` byte, or Index/Temperture/Value).
- GCC calls `ManagementClass.InvokeMethod` with `EnablePrivileges = true` and Impersonate. Admin rights are required.
- The class exists only when the MOF is registered: Gigabyte's `acpimof.dll`, or aero-keyboard-fan's clean-room `aero_fan.mof`. The installer or probe must detect this and offer to register the clean-room MOF.
- Quirks to replicate:
  - About 500 ms sleep between calls.
  - Some setters throw (WBEM error) by design while still taking effect. Treat errors per method, and verify with a getter rather than trusting the return code.

### 3.3 Fan modes
Each mode is a sequence of switch writes, mirrored from `DeepFan.cs`. GCC persists `SMARTFAN` 0–7 and `CustomFanTable` under `HKLM\SOFTWARE\Gigabyte\SmartManager`. aero-control uses its own TOML and does not write GCC's registry.
- **Max:** `SetAutoFanStatus 0`, `SetFixedFanStatus 0`, `SetStepFanStatus 1`, `SetFixedFanStatus 1`, `SetFixedFanSpeed 229`, `SetGPUFanDuty 229`.
- **Restore firmware auto:** `SetFixedFanStatus 0`, `SetStepFanStatus 0`, `SetAutoFanStatus 1`.
- **Custom:** `SetStepFanStatus 1` → upload the curve (§5) → (0,0) terminator → readback.
- `SetDeepFan` (5-point) exists but GCC doesn't use it. Out of scope.

### 3.4 Fan control principle
Firmware-only. The service never polls temperatures to drive fan speeds. It writes settings only on demand, at startup, on AC/DC change and on resume. Sensor reads are for display only, at a low rate (1–2 s, and only while the GUI is open).

### 3.5 GPU power (findings from the decompiled code)
- `AiBoost.Helper.SetGpu`:
  - If the model supports `bNvPowerCfg`, it calls `SetNvPowerCfg` → WMI `GB_WMIACPI_Set.SetNvPowerConfig` with `Data` 0/1.
  - Otherwise it calls `SetGpuOC(value, sBiosModelName)` with a per-model table of core/memory clock offsets, applied through `ModifyNvGPUClock` / `SetGPUMemOC` in Gigabyte's **`NvFunc.dll`** (an NVAPI wrapper, `DllImport`).
- `SetDynamicBoost` → WMI `SetDynamicBoostStatus` (input 0/2 → Data 0; 1/3 → Data 1).
- SmartDashboard `Helpers.cs`: `SetNvThermalTarget(byte)` / `GetNvThermalTarget`, `SetNvPowerConfig` / `GetNvPowerConfig`.
- gigabyte-laptop-wmi `USAGE.md`: "GPU boost" is NVIDIA Dynamic Boost. The EC can toggle it, and RTX 20-series parts have a single mode.
- **Scope:** GPU power limits are set only through these three WMI methods. No numeric TGP slider: the firmware exposes modes and on/off, not watts. NVAPI clock offsets (`NvFunc.dll` path) are out of scope. They are an overclock rather than a power limit, and would need NVAPI bindings (`nvapi-rs` or raw FFI to `nvapi64.dll`).

### 3.6 CPU power limits PL1/PL2: **not via GB_WMIACPI**
- `AiBoost.Helper.SetCpu(stage)` maps stages 0–8 to preset names ("zero", "one"…"six", "dc", "dc2") and calls `new CPU().OC(ref text)` from Gigabyte's **`CPUOC`** assembly. That assembly is not among the decompiled outputs.
- alfc's `CPUOC.cs` shows how the same thing works: the **Intel XTU SDK** (`Intel.Overclocking.SDK.Tuning.ITuningLibrary`, `TuningLibrary.Instance.Tune(id, value)`), with **control ID 48 = PL1 and 47 = PL2**, 500 ms apart.
- GCC disables all of this when `bXTUblock = HyperV_VM_HVCI_exist()`, meaning Hyper-V/VBS/HVCI (Memory Integrity) is on. XTU's driver can't work under HVCI.
- **Rust approach (pick one; decide in M6):**
  1. **XTU bridge (matches GCC, recommended):** a small helper that loads the XTU SDK. It's a .NET assembly, so either ship a tiny C# shim (`xtu-bridge.exe`, invoked by the service with JSON args) or host the CLR from Rust (`netcorehost`, more complex). It requires Intel XTU (the XTU service) to be installed. The XTU SDK licence must be checked for redistribution: we depend on the user's install and don't ship it.
  2. **Direct MSR write** (`MSR_PKG_POWER_LIMIT` 0x610) or the MMIO mirror. Needs a kernel driver (WinRing0 is vulnerable and blocklisted by Defender/HVCI). **Rejected**: security risk and blocked under HVCI.
  3. **Windows power plan only** (`powercfg` processor max state / boost mode). Coarse but safe, and a useful fallback when XTU is blocked.
- **Risks:**
  - XTU must be present and compatible with this CPU (Comet Lake H is supported by XTU).
  - It's unavailable when HVCI/Memory Integrity is on.
  - The BIOS may lock PL registers (the PL lock bit), and the EC/BIOS may override PLs on AC/DC change. That's why we re-apply on power events.
  - Intel deprecated XTU support for some mobile SKUs in newer releases, so pin a known-good version.
  - It's a second elevated component.
- **Scope:** Optional "CPU power" feature, behind a Cargo feature and a runtime capability check. PL1/PL2 sliders are clamped to safe bounds (e.g. PL1 15–65 W, PL2 ≤ 107 W, with the exact bounds from the probe and XTU's reported ranges). The default is "don't touch".

---

## 4. Architecture

### 4.1 Recommended: service + unprivileged GUI
```
┌────────────────────┐   named pipe \\.\pipe\aero-control (JSON)   ┌───────────────────────────┐
│ aero-gui (user)    │ ◄──────────────────────────────────────────► │ aero-svc (LocalSystem)    │
│ eframe/egui + tray │   SDDL: SYSTEM + Administrators full;        │ windows-service crate     │
│ global hotkey      │   Interactive users read/write (no remote)   │ WMI layer, profile engine │
└────────────────────┘                                              │ power-event re-apply      │
                                                                     │ [optional] xtu-bridge.exe │
                                                                     └───────────────────────────┘
```
- **Service** (`windows-service`):
  - Owns all WMI writes and serialises calls with the 500 ms spacing.
  - Applies the active profile at start, on `PBT_APMRESUMEAUTOMATIC`/`PBT_APMRESUMESUSPEND`, and on `PBT_APMPOWERSTATUSCHANGE` (AC/DC).
  - Restores firmware auto on stop or failure.
  - Accepts commands only from the pipe.
- **Pipe security:**
  - An explicit SDDL denies the Network SID, grants the local interactive user and Administrators, and rejects remote clients (`PIPE_REJECT_REMOTE_CLIENTS`).
  - Every request is validated server-side (same curve validation as the GUI), and versioned (`protocol_version`).
- **GUI:**
  - egui window with tabs (Dashboard, Fans/Curve Editor, Power/GPU, Battery, Profiles, Settings) and a tray icon (mode menu).
  - The global hotkey cycles fan modes.
  - Autostarts per user (Run key). Holds no privileges.

### 4.2 Simpler MVP alternative
A single elevated egui process, launched at logon by a Scheduled Task with "Run with highest privileges". That's the aero-keyboard-fan pattern, and it avoids a UAC prompt. Same core crate.
- Cons: no re-apply while logged out, and an elevated GUI is a bigger attack surface.
- Can migrate to the service later without changing the core.

### 4.3 Configuration
`%ProgramData%\aero-control\config.toml` plus `profiles\*.toml`. The service creates the ACLs: writable by Admins/SYSTEM only, with the GUI writing through the service.

Example profile fields: `name`, `fan_mode`, `fixed_duty`, `curve = [[temp, duty], …]`, `dynamic_boost`, `nv_power_config`, `nv_thermal_target`, `ai_boost`, `charge_limit`, optional `cpu_pl1_w`/`cpu_pl2_w`, plus `ac_profile`/`dc_profile` bindings.

### 4.4 Coexistence
- At first run, detect GCC (`ControlCenter.exe`, the SmartManager services and scheduled tasks) and XTU.
- GCC re-applies its mode at launch and will fight aero-control. Offer guided disable or uninstall of GCC's autostart and services (with a clear explanation, done by the user or with explicit consent), and keep `acpimof.dll`.

### 4.5 Safety
- On exit, service stop, panic or a WMI failure in a fan sequence: **restore firmware auto** (`SetFixedFanStatus 0`, `SetStepFanStatus 0`, `SetAutoFanStatus 1`).
- **Curve validation:**
  - 2–15 points, temperatures strictly increasing, duty non-decreasing.
  - Temperatures 0–100 °C (UI range 30–95).
  - Duty 0–229.
  - Minimum duty at or above 85 °C, e.g. ≥ 50%. The last point must be ≥ 80% duty, and the last temperature ≤ 90 °C.
- **Fixed mode floor:** e.g. at least ~35% (≈ 80/229). Configurable down only with an explicit "expert" toggle.
- **Readback** after every curve upload (`GetFanIndexValue`) and after toggles (getters). On mismatch: retry once, then restore auto and report.
- Thermal protection ultimately relies on firmware/CPU/GPU throttling (PROCHOT, GPU slowdown). This must be stated in the UI and README.
- Battery limit clamped to 60–100%. CPU PL clamped (§3.6).

---

## 5. Curve editor (dedicated section)

Purpose: create, edit, save and load custom fan curves of up to 15 temperature → duty points, handed to the fan chip, like GCC's DeepFan custom editor.

- **Plot (`egui_plot`):**
  - X = temperature (°C, 20–100), Y = duty shown as % with the raw 0–229 value in a tooltip.
  - Line plus point markers; drag points (built from `PlotResponse` pointer coordinates).
  - Drags are constrained between neighbours (monotonic) and snap to 1 °C / 1%.
  - Double-click adds a point (≤ 15); right-click deletes one (≥ 2).
  - Live CPU/GPU temperatures shown as vertical lines (display only).
- **Numeric entry:** an editable table (Index, Temp °C, Duty %, Raw) with `DragValue` fields, kept in sync with the plot.
- **Validation:** the rules from §4.5, shown inline with per-point error highlighting. Save/Upload stays disabled while the curve is invalid.
- **Presets:** Silent, Balanced, Performance, Max-ish, and "Import GCC curve". The import reads `CustomFanTable` from GCC's registry (read-only) and converts it.
- **Save/load:** curves are part of profiles (TOML). Export and import as standalone `.toml`.
- **Upload:**
  1. Send to the service.
  2. `SetStepFanStatus 1`.
  3. `SetFanIndexValue(i, t, v)` for each point, 500 ms apart, with a progress bar.
  4. `(0,0)` terminator.
  5. Readback.
- **"What the chip applies":**
  - After readback, plot the chip's table as a second series (dashed) over the edited one. Differences (rounding, clamping, dropped points) are highlighted.
  - Show the scale conversion (% ↔ 0–229) and note the firmware's interpolation (step vs linear, unknown until tested). Once known, render step or linear accordingly.
  - Optionally show the live RPM next to the expected duty for the current temperature.
- **Open questions:**
  - Whether the CPU and GPU fans share one table (GCC uses one).
  - What value format `GetFanIndexValue` returns (packed temperature/duty bytes?). Check against `aorus-laptop.c` `fan_curve_data`, which packs `temp*256+duty`.
  - Whether the curve persists across reboot.

### 5.1 Why Control Center's curves feel inaccurate, and how this app fixes it
**What GCC does (decompiled `DeepFan.cs` ~2763–2900):**
- **Partial uploads:** Apply sends only the points flagged as changed (`bLevel` flags). It swallows send failures and never reads the curve back, so the chip can quietly hold a mix of old and new points.
- **No re-apply after resume from sleep.**
- **Incomplete mode switch:** the mode path skips `SetCurrentFanStep(0)` and never clears the `SetFanCustomStatus` cap. A leftover step or cap can override the curve.
- **One shared table:** a single 15-point table, likely shared by both fans. GPU fan duty is used only in fixed and max modes, and which temperature (CPU, GPU or the higher of the two) drives the table is unknown.
- **Stepping:** the chip likely steps between points rather than interpolating (speculative). That makes speeds jump at thresholds.
- **Duty vs RPM:** duty isn't linear to RPM, and the UI floor is 25%. The % on screen doesn't match what you hear.
- **Rounding:** the % → raw conversion rounds down (`duty*229/100`) while the read-back rounds up, so displayed values drift by 1%.
- **Truncation:** a (0,0) point in the middle of the curve acts as the terminator and cuts off every point after it.

**aero-control fixes (scope):**
- **Full upload and verify:** always send all 15 points in order (padding unused slots consistently and placing the (0,0) terminator correctly), then read every point back. Retry once on mismatch, then report it and fall back to firmware auto.
- **Full re-apply sequence at boot, resume, mode change and AC/DC change:**
  1. Clear auto, fixed and the custom cap.
  2. `SetCurrentFanStep(0)`.
  3. `SetStepFanStatus 1`.
  4. All points.
  5. Verify.
  
  A periodic drift check (e.g. every 5 min, getters only, display/verify only, never fan driving) re-applies if the chip's table changed.
- **Correct rounding:** round-to-nearest for % ↔ raw, and show the raw 0–229 value sent next to each point.
- **Validation:** reject curves that go downward and any (0,0) point in the middle (in addition to §4.5).
- **Stair-step preview:** draw the curve as the chip is believed to apply it (steps), overlaid on the readback. Switch to linear if diagnostics show interpolation.
- **Diagnostic logging mode** (opt-in): log CPU temp, GPU temp, RPM1 and RPM2 once a second to CSV, while recording the active curve and mode. This settles which temperature drives the curve, step vs interpolation, and reaction time, and produces a %-to-RPM chart shown in the editor. This is measurement only and never feeds back into fan control.

---

## 6. Crates

| Purpose | Crate |
|---|---|
| WMI | `wmi`: `exec_method` with in-params via `get_object("GB_WMIACPI_Set")` → `get_method(name)` → `spawn_instance()` → set props → exec on the class or instance path. **Fallback:** the raw `windows` crate (`Win32_System_Wmi`: `IWbemServices::ExecMethod`, `IWbemClassObject::Put` with an explicit `VT_UI1`/`CIM_UINT8`) |
| ⚠ type risk | MOF params are `uint8`. A `Variant`/`VARIANT` of I4/UI4 may be rejected (type mismatch) or silently truncated. Put an explicit `VT_UI1` (or the CIM type), and test each method. RPM/temperature out-params may come back as UI1/UI2/I4. Normalise them, and handle byte-swapped RPM |
| Service | `windows-service` |
| GUI | `eframe`/`egui`, `egui_plot` (curve editor) |
| Tray, hotkey | `tray-icon` (+ `muda` menus), `global-hotkey` |
| Config/IPC | `serde`, `toml`, `serde_json` |
| Pipe | `interprocess` (local sockets / named pipes) or `tokio::net::windows::named_pipe` (custom SDDL via `windows` `SECURITY_ATTRIBUTES`) |
| Power events | `windows` (`RegisterPowerSettingNotification`, `WM_POWERBROADCAST`, service `POWEREVENT` control) |
| Logging/errors | `tracing`, `tracing-subscriber`, `tracing-appender`; `anyhow` (bins), `thiserror` (libs) |
| CPU PL (optional) | XTU bridge: a C# shim process (`std::process`) or `netcorehost`. No MSR driver |
| Packaging | `cargo-wix` or `winres` for the manifest |

---

## 7. Suggested workspace layout
```
aero-control/
  Cargo.toml                 # [workspace]
  crates/
    aero-wmi/                # GB_WMIACPI Get/Set wrappers, typed methods, quirks, probe
    aero-core/               # modes, curve model + validation, profiles (TOML), safety/restore
    aero-proto/              # IPC request/response types (serde), versioning
    aero-svc/                # Windows service: pipe server, power events, apply engine
    aero-gui/                # eframe app, curve editor, tray, hotkey, pipe client
    aero-probe/              # CLI: SKU check, MOF check, getter-only capability dump
    aero-xtu/ (optional)     # PL1/PL2 via XTU bridge (feature "cpu-power")
  tools/xtu-bridge/          # tiny C# shim (optional)
  docs/  SCOPE.md
```

---

## 8. Hard parts and risks
1. **Unconfirmed model and undocumented methods.**
   - The AERO 17 YB isn't confirmed. The first step is to read the SKU and BIOS model (`Win32_ComputerSystem`, `MS_SystemInformation`, the BIOS model name GCC uses: `sBiosModelName`) and run **aero-probe as admin, getters only**.
   - Methods vary by model, and some setters throw by design.
2. **CPU power limits via XTU**: an external dependency, blocked by HVCI, possible BIOS PL lock, EC overrides on AC/DC, and licensing. Ideally treated as optional.
3. **WMI typing:** `uint8` in-params, out-param types, byte-swapped RPM, the 0–229 scale (not 0–255 or 0–100), and the curve value encoding.
4. **Persistence:** whether modes, curves, the charge limit and GPU settings survive reboot, sleep, hibernate and AC/DC change. Re-apply anyway; measure what's needed.
5. **Elevation:** the service install and the pipe ACL. Or, for the MVP, a Scheduled Task. MOF registration if `acpimof.dll` is missing.
6. **Coexistence:** GCC re-applies on launch, and XTU and other OEM tools may also write. Detection, and guided disable.
7. **Safety relies on firmware protections.** We validate, but can't prevent every bad state.
8. **Hardware testing:** only real hardware will do. Bad writes need care (restore path, a BIOS reset as last resort).
9. **Hotkey:** the Fn fan key may be handled by the EC or GCC via a WMI event. Detecting the native event is uncertain; a fallback is a user-chosen global hotkey.

---

## 9. Milestones
- **M0 – Probe (2–3 days):** `aero-probe`. SKU and BIOS model, MOF presence, list `GB_WMIACPI_Get`/`Set` methods and signatures, call every getter, dump JSON. No writes.
- **M1 – WMI layer (3–4 days):** typed getters/setters, `uint8` handling, 500 ms pacing, error policy, restore-auto. Single-write tests on hardware (e.g. Max then restore).
- **M2 – Fan modes + minimal GUI (MVP, elevated process via Scheduled Task) (4–5 days):** mode buttons, sensor display, tray.
- **M3 – Curve editor (5–7 days, incl. full-upload/verify, re-apply sequence, drift check, diagnostics mode):** plot and table editing, validation, presets, upload and readback, "chip applies" overlay.
- **M4 – Battery + GPU (2–3 days):** charge limit 60–100, Dynamic Boost, NvPowerConfig, thermal target, AI Boost, PEGorSG with a reboot warning.
- **M5 – Profiles + events (3–4 days):** TOML profiles, AC/DC bindings, re-apply on resume and power change, hotkey. **← MVP done**
- **M6 – Service split (5–7 days):** `windows-service`, pipe with SDDL, proto, installer, GCC coexistence wizard.
- **M7 – CPU power (optional, 4–7 days):** XTU bridge, capability and HVCI detection, PL1/PL2 sliders with clamps, re-apply.
- **M8 – Hardening + hardware test (1–2 weeks):** sleep/hibernate cycles, AC/DC flapping, failure injection, long-run, docs, release.

---

## 10. Effort estimate (one experienced developer)
The earlier software-loop version was MVP 2–3 weeks, full 6–10 weeks plus 1–2 weeks of hardware testing. Removing the fan loop cuts the control logic, tuning and fail-safe-loop work. The added curve editor and optional CPU PL feature put some of it back.

| Scope | Estimate |
|---|---|
| **MVP** (M0–M5, ~+1 day for curve-accuracy fixes): elevated single process, fan modes, curve editor, battery, GPU, profiles, AC/DC/resume re-apply) | **~2–2.5 weeks** |
| **Full without CPU PL** (+ service/pipe, installer, coexistence, polish) | **~4–6 weeks** |
| **+ CPU PL via XTU** | **+1–1.5 weeks** |
| **Hardware testing/hardening** | **+1–2 weeks** (unchanged; it's hardware-bound) |
| **Total full** | **~5–7.5 weeks + 1–2 weeks testing** |

The main schedule risk is M0. If key methods are missing or behave differently on the AERO 17 YB, add time to read the DSDT and consult the fallback sources.
