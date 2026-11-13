param([switch]$Offline)

$ErrorActionPreference = 'Stop'
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
    throw 'This helper is for Windows. On other platforms, use cargo test --workspace.'
}
$repoRoot = Split-Path -Parent $PSScriptRoot
$previousVersion = [Environment]::GetEnvironmentVariable('CARGO_PKG_VERSION', 'Process')
Push-Location $repoRoot
try {
    $cargoOptions = @('--locked')
    if ($Offline) { $cargoOptions += '--offline' }
    $metadataText = & cargo metadata --format-version 1 --no-deps @cargoOptions
    if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed.' }
    $metadata = $metadataText | ConvertFrom-Json
    $package = $metadata.packages | Where-Object name -eq 'aero-gui'
    $testDirectory = Join-Path $metadata.target_directory 'gui-tests'
    $null = New-Item -ItemType Directory -Path $testDirectory -Force
    $buildLog = Join-Path $testDirectory 'compiler-artifacts.jsonl'
    & cargo build -p aero-gui --message-format=json @cargoOptions > $buildLog
    if ($LASTEXITCODE -ne 0) { throw 'GUI dependency build failed.' }
    $events = @(Get-Content -LiteralPath $buildLog | ForEach-Object { $_ | ConvertFrom-Json })
    $testBinary = Join-Path $testDirectory 'aero-gui-tests.exe'
    $compilerArgs = @('--edition=2021', '--test', (Join-Path $repoRoot 'crates/aero-gui/src/main.rs'),
        '--crate-name', 'aero_gui_tests', '-o', $testBinary)

    # Cargo's intermediate layout is not stable (it changed in Rust 1.100).
    # Locate transitive libraries and proc macros from this build's artifacts,
    # just as we locate the direct dependencies below.
    $dependencyDirectories = @($events | Where-Object reason -eq 'compiler-artifact' |
        ForEach-Object filenames | Where-Object { [IO.Path]::GetExtension($_) -in @('.rlib', '.rmeta', '.dll') } |
        ForEach-Object { Split-Path -Parent $_ } | Select-Object -Unique)
    if ($dependencyDirectories.Count -eq 0) { throw 'Cargo reported no Rust dependency directories.' }
    foreach ($directory in $dependencyDirectories) {
        $compilerArgs += @('-L', ('dependency=' + $directory))
    }

    # These are the GUI's direct dependencies. Select only artifacts from this
    # Cargo build, including the direct windows/toml versions in Cargo.toml.
    foreach ($name in @('aero_core', 'aero_wmi', 'aero_kbd', 'eframe', 'windows', 'toml', 'winit')) {
        $artifacts = @($events | Where-Object {
            $_.reason -eq 'compiler-artifact' -and $_.target.name -eq $name -and
            ($name -ne 'windows' -or $_.package_id -match 'windows@0\.59\.') -and
            ($name -ne 'toml' -or $_.package_id -match 'toml@0\.8\.')
        })
        $libraries = @($artifacts.filenames | Where-Object { $_ -like '*.rlib' } | Select-Object -Unique)
        if ($libraries.Count -ne 1) { throw "Expected one coherent Rust library for $name; found $($libraries.Count)." }
        $compilerArgs += @('--extern', ($name + '=' + $libraries[0]))
    }
    foreach ($linkPath in @($events | Where-Object reason -eq 'build-script-executed' | ForEach-Object linked_paths | Select-Object -Unique)) {
        $compilerArgs += @('-L', $linkPath)
    }
    [Environment]::SetEnvironmentVariable('CARGO_PKG_VERSION', $package.version, 'Process')
    # Compile the actual source as tests without the production UAC/resource
    # linker arguments. This never changes the production manifest or binary.
    & rustc @compilerArgs
    if ($LASTEXITCODE -ne 0) { throw 'GUI test compilation failed.' }
    & $testBinary --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw 'GUI tests failed.' }
} finally {
    [Environment]::SetEnvironmentVariable('CARGO_PKG_VERSION', $previousVersion, 'Process')
    Pop-Location
}
