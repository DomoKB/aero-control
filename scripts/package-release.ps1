param([switch]$Offline)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot
try {
    if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) { throw 'Package the Windows release on Windows.' }
    $options = @('--locked')
    if ($Offline) { $options += '--offline' }
    $metadataText = & cargo metadata --format-version 1 --no-deps @options
    if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed.' }
    $metadata = $metadataText | ConvertFrom-Json
    $package = $metadata.packages | Where-Object name -eq 'aero-gui'
    $null = New-Item -ItemType Directory -Path $metadata.target_directory -Force
    $buildLog = Join-Path $metadata.target_directory 'release-package-artifacts.jsonl'
    & cargo build --release -p aero-gui --message-format=json @options > $buildLog
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
    $events = @(Get-Content -LiteralPath $buildLog | ForEach-Object { $_ | ConvertFrom-Json })
    $binary = @($events | Where-Object { $_.reason -eq 'compiler-artifact' -and $_.package_id -eq $package.id -and $_.executable } | ForEach-Object executable)
    if ($binary.Count -ne 1) { throw 'Cannot identify the release executable.' }
    $version = [Diagnostics.FileVersionInfo]::GetVersionInfo($binary[0])
    if ($version.ProductVersion -ne $package.version -or $version.FileVersion -ne $package.version) { throw 'Executable version does not match Cargo.' }
    $peBytes = [IO.File]::ReadAllBytes($binary[0])
    $peOffset = [BitConverter]::ToInt32($peBytes, 0x3c)
    if ([BitConverter]::ToUInt16($peBytes, $peOffset + 4) -ne 0x8664) { throw 'This package expects an x64 executable.' }
    if ([BitConverter]::ToUInt16($peBytes, $peOffset + 24 + 68) -ne 2) { throw 'Release must use the Windows GUI subsystem.' }
    if (-not [Text.Encoding]::UTF8.GetString($peBytes).Contains('requireAdministrator')) { throw 'Administrator manifest is missing.' }

    $distRoot = Join-Path $repoRoot 'dist'
    $null = New-Item -ItemType Directory -Path $distRoot -Force
    $stem = 'aero-control-v' + $package.version + '-windows-x64'
    $staging = Join-Path $metadata.target_directory ('package-' + [guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Path $staging
    try {
        Copy-Item -LiteralPath $binary[0] -Destination (Join-Path $staging 'aero-gui.exe')
        foreach ($name in @('README.md', 'CHANGELOG.md', 'CONTRIBUTING.md', 'LICENSE-MIT', 'LICENSE-APACHE', 'Cargo.lock')) {
            Copy-Item -LiteralPath (Join-Path $repoRoot $name) -Destination $staging
        }
        Copy-Item -LiteralPath (Join-Path $repoRoot 'docs') -Destination $staging -Recurse
        Copy-Item -LiteralPath (Join-Path $repoRoot 'profiles') -Destination $staging -Recurse

        # Preserve license/notice files from packages actually used by this
        # build, including bundled font notices. No dependency DLLs are copied.
        $notices = @('# Third-party notices', '', 'Dependency license files collected from the packages used by this Cargo build. Includes build-time packages; this is intentionally broader than runtime-only dependencies.', '')
        $licenseMap = Get-Content -LiteralPath (Join-Path $repoRoot 'third-party/licenses.json') -Raw | ConvertFrom-Json
        $artifacts = @($events | Where-Object reason -eq 'compiler-artifact' | Group-Object package_id | ForEach-Object { $_.Group[0] })
        foreach ($artifact in $artifacts) {
            $manifestDirectory = Split-Path -Parent $artifact.manifest_path
            if ($manifestDirectory.StartsWith($repoRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { continue }
            $dependencyName = Split-Path -Leaf $manifestDirectory
            $licenseDirectory = $manifestDirectory
            $licenseFiles = @(Get-ChildItem -LiteralPath $licenseDirectory -File -Recurse | Where-Object {
                $_.Name -match '(?i)(licen[cs]e|copying|copyright|notice|^ofl)' -and $_.Extension -notin @('.rs', '.c', '.h', '.cpp', '.ttf', '.otf', '.exe', '.dll', '.sys')
            })
            $override = $null
            if ($licenseFiles.Count -eq 0) {
                $override = $licenseMap.PSObject.Properties[$dependencyName].Value
                if ($null -ne $override) {
                    $licenseDirectory = Join-Path (Join-Path $repoRoot 'third-party') $override.directory
                    $licenseFiles = @(Get-ChildItem -LiteralPath $licenseDirectory -File -Recurse)
                }
            }
            if ($licenseFiles.Count -eq 0) { throw "No license/notice files found for $dependencyName; review before packaging." }
            $notices += '- ' + $dependencyName + ': `licenses/' + $dependencyName + '/`'
            if ($null -ne $override) { $notices += '  - Upstream license source: ' + $override.source }
            foreach ($file in $licenseFiles) {
                $relative = $file.FullName.Substring($licenseDirectory.Length).TrimStart('\', '/')
                $destination = Join-Path (Join-Path (Join-Path $staging 'licenses') $dependencyName) $relative
                $null = New-Item -ItemType Directory -Path (Split-Path -Parent $destination) -Force
                Copy-Item -LiteralPath $file.FullName -Destination $destination
            }
        }
        [IO.File]::WriteAllLines((Join-Path $staging 'THIRD_PARTY_NOTICES.md'), $notices)
        $archive = Join-Path $distRoot ($stem + '.zip')
        $minimumZipTime = [datetime]::SpecifyKind([datetime]'1980-01-01', [DateTimeKind]::Utc)
        Get-ChildItem -LiteralPath $staging -File -Recurse | Where-Object { $_.LastWriteTimeUtc -lt $minimumZipTime } | ForEach-Object { $_.LastWriteTimeUtc = $minimumZipTime }
        Compress-Archive -Path (Join-Path $staging '*') -DestinationPath $archive -Force
        $hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
        [IO.File]::WriteAllText(($archive + '.sha256'), $hash + '  ' + [IO.Path]::GetFileName($archive) + [Environment]::NewLine)
        Write-Output $archive
        Write-Output ('SHA256: ' + $hash)
    } finally {
        # Delete only the unique staging directory created above, after checking
        # its absolute path is inside Cargo's target directory.
        $resolvedStaging = [IO.Path]::GetFullPath($staging)
        $targetPrefix = [IO.Path]::GetFullPath($metadata.target_directory).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
        if (-not $resolvedStaging.StartsWith($targetPrefix, [StringComparison]::OrdinalIgnoreCase)) { throw 'Refusing to remove staging outside the target directory.' }
        Remove-Item -LiteralPath $resolvedStaging -Recurse -Force
    }
} finally {
    Pop-Location
}
