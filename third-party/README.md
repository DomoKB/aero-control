# Dependency license supplements

These are license texts and notices only. Dependency implementation code, vendor applications, drivers and DLLs are not vendored here.

Some Cargo packages used by the GUI omit the repository-level license files from their published crate. `licenses.json` maps those exact package versions to license texts obtained from the upstream Git commit recorded in each package's `.cargo_vcs_info.json`:

| Upstream project | Revision / license source |
|---|---|
| AccessKit (`accesskit`, consumer, Windows backend) | [02cf1aa5](https://github.com/AccessKit/accesskit/tree/02cf1aa5dda9cb4d97ef7bbab486c039c35960a8) |
| AccessKit (`accesskit_winit`) | [c1bd2d61](https://github.com/AccessKit/accesskit/tree/c1bd2d610615a244fd9e8bba0a6c3aecbfe46821) |
| egui / eframe / epaint / emath / ecolor / windowing integration | [fe368bac](https://github.com/emilk/egui/tree/fe368bacc4a15697e347607e73a56c0299b3d42d) |
| clipboard-win | [ed27008a](https://github.com/DoumanAsh/clipboard-win/tree/ed27008ae5d1f10cfbb266de039f0fb35fd05a56) |
| gl-rs (`gl_generator`) | [ea503e8d](https://github.com/rust-windowing/gl-rs/tree/ea503e8d5fb6d73c6030e6191ce738cd3bf3433e) |
| gl-rs (`khronos_api`) | [f150967b](https://github.com/rust-windowing/gl-rs/tree/f150967b1c44ae888e6676f93f639ebc82771bdc) |

The release packager first collects license/notice files from each package actually used by Cargo, including font notices and build-time packages. It uses these version-specific supplements only when a package has none. It stops if neither source supplies notices. Review this mapping when changing dependencies; do not silently reuse an old notice for a different version.
