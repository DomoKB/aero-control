# User guide

## First launch

Run the release executable directly and approve Windows administrator access when using real firmware controls. The header shows the detected model and app version. Unsupported models retain their own name and show unavailable controls. Simulation is labelled explicitly; successful administrator access is not shown as a badge during normal use.

Use one hardware-control app at a time while comparing behaviour. aero-control does not uninstall or disable Control Center, its services, or Intel XTU.

## Fans and curves

Choose Normal, Quiet or Gaming for the corresponding firmware mode. Gaming also sends GPU Level 2 and uses the Standard thermal profile. A later fan-mode selection can replace a manually selected GPU request.

Fixed sets the same duty for both fans. The normal fixed-duty floor is 35%; Expert mode permits lower fixed values. Max requires confirmation and requests full-scale duty. The separate ten-second Max test restores Normal afterward.

On Curve, Factory loads the official reference YB table; the quiet reference curve is a separate preset. Editing a preview does not apply it until **Apply firmware curve**. When Custom is active, valid edits apply automatically after editing finishes. The complete table is written and read back. An unused slot is a `(0, 0)` terminator, so such points cannot appear inside a user curve.

At restart, the app recognizes durable fan flags and thermal settings without writing a mode. A matching Fixed 100% state is labelled Max because those configurations are indistinguishable. Step control without enough evidence for a particular curve remains unknown.

## Power controls

GPU Level 1 and Level 2 send the official model-specific requests. The highlighted choice is the last request sent by this app in this session, not measured GPU power. A zero request flag can mean the request has already cleared; it does not identify an active Level 1 budget. After restart the active GPU budget is unknown.

GPU thermal profiles are Standard and Quiet. This target stores that control as a single bit; there is no verified third profile or user-defined temperature slider.

CPU controls show the reference sustained and short-burst limits. Select a supported preset and click **Apply CPU preset**. Locked controls, unsupported CPUs and Intel XTU compatibility/connection failures leave the controls unavailable. Reading the page does not tune the CPU. No Intel SDK or driver is installed automatically.

## Profiles and automation

**Save current** captures settings without applying them. Built-in profile names cannot be overwritten. A GPU request is captured only if this session sent one; the app does not manufacture a current level from a cleared flag.

Automation options are separate opt-ins: startup apply, charger/battery switching, resume reapply, tray operation and native Fn handling. Saving automation settings does not immediately apply a profile. Display refresh-rate switching is also opt-in and tied to a selected monitor's identity.

The optional Windows logon task launches this executable at its current path with administrator privileges. Keep a release in a permanent folder before enabling that task. If moving an installation, disable the startup task using the old copy and recreate it from the new copy. No task is created automatically.

Profiles may use `cpu_power_preset` values `level-1` through `level-5`, `default` or `battery`. Legacy `cpu_pl1_w` / `cpu_pl2_w` entries are retained as data but do not apply arbitrary CPU limits; do not combine them with a preset. CPU is applied before a profile's firmware writes. CPU failure prevents those writes. If firmware application later fails, its recovery runs and the already-applied CPU preset remains, as the report explains.

## Closing the app

With tray operation enabled and a registered icon, closing the window can hide it instead of exiting. Use the tray's Exit command to end the session. Background automation needs the app running.

Normal, Quiet, Gaming and an uploaded hardware curve run in firmware. Normal exit attempts the Normal sequence after Fixed or Max. Recovery can fail and cannot run after a force-kill or crash. CPU limits are not automatically restored on exit. Keyboard lighting can remain in the device after closing.

## System and keyboard

System readings refresh when entering the page, on **Refresh readings**, or after a local change. Bluetooth radio enumeration is not nearby-device discovery. Use Refresh for changes made in another application. Permission, policy and hardware-disabled radio states are reported.

Keyboard controls use compatible built-in Gigabyte ITE HID collections. Effects are read back when supported. The custom picture controls fill all keys with a colour; a physical per-key editor and Fusion macro tools are not implemented.

## Troubleshooting

| Symptom | Meaning / next step |
|---|---|
| Administrator access needed | Run the executable directly with administrator access for firmware changes. |
| Model not verified | Naming a model is not support for its command set. Collect read-only device information before proposing model support. |
| Intel XTU unavailable | Check its installed version, service and compatibility diagnostics. The core firmware controls are independent of Intel tuning. |
| GPU request flag returns to 0 | This can happen within milliseconds. Use the last-request label; do not infer actual watts from the flag. |
| Firmware mismatch or recovery failure | Read the named getter and active log. Do not assume a failed request had no side effects. |
| Fan mode unknown after restart | The configuration is ambiguous or a required fan/thermal getter failed. Choose a supported mode explicitly if appropriate. |
| Settings file invalid | The file is preserved and profile automation is blocked. Back it up and correct it before reloading. |

The Log page gives the active call-log location. Before sharing logs, remove any account paths or identifiers you do not want public. Include the app version, model/SKU, BIOS, CPU/GPU, driver version and exact action that failed.
