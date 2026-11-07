# User guide

## First launch

Run the release executable directly and approve Windows administrator access when using real firmware controls. The header shows the detected model and app version. Unsupported models retain their own name and show unavailable controls. Simulation is labelled explicitly; successful administrator access is not shown as a badge during normal use.

Extract the app to a permanent folder before this first launch. On supported hardware, the release automatically configures background startup using the same administrator approval; there is no separate setup button to find. Choose a fan mode and it is remembered for later Windows sign-ins. You can disable startup on Profiles. Simulation, debug builds, unsupported hardware, non-elevated sessions and invalid saved settings never trigger automatic registration.

Use one hardware-control app at a time while comparing behaviour. aero-control does not uninstall or disable Control Center, its services, or Intel XTU.

## Fans and curves

Choose Normal, Quiet or Gaming for the corresponding firmware mode. Gaming also sends GPU Level 2 and uses the Standard thermal profile. A later fan-mode selection can replace a manually selected GPU request.

Fixed sets the same duty for both fans. The normal fixed-duty floor is 35%; Expert mode permits lower fixed values. Max requires confirmation and requests full-scale duty. The separate ten-second Max test restores Normal afterward.

On Curve, Factory loads the official reference YB table; the quiet reference curve is a separate preset. Editing a preview does not apply it until **Apply firmware curve**. When Custom is active, valid edits apply automatically after editing finishes. The complete table is written and read back. An unused slot is a `(0, 0)` terminator, so such points cannot appear inside a user curve.

On opening, the app first reads the durable fan flags and thermal settings to identify the current firmware state. It then restores the remembered fan preference by default. A matching Fixed 100% state is labelled Max because those configurations are indistinguishable. Step control without enough evidence for a particular curve remains unknown until a remembered custom curve is successfully reapplied.

## Power controls

GPU Level 1 and Level 2 send the official model-specific requests. The highlighted choice is the last request sent by this app in this session, not measured GPU power. A zero request flag can mean the request has already cleared; it does not identify an active Level 1 budget. After restart the active GPU budget is unknown.

GPU thermal profiles are Standard and Quiet. This target stores that control as a single bit; there is no verified third profile or user-defined temperature slider.

CPU controls show the reference sustained and short-burst limits. Select a supported preset and click **Apply CPU preset**. Locked controls, unsupported CPUs and Intel XTU compatibility/connection failures leave the controls unavailable. Reading the page does not tune the CPU. No Intel SDK or driver is installed automatically.

## Profiles and automation

**Save current** captures settings without applying them. Built-in profile names cannot be overwritten. A GPU request is captured only if this session sent one; the app does not manufacture a current level from a cleared flag.

Successful fan changes are saved automatically, including raw fixed duty and custom-curve bytes. **Restore the last fan mode when the app opens and after sleep** is enabled by default on Profiles. Fan-only restoration sends the mode's normal firmware/GPU sequence and leaves CPU presets, charging settings and separate GPU overrides alone. Failed commands, initial firmware readings and the temporary ten-second Max test do not replace the saved preference. Existing settings use their last applied named profile's fan settings until the next successful fan change is remembered.

Full-profile startup/resume apply, charger/battery switching and native Fn handling remain separate opt-ins. Tray operation is optional for manual launches and automatic for Windows background startup. Enabled full-profile rules take priority over fan-only restoration. Saving automation settings does not immediately apply a profile. Display refresh-rate switching is also opt-in and tied to a selected monitor's identity.

Cold-boot restoration is configured automatically on the first supported release launch. The task starts this executable with `--background` and administrator privileges after signing in. It restores the saved preference without creating the control window and keeps a notification icon for opening the app or exiting. You do not need to launch the app manually, enable "Start in the notification area", or run Gigabyte Control Center. Restoration occurs after Windows sign-in, not before login or while the laptop is off. It requires this background process; the preference is not stored permanently in the firmware.

**Profiles → Start with Windows** shows the verified result and provides Disable/Enable controls. Existing disabled tasks are respected. First-launch setup upgrades a verified enabled legacy task to background launch; **Use background startup** remains available for manual upgrades. Successful setup or a manual startup choice is saved, so opening the app again will not recreate a subsequently removed task or undo an opt-out. A setup failure is reported and can be retried using the controls or on the next launch. No other profile automation is enabled by this setup.

Keep the registered executable path stable. If moving an installation, disable the task using the old copy and inspect/remove the old task in Windows Task Scheduler before enabling startup from the new copy; an existing task pointing elsewhere is never silently taken over. For deployment, `aero-gui.exe --enable-startup` registers or upgrades the task and exits without touching fan hardware; `--disable-startup` disables it.

Opening aero-control again brings up the running controller instead of starting a second one. Startup failures, invalid saved settings or an unavailable tray icon open the window so the problem is visible. Simulation has no real tray icon and falls back to a visible window.

Profiles may use `cpu_power_preset` values `level-1` through `level-5`, `default` or `battery`. Legacy `cpu_pl1_w` / `cpu_pl2_w` entries are retained as data but do not apply arbitrary CPU limits; do not combine them with a preset. CPU is applied before a profile's firmware writes. CPU failure prevents those writes. If firmware application later fails, its recovery runs and the already-applied CPU preset remains, as the report explains.

## Closing the app

With tray operation enabled and a registered icon, closing the window hides it instead of exiting. Windows background startup always uses this behaviour for its running session. Use the tray's Exit command to end the session. Background automation needs the app running; exiting stops resume handling until the next launch or Windows sign-in. Disabling the startup task prevents future automatic launches but does not stop the current session.

Normal, Quiet, Gaming and an uploaded hardware curve run in firmware. Normal exit attempts the Normal sequence after Fixed or Max. This cleanup does not erase the remembered preference: reopening restores Fixed or Max if that was the last successful selection and fan restoration remains enabled. Recovery can fail and cannot run after a force-kill or crash. CPU limits are not automatically restored on exit. Keyboard lighting can remain in the device after closing.

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
