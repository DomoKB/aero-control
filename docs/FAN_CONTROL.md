# Fan modes and curves

Mode commands follow the AERO 17 YB Control Center manual button paths with AI disabled. Normal, Quiet and Gaming do not reset the current fan step. Aero-control retains paced calls, curve validation, readback and recovery; Control Center registry and AI features are not replicated. Gaming command completion and Normal restoration were verified on this laptop; full RPM, noise and thermal behaviour remain unvalidated.

| Control | What is sent | Confirmed? |
|---|---|---|
| Normal / recovery | Fixed and step off, auto flag 0, thermal target 0, NVIDIA power config 0 | Control Center Normal sequence |
| Quiet | Fixed and step off, auto flag 0, thermal target 1 | Control Center YB sequence |
| Gaming | Fixed and step off, auto flag 1, NVIDIA power config 1 | Control Center YB sequence |
| Max / Fixed | Step and fixed enabled; both fan duty fields written, thermal target and NVIDIA power config cleared | Physical response needs hardware validation |
| Upload curve | Full 15-slot write and readback | Readback does not prove fan response |
| Factory / Deep Control | YB default custom curve | Based on DeepFan and getter data |

The editor opens on the separate Quiet curve. **Factory** loads the official YB table, including its exact raw duty bytes. **Apply firmware curve** uploads it; if Custom mode is already active, editor changes are applied automatically. Readback compares against the raw bytes sent; a mismatch after retry triggers recovery. See [source notes](SOURCES.md).

At startup, durable fan flags and the thermal profile identify Normal, Quiet, Gaming, Fixed or Max and restore the highlight without sending a mode command. The transient NVIDIA request flag is not used to recognize a fan mode. Ambiguous step/custom configurations remain unknown. Fixed at 100% is indistinguishable from Max in firmware and is labelled Max.

What the upload does, and Control Center does not:

- Sends all 15 slots in order. Unused slots are `(0, 0)`, which is the terminator, so an old point cannot linger after it.
- Reads every slot back with `GetFanIndexValue`. One mismatch retries the whole upload once, then restores firmware auto.
- Clears auto and fixed, resets the current step and enables step mode before the points.
- Uses the official truncated raw bytes for Factory. Other percent-based custom points round half up; explicit raw values are preserved.
- Rejects a `(0, 0)` point anywhere in the curve, including the middle, because the firmware treats it as the end of the table.

Pasting `HKLM\SOFTWARE\Gigabyte\SmartManager\CustomFanTable` into the curve tab parses it. On a Windows build, **Read CustomFanTable from the registry** opens that key read-only. Nothing in this repo writes the Gigabyte registry.

Profiles are TOML. `profiles/config.toml` binds Gaming to AC and Quiet to DC. `curve_pairs` are percent. A `duty_raw` field, when present, is the exact byte that will be sent. Legacy `cpu_pl1_w` / `cpu_pl2_w` fields remain stored only; use `cpu_power_preset` for supported CPU tuning in the desktop app.
