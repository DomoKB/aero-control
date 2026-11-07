# Changelog

## 0.2.1 — 2026-11-08

- Remember successful fan-mode changes, fixed duties and exact custom curves automatically, and restore them when the app opens or resumes from sleep, matching Control Center's manual fan lifecycle.
- Migrate existing settings using the active profile's fan settings without enabling CPU or charging automation. Full-profile rules remain optional and take priority when enabled.
- Keep failed commands, startup readbacks and the temporary Max test out of fan-mode memory. Resolve queued resume restoration from the latest confirmed preference.
- Clarify fan restoration and Windows logon startup on Profiles. Invalid settings files continue to block automatic writes.
- Start silently at Windows sign-in, restoring the fan preference without creating the control window. Keep a tray icon for opening the app, resume handling and Exit; report startup failures visibly.
- Prevent duplicate Windows controllers: an interactive second launch opens the existing copy, while a repeated background launch stays silent. Existing startup tasks can be upgraded to background launch from Profiles.
- Resolve Task Scheduler account names to Windows user IDs before checking ownership, so Windows' normalization of a logon trigger cannot falsely reject the app's own task.

## 0.2.0 — 2026-11-01

First documented pre-release for the AERO 17 YB / P77YB / BIOS FB10 reference hardware.

- Rust desktop interface with direct firmware WMI access, without Gigabyte's `acpimof.dll` or MOF registration.
- Firmware fan modes, validated curve editing/import/upload, reference Factory bytes, sensor history and mode recognition after restart.
- GPU Level 1/2 requests and Standard/Quiet thermal profiles. Corrected the assumption that the transient NVIDIA request flag must remain set; Gaming no longer rolls back solely because that flag clears.
- Optional official i7 CPU power presets through the installed Intel XTU SDK, with compatibility checks, readback and rollback. Real CPU tuning remains unavailable on the inspected Intel installation.
- Charging policies, native keyboard lighting, named profiles, opt-in tray/startup/resume/charger automation and laptop hotkeys.
- Windows audio, brightness, radio, camera, USB charging, display-refresh and device-information controls. System readings refresh on demand, without continuous Bluetooth discovery.
- Dynamic device names, clearer request/confirmed-state labels and a simplified header. Version shown in the header and Windows executable metadata.
- Firmware-only fan control; software fan control was removed. Legacy software settings remain loadable and are omitted when settings are next saved.
- Source/provenance notes, coverage tables, user/developer guides, Windows CI and release packaging with dependency notices and a checksum.

Gaming flags, the corrected GPU request path and Normal restoration were checked on the reference laptop. GPU wattage under load and the remaining physical-device behaviours are not fully validated. This is not a universal-model or full Control Center parity release.
