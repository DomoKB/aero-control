# Architecture

## Implementation boundaries

The GUI is a Rust `eframe`/`egui` application. Its normal Windows backend calls the supported Gigabyte firmware WMI data blocks through `advapi32`'s native WMI API. It does not load Gigabyte's `acpimof.dll`, require registered Gigabyte CIM classes, install a driver, or run a vendor service.

Keyboard lighting uses native Windows HID access. Windows power, audio, display, radio, device-identity, hotkey, tray and scheduled-task controls use their corresponding Windows APIs. Optional CPU tuning is the one non-Rust runtime bridge: an embedded PowerShell script invokes the installed Intel XTU .NET SDK in a hidden, bounded child process.

## Workspace

| Crate | Responsibility |
|---|---|
| `aero-wmi` | Allow-listed method catalog, input packing/output decoding, direct Windows transport, model information, mocks and RPM decoding |
| `aero-core` | Firmware sequences, curve validation/upload/readback, recovery, power requests, CPU preset values and TOML profile types |
| `aero-gui` | Desktop UI, serialized firmware session, Windows controls, profiles/automation, optional Intel bridge and application lifecycle |
| `aero-kbd` | Gigabyte ITE lighting report encoding, HID connection/readback and keyboard profile storage |
| `aero-probe` | Getter-only diagnostic executable; its normal dependency disables the writer feature |
| `aero-proto` | Versioned messages for possible future inter-process use |
| `aero-svc` | In-process request dispatcher; the Windows service/pipe host is not implemented |

`profiles/config.toml` is an example configuration, not an automatic installation or startup policy. Per-user full-profile automation defaults to off. Fan-only restoration defaults to on and uses a separate `last_fan` snapshot; older settings fall back to the fan part of their active profile.

## Firmware execution

One worker owns the firmware connection and serializes commands. The UI sends typed requests and receives snapshots, completion results and errors. Normal sensor polling uses getters only. There is no temperature-driven software fan controller.

The catalog limits callable methods and defines their MOF layouts. `TurnOffFan` is explicitly refused. Setters are paced; transport success is not proof that firmware acted. Curve uploads write all 15 slots, clear unused entries, and compare the raw bytes read back. Mode verification checks durable fan flags and thermal state. A mismatch retries a complete guarded operation once, then attempts Normal recovery; failed recovery is a distinct error.

The NVIDIA power configuration is different: the request stored in `TEMQ` can clear shortly after the device notification. It cannot prove the current GPU power budget. GPU command completion therefore carries a separate typed last-request result. Refreshing raw firmware data does not replace that request history with an invented current GPU mode. See the hardware observations in [SOURCES.md](SOURCES.md).

## Model support

SMBIOS registry fields supply the displayed model/SKU/BIOS. Presentation is independent of support: other detected names appear, but the current write gate recognizes only the verified YB command set. CPU presets additionally require the verified processor and compatible Intel controls. USB identity/report rules gate keyboard access separately. Reference curves retain their actual provenance rather than being relabelled as measurements from the current machine.

## Optional Intel bridge

The bridge checks the installed SDK's signature, CPU identity, platform compatibility, control IDs/units, writable state and supported values before tuning. It applies only the official preset pairs, preserves the fractional battery limit, refreshes active values and attempts verified rollback after a failed pair. Output is bounded and parsed strictly; a timeout leaves the CPU limits unconfirmed. It does not install binaries, change security settings or automatically restore limits on application exit.

## State and lifecycle

Fan completion, CPU results and GPU requests are separate event fields. UI drafts are kept apart from confirmed readings. Profiles use per-user TOML with validation and atomic replacement; invalid files are preserved and do not enable automation. Settings and imports do not overwrite Control Center's registry data.

Verified fan completions update `last_fan` atomically with any named-profile selection. Initial readbacks, progress events, failed writes and temporary Max tests cannot replace it. The snapshot contains fan settings only. Startup/resume restoration resolves the latest snapshot after any in-flight command completes, avoiding stale resume requests. Full-profile automation and hotkeys remain explicit opt-ins. No aero-control Windows service is installed, so ongoing automation ends with the process.

The per-user logon task passes `--background`. This path polls the same session/automation state without creating an eframe control window or renderer. It registers the native tray icon and handles resume broadcasts. Open requests move that same app into eframe; errors fall back to the visible UI. Background launches always keep the tray available. Manual launches retain the saved tray/minimized preferences. A session-local named mutex excludes duplicate controllers; an auto-reset event brings up the existing instance on a second manual launch. Mock instances use a separate name. The guard outlives the hardware worker. Legacy tasks are recognized, but their arguments change only through an explicit enable/upgrade request after ownership and action validation.

## Verification boundaries

Mock tests check protocol bytes, sequencing, state transitions and failure handling. Fake Intel SDK tests check tuning/rollback behaviour without Intel software or hardware. A bounded real-laptop check verified Gaming flags, the transient GPU request and Normal restoration. These do not measure sustained GPU watts, temperatures, battery charging, or universal model compatibility. CI executes simulated tests and builds executables; hardware evidence is recorded separately.
