# Changelog

## 0.2.5 — 2026-11-13

- Fix the frozen window left behind when closing a startup/tray session. Process deferred Windows window destruction after the renderer exits and continue servicing those messages in the background.
- Test the actual Windows close-message path, verify the old native window is destroyed before reopening, and cover repeated close/open and tray Exit with simulated fan hardware.

## 0.2.4

- Detect NVIDIA availability through the Windows device tree. Keep fan modes, saved-mode restoration and recovery working when the GPU is disabled or unavailable, without NVIDIA power requests or GPU temperature queries. Preserve the official sequence when the GPU is available.
- Retain the firmware quiet/thermal flag in both configurations because it controls the embedded controller’s fan state independently of the GPU. Show GPU availability and withhold GPU performance controls when unavailable.
- Allow a remembered AERO 17 YB model selection when a Gigabyte AERO BIOS leaves model, SKU and baseboard fields blank. Keep original identity readings visible and ignore the selection if another model is reported.

## 0.2.3

- Keep Windows sign-in launches in the notification area. Retry adding the icon while Explorer starts, and show the control window when the user opens it from the icon or launches aero-control manually.

## 0.2.2 — 2026-11-09

- Fix tray Exit and Open becoming unresponsive after closing the control window. Closing now returns the same controller to its background loop, so tray commands and resume handling do not depend on Windows repainting a hidden window.
- Keep the saved fan choice and Windows startup enabled when exiting. Stop queued automation once Exit is requested, and preserve the controller and settings when reopening its window.
- Add native Windows regression tests for process exit from background, visible and closed windows, repeated reopening, fan changes while in the tray, and closing without tray operation.

## 0.2.1 — 2026-11-08

- Remember successful fan-mode changes, fixed duties and exact custom curves automatically, and restore them when the app opens or resumes from sleep, matching Control Center's manual fan lifecycle.
- Migrate existing settings using the active profile's fan settings without enabling CPU or charging automation. Full-profile rules remain optional and take priority when enabled.
- Keep failed commands, startup readbacks and the temporary Max test out of fan-mode memory. Resolve queued resume restoration from the latest confirmed preference.
- Clarify fan restoration and Windows logon startup on Profiles. Invalid settings files continue to block automatic writes.
- Start silently at Windows sign-in, restoring the fan preference without creating the control window. Keep a tray icon for opening the app, resume handling and Exit; report startup failures visibly.
- Prevent duplicate Windows controllers: an interactive second launch opens the existing copy, while a repeated background launch stays silent. Existing startup tasks can be upgraded to background launch from Profiles.
- Resolve Task Scheduler account names to Windows user IDs before checking ownership, so Windows' normalization of a logon trigger cannot falsely reject the app's own task.
- Automatically configure background startup on the first supported, elevated release launch. Preserve disabled tasks and later opt-outs, verify registration before saving completion, and report failures with retry available. Simulation, debug builds, unsupported hardware and invalid settings skip setup.

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
