# Contributing

Read the [README](README.md), [architecture](docs/ARCHITECTURE.md) and [feature coverage](docs/PARITY.md) before changing hardware behaviour. Keep source facts, simulations and physical observations separate.

## Build and test

Windows development needs Rust, the MSVC C++ tools and a Windows SDK. The workspace declares Rust 1.85 as its minimum; use a current stable toolchain for normal development. Commit Cargo.lock changes intentionally.

```powershell
cargo fmt --all -- --check
cargo test --locked --workspace --exclude aero-gui
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\test-gui.ps1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\crates\aero-gui\tests\cpu_power_bridge.tests.ps1
cargo build --locked --release -p aero-gui
```

Use `-Offline` on the GUI helper when all dependencies are cached. The production GUI embeds an administrator manifest, so ordinary `cargo test -p aero-gui` can require elevation. The helper compiles the actual GUI/session source against Cargo's coherent dependency artifacts without that production manifest. Its direct dependency selection must stay aligned with `crates/aero-gui/Cargo.toml`. No alternate application implementation is substituted.

Run `aero-gui.exe --mock` for interface work. Unit tests and the fake-SDK bridge test do not intentionally control physical hardware. The workflow in `.github/workflows/ci.yml` uses the upstream [checkout](https://github.com/actions/checkout) and [Rust toolchain](https://github.com/dtolnay/rust-toolchain) actions and builds/tests on Windows; it cannot establish laptop compatibility.

## Hardware changes

Describe the supported model/BIOS, exact method or report, source evidence, parameter layout and expected readback. Add meaningful failure coverage when changing firmware sequences. Preserve explicit unknown states; a transport success, a changed UI highlight or a transient request is not proof of effective hardware behaviour.

New model support requires verified command/layout/meaning differences and appropriate write gates. Detecting a new model name alone is insufficient. Do not add guessed firmware setters, disable thermal protection, or replace request-only GPU evidence with an invented wattage reading.

Keep proprietary installers, drivers, vendor DLLs, raw decompiled source trees, user settings and local hardware logs out of this repository. Facts and observations can be recorded with their provenance in `docs/SOURCES.md`. Do not copy AeroCtl's GPLv3 implementation into this dual-licensed project.

## Pull requests and reports

State the problem, resulting behaviour and validation. Identify simulated tests separately from hardware checks and mention any remaining release checks. For bugs, provide the app version, model/SKU, BIOS, CPU/GPU, driver version, exact action and relevant redacted log excerpt. Preserve invalid user settings files when diagnosing them.

Contributions are intended to use the repository's existing MIT OR Apache-2.0 licensing; dependency licenses remain separate.
