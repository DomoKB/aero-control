# Release preparation

The current version is defined once in the root `Cargo.toml` under `workspace.package.version`. Cargo supplies the header version and Windows version resource. Do not maintain a separate UI version string.

## Prepare an artifact

1. Update the workspace version and `CHANGELOG.md`; refresh Cargo.lock with a normal build.
2. Run the checks in [CONTRIBUTING.md](../CONTRIBUTING.md). Review supported-model gates and the current hardware limitations.
3. Package on Windows x64:

   ```powershell
   powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\package-release.ps1
   ```

   Add `-Offline` if dependencies are already cached. The script builds the release, checks the version metadata, x64 architecture, GUI subsystem and administrator manifest, and creates a ZIP plus SHA-256 sidecar in `dist/`.

4. Inspect the ZIP. It includes the executable, guides, example profiles, project licenses, Cargo.lock and collected dependency license/notice files. It contains no Intel/Gigabyte DLLs or drivers. License notices are collected from actual build packages, including build-time packages and bundled font notices. Exact-version upstream supplements are documented in `third-party/README.md` when crate archives omit their license files.
5. Smoke-test an extracted copy with `--mock`. On supported hardware, check actual firmware controls separately and record observed results. Do not describe the GPU's transient request as measured power or mark CPU tuning verified from mock tests.

This repository has no signing certificate or automated installer/release publisher configured. The local executable is unsigned. Packaging is not publishing.

## Before uploading source

Review the Git diff and untracked files. Include the Rust crates, scripts, profiles, documentation, license files, workflow and lockfile. Build output, generated archives, local logs, `.env` files and the local root scope draft are ignored. The historical `docs/SCOPE.md` is preserved but explicitly marked as a superseded proposal.

Keep the separate reverse-engineered reference project and vendor binaries outside this repository. The current remote configuration is a local repository setting, not part of the documentation; verify the intended GitHub destination before pushing. No repository owner/URL or release download link is invented here.

## Publish when authorized

Use tag `v<workspace version>` and mark pre-1.0 artifacts as a pre-release while their documented validation remains incomplete. Upload the ZIP and its `.sha256` file alongside the release notes. The Windows CI workflow checks source/build behaviour; a successful CI run is not a hardware certification.
