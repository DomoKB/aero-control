# AeroCtl replacement coverage

AeroCtl is an additional functional target alongside Control Center. Scope is the AERO 17 YB / P77YB / FB10. Other model names are detected and displayed, but detection does not establish support: their firmware fan/power writes are disabled.

The comparison uses [AeroCtl's README](https://gitlab.com/wtwrp/aeroctl/-/raw/master/README.md), [public UI labels](https://gitlab.com/wtwrp/aeroctl/-/raw/master/AeroCtl.UI/MainWindow.xaml), and public feature/configuration declarations. Implementation uses the local official Control Center, target firmware, Windows APIs and independently written Rust. No AeroCtl code is copied or vendored.

| AeroCtl capability | aero-control coverage | Limits |
|---|---|---|
| Temperatures and fan RPM | Live CPU/GPU temperatures and both RPM readings | Readback is not a load/thermal validation. |
| Model/SKU/BIOS information | Detected header and System device-information card; keyboard firmware on Keyboard | No fabricated identity/serial; missing SMBIOS fields remain unavailable. |
| Hardware fan modes and curve | Normal, Quiet, Gaming, Fixed, Max, full validated hardware curve | Separate capped Auto semantics are not reproduced; target MOF lacks SetFanCustomStatus. |
| Restart mode highlight | Durable fan flags and GPU thermal profile | The transient NVIDIA power-request flag does not invalidate Gaming. Ambiguous step/custom stays unknown; Fixed at 100% is labelled Max. |
| Software fan controller | Not provided at the user's request | Firmware modes and curves remain available. |
| Saved profiles/lifecycle | Automatic fan-mode memory and startup/resume restoration, exact curve bytes, optional full-profile and charger rules | Legacy software tuning is ignored and omitted when settings are next saved. The first supported release launch sets up Windows background startup and respects opt-outs. |
| Notification area and minimized launch | Silent Windows startup with tray access; optional tray/minimized behaviour for manual launch | Background startup creates no control window; native icon registration is checked and failures open the UI. A second launch opens the same controller. |
| Wi-Fi/Bluetooth | Explicit per-radio controls and opt-in native Wi-Fi Fn group toggle | Windows consent and hardware/policy restrictions apply. |
| Fan Fn and Fn+F6 | Opt-in fan toggle; explicit Fn+F6 display-off assignment | No inferred OLED workstation-lock assignment. |
| Brightness and keyboard Fn behaviour | Windows/firmware handling is preserved; ordinary brightness controls available | No second toggle for already-handled brightness/lighting events. Touchpad vendor toggle is not blindly applied to YB. |
| Camera | On/off with getter verification | Physical camera validation remains. |
| Sleep/hibernation USB charging | Independent on/off with getter verification | Actual port/sleep-state support needs hardware validation. |
| GPU modes/thermal setting | Target-specific Level 1/2 requests; verified thermal profile | The request flag can reset immediately, so active GPU power is not inferred from it. Newer MUX/Dynamic Boost IDs are not supported; no wattage claim. |
| Charging limit and battery status | Standard/custom charge stop, Windows battery state, known health code labels | Separate Smart Charge semantics and cycle-byte combination are unverified; no invented cycle total. |
| Display refresh rate | Explicit enumerated rates; optional AC/DC rule tied to monitor identity | No arbitrary resolution/Hz, no timer enumeration; live display switching remains untested. |
| RGB keyboard | Effects, colours, brightness/speed/direction, five profiles and solid custom pictures | Full physical per-key editor, controller-reset equivalent and external custom-effect API remain missing. |
| Auto Static Control | Not implemented for the current non-OLED target | No OLED-care feature is implied by a model-name match. |

Vendor AI and Pantone profiles remain excluded as requested. Official YB i7 CPU power presets now use a compatible installed Intel XTU SDK/service; this does not add undervolting, clock-ratio tuning or a software fan controller. This is not a universal AeroCtl-compatible implementation or a claim of full feature parity. Touchpad-specific control, the remaining RGB tools, Smart Charge/cycle semantics and capped Auto require further verified mechanisms. AeroCtl settings files are not imported automatically.

## Verification (2026-11-01)

Physical fan, camera, radio, display and scheduled-task behaviour still requires hardware validation. The laptop's SMBIOS registry identifies AERO 17 YB / P77YB / FB10; this confirms identification, not physical control behaviour or compatibility with another model.
