# Control Center feature coverage

Target: AERO 17 YB (P77YB / BIOS FB10). This records implemented behaviour, not proof of identical fan response or CPU/GPU wattage under load. Sources are the local decompiled Control Center and DSDT; see SOURCES.md. Vendor AI and Pantone calibration remain excluded. CPU presets require an Intel Core i7-10875H and a compatible installed Intel XTU SDK/service.

| Official area | aero-control behaviour | Verification / limits |
|---|---|---|
| Normal, Quiet, Gaming, Fixed and Max | Model-specific fan/GPU mode commands | Manual paths checked against DeepFan; mock command/readback tests. Gaming flags and Normal restoration verified on hardware; full thermal/RPM response remains unvalidated. |
| Deep Control | Editor, official Factory curve, import, exact raw bytes, full upload/readback | Adds validation and recovery. The plotted step response is an estimate. |
| GPU performance | Level 1/2 requests and thermal profiles | The NVIDIA flag is transient; selection records the last request, not confirmed power. The thermal target has durable readback. No wattage claim. |
| CPU power levels | Official i7 CPU levels 1–5, default and battery presets; explicit apply/readback; profile capture/apply | Signed installed Intel XTU SDK, supported unlocked controls and compatibility required. Failed pair writes attempt rollback. Live CPU tuning is currently unavailable on the inspected installation; see SOURCES.md. |
| Charging | Standard or 60–100% custom limit, quick presets, profile capture/apply | Setter/getter tests; actual charging behaviour needs validation. |
| Saved settings | Named profiles preserving exact fan duty/curve values, last locally requested GPU level and charging policy | Per-user TOML, protected built-ins, invalid-file preservation, atomic replacement. Saving does not apply. |
| AC/DC switching and resume | Automatic fan-only resume restoration; optional full-profile automation while the app runs | Windows power broadcasts plus power-source polling; wake notifications coalesced. Persisted cold-start simulation and policy tests; live wake/charger tests remain. |
| Notification area | Tray on background startup; optional for manual launch; show window, Normal/Quiet/Gaming, explicit Exit | No control window is created for background startup. Native icon registration with mock restoration verified; Explorer restart/menu testing remains. |
| Native fan Fn key | Opt-in Max / previous confirmed mode toggle using the exact Gigabyte vendor-HID report | Known VID/PID and usage filtering; bounded packet parser tests. Physical key testing remains. |
| Other native Fn keys | Opt-in Wi-Fi group toggle and explicitly selected Fn+F6 display off | Fresh Wi-Fi state and full readback. OS/firmware brightness/lighting handling is not repeated. |
| Camera and USB charging | Camera, sleep USB charge and hibernation USB charge | Serialized setters with getter verification. Physical device validation remains. |
| Model identification | Detected model/SKU/BIOS and model-aware firmware write gate | Read from SMBIOS registry independently of Gigabyte provider availability. |
| Fan state on restart | Initial firmware recognition followed by automatic restoration of the last verified fan preference | Exact duty/curve persistence; legacy settings migrate from the active profile. Invalid files block restoration. Windows background startup is configured on the first supported release launch. |
| Software fan control | Not provided at the user's request | Firmware fan modes and curves remain available. See AEROCTL_PARITY.md. |
| Windows logon startup | Automatic first-release-launch setup, with opt-out; per-user interactive logon, highest available privileges and background launch | First-run/opt-out/failure tests; live registration, ownership and task-launch checks: Gaming restored with no control window. Account names are normalized. A second launch opens the existing controller. Physical boot remains to be checked. |
| Volume/mute | Default multimedia output, separate drafts and confirmed values | Read on entry, explicit refresh or a local change; no timer polling. Core Audio backend and mock/UI tests. |
| Brightness | Active power-plan setting for AC/DC, matching SmartManager's mechanism | Read on entry, explicit refresh or a local change; no timer polling. Confirms the stored setting, not measured panel luminance. |
| Display refresh rate | Enumerated rates and opt-in charger/battery switching | Actual modes only, stable monitor identity, test/apply/readback and failure recovery. |
| Wi-Fi and Bluetooth | Read and explicitly toggle individual Windows radios | Reads on page entry, explicit refresh and after a change; no periodic enumeration or nearby-device discovery. Consent/denied/hardware-disabled states, asynchronous operations, readback. Mock tests; live radio tests remain. |
| Keyboard lighting | Effects, colours, brightness, speed, direction, five saved profiles, solid custom pictures | HID readback where supported. Full Fusion macro/per-key editor source is absent. |
| Touchpad, projection and advanced display settings | Clearly labelled Windows settings links | Navigation only; these are not vendor hardware toggles or calibrated colour modes. |

## Remaining differences

- The GUI's profile automation requires the app to be running. There is no installed Windows service. The vendor OSD process is not replicated; status appears in the app.
- Full Fusion keyboard macros, application hotkeys and physical per-key painting require the missing module/protocol mapping. Existing lighting and custom-picture paths remain available.
- Vendor AI/cloud components and Pantone profiles are intentionally excluded. CPU power uses Intel XTU's existing SDK/service, with no bundled driver or security-setting changes.
- MUX, newer AI Boost and the generic Dynamic Boost method are not enabled for this YB in the official model gates; the DSDT has no handler for Dynamic Boost ID 231.
- Settings are stored separately from Control Center. Running both apps can overwrite the same firmware settings; aero-control does not disable or uninstall the official app.

The source fact paths are `ControlCenter.exe/SoftwareCenter/MainWindow.cs` (YB gates), `DeepFan.dll/testUsercontrolDll/DeepFan.cs` (fan/GPU modes), `SmartManager.dll/SmartManager/CPOWERAPI.cs` (brightness/power), `SmartManager.dll/SmartManager/UserControl1.cs` (charging/audio/radios), `CPUOC.dll/CPUOC/CPU.cs` and `CPUOC.Resource.P77VB.xml` (CPU SDK calls and limits), `SmartDashboard.dll/SmartDashboard/Dashboard.cs` (CPU/GPU level buttons), and firmware `dsdt.dsl` (register effects).

Verification for the latest GPU fix: 198 Rust tests passed (110 outside the GUI, 88 GUI/session tests linked without the production administrator manifest). The earlier CPU update also passed seven fake-SDK bridge scenarios under Windows PowerShell 5.1. Release build, formatting and diff checks passed. A bounded hardware check confirmed the corrected Level 2 request, Gaming flags and Normal restoration with a cleared NVIDIA request flag. The read-only Intel SDK probe failed closed with `SYSTEM_UNCHECKED`; live CPU power writes and physical load behaviour remain unverified.
